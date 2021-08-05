pub mod file_list;
pub mod header;
pub mod index;
pub mod pool;
pub mod types;

use crate::file_list::*;
use crate::header::*;
use crate::index::*;
use crate::pool::*;
use inotify::*;
use log::*;
use mio_more::channel as mio_chan;
use std::env::*;
use std::fs::File;
use std::io::prelude::*;
use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::usize;
use structopt::StructOpt;

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

fn main() {
    // Define CLI options
    let opts = Opts::from_args();

    // Init logger
    let log_level = if opts.quiet {
        LogLevel::Warn
    } else {
        LogLevel::Info
    };
    loggerv::init_with_level(log_level).unwrap();

    if opts.index {
        warn!("Index files are not implemented yet");
    }

    // Init epoll, allocate buffer for epoll events
    // const MAX_CLIENTS: usize  = 1024;
    const EPOLL_LISTENER: mio::Token = mio::Token(usize::MAX - 1);
    const EPOLL_NEW_CLIENT: mio::Token = mio::Token(usize::MAX - 2);
    const EPOLL_INOTIFY: mio::Token = mio::Token(usize::MAX - 3);
    const EPOLL_WORK: mio::Token = mio::Token(usize::MAX - 4);
    let poll = mio::Poll::new().unwrap();
    let mut mio_events = mio::Events::with_capacity(1024);

    // Init inotify and register the inotify fd with epoll
    let inotify = Inotify::init().unwrap();
    poll.register(
        &inotify,
        EPOLL_INOTIFY,
        mio::Ready::readable(),
        mio::PollOpt::level(),
    )
    .unwrap();

    // Bind the listen socket and register it with epoll
    let inaddr_any = "0.0.0.0".parse().unwrap();
    let listen_addr = SocketAddr::new(inaddr_any, opts.port);
    let listener = mio::net::TcpListener::bind(&listen_addr).expect("Bind listen sock");
    poll.register(
        &listener,
        EPOLL_LISTENER,
        mio::Ready::readable(),
        mio::PollOpt::level(),
    )
    .unwrap();

    let (new_clients_tx, new_clients_rx) = mio_chan::channel();
    poll.register(
        &new_clients_rx,
        EPOLL_NEW_CLIENT,
        mio::Ready::readable(),
        mio::PollOpt::level(),
    )
    .unwrap();

    let (work_tx, work_rx) = mio_chan::channel();
    poll.register(
        &work_rx,
        EPOLL_WORK,
        mio::Ready::readable(),
        mio::PollOpt::level(),
    )
    .unwrap();

    // If the client sends a "stream" header, it is then moved to the pool, which tracks which
    // clients are interested in which files.
    let mut pool = WatcherPool::new(inotify);

    // Enter runloop
    info!(
        "Serving files from {:?} on {}",
        current_dir().unwrap(),
        listen_addr
    );
    loop {
        // Wait for something to happen
        poll.poll(&mut mio_events, None).unwrap();
        for mio_event in mio_events.iter() {
            match mio_event.token() {
                EPOLL_LISTENER => {
                    // The listen socket is readable => a new client is trying to connect
                    let (sock, _) = listener.accept_std().unwrap();
                    info!("Client connected. Waiting for it to send a header...");
                    // The first thing the client will do is send a header
                    let new_clients_tx = new_clients_tx.clone();
                    thread::spawn(move || foobar(sock, new_clients_tx));
                }
                EPOLL_NEW_CLIENT => {
                    let (sock, path, offset) = new_clients_rx.try_recv().unwrap();
                    let cid = {
                        let entry = pool.socks.vacant_entry();
                        let cid = entry.key();
                        let sock = mio::net::TcpStream::from_stream(sock).unwrap();
                        poll.register(
                            &sock,
                            mio::Token(cid),
                            mio::Ready::writable(),
                            mio::PollOpt::edge(),
                        )
                        .unwrap();
                        entry.insert(sock);
                        cid
                    };
                    // And then we put it in the pool. This function also
                    // handles setting up inotify watches etc.
                    pool.register_client(cid, &path, offset).unwrap();
                }
                EPOLL_WORK => {
                    let cid = work_rx.try_recv().unwrap();
                    let requeue = pool.handle_client(cid).unwrap();
                    if requeue {
                        work_tx.send(cid).unwrap();
                    }
                }
                EPOLL_INOTIFY => {
                    // The inotify FD is readable => a watched file has been modified
                    info!("Watched files have been modified");
                    // First, mark all clients interested in modifed files as dirty.
                    for cid in pool.check_watches().unwrap() {
                        work_tx.send(cid).unwrap();
                    }
                }
                mio::Token(cid) => {
                    if mio_event.readiness().is_writable() {
                        // A client in the pool has become writable => send some data
                        info!("Client {} has become writable", cid);
                        work_tx.send(cid).unwrap();
                    }
                }
            }
        }
    }
}

fn foobar(sock: TcpStream, chan: mio_chan::Sender<(TcpStream, PathBuf, usize)>) {
    let mut buf = String::new();
    let mut sock = BufReader::new(sock);
    // TODO: timeout
    // TODO: length limit
    sock.read_line(&mut buf).unwrap();
    debug!("Client sent header: {:?}", &buf);
    let hdr = match header(buf.as_bytes()) {
        nom::IResult::Done(_, x) => x,
        nom::IResult::Error(e) => {
            error!("Bad header: {}", buf);
            panic!("{}", e);
        }
        nom::IResult::Incomplete { .. } => {
            error!("Partial header: {}", buf);
            panic!();
        }
    };
    info!("Client sent header {:?}", hdr);
    let mut sock = sock.into_inner();
    match hdr {
        Header::List => {
            // Listing files could be expensive, let's do it in this thread.
            sock.write(list_files().unwrap().as_bytes()).unwrap();
        }
        Header::Stream { path, index } => {
            if file_is_valid(&path) {
                // OK! This client will start watching a file. Let's remove
                // it from the nursery and change its epoll parameters.
                // TODO: If resolving returns `None`, we should re-resolve it every time there's new data.
                let mut file = File::open(&path).unwrap();
                let offset = resolve_index(&mut file, index).expect("index").unwrap();
                chan.send((sock, path, offset)).unwrap();
            } else {
                warn!("Client tried to access {:?} but isn't allowed", path);
            }
        }
        Header::Stats => {
            if sock.peer_addr().unwrap().ip().is_loopback() {
                writeln!(sock, "TODO").unwrap();
            } else {
                warn!("Client requested stats but isn't localhost");
            }
        }
    }
}
