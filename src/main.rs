extern crate clap;
extern crate env_logger;
#[macro_use] extern crate error_chain;
extern crate ignore;
extern crate inotify;
#[macro_use] extern crate log;
extern crate mio;
extern crate nix;
#[macro_use] extern crate nom;
extern crate same_file;

use clap::App;
use env_logger::LogBuilder;
use inotify::*;
use log::LogLevelFilter;
use mio::net::*;
use mio::unix::*;
use std::env::*;
use std::io::prelude::*;
use std::net::SocketAddr;

mod file_list; pub use file_list::*;
mod header;  pub use header::*;
mod librarian; pub use librarian::*;
mod nursery; pub use nursery::*;
mod token_box; pub use token_box::*;
mod types; pub use types::*;

fn main() {
    // Define CLI options
    let args = App::new("tailsrv")
        .version("1.0")
        .author("Alex Sayers <alex.sayers@gmail.com>")
        .about("A server which allows clients to tail files in the working directory")
        .args_from_usage(
            "-p --port=<port> 'The port number on which to listen for new connections'
             -q --quiet       'Don't produce output unless there's a problem'")
        .get_matches();

    // Init logger
    let log_level = if args.is_present("quiet") { LogLevelFilter::Warn }
                    else                        { LogLevelFilter::Info };
    LogBuilder::new().filter(None, log_level).init().unwrap();

    // Init epoll and define tokens
    let poll = mio::Poll::new().unwrap();

    // Init inotify and register with epoll
    let mut inotify = Inotify::init().unwrap();
    poll.register(&inotify, to_token(TypedToken::Inotify), mio::Ready::readable(), mio::PollOpt::level()).unwrap();

    // Bind the listen socket and register with epoll
    let inaddr_any = "0.0.0.0".parse().unwrap();
    let port = args.value_of("port").unwrap().parse().unwrap();
    let listen_addr = SocketAddr::new(inaddr_any, port);
    let listener = TcpListener::bind(&listen_addr).expect("Bind listen sock");
    poll.register(&listener, to_token(TypedToken::Listener), mio::Ready::readable(), mio::PollOpt::level()).unwrap();

    // Init the server state, allocate some buffers
    let mut mio_events = mio::Events::with_capacity(1024);
    let mut inotify_buf = [0u8; 4096];
    let mut nursery = Nursery::new(&poll);
    let mut librarian = Librarian::new();

    // Enter runloop
    info!("Serving files from {:?} on {}", current_dir().unwrap(), listen_addr);
    loop {
        poll.poll(&mut mio_events, None).unwrap();
        for mio_event in mio_events.iter() {
            match from_token(mio_event.token()) {
                TypedToken::Listener => {
                    // The listen socket is readable => a new client is trying to connect
                    let (sock, _) = listener.accept().unwrap();
                    nursery.register(sock).unwrap();
                }
                TypedToken::Inotify => {
                    // The inotify FD is readable => a watched file has been modified
                    let inotify_events = inotify.read_events_blocking(&mut inotify_buf).unwrap();
                    for ev in inotify_events {
                        if ev.mask.contains(event_mask::MODIFY) {
                            librarian.file_modified(ev.wd).unwrap();
                            librarian.handle_all_dirty().unwrap();
                        } else if ev.mask.contains(event_mask::DELETE_SELF) {
                            librarian.deregister_file(ev.wd).unwrap();
                        }
                    }
                }
                TypedToken::NurseryToken(cid) => {
                    if UnixReady::from(mio_event.readiness()).is_hup() {
                        // A client socket has disconnected => remove
                        nursery.deregister(cid).unwrap();
                    }
                    if mio_event.readiness().is_readable() {
                        // A nursery client has sent some data. Try to parse it as a header!
                        match nursery.try_read_header(cid).unwrap() {
                            None => { /* do nothing */ }
                            Some(Header::List) => {
                                let mut sock = nursery.deregister(cid).unwrap();
                                for entry in valid_files() {
                                    writeln!(sock, "{}", entry.unwrap().path().display()).unwrap();
                                }
                            }
                            Some(Header::Stream{ path, offset }) => {
                                if file_is_valid(&path) {
                                    let sock = nursery.graduate(cid).unwrap();
                                    let fid = inotify.add_watch(&path,
                                        watch_mask::MODIFY | watch_mask::DELETE_SELF).unwrap();
                                    librarian.register_client(cid, sock, fid, &path, offset).unwrap();
                                } else {
                                    error!("Client tried to access illegal file");
                                    nursery.deregister(cid).unwrap();
                                }
                            }
                            Some(Header::Stats) => {
                                let mut sock = nursery.deregister(cid).unwrap();
                                if sock.peer_addr().unwrap().ip().is_loopback() {
                                    writeln!(sock, "{:?}\n{:?}", nursery, librarian).unwrap();
                                }
                            }
                        }
                    }
                }
                TypedToken::LibraryToken(cid) => {
                    if UnixReady::from(mio_event.readiness()).is_hup() {
                        // A client socket has disconnected => remove
                        librarian.deregister_client(cid).unwrap();
                    }
                    if mio_event.readiness().is_writable() {
                        // A client socket has become writable => send some data
                        librarian.client_writable(cid).unwrap();
                        librarian.handle_all_dirty().unwrap();
                    }
                }
            }
        }
    }
}
