pub mod file_list;
pub mod header;
pub mod index;
pub mod pool;
pub mod types;

use crate::file_list::*;
use crate::header::*;
use crate::index::*;
use crate::pool::*;
use crate::types::*;
use inotify::*;
use log::*;
use std::os::unix::io::AsRawFd;
use std::os::unix::prelude::RawFd;
use std::{convert::TryFrom, env::*, fs::File, net::SocketAddr, path::PathBuf};
use structopt::StructOpt;
use tokio::io::{unix::AsyncFd, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::watch;

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
        tokio::task::spawn(new_client(sock, file_fd, rx.clone()));
    }
}

async fn new_client(mut sock: TcpStream, fd: RawFd, rx: watch::Receiver<FileLength>) {
    // The first thing the client will do is send a header
    let hdr = {
        // TODO: timeout
        // TODO: length limit
        let mut buf = String::new();
        BufReader::new(&mut sock).read_line(&mut buf).await.unwrap();
        debug!("Client sent header: {:?}", &buf);
        match header(buf.as_bytes()) {
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
    info!("Client sent header {:?}", hdr);
    match hdr {
        Header::List => {
            // Listing files could be expensive, let's do it in this thread.
            sock.write_all(list_files().unwrap().as_bytes())
                .await
                .unwrap();
        }
        Header::Stream { path, index } => {
            if file_is_valid(&path) {
                // OK! This client will start watching a file. Let's remove
                // it from the nursery and change its epoll parameters.
                // TODO: If resolving returns `None`, we should re-resolve it every time there's new data.
                let mut file = File::open(&path).unwrap();
                let offset = resolve_index(&mut file, index).expect("index").unwrap();
                let offset = i64::try_from(offset).unwrap();
                // This is long-running:
                client_task(sock, offset, fd, rx).await;
            } else {
                warn!("Client tried to access {:?} but isn't allowed", path);
            }
        }
        Header::Stats => {
            if sock.peer_addr().unwrap().ip().is_loopback() {
                sock.write_all(b"TODO\n").await.unwrap();
            } else {
                warn!("Client requested stats but isn't localhost");
            }
        }
    }
}
