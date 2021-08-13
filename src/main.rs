pub mod index;

use crate::index::*;
use inotify::*;
use log::*;
use std::{
    convert::TryFrom,
    env::*,
    fs::File,
    net::SocketAddr,
    os::unix::{io::AsRawFd, prelude::RawFd},
    path::{Path, PathBuf},
};
use structopt::StructOpt;
use tokio::io::{unix::AsyncFd, AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::watch;

pub type FileLength = u64; /* bytes */

#[derive(StructOpt)]
struct Opts {
    /// The file which will be broadcast to all clients
    path: PathBuf,
    /// The port number on which to listen for new connections
    #[structopt(long, short)]
    port: u16,
    /// Lazily maintain index files in /tmp for faster seeking
    #[structopt(long, short)]
    index: bool,
    /// Don't produce output unless there's a problem
    #[structopt(long, short)]
    quiet: bool,
}

#[tokio::main]
async fn main() {
    // Define CLI options
    let opts = Opts::from_args();
    if !file_is_valid(&opts.path) {
        error!("{}: File not valid", opts.path.display());
        std::process::exit(1);
    }

    // Init logger
    let log_level = if opts.quiet {
        log::Level::Warn
    } else {
        log::Level::Info
    };
    loggerv::init_with_level(log_level).unwrap();

    if opts.index {
        warn!("Index files are not implemented yet");
    }

    let file = File::open(&opts.path).unwrap();
    let file_fd = file.as_raw_fd();
    let file_len = file.metadata().unwrap().len();
    let (tx, rx) = watch::channel::<FileLength>(file_len);
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
                    if ev.mask.contains(EventMask::DELETE_SELF)
                        || ev.mask.contains(EventMask::MOVE_SELF)
                    {
                        info!("Watched file disappeared");
                        std::process::exit(0);
                    } else if ev.mask.contains(EventMask::MODIFY) {
                        let file_len = file.metadata().unwrap().len();
                        info!("{:?}: File length is now {}", ev.wd, file_len);
                        tx.send(file_len).unwrap();
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
        let file = File::open(&opts.path).unwrap();
        tokio::task::spawn(handle_client(file, sock, file_fd, rx.clone()));
    }
}

async fn handle_client(
    mut file: File,
    mut sock: TcpStream,
    fd: RawFd,
    mut rx: watch::Receiver<FileLength>,
) {
    // The first thing the client will do is send a header
    let idx = {
        // TODO: timeout
        // TODO: length limit
        let mut buf = String::new();
        BufReader::new(&mut sock).read_line(&mut buf).await.unwrap();
        debug!("Client sent header: {:?}", &buf);
        match parse_index(buf.as_bytes()) {
            nom::IResult::Done(_, x) => x,
            nom::IResult::Error(e) => {
                error!("Bad header: {}", buf);
                panic!("{}", e);
            }
            nom::IResult::Incomplete { .. } => {
                error!("Partial header: {}", buf);
                panic!();
            }
        }
    };
    info!("Client sent header {:?}", idx);
    // OK! This client will start watching a file. Let's remove
    // it from the nursery and change its epoll parameters.
    // TODO: If resolving returns `None`, we should re-resolve it every time there's new data.
    let initial_offset = resolve_index(&mut file, idx).expect("index").unwrap();
    let initial_offset = i64::try_from(initial_offset).unwrap();
    std::mem::drop(file);

    /// The maximum number of bytes which will be `sendfile()`'d to a client before moving onto the
    /// next waiting client.
    ///
    /// A bigger size increases total throughput, but may allow a client who is reading a lot of data
    /// to hurt reaction latency for other clients.
    const CHUNK_SIZE: usize = 1024 * 1024;

    let mut offset = initial_offset;
    loop {
        sock.writable().await.unwrap();
        info!("Socket has become writable");
        // How many bytes the client wants
        let wanted = i64::try_from(*rx.borrow()).unwrap() - offset;
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
        // How many bytes the client will get
        let cnt = wanted.min(CHUNK_SIZE as i64);
        info!("Sending {} bytes from offset {}", cnt, offset);
        let ret = sock.try_io(tokio::io::Interest::WRITABLE, || {
            nix::sys::sendfile::sendfile(sock.as_raw_fd(), fd, Some(&mut offset), cnt as usize)
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

pub fn file_is_valid(path: &Path) -> bool {
    use ignore::WalkBuilder;
    use same_file::*;
    let valid_files = WalkBuilder::new(".")
        .git_global(false) // Parsing git-related files is surprising
        .git_ignore(false) // behaviour in the context of tailsrv, so
        .git_exclude(false) // let's not read those files.
        .ignore(true) // However, we *should* read generic ".ignore" files...
        .hidden(true) // and ignore dotfiles (so clients can't read the .ignore files)
        .parents(false) // Don't search the parent directory for .ignore files.
        .build();
    for entry in valid_files {
        let entry = match entry {
            Err(e) => {
                warn!("{}", e);
                continue;
            }
            Ok(entry) => entry,
        };
        if entry.file_type().map(|x| x.is_file()).unwrap_or(false)
            && is_same_file(path, entry.path()).unwrap_or(false)
        {
            return true;
        }
    }
    false
}
