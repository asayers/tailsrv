use index::*;
use inotify::*;
use mio::net::TcpStream;
use nix::sys::sendfile::sendfile;
use nix;
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::fs::File;
use std::iter::FromIterator;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::fmt::{self, Debug};
use types::*;

/// Keeps track of which clients are interested in which files.
///
/// tailsrv sends data to clients in response to two kinds of event:
///
/// - A watched file was modified;
/// - A client became writable;
pub struct WatcherPool {
    files: Map<FileId, (File, Set<ClientId>)>,
    clients: Map<ClientId, (TcpStream, FileId, Offset)>,
    dirty_clients: VecDeque<ClientId>,
    inotify: Inotify,
    inotify_buf: Vec<u8>,
}

impl Debug for WatcherPool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        (&self.files, &self.clients, &self.dirty_clients).fmt(f)
    }
}

/// The maximum number of bytes which will be `sendfile()`'d to a client before moving onto the
/// next waiting client.
///
/// A bigger size increases total throughput, but may allow a client who is reading a lot of data
/// to hurt reaction latency for other clients.
pub const CHUNK_SIZE: usize = 1024 * 1024;

impl WatcherPool {
    pub fn new(inotify: Inotify) -> WatcherPool {
        WatcherPool {
            files: Map::new(),
            clients: Map::new(),
            dirty_clients: VecDeque::new(),
            inotify: inotify,
            inotify_buf: vec![0;4096],
        }
    }

    /// Send data to all dirty clients until they're up-do-date, become unwritable, or hang up.
    pub fn handle_all_dirty(&mut self) -> Result<()> {
        loop {
            let cid = match self.dirty_clients.pop_front() {
                None => break,
                Some(x) => x,
            };
            self.handle_client(cid)?;
        }
        Ok(())
    }

    /// Send data to the given client, sending up to `CHUNK_SIZE` bytes from the file it's
    /// interested in.
    fn handle_client(&mut self, cid: ClientId) -> Result<usize> {
        let ret = {
            let (ref mut sock, fid, ref mut offset) = *self.clients.get_mut(&cid)
                .ok_or(ErrorKind::ClientNotFound)?;
            let (ref mut file, _) = *self.files.get_mut(&fid)
                .ok_or(ErrorKind::FileNotWatched)?;
            update_client(file, sock, offset)
        };
        match ret {
            Err(Error(ErrorKind::Nix(nix::Error::Sys(nix::Errno::EPIPE)),_)) => {
                // The client hung up
                self.deregister_client(cid)?;
                Ok(0)
            }
            Err(e) => bail!(e),
            Ok(n) => {
                if n >= CHUNK_SIZE {
                    // We're (probably) not done yet.
                    self.dirty_clients.push_back(cid);
                }
                Ok(n)
            }
        }
    }

    /// Check the inotify fd and mark appropriate clients as dirty
    pub fn check_watches(&mut self) -> Result<()> {
        // FIXME: ugly, inefficient implementation
        let mut modified_files = vec![];
        let mut deleted_files = vec![];
        {
            let events = self.inotify.read_events_blocking(&mut self.inotify_buf)?;
            for ev in events {
                if ev.mask.contains(event_mask::MODIFY) {
                    modified_files.push(ev.wd.clone());
                }
                if ev.mask.contains(event_mask::DELETE_SELF) {
                    deleted_files.push(ev.wd.clone());
                }
            }
        }
        for fid in modified_files {
            self.file_modified(fid)?;
        }
        for fid in deleted_files {
            self.deregister_file(fid)?;
        }
        Ok(())
    }

    /// Mark all the given file's watchers as dirty.
    fn file_modified(&mut self, fid: FileId) -> Result<()> {
        let (_, ref watchers) = *self.files.get(&fid).ok_or(ErrorKind::FileNotWatched)?;
        for &cid in watchers {
            info!("Client {} marked as dirty", cid);
            self.dirty_clients.push_back(cid);
        }
        Ok(())
    }

    /// Mark the given client as dirty.
    pub fn client_writable(&mut self, cid: ClientId) -> Result<()> {
        info!("Client {} marked as dirty", cid);
        self.dirty_clients.push_back(cid);
        Ok(())
    }

    pub fn register_client(&mut self, cid: ClientId, sock: TcpStream, path: &Path,
                           index: Index) -> Result<()> {
        info!("Registering client {}", cid);
        let fid = self.inotify.add_watch(&path, watch_mask::MODIFY | watch_mask::DELETE_SELF).unwrap();
        match self.files.entry(fid) {
            Entry::Occupied(x) => {
                let (_, ref mut watchers) = *x.into_mut();
                watchers.insert(cid);
            }
            Entry::Vacant(x) => {
                let file = File::open(path)?;
                let watchers = Set::from_iter(vec![cid]);
                x.insert((file, watchers));
            }
        }
        let &(ref file, _) = self.files.get(&fid).unwrap();
        let offset = resolve_index(file, index)?.unwrap();
        self.clients.insert(cid, (sock, fid, offset as i64));
        Ok(())
    }

    /// HUPs the sock, dereg's the file if empty.
    // FIXME: I guess we should remove epoll watch (although once the socket HUPs, it probably gets
    // removed automatically, right?)
    pub fn deregister_client(&mut self, cid: ClientId) -> Result<()> {
        info!("Deregistering client {}", cid);
        let (_, fid, _) = self.clients.remove(&cid).ok_or(ErrorKind::ClientNotFound)?;
        vecdeque_remove(&mut self.dirty_clients, cid);
        let noones_interested = {
            let (_, ref mut watchers) = *self.files.get_mut(&fid).ok_or(ErrorKind::FileNotWatched)?;
            watchers.remove(&cid);
            watchers.is_empty()
        };
        if noones_interested {
            self.deregister_file(fid)?;
        }
        Ok(())
    }

    /// Closes the file, dereg's all clients.
    pub fn deregister_file(&mut self, fid: FileId) -> Result<()> {
        info!("Deregistering file {:?}", fid);
        let (_, watchers) = self.files.remove(&fid).ok_or(ErrorKind::FileNotWatched)?;
        for cid in watchers {
            self.clients.remove(&cid);
        }
        self.inotify.rm_watch(fid)?;
        Ok(())
    }
}

/// Removes all occurances of `target` from `xs`.
// TODO: Unit tests
fn vecdeque_remove<T: PartialEq>(xs: &mut VecDeque<T>, target: T) {
    let mut end = xs.len();
    let mut i = 0;
    while i < end {
        if xs[i] == target {
            xs.remove(i);
            end -= 1;
        } else {
            i += 1;
        }
    }
}

/// Send up to `CHUNK_SIZE` bytes from the given file to the given sock, updating its offset.
///
/// If the client is up-to-date, the function will return with 0.
/// If the client is unwritable, the function will return with 0.
/// If the client has disconnected, the function will return with EPIPE.
/// If the client is writeable and needed more than `CHUNK_SIZE`, the function will return
fn update_client(file: &mut File, sock: &mut TcpStream, offset: &mut Offset) -> Result<usize> {
    let len = file.metadata()?.len();
    let cnt = match len as i64 - *offset {
        x if x <= 0 => return Ok(0),
        x if x <= CHUNK_SIZE as i64=> x,
        _ => CHUNK_SIZE as i64,
    };
    info!("Sending {} bytes from offset {}", cnt, offset);
    match sendfile(sock.as_raw_fd(), file.as_raw_fd(), Some(offset), cnt as usize) {
        Err(nix::Error::Sys(nix::Errno::EAGAIN)) => Ok(0),
        Err(e) => bail!(e),
        Ok(n) => Ok(n),
    }
}
