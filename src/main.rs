pub mod file_list;
pub mod header;
pub mod index;
pub mod pool;
pub mod types;

use crate::file_list::*;
use crate::header::*;
use crate::index::*;
use crate::pool::*;
use log::*;
use std::os::unix::io::AsRawFd;
use std::{
    convert::TryFrom,
    env::*,
    fs::File,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use structopt::StructOpt;
use tokio::io::{unix::AsyncFd, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[derive(StructOpt)]
struct Opts {
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

    let pool = Arc::new(Mutex::new(WatcherPool::new()));

    {
        // Start the file-watching task
        let pool = pool.clone();
        let inotify_fd = AsyncFd::new(pool.lock().unwrap().inotify.as_raw_fd()).unwrap();
        tokio::task::spawn(async move {
            loop {
                let mut guard = inotify_fd.readable().await.unwrap();
                pool.lock().unwrap().update_all().unwrap();
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
        tokio::task::spawn(new_client(sock, pool.clone()));
    }
}

async fn new_client(mut sock: TcpStream, pool: Arc<Mutex<WatcherPool>>) {
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
                let (file, rx) = pool.lock().unwrap().register_client(&path).unwrap();
                // This is long-running:
                client_task(sock, offset, file, rx).await;
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
