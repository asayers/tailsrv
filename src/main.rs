use clap::Parser;
use inotify::{EventMask, Inotify, WatchMask};
use std::fs::File;
use std::io::BufRead;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::{io::AsRawFd, prelude::RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use tracing::*;

#[derive(Parser)]
struct Opts {
    /// The file which will be broadcast to all clients
    path: PathBuf,
    /// The port number on which to listen for new connections
    #[clap(long, short)]
    port: u16,
    #[cfg(feature = "tracing-journald")]
    /// Send traces to journald instead of the terminal.
    #[clap(long)]
    journald: bool,
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

pub static FILE_LENGTH: AtomicU64 = AtomicU64::new(0);

fn main() -> Result<()> {
    let opts = Opts::parse();

    #[cfg(feature = "tracing-journald")]
    if opts.journald {
        use tracing_subscriber::prelude::*;
        tracing_subscriber::registry()
            .with(tracing_journald::layer()?)
            .init()
    } else {
        tracing_subscriber::fmt::init();
    }
    #[cfg(not(feature = "tracing-journald"))]
    tracing_subscriber::fmt::init();

    let file;
    let mut inotify;
    let threads: Arc<Mutex<Vec<Thread>>> = Arc::new(Mutex::new(vec![]));

    // Open the file and set up the inotify watch
    {
        let _g = info_span!("file", path = %opts.path.display()).entered();
        file = loop {
            match File::open(&opts.path) {
                Ok(f) => break f,
                Err(e) => match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        info!("Waiting for file to be created");
                        std::thread::sleep(std::time::Duration::from_secs(3))
                    }
                    _ => return Err(e.into()),
                },
            }
        };
        if !file.metadata()?.is_file() {
            return Err(format!("{}: Not a file", opts.path.display()).into());
        }

        let file_len = file.metadata()?.len();
        FILE_LENGTH.store(file_len, Ordering::SeqCst);
        info!("Opened file (initial length: {}kB)", file_len / 1024);

        inotify = Inotify::init()?;
        inotify.add_watch(
            &opts.path,
            WatchMask::MODIFY | WatchMask::DELETE_SELF | WatchMask::MOVE_SELF,
        )?;
        info!("Created an inotify watch");
    }

    // Bind the socket and start listening for client connections
    {
        let listen_addr = SocketAddr::new([0, 0, 0, 0].into(), opts.port);
        let _g = info_span!("listener", addr = %listen_addr).entered();
        let listener = TcpListener::bind(&listen_addr)?;
        info!("Bound socket");

        let threads2 = threads.clone();
        let file_fd = file.as_raw_fd();
        std::thread::spawn(move || listen_for_clients(listener, threads2, file_fd));
        info!("Handling client connections");
        #[cfg(feature = "sd-notify")]
        sd_notify::notify(true, &[sd_notify::NotifyState::Ready])?;
    }

    // Monitor the file and wake up clients when it changes
    loop {
        let mut buf = [0; 1024];
        let events = inotify.read_events_blocking(&mut buf).unwrap();
        for ev in events {
            if ev
                .mask
                .intersects(EventMask::IGNORED | EventMask::DELETE_SELF | EventMask::MOVE_SELF)
            {
                info!("Watched file disappeared");
                std::process::exit(0);
            } else if ev.mask.contains(EventMask::MODIFY) {
                let file_len = file.metadata().unwrap().len();
                debug!("File length is now {}", file_len);
                FILE_LENGTH.store(file_len, Ordering::SeqCst);
                for t in threads.lock().unwrap().iter() {
                    t.unpark();
                }
            } else {
                warn!("Unknown inotify event: {ev:?}");
            }
        }
    }
}

fn listen_for_clients(listener: TcpListener, threads: Arc<Mutex<Vec<Thread>>>, file_fd: i32) {
    for conn in listener.incoming() {
        let conn = match conn {
            Ok(x) => x,
            Err(e) => {
                error!("Bad connection: {e}");
                continue;
            }
        };
        let threads2 = threads.clone();
        let join_handle = std::thread::spawn(move || {
            let _g = match conn.peer_addr() {
                Ok(addr) => info_span!("client", %addr).entered(),
                Err(e) => info_span!("client", no_addr = %e).entered(),
            };
            match handle_client(conn, file_fd) {
                Ok(()) => (),
                Err(e) => error!("{e}"),
            }
            threads2
                .lock()
                .unwrap()
                .retain(|t| t.id() != std::thread::current().id());
            info!("Cleaned up the thread");
        });
        threads.lock().unwrap().push(join_handle.thread().clone());
    }
    error!("Listening socket was closed!");
    std::process::exit(1);
}

fn read_header(conn: &mut TcpStream) -> Result<i64> {
    // Read the header
    let mut buf = String::new();
    std::io::BufReader::new(conn).read_line(&mut buf)?;
    // TODO: timeout
    // TODO: length limit

    // Parse the header (it's just a signed int)
    let header: i64 = buf.as_str().trim().parse()?;

    // Resolve the header to a byte offset
    if header >= 0 {
        Ok(header)
    } else {
        let cur_len = FILE_LENGTH.load(Ordering::SeqCst);
        Ok(i64::try_from(cur_len)? + header)
    }
}

fn handle_client(mut conn: TcpStream, fd: RawFd) -> Result<()> {
    info!("Connected");
    // The first thing the client will do is send a header
    let mut offset = read_header(&mut conn)?;
    info!("Starting from offset {}", offset);
    loop {
        // How many bytes the client wants
        let file_len = FILE_LENGTH.load(Ordering::SeqCst) as usize;
        let wanted = file_len.saturating_sub(offset as usize);
        if wanted == 0 {
            // We're all caught-up.  Wait for new data to be written
            // to the file before continuing.
            debug!("Waiting for changes");
            std::thread::park();
        } else {
            debug!("Sending {wanted} bytes from offset {offset}");
            if let Err(e) =
                nix::sys::sendfile::sendfile(conn.as_raw_fd(), fd, Some(&mut offset), wanted)
            {
                match std::io::Error::from(e).kind() {
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => {
                        // The client hung up
                        info!("Socket closed by other side");
                        return Ok(());
                    }
                    _ => return Err(e.into()),
                }
            }
        }
    }
}
