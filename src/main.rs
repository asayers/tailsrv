#![feature(vec_remove_item)]
#![allow(unused_doc_comment)]

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
use ignore::{WalkBuilder,Walk};
use inotify::*;
use log::LogLevelFilter;
use mio::net::*;
use nix::sys::sendfile::sendfile;
use same_file::*;
use std::collections::hash_map::*;
use std::env::*;
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
use std::io;
use std::net::SocketAddr;
use std::os::unix::io::AsRawFd;
use std::path::*;

mod header; use header::*;
mod error; use error::*;

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
    const INOTIFY: mio::Token = mio::Token(0);
    const LISTENER: mio::Token = mio::Token(1);

    // Init inotify and register with epoll
    let inotify = Inotify::init().unwrap();
    poll.register(&inotify, INOTIFY, mio::Ready::readable(), mio::PollOpt::edge()).unwrap();

    // Bind the listen socket and register with epoll
    let inaddr_any = "0.0.0.0".parse().unwrap();
    let port = args.value_of("port").unwrap().parse().unwrap();
    let listen_addr = SocketAddr::new(inaddr_any, port);
    let listener = TcpListener::bind(&listen_addr).expect("Bind listen sock");
    poll.register(&listener, LISTENER, mio::Ready::readable(), mio::PollOpt::edge()).unwrap();

    // Init the server state, allocate some buffers
    let mut state = ServerState::new(inotify);
    let mut mio_events = mio::Events::with_capacity(1024);
    let mut inotify_buf = [0u8; 4096];

    // Enter runloop
    info!("Serving files from {:?} on {}", current_dir().unwrap(), listen_addr);
    loop {
        poll.poll(&mut mio_events, None).unwrap();
        for mio_event in mio_events.iter() {
            match mio_event.token() {
                LISTENER => {
                    // The listen socket is readable => a new client is trying to connect
                    let (mut sock, addr) = listener.accept().unwrap();
                    match state.handshake(&mut sock, &addr) {
                        Ok(()) => {}
                        Err(e) => {
                            error!("Error while handshaking with {}: {}", addr, e);
                            writeln!(sock, "Error: {}", e).unwrap();
                        }
                    }
                }
                INOTIFY => {
                    // The inotify FD is readable => a watched file has been modified
                    let inotify_events = state.inotify
                        .read_events_blocking(&mut inotify_buf).unwrap();
                    for ev in inotify_events {
                        if ev.mask.contains(event_mask::MODIFY) {
                            state.update_all(&ev.wd).unwrap_or_else(|e| error!("{}", e));
                        } else if ev.mask.contains(event_mask::DELETE_SELF) {
                            state.kick_all(&ev.wd).unwrap_or_else(|e| error!("{}", e));
                        }
                    }
                }
                mio::Token(_) => unreachable!(),
            }
        }
    }
}

struct ServerState {
    inotify: Inotify,
    interested_peers: HashMap<WatchDescriptor, (PathBuf, Vec<SocketAddr>), RandomState>,
    bookmarks: HashMap<SocketAddr, (TcpStream, i64), RandomState>,
}

impl ServerState {
    fn new(inotify: Inotify) -> ServerState {
        ServerState {
            inotify: inotify,
            interested_peers: HashMap::new(),
            bookmarks: HashMap::new(),
        }
    }

    fn handshake(&mut self, sock: &mut TcpStream, addr: &SocketAddr) -> Result<()> {
        let mut rdr = BufReader::new(sock.try_clone()?);
        let mut buf = String::new();
        rdr.read_line(&mut buf).map_err(|e| -> Error {
            if e.kind() == io::ErrorKind::WouldBlock {
                return ErrorKind::HeaderTooSlow.into();
            } else {
                return e.into();
            }
        })?;
        let header = parse_header(&buf)?;

        info!("{} connected and sent header {:?}", addr, header);
        match header {
            Header::Stream{ path, offset } => {
                if !file_is_valid(&path) {
                    bail!(ErrorKind::IllegalFile);
                }
                let wd = self.add_peer(addr.clone(), sock.try_clone()?, path, offset)?;
                self.update_all(&wd).unwrap_or_else(|e| error!("{}", e));
            }
            Header::List => {
                for entry in valid_files() {
                    writeln!(sock, "{}", entry?.path().display())?;
                }
            }
        }
        Ok(())
    }

    fn get_interested(&mut self, wd: &WatchDescriptor) -> Result<(PathBuf, Vec<SocketAddr>)> {
        Ok(self.interested_peers.get(wd)
           .ok_or(ErrorKind::NoonesInterested)?
           .clone())
    }

    /// Register a peer's interest in the given file, adding an inotify watch if necessary.
    fn add_peer(&mut self, addr: SocketAddr, sock: TcpStream, path: PathBuf, offset: i64)
            -> Result<WatchDescriptor> {
        info!("Peer {} connected, adding bookmark", addr);
        let wd = self.inotify.add_watch(&path, watch_mask::MODIFY | watch_mask::DELETE_SELF)?;
        let entry = self.interested_peers.entry(wd).or_insert((path, Vec::new()));
        entry.1.push(addr);
        self.bookmarks.insert(addr, (sock, offset));
        Ok(wd)
    }

    /// Remove a peer, also removing an inotify watch if necessary.
    fn remove_peer(&mut self, addr: &SocketAddr, wd: &WatchDescriptor) -> Result<()> {
        self.bookmarks.remove(addr);
        let empty = {
            let (ref path, ref mut peers) = *self.interested_peers.get_mut(wd)
                .ok_or(ErrorKind::NoonesInterested)?;
            peers.remove_item(addr);
            if peers.is_empty() {
                info!("File {:?} no longer has any interested peers. Removing watch.", path);
            }
            peers.is_empty()
        };
        if empty {
            self.interested_peers.remove(wd);
            self.inotify.rm_watch(wd.clone())?;
            // ^ Two bad APIs cancel out - BTreeMap doesn't provide a remove() which returns
            // ownership of the key, but inotify allows us to break its abstraction by
            // providing WatchDescriptor::clone().
        }
        Ok(())
    }

    /// Open the file corresponding to the given wd and update all interested peers.
    fn update_all(&mut self, wd: &WatchDescriptor) -> Result<()> {
        let (path, peers) = self.get_interested(wd)?;
        info!("Sending file {:?} to interested peers", path);
        let mut file = File::open(path)?;
        for addr in peers {
            match self.update_peer(&mut file, &addr) {
                Ok(()) => {}
                Err(Error(ErrorKind::Nix(nix::Error::Sys(nix::Errno::EPIPE)),_)) => {
                    info!("Peer {} disconnected, removing bookmark", addr);
                    self.remove_peer(&addr, wd)?;
                }
                Err(e) => bail!(e),
            }
        }
        Ok(())
    }

    /// Update the given peer with new data from the given file.
    fn update_peer(&mut self, file: &mut File, addr: &SocketAddr) -> Result<()> {
        let len = file.metadata()?.len();
        let (ref mut sock, ref mut offset) = *self.bookmarks.get_mut(addr)
            .ok_or(ErrorKind::BookmarkMissing)?;
        let cnt = len as i64 - *offset;
        if cnt > 0 {
            info!("Sending [{}..{}] to peer {}", offset, len, addr);
            sendfile(sock.as_raw_fd(), file.as_raw_fd(), Some(offset), cnt as usize)?;
        }
        Ok(())
    }

    /// Remove all peers interested in the given wd, closing their connections and removing the
    /// inotify watch.
    fn kick_all(&mut self, wd: &WatchDescriptor) -> Result<()> {
        let (path, peers) = self.get_interested(wd)?;
        info!("File {:?} was deleted. Kicking interested peers.", path);
        for addr in peers {
            self.remove_peer(&addr, wd).unwrap_or_else(|e| error!("{}", e));
        }
        Ok(())
    }
}

fn valid_files() -> Walk {
    WalkBuilder::new(".")
        .git_global(false)   // Parsing git-related files is surprising
        .git_ignore(false)   // behaviour in the context of tailsrv, so
        .git_exclude(false)  // let's not read those files.
        .ignore(true)   // However, we *should* read generic ".ignore" files...
        .hidden(true)   // and ignore dotfiles (so clients can't read the .ignore files)
        .parents(false) // Don't search the parent directory for .ignore files.
        .build()
        // .filter(|e| {
        //     e.unwrap_or_else(return false)
        //         .file_type().unwrap_or_else(return false);
        //         .is_file()
        // })
}

fn file_is_valid(path: &Path) -> bool {
    valid_files().any(|entry| {
        is_same_file(entry.unwrap().path(), path).unwrap_or(false)
    })
}

