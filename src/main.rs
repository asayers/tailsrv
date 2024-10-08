use bpaf::{Bpaf, Parser};
use rustix::fs::inotify;
use rustix::io::Errno;
use std::fs::File;
use std::io::BufRead;
use std::mem::MaybeUninit;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::Thread;
use tracing::*;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Bpaf)]
struct Opts {
    /// The port number on which to listen for new connections
    #[bpaf(long, short, argument("PORT"))]
    port: u16,
    /// By default tailsrv will quit when the underlying file is moved/deleted,
    /// causing any attached clients to be disconnected.  This option causes
    /// it to continue to run.
    linger_after_file_is_gone: bool,
    /// Send traces to journald instead of the terminal.
    #[cfg(feature = "tracing-journald")]
    journald: bool,
    /// The file which will be broadcast to all clients
    #[bpaf(positional("PATH"))]
    path: PathBuf,
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

static FILE_LENGTH: AtomicUsize = AtomicUsize::new(0);
static FILE: OnceLock<File> = OnceLock::new();
static CLIENT_THREADS: Mutex<Vec<Thread>> = Mutex::new(vec![]);

fn main() -> Result<()> {
    let opts = opts().run();
    log_init(
        #[cfg(feature = "tracing-journald")]
        opts.journald,
    );

    // Bind the listener socket.  We do this ASAP, so clients can start
    // connecting immediately. It's fine for them to connect even before the
    // file exists.  Of course, they won't recieve any data until it _does_
    // exist.
    let listen_addr = SocketAddr::new([0, 0, 0, 0].into(), opts.port);
    let listener = TcpListener::bind(listen_addr)?;
    info!(%listen_addr, "Bound socket");

    // Handle incoming client connections in a separate thread
    std::thread::spawn(move || listen_for_clients(listener));

    // We're ready to accept clients now; let systemd know it can start them
    #[cfg(feature = "sd-notify")]
    sd_notify::notify(true, &[sd_notify::NotifyState::Ready])?;

    // Now we wait until the file exists
    let file = wait_for_file(&opts.path)?;

    // Initialise tailsrv's state
    FILE.set(file).map_err(|_| "Set FILE twice")?;
    let file = FILE.get().unwrap();

    let file_len = usize::try_from(file.metadata()?.len())?;
    FILE_LENGTH.store(file_len, Ordering::Release);
    info!("Initial file size: {} kiB", file_len / 1024);

    // Wake up any clients who were waiting for the file to exist
    wake_all_clients();

    // Set up the inotify watch
    let ino_fd = inotify::init(inotify::CreateFlags::empty())?;
    inotify::add_watch(
        &ino_fd,
        &opts.path,
        inotify::WatchFlags::MODIFY | inotify::WatchFlags::MOVE_SELF | inotify::WatchFlags::ATTRIB,
    )?;
    info!(
        path = %opts.path.display(),
        fd = ino_fd.as_raw_fd(),
        "Created an inotify watch",
    );

    // Monitor the file and wake up clients when it changes
    info!("Starting runloop");
    let mut buf = [const { MaybeUninit::uninit() }; 1024];
    let mut evs = inotify::Reader::new(&ino_fd, &mut buf);
    loop {
        let ev = evs.next()?;
        handle_file_event(ev, file, opts.linger_after_file_is_gone)?;
    }
}

fn handle_file_event(ev: inotify::InotifyEvent, file: &File, linger: bool) -> Result<()> {
    trace!("inotify event: {:?}", ev);
    if ev.events().contains(inotify::ReadFlags::MOVE_SELF) {
        info!("File was moved");
        if !linger {
            std::process::exit(0);
        }
    }
    if ev.events().contains(inotify::ReadFlags::ATTRIB) {
        // The DELETE_SELF event only occurs when the file is unlinked and all FDs are
        // closed.  Since tailsrv itself keeps an FD open, this means we never recieve
        // DELETE_SELF events.  Instead we have to rely on the ATTRIB event which occurs
        // when the user unlinks the file (and at other times too).
        if file.metadata()?.nlink() == 0 {
            info!("File was deleted");
            if !linger {
                std::process::exit(0);
            }
        }
    }
    if ev.events().contains(inotify::ReadFlags::MODIFY) {
        let file_len = usize::try_from(file.metadata().unwrap().len())?;
        trace!("New file size: {}", file_len);
        FILE_LENGTH.store(file_len, Ordering::Release);
        wake_all_clients();
    }
    Ok(())
}

fn wake_all_clients() {
    for t in CLIENT_THREADS.lock().unwrap().iter() {
        t.unpark();
    }
}

/// Wait until the file exists and open it.  If it already exists then this
/// returns immediately.  If not, we just poll every few seconds.  I don't
/// think it's important to be extremely prompt here.
fn wait_for_file(path: &Path) -> Result<File> {
    let _g = info_span!("", path = %path.display()).entered();
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

fn listen_for_clients(listener: TcpListener) {
    for conn in listener.incoming() {
        let conn = match conn {
            Ok(x) => x,
            Err(e) => {
                error!("Bad connection: {e}");
                continue;
            }
        };
        let join_handle = std::thread::spawn(move || {
            let client_id = conn.peer_addr().ok().map(|x| x.port());
            let _g = info_span!("", client_id).entered();
            match Client::new(conn) {
                Ok(client) => {
                    trace!("Prepared client: {client:?}");
                    match client.run() {
                        Ok(()) => (),
                        Err(e) => error!("{e}"),
                    }
                }
                Err(e) => error!("{e}"),
            }
            let this_thread = std::thread::current().id();
            CLIENT_THREADS
                .lock()
                .unwrap()
                .retain(|t| t.id() != this_thread);
            info!("Cleaned up the thread");
        });
        CLIENT_THREADS
            .lock()
            .unwrap()
            .push(join_handle.thread().clone());
    }
    error!("Listening socket was closed!");
    std::process::exit(1);
}

#[derive(Debug)]
struct Client {
    conn: TcpStream,
    offset: usize,
}

impl Client {
    fn new(mut conn: TcpStream) -> Result<Client> {
        info!("Connected");
        // The first thing the client will do is send a header
        // TODO: timeout
        // TODO: length limit
        let mut buf = String::new();
        std::io::BufReader::new(&mut conn).read_line(&mut buf)?;

        // Parse the header (it's just a signed int)
        let header: isize = buf.as_str().trim().parse()?;

        // Resolve the header to a byte offset
        let offset = match usize::try_from(header) {
            Ok(x) => x,
            Err(_) => {
                let cur_len = FILE_LENGTH.load(Ordering::Acquire);
                cur_len.saturating_add_signed(header)
            }
        };
        info!("Starting from initial offset {offset}");

        Ok(Client { conn, offset })
    }

    /// Send file data to the client; sleep until the file grows; repeat.
    fn run(&mut self) -> Result<()> {
        loop {
            // How many bytes the client wants
            let file_len = FILE_LENGTH.load(Ordering::Acquire);
            let wanted = file_len.saturating_sub(offset);
            if wanted == 0 {
                // We're all caught-up.  Wait for new data to be written
                // to the file before continuing.
                trace!("Waiting for changes");
                std::thread::park();
            } else {
                let Some(file) = FILE.get() else {
                    error!("FILE_LENGTH is {file_len}, but FILE isn't set yet. This is a bug.");
                    continue;
                };
                trace!("Sending {wanted} bytes from offset {offset}");
                let ret = rustix::fs::sendfile(conn, file, Some(&mut offset), wanted);
                match ret {
                    Ok(_) => (),
                    Err(Errno::PIPE | Errno::CONNRESET) => {
                        // The client hung up
                        info!("Socket closed by other side");
                        return Ok(());
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
}

fn log_init(#[cfg(feature = "tracing-journald")] journald: bool) {
    let subscriber = tracing_subscriber::registry();

    // Respect RUST_LOG, falling back to INFO
    let filter = EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .from_env_lossy();
    let subscriber = subscriber.with(filter);

    #[cfg(feature = "tracing-journald")]
    if opts.journald {
        let subscriber = subscriber.with(tracing_journald::layer()?);
        return subscriber.init();
    }

    let layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let subscriber = subscriber.with(layer);
    subscriber.init();
}
