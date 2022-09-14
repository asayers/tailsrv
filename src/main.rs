use clap::Parser;
use inotify::{EventMask, Inotify, WatchMask};
use once_cell::sync::OnceCell;
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
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

static FILE_LENGTH: AtomicU64 = AtomicU64::new(0);
static FILE_FD: OnceCell<RawFd> = OnceCell::new();

fn main() -> Result<()> {
    let opts = Opts::parse();
    tracing_subscriber::fmt::init();

    let threads: Arc<Mutex<Vec<Thread>>> = Arc::new(Mutex::new(vec![]));

    // Bind the socket and start listening for client connections
    {
        let listen_addr = SocketAddr::new([0, 0, 0, 0].into(), opts.port);
        let _g = info_span!("listener", addr = %listen_addr).entered();
        let listener = TcpListener::bind(&listen_addr)?;
        info!("Bound socket");

        let threads2 = threads.clone();
        std::thread::spawn(move || listen_for_clients(listener, threads2));
        info!("Handling client connections");
    }

    let mut inotify;
    let file;

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
        FILE_FD
            .set(file.as_raw_fd())
            .map_err(|_| "Set FILE_FD twice")?;
        info!("Opened file (initial length: {}kB)", file_len / 1024);

        // Wake up any clients who were waiting for the file to exist
        for t in threads.lock().unwrap().iter() {
            t.unpark();
        }

        inotify = Inotify::init()?;
        inotify.add_watch(
            &opts.path,
            WatchMask::MODIFY | WatchMask::DELETE_SELF | WatchMask::MOVE_SELF,
        )?;
        info!("Created an inotify watch");
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

fn listen_for_clients(listener: TcpListener, threads: Arc<Mutex<Vec<Thread>>>) {
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
            match handle_client(conn) {
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

fn handle_client(mut conn: TcpStream) -> Result<()> {
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
            let fd = match FILE_FD.get() {
                Some(x) => *x,
                None => {
                    error!(
                        "FILE_LENGTH is {file_len}, but FILE_FD isn't set yet.\
                        This is a bug."
                    );
                    continue;
                }
            };
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
