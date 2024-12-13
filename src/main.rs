use bpaf::{Bpaf, Parser};
use rustix::event::EventfdFlags;
use rustix::fd::{AsRawFd, OwnedFd};
use rustix::fs::inotify;
use rustix::io::Errno;
use rustix_uring::IoUring;
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::BufRead;
use std::mem::MaybeUninit;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use tracing::*;
use tracing_subscriber::{prelude::*, EnvFilter};

pub const FLAG_POLLIN: u32 = 0x1;

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
static CLIENTS: Mutex<BTreeMap<u16, Client>> = Mutex::new(BTreeMap::new());
static EVENTFD: LazyLock<OwnedFd> =
    LazyLock::new(|| rustix::event::eventfd(0, EventfdFlags::NONBLOCK).unwrap());

fn main() -> Result<()> {
    let opts = opts().run();
    log_init(
        #[cfg(feature = "tracing-journald")]
        opts.journald,
    );

    let mut uring = IoUring::new(256)?;
    info!("Set up the io_uring");

    info!(fd = EVENTFD.as_raw_fd(), "Created an eventfd");
    let poll_eventfd = rustix_uring::opcode::PollAdd::new(
        rustix_uring::types::Fd(EVENTFD.as_raw_fd()),
        FLAG_POLLIN,
    )
    .multi(true)
    .build()
    .user_data(UserData::NewClient.into());
    unsafe { uring.submission().push(&poll_eventfd)? };
    info!("Polling the eventfd for events");

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

    let file_len = usize::try_from(file.metadata()?.len())?;
    FILE_LENGTH.store(file_len, Ordering::Release);
    info!("Initial file size: {} kiB", file_len / 1024);

    uring.submitter().register_files(&[file.as_raw_fd()])?;
    let file_fd = rustix_uring::types::Fixed(0);
    info!(?file_fd, "Registered file with the io_uring");

    // Set up the inotify watch
    let ino_fd = inotify::init(inotify::CreateFlags::NONBLOCK)?;
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

    let poll_ino = rustix_uring::opcode::PollAdd::new(
        rustix_uring::types::Fd(ino_fd.as_raw_fd()),
        FLAG_POLLIN,
    )
    .multi(true)
    .build()
    .user_data(UserData::Inotify.into());
    unsafe { uring.submission().push(&poll_ino)? };
    info!("Polling the inotify watch for events");

    info!("Starting runloop");
    let mut reqs = VecDeque::new();
    loop {
        issue_requests(&mut reqs, &mut uring, file_fd)?;
        trace!("Waiting for wake-ups");
        uring.submit_and_wait(1)?;
        trace!("Woke up!");
        handle_completions(&mut uring, &file, &ino_fd, opts.linger_after_file_is_gone)?;
    }
}

fn issue_requests(
    reqs: &mut VecDeque<rustix_uring::squeue::Entry>,
    uring: &mut IoUring,
    file_fd: rustix_uring::types::Fixed,
) -> Result<()> {
    let file_len = FILE_LENGTH.load(Ordering::Acquire);
    for (&client_id, client) in CLIENTS.lock().unwrap().iter_mut() {
        if client.in_flight {
            // Nothing to do
        } else if client.bytes_in_pipe > 0 {
            trace!("Payload only partially delivered. Retrying...");
            reqs.push_back(drain_pipe(client_id, client));
        } else if client.offset < file_len {
            trace!(
                client_id,
                file_len,
                offset = client.offset,
                "Filling and draining the pipe"
            );
            // Why fill and drain a pipe?
            //
            // There's no sendfile() opcode for io_uring (yet).  However,
            // we can emulate it by splicing once from the file to a pipe,
            // and then again from the pipe to the socket.  This is exactly
            // how sendfile() works under the hood, so there should be no
            // performance impact from this.
            let fill = fill_pipe(client_id, client, file_fd);
            let drain = drain_pipe(client_id, client);
            // Why IO_HARDLINK, not just IO_LINK?
            //
            // We're asking the kernel to splice u32::MAX bytes from
            // the file into the pipe.  This is certainly going to
            // fail - the kernel will splice in at most u16::MAX bytes,
            // possibly less (even if there are more bytes than this
            // waiting in the file). It's ok though - the kernel will
            // splice as much data as it can into the pipe and tell us
            // how much it managed.  That's what we want.
            //
            // However, if we used IO_LINK here then the second splice
            // (pipe -> socket) would be cancelled.  That's not what we
            // want!  IO_HARDLINK means "sequence these requests, but
            // don't cancel the second if the first fails".
            let fill = fill.flags(rustix_uring::squeue::Flags::IO_HARDLINK);
            reqs.extend([fill, drain]);
            client.in_flight = true;
        }
    }
    trace!("Pushing {} reqs to the ring:", reqs.len());
    while let Some(req) = reqs.front() {
        let is_full = unsafe { uring.submission().push(req) }.is_err();
        if is_full {
            trace!("Queue is full; submit and retry");
            uring.submit()?;
        } else {
            trace!(">> {req:?}");
            reqs.pop_front();
        }
    }
    Ok(())
}

fn fill_pipe(
    client_id: u16,
    client: &Client,
    file_fd: rustix_uring::types::Fixed,
) -> rustix_uring::squeue::Entry {
    rustix_uring::opcode::Splice::new(
        file_fd,
        i64::try_from(client.offset).unwrap(),
        rustix_uring::types::Fd(client.pipe_wtr.as_raw_fd()),
        -1,
        u32::MAX,
    )
    .build()
    .user_data(UserData::FillPipe(client_id).into())
}

fn drain_pipe(client_id: u16, client: &Client) -> rustix_uring::squeue::Entry {
    rustix_uring::opcode::Splice::new(
        rustix_uring::types::Fd(client.pipe_rdr.as_raw_fd()),
        -1,
        rustix_uring::types::Fd(client.conn.as_raw_fd()),
        -1,
        u32::MAX,
    )
    .build()
    .user_data(UserData::DrainPipe(client_id).into())
}

fn handle_completions(
    uring: &mut IoUring,
    file: &File,
    ino_fd: &OwnedFd,
    linger: bool,
) -> Result<()> {
    for cqe in uring.completion() {
        let user_data = UserData::try_from(cqe.user_data())?;
        let result = cqe.result();
        let result = usize::try_from(result).map_err(|_| Errno::from_raw_os_error(-result));
        trace!("io_uring completion: {:?}: {:?}", user_data, result);
        match (user_data, result) {
            (UserData::NewClient, Ok(_)) => {
                trace!("New client");
                assert!(cqe.flags().contains(rustix_uring::cqueue::Flags::MORE));
                let mut buf = [0; 8];
                match rustix::io::read(&*EVENTFD, &mut buf) {
                    Ok(8) | Err(Errno::AGAIN) => {
                        let x = u64::from_ne_bytes(buf);
                        trace!("Received notification of {x} new clients");
                    }
                    Ok(x) => error!("Incomplete read: {x}"),
                    Err(e) => error!("{e}"),
                }
            }
            (UserData::Inotify, Ok(_)) => {
                assert!(cqe.flags().contains(rustix_uring::cqueue::Flags::MORE));
                let mut buf = [const { MaybeUninit::uninit() }; 1024];
                let mut evs = inotify::Reader::new(&ino_fd, &mut buf);
                loop {
                    match evs.next() {
                        Ok(ev) => handle_file_event(ev, file, linger)?,
                        Err(Errno::AGAIN) => break,
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            (UserData::NewClient | UserData::Inotify, Err(e)) => error!("{e}"),
            (UserData::FillPipe(client_id), Ok(n_copied)) => {
                let _g = info_span!("", client_id).entered();
                trace!("Filled pipe with {} bytes", n_copied);
                assert!(n_copied != 0);
                let mut clients = CLIENTS.lock().unwrap();
                let client = clients.get_mut(&client_id).unwrap();
                client.bytes_in_pipe += n_copied;
            }
            (UserData::DrainPipe(client_id), Ok(n_sent)) => {
                let _g = info_span!("", client_id).entered();
                trace!("Sent {} bytes to client", n_sent);
                let mut clients = CLIENTS.lock().unwrap();
                let client = clients.get_mut(&client_id).unwrap();
                client.bytes_in_pipe -= n_sent;
                client.offset += n_sent;
                client.in_flight = false;
            }
            (UserData::FillPipe(client_id) | UserData::DrainPipe(client_id), Err(e)) => {
                let _g = info_span!("", client_id).entered();
                match e {
                    Errno::PIPE | Errno::CONNRESET => info!("Socket closed by other side"),
                    _ => error!("{e}"),
                }
                CLIENTS.lock().unwrap().remove(&client_id);
            }
        }
    }
    Ok(())
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
    }
    Ok(())
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
        let (conn, client_id) = match conn.and_then(|c| {
            let port = c.peer_addr()?.port();
            Ok((c, port))
        }) {
            Ok(x) => x,
            Err(e) => {
                error!("Bad connection: {e}");
                continue;
            }
        };
        std::thread::spawn(move || {
            let _g = info_span!("", client_id).entered();
            match Client::new(conn) {
                Ok(client) => {
                    trace!("Prepared client: {client:?}");
                    CLIENTS.lock().unwrap().insert(client_id, client);
                    rustix::io::write(&*EVENTFD, &1u64.to_ne_bytes()).unwrap();
                    trace!("Wrote to eventfd");
                }
                Err(e) => error!("{e}"),
            }
        });
    }
    error!("Listening socket was closed!");
    std::process::exit(1);
}

#[derive(Debug)]
struct Client {
    conn: TcpStream,
    offset: usize,
    bytes_in_pipe: usize,
    in_flight: bool,
    pipe_rdr: OwnedFd,
    pipe_wtr: OwnedFd,
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

        let (pipe_rdr, pipe_wtr) = rustix::pipe::pipe()?;
        Ok(Client {
            conn,
            offset,
            bytes_in_pipe: 0,
            in_flight: false,
            pipe_rdr,
            pipe_wtr,
        })
    }
}

#[derive(Debug)]
enum UserData {
    NewClient,
    Inotify,
    FillPipe(u16),
    DrainPipe(u16),
}
const FILL_FROM: u64 = 100_000;
const FILL_TO: u64 = FILL_FROM + u16::MAX as u64;
const DRAIN_FROM: u64 = 200_000;
const DRAIN_TO: u64 = DRAIN_FROM + u16::MAX as u64;
impl From<UserData> for u64 {
    fn from(value: UserData) -> Self {
        match value {
            UserData::NewClient => 0,
            UserData::Inotify => 1,
            UserData::FillPipe(port) => u64::from(port) + FILL_FROM,
            UserData::DrainPipe(port) => u64::from(port) + DRAIN_FROM,
        }
    }
}
impl TryFrom<u64> for UserData {
    type Error = Box<dyn std::error::Error>;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(UserData::NewClient),
            1 => Ok(UserData::Inotify),
            FILL_FROM..FILL_TO => Ok(UserData::FillPipe(
                u16::try_from(value - FILL_FROM).unwrap(),
            )),
            DRAIN_FROM..DRAIN_TO => Ok(UserData::DrainPipe(
                u16::try_from(value - DRAIN_FROM).unwrap(),
            )),
            _ => Err(format!("Unknown user data: {value}").into()),
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
    if journald {
        let subscriber = subscriber.with(tracing_journald::layer().unwrap());
        return subscriber.init();
    }

    let layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let subscriber = subscriber.with(layer);
    subscriber.init();
}
