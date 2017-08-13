extern crate clap;
extern crate env_logger;
#[macro_use] extern crate error_chain;
extern crate ignore;
extern crate inotify;
#[macro_use] extern crate log;
extern crate memchr;
extern crate memmap;
extern crate mio;
extern crate nix;
#[macro_use] extern crate nom;
extern crate same_file;

use clap::App;
use env_logger::LogBuilder;
use inotify::*;
use log::LogLevelFilter;
use mio::net::*;
use std::env::*;
use std::io::prelude::*;
use std::io::{self, BufReader, BufRead};
use std::net::SocketAddr;

mod file_list; pub use file_list::*;
mod header;  pub use header::*;
mod index;  pub use index::*;
mod pool; pub use pool::*;
mod token_box; pub use token_box::*;
mod types; pub use types::*;

fn main() {
    // Define CLI options
    let args = App::new("tailsrv")
        .version("0.2")
        .about("A server which allows clients to tail files in the working directory")
        .args_from_usage(
            "-p --port=<port> 'The port number on which to listen for new connections'
             -q --quiet       'Don't produce output unless there's a problem'")
        .get_matches();

    // Init logger
    let log_level = if args.is_present("quiet") { LogLevelFilter::Warn }
                    else                        { LogLevelFilter::Info };
    LogBuilder::new().filter(None, log_level).init().unwrap();

    // Init epoll, allocate buffer for epoll events
    let poll = mio::Poll::new().unwrap();
    let mut mio_events = mio::Events::with_capacity(1024);

    // Init inotify and register the inotify fd with epoll
    let inotify = Inotify::init().unwrap();
    poll.register(&inotify,
        to_token(TypedToken::Inotify),
        mio::Ready::readable(),
        mio::PollOpt::level()).unwrap();

    // Bind the listen socket and register it with epoll
    let inaddr_any = "0.0.0.0".parse().unwrap();
    let port = args.value_of("port").unwrap().parse().unwrap();
    let listen_addr = SocketAddr::new(inaddr_any, port);
    let listener = TcpListener::bind(&listen_addr).expect("Bind listen sock");
    poll.register(&listener,
        to_token(TypedToken::Listener),
        mio::Ready::readable(),
        mio::PollOpt::level()).unwrap();

    // When a client first connects, it is asigned a ClientId by calling `next_cid()`.
    let mut last_cid = 0;
    let mut next_cid = move || -> ClientId { last_cid += 1; last_cid };

    // We then put the newly connected client in the nursery. It will stay there until it has sent
    // a complete header.
    let mut nursery: Map<ClientId, BufReader<TcpStream>> = Map::new();

    // If the client sends a "stream" header, it is then moved to the pool, which tracks which
    // clients are interested in which files.
    let mut pool = WatcherPool::new(inotify);

    // Enter runloop
    info!("Serving files from {:?} on {}", current_dir().unwrap(), listen_addr);
    loop {
        // Wait for something to happen
        poll.poll(&mut mio_events, None).unwrap();
        for mio_event in mio_events.iter() {
            match from_token(mio_event.token()) {
                TypedToken::Listener => {
                    // The listen socket is readable => a new client is trying to connect
                    let (sock, _) = listener.accept().unwrap();
                    let cid = next_cid();
                    info!("Client {} connected. Waiting for it to send a header...", cid);
                    // The first thing the client will do is send a header
                    poll.register(&sock,
                        to_token(TypedToken::NurseryToken(cid)),  // ...so we give it a nursery token
                        mio::Ready::readable(),                   // ...watch for new data
                        mio::PollOpt::edge()).unwrap();
                    nursery.insert(cid, BufReader::new(sock));    // ...and store it in the nursery
                }
                TypedToken::NurseryToken(cid) => {
                    if mio_event.readiness().is_readable() {
                        // A client whih is in the nursery has sent some data. Let's try to read it
                        // and parse it into a header.
                        let header = {
                            let rdr = nursery.get_mut(&cid)
                                .ok_or(ErrorKind::ClientNotFound).unwrap();
                            try_read_header(rdr).unwrap()
                        };
                        // If header is None, then we don't have enough data yet and should do
                        // nothing.
                        if let Some(header) = header {
                            info!("Client {} sent header {:?}", cid, header);
                            // FIXME: Some of the computations we do at this point may be expensive,
                            // and block the whole server. It may be a good idea to set a timeout here,
                            // somehow.
                            match header {
                                Header::List => {
                                    let mut sock = nursery.remove(&cid)
                                        .ok_or(ErrorKind::ClientNotFound).unwrap()
                                        .into_inner();
                                    poll.deregister(&sock).unwrap();
                                    sock.write(list_files().unwrap().as_bytes()).unwrap();
                                }
                                Header::Stream{ path, index } => {
                                    if file_is_valid(&path) {
                                        // OK! This client will start watching a file. Let's remove
                                        // it from the nursery and change its epoll parameters.
                                        let sock = nursery.remove(&cid)
                                            .ok_or(ErrorKind::ClientNotFound).unwrap()
                                            .into_inner();
                                        poll.reregister(&sock,
                                            to_token(TypedToken::PoolToken(cid)), // with a pool token
                                            mio::Ready::writable(), // Watching for writability
                                            mio::PollOpt::edge()).unwrap();
                                        // And then we put it in the pool. This function also
                                        // handles setting up inotify watches etc.
                                        pool.register_client(cid, sock, &path, index).unwrap();
                                    } else {
                                        warn!("Client {} tried to access {:?} but isn't allowed", cid, path);
                                        let sock = nursery.remove(&cid)
                                            .ok_or(ErrorKind::ClientNotFound).unwrap()
                                            .into_inner();
                                        poll.deregister(&sock).unwrap();
                                    }
                                }
                                Header::Stats => {
                                    let mut sock = nursery.remove(&cid)
                                        .ok_or(ErrorKind::ClientNotFound).unwrap()
                                        .into_inner();
                                    poll.deregister(&sock).unwrap();
                                    if sock.peer_addr().unwrap().ip().is_loopback() {
                                        writeln!(sock, "{:?}\n{:?}", nursery, pool).unwrap();
                                    } else {
                                        warn!("Client {} requested stats but isn't localhost", cid);
                                    }
                                }
                            }
                        }
                    }
                }
                TypedToken::Inotify => {
                    // The inotify FD is readable => a watched file has been modified
                    // First, mark all clients interested in modifed files as dirty.
                    pool.check_watches().unwrap();
                    // Then send data until they're up-do-date.
                    pool.handle_all_dirty().unwrap();
                }
                TypedToken::PoolToken(cid) => {
                    if mio_event.readiness().is_writable() {
                        // A client in the pool has become writable => send some data
                        pool.client_writable(cid).unwrap();
                        pool.handle_all_dirty().unwrap();
                    }
                }
            }
        }
    }
}

/// Try to read a header from cid's socket
//
// FIXME: Clients can attack the server by sending lots of header data with no newline. Eventually
// tailsrv will run out of memory and crash. TODO: length limit.
// TODO: Perhaps we should also impose a time limit for sending a header.
fn try_read_header(rdr: &mut BufReader<TcpStream>) -> Result<Option<Header>> {
    match rdr.fill_buf() {
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => bail!(e),
        Ok(buf) => {
            info!("Current buffer: {:?}", buf);
            match header(buf) {
                nom::IResult::Done(_, x) => Ok(Some(x)), // Leave the data in the buffer, it's fine
                nom::IResult::Error(e) => bail!(e),
                nom::IResult::Incomplete{..} => Ok(None), // FIXME: data gets removed somehow
            }
        }
    }
}
