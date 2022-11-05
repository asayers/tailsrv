use clap::Parser;
use inotify::{EventMask, Inotify, WatchMask};
use once_cell::sync::OnceCell;
use std::fs::File;
use std::io::BufRead;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::{fs::MetadataExt, io::AsRawFd, prelude::RawFd};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use tracing::*;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser)]
struct Opts {
    /// The file which will be broadcast to all clients
    path: PathBuf,
    /// The port number on which to listen for new connections
    #[clap(long, short)]
    port: u16,
    /// By default tailsrv will quit when the underlying file is moved/deleted,
    /// causing any attached clients to be disconnected.  This option causes
    /// it to continue to run.
    #[clap(long)]
    linger_after_file_is_gone: bool,
    /// Send traces to journald instead of the terminal.
    #[cfg(feature = "tracing-journald")]
    #[clap(long)]
    journald: bool,
    /// How to intepret the header.  Can be one of "bytes", "lines", or
    /// "nulls".
    #[clap(long, default_value = "bytes")]
    mode: Mode,
}

#[derive(Debug, Copy, Clone)]
enum Mode {
    Bytes,
    Lines,
    Nulls,
}

impl FromStr for Mode {
    type Err = String;
    fn from_str(x: &str) -> Result<Mode, String> {
        match x {
            "bytes" => Ok(Mode::Bytes),
            "lines" => Ok(Mode::Lines),
            "nulls" => Ok(Mode::Nulls),
            _ => Err(format!("{x}: Unknown mode")),
        }
    }
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

static FILE_LENGTH: AtomicU64 = AtomicU64::new(0);
static FILE_FD: OnceCell<RawFd> = OnceCell::new();

fn main() -> Result<()> {
    let opts = Opts::parse();

    let subscriber = tracing_subscriber::registry().with(
        EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy(),
    );

    #[cfg(feature = "tracing-journald")]
    if opts.journald {
        subscriber.with(tracing_journald::layer()?).init()
    } else {
        subscriber.with(tracing_subscriber::fmt::layer()).init();
    }
    #[cfg(not(feature = "tracing-journald"))]
    subscriber.with(tracing_subscriber::fmt::layer()).init();

    // Bind the listener first, so clients can start connecting immediately.
    // It's fine for them to connect even before the file exists; of course,
    // they won't recieve any data until it _does_ exist.
    let threads = bind_listener(opts.port, opts.mode)?;
    let wake_all_clients = || {
        for t in threads.lock().unwrap().iter() {
            t.unpark();
        }
    };

    // We're ready to accept clients now; let systemd know it can start them
    #[cfg(feature = "sd-notify")]
    sd_notify::notify(true, &[sd_notify::NotifyState::Ready])?;

    // Now we wait until the file exists
    let file = wait_for_file(&opts.path)?;

    // Initialise tailsrv's state
    FILE_FD
        .set(file.as_raw_fd())
        .map_err(|_| "Set FILE_FD twice")?;

    let file_len = file.metadata()?.len();
    FILE_LENGTH.store(file_len, Ordering::SeqCst);
    info!("Initial file size: {}kB", file_len / 1024);

    // Wake up any clients who were waiting for the file to exist
    wake_all_clients();

    // Set up the inotify watch
    let mut inotify = Inotify::init()?;
    inotify.add_watch(
        &opts.path,
        WatchMask::MODIFY | WatchMask::MOVE_SELF | WatchMask::ATTRIB,
    )?;
    info!("Created an inotify watch");

    // Monitor the file and wake up clients when it changes
    loop {
        let mut buf = [0; 1024];
        let events = inotify.read_events_blocking(&mut buf).unwrap();
        for ev in events {
            if ev.mask.intersects(EventMask::MOVE_SELF) {
                info!("File was moved");
                if !opts.linger_after_file_is_gone {
                    std::process::exit(0);
                }
            } else if ev.mask.intersects(EventMask::ATTRIB) {
                // The DELETE_SELF event only occurs when the file is unlinked and all FDs are
                // closed.  Since tailsrv itself keeps an FD open, this means we never recieve
                // DELETE_SELF events.  Instead we have to rely on the ATTRIB event which occurs
                // when the user unlinks the file (and at other times too).
                if file.metadata()?.nlink() == 0 {
                    info!("File was deleted");
                    if !opts.linger_after_file_is_gone {
                        std::process::exit(0);
                    }
                }
            } else if ev.mask.contains(EventMask::MODIFY) {
                let file_len = file.metadata().unwrap().len();
                trace!("New file size: {}", file_len);
                FILE_LENGTH.store(file_len, Ordering::SeqCst);
                wake_all_clients();
            } else {
                warn!("Unknown inotify event: {ev:?}");
            }
        }
    }
}

/// Bind the socket and start listening for client connections
fn bind_listener(port: u16, mode: Mode) -> Result<Arc<Mutex<Vec<Thread>>>> {
    let listen_addr = SocketAddr::new([0, 0, 0, 0].into(), port);
    let _g = info_span!("listener", addr = %listen_addr).entered();

    let threads: Arc<Mutex<Vec<Thread>>> = Arc::new(Mutex::new(vec![]));

    let listener = TcpListener::bind(&listen_addr)?;
    info!("Bound socket");

    let threads2 = threads.clone();
    std::thread::spawn(move || listen_for_clients(listener, threads2, mode));
    info!("Handling client connections");

    Ok(threads)
}

/// Wait until the file exists and open it.  If it already exists then this
/// returns immediately.  If not, we just poll every few seconds.  I don't
/// think it's important to be extremely promt here.
fn wait_for_file(path: &Path) -> Result<File> {
    let _g = info_span!("file", path = %path.display()).entered();
    let file = loop {
        match File::open(path) {
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
        return Err(format!("{}: Not a file", path.display()).into());
    }
    info!("Opened file");
    Ok(file)
}

fn listen_for_clients(listener: TcpListener, threads: Arc<Mutex<Vec<Thread>>>, mode: Mode) {
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
            match handle_client(conn, mode) {
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

fn read_header(conn: &mut TcpStream, mode: Mode) -> Result<i64> {
    // Read the header
    let mut buf = String::new();
    std::io::BufReader::new(conn).read_line(&mut buf)?;
    // TODO: timeout
    // TODO: length limit

    // Parse the header (it's just a signed int)
    let header: i64 = buf.as_str().trim().parse()?;

    // Resolve the header to a byte offset
    match mode {
        Mode::Bytes => byte_to_offset(header),
        Mode::Lines => line_to_offset(header),
        Mode::Nulls => null_to_offset(header),
    }
}

fn byte_to_offset(header: i64) -> Result<i64> {
    if header >= 0 {
        Ok(header)
    } else {
        let cur_len = FILE_LENGTH.load(Ordering::SeqCst);
        Ok(i64::try_from(cur_len)? + header)
    }
}
fn line_to_offset(header: i64) -> Result<i64> {
    Err("Not implemented yet".into())
}
fn null_to_offset(header: i64) -> Result<i64> {
    Err("Not implemented yet".into())
}

fn handle_client(mut conn: TcpStream, mode: Mode) -> Result<()> {
    info!("Connected");
    // The first thing the client will do is send a header
    let mut offset = read_header(&mut conn, mode)?;
    info!("Starting from offset {}", offset);
    // Spawn a thread to read (and discard) anything send by the client.
    // This is so that clients can use writability as way to test whether
    // their connection is alive.
    let mut conn2 = conn.try_clone()?;
    std::thread::spawn(move || std::io::copy(&mut conn2, &mut std::io::sink()));
    // Send file data to the client; sleep until the file grows; repeat.
    loop {
        // How many bytes the client wants
        let file_len = FILE_LENGTH.load(Ordering::SeqCst) as usize;
        let wanted = file_len.saturating_sub(offset as usize);
        if wanted == 0 {
            // We're all caught-up.  Wait for new data to be written
            // to the file before continuing.
            trace!("Waiting for changes");
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
            trace!("Sending {wanted} bytes from offset {offset}");
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
