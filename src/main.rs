use clap::Parser;
use inotify::*;
use log::*;
use std::{
    convert::TryFrom,
    env::*,
    fs::File,
    net::SocketAddr,
    os::unix::{io::AsRawFd, prelude::RawFd},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tokio::io::{unix::AsyncFd, AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::watch;

#[derive(Parser)]
struct Opts {
    /// The file which will be broadcast to all clients
    path: PathBuf,
    /// The port number on which to listen for new connections
    #[clap(long, short)]
    port: u16,
    /// Don't produce output unless there's a problem
    #[clap(long, short)]
    quiet: bool,
    /// Line delimiter is NUL, not newline
    #[clap(long, short)]
    zero_terminated: bool,
    /// Use the binary protocol instead of text
    #[clap(long, short)]
    binary_proto: bool,
}

pub static FILE_LENGTH: AtomicU64 = AtomicU64::new(0);

#[tokio::main]
async fn main() {
    // Define CLI options
    let opts = Opts::parse();

    // Init logger
    let log_level = if opts.quiet {
        log::Level::Warn
    } else {
        log::Level::Info
    };
    loggerv::init_with_level(log_level).unwrap();

    let file = File::open(&opts.path).unwrap();
    let file_fd = file.as_raw_fd();
    let file_len = file.metadata().unwrap().len();
    FILE_LENGTH.store(file_len, Ordering::SeqCst);
    let (tx, rx) = watch::channel::<()>(());
    let mut inotify = Inotify::init().unwrap();
    inotify
        .add_watch(
            &opts.path,
            WatchMask::MODIFY | WatchMask::DELETE_SELF | WatchMask::MOVE_SELF,
        )
        .unwrap();

    {
        // Start the file-watching task
        let inotify_fd = AsyncFd::new(inotify.as_raw_fd()).unwrap();
        let mut inotify_buf = vec![0; 4096];
        tokio::task::spawn(async move {
            loop {
                let mut guard = inotify_fd.readable().await.unwrap();
                for ev in inotify.read_events(&mut inotify_buf).unwrap() {
                    if ev.mask.contains(EventMask::MODIFY) {
                        let file_len = file.metadata().unwrap().len();
                        info!("{:?}: File length is now {}", ev.wd, file_len);
                        FILE_LENGTH.store(file_len, Ordering::SeqCst);
                        tx.send(()).unwrap();
                    } else if ev.mask.contains(EventMask::DELETE_SELF)
                        || ev.mask.contains(EventMask::MOVE_SELF)
                    {
                        info!("Watched file disappeared");
                        std::process::exit(0);
                    }
                }
                guard.clear_ready();
            }
        });
    }

    let listen_addr = SocketAddr::new([0, 0, 0, 0].into(), opts.port);
    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .expect("Bind listen sock");
    info!(
        "Serving files from {} on {}",
        current_dir().unwrap().display(),
        listen_addr
    );
    loop {
        let (sock, addr) = listener.accept().await.unwrap();
        info!("{}: New client connected", addr);
        tokio::task::spawn(handle_client(opts.binary_proto, sock, file_fd, rx.clone()));
    }
}

async fn handle_client(
    binary_proto: bool,
    mut sock: TcpStream,
    fd: RawFd,
    mut rx: watch::Receiver<()>,
) {
    // The first thing the client will do is send a header
    // TODO: timeout
    let idx = if binary_proto {
        use tokio::io::AsyncReadExt;
        sock.read_i64().await.unwrap()
    } else {
        // TODO: length limit
        let mut buf = String::new();
        tokio::io::BufReader::new(sock).read_line(&mut buf).await?;
        info!("Client sent header bytes {:?}", &buf);
        buf.as_str().trim().parse()?.unwrap()
    };
    info!("Client sent header {:?}", idx);
    let initial_offset = if header >= 0 {
        Ok(header)
    } else {
        let cur_len = i64::try_from(FILE_LENGTH.load(Ordering::SeqCst))?;
        Ok(cur_len - header.neg())
    };

    let mut offset = initial_offset;
    loop {
        sock.writable().await.unwrap();
        info!("Socket has become writable");
        // How many bytes the client wants
        let file_len = FILE_LENGTH.load(Ordering::SeqCst);
        let wanted = i64::try_from(file_len).unwrap() - offset;
        if wanted <= 0 {
            // We're all caught-up.  Wait for new data to be written
            // to the file before continuing.
            info!("Waiting for changes");
            match rx.changed().await {
                Ok(()) => continue,
                Err(_) => {
                    // The sender is gone.  This means that the file has
                    // been deleted.
                    info!("Closing socket: file was deleted");
                    return;
                }
            }
        }

        /// The maximum number of bytes which will be `sendfile()`'d to a client before moving onto the
        /// next waiting client.
        ///
        /// A bigger size increases total throughput, but may allow a client who is reading a lot of data
        /// to hurt reaction latency for other clients.
        const CHUNK_SIZE: i64 = 1024 * 1024;
        // How many bytes the client will get
        let cnt = usize::try_from(wanted.min(CHUNK_SIZE)).unwrap();

        info!("Sending {} bytes from offset {}", cnt, offset);
        let ret = sock.try_io(tokio::io::Interest::WRITABLE, || {
            nix::sys::sendfile::sendfile(sock.as_raw_fd(), fd, Some(&mut offset), cnt)
                .map_err(std::io::Error::from)
        });
        if let Err(e) = ret {
            match e.kind() {
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => {
                    // The client hung up
                    info!("Socket closed by other side");
                    return;
                }
                std::io::ErrorKind::WouldBlock => {
                    // The socket is not writeable. Wait for it to become writable
                    // again before continuing.
                }
                _ => panic!("{}", e),
            }
        }
    }
}
