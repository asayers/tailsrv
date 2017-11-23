use inotify::*;
use mio::net::TcpStream;
use nix::sys::sendfile::sendfile;
use nix;
use slab::*;
use std::collections::hash_map::Entry;
use std::fmt::{self, Debug};
use std::fs::File;
use std::iter::FromIterator;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use types::*;

/// Keeps track of which clients are interested in which files.
///
/// tailsrv sends data to clients in response to two kinds of event:
///
/// - A watched file was modified;
/// - A client became writable;
pub struct WatcherPool {
    pub socks: Slab<TcpStream>,
    files: Map<FileId, (File, Set<ClientId>)>,
    offsets: Map<ClientId, (FileId, Offset)>,
    inotify: Inotify,
    inotify_buf: Vec<u8>,
}

impl Debug for WatcherPool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        (&self.files, &self.offsets).fmt(f)
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
            socks: Slab::new(),
            offsets: Map::new(),
            inotify: inotify,
            inotify_buf: vec![0;4096],
        }
    }

    /// Send data to the given client, sending up to `CHUNK_SIZE` bytes from the file it's
    /// interested in.
    ///
    /// The return value indicates whether this client has more work to do.
    pub fn handle_client(&mut self, cid: ClientId) -> Result<bool> {
        let ret = {
            let (fid, ref mut offset) = *self.offsets.get_mut(&cid)
                .ok_or(ErrorKind::ClientNotFound)?;
            let (ref mut file, _) = *self.files.get_mut(&fid)
                .ok_or(ErrorKind::FileNotWatched)?;
            let sock = self.socks.get_mut(cid).unwrap();
            update_client(file, sock, offset)
        };
        match ret {
            Err(Error(ErrorKind::Nix(nix::Error::Sys(nix::Errno::EPIPE)),_)) |
            Err(Error(ErrorKind::Nix(nix::Error::Sys(nix::Errno::ECONNRESET)),_)) => {
                // The client hung up
                self.deregister_client(cid)?;
                Ok(false)
            }
            Err(Error(ErrorKind::Nix(nix::Error::Sys(nix::Errno::EAGAIN)), _)) => {
                // The socket is not writeable. Don't requeue.
                Ok(false)
            }
            Err(e) => bail!(e),
            Ok((sent, wanted)) => {
                Ok(wanted > sent as i64)
            }
        }
    }

    /// Check the inotify fd and mark appropriate clients as dirty
    pub fn check_watches(&mut self) -> Result<Vec<ClientId>> {
        // FIXME: ugly, inefficient implementation
        let mut dirty_clients = vec![];
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
            // Mark all the given file's watchers as dirty.
            debug!("File {:?} marked as dirty", fid);
            let (_, ref watchers) = *self.files.get(&fid).ok_or(ErrorKind::FileNotWatched)?;
            for &cid in watchers {
                debug!("Client {} marked as dirty", cid);
                dirty_clients.push(cid);
            }
        }
        for fid in deleted_files {
            self.deregister_file(fid)?;
        }
        Ok(dirty_clients)
    }

    pub fn register_client(&mut self, cid: ClientId, path: &Path, offset: usize) -> Result<ClientId> {
        info!("Registering client {}", cid);
        let fid = self.inotify.add_watch(&path, watch_mask::MODIFY | watch_mask::DELETE_SELF).unwrap();
        self.offsets.insert(cid, (fid, offset as i64));
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
        Ok(cid)
    }

    /// HUPs the sock, dereg's the file if empty.
    // FIXME: I guess we should remove epoll watch (although once the socket HUPs, it probably gets
    // removed automatically, right?)
    fn deregister_client(&mut self, cid: ClientId) -> Result<()> {
        info!("Deregistering client {}", cid);
        self.socks.remove(cid);
        let (fid, _) = self.offsets.remove(&cid).unwrap();
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
    fn deregister_file(&mut self, fid: FileId) -> Result<()> {
        info!("Deregistering file {:?}", fid);
        let (_, watchers) = self.files.remove(&fid).ok_or(ErrorKind::FileNotWatched)?;
        for cid in watchers {
            self.offsets.remove(&cid);
        }
        self.inotify.rm_watch(fid)?;
        Ok(())
    }
}

/// Send up to `CHUNK_SIZE` bytes from the given file to the given sock, updating its offset.
///
/// If the client is up-to-date, the function will return with 0.
/// If the client is unwritable, the function will return with 0.
/// If the client has disconnected, the function will return with EPIPE.
/// If the client is writeable and needed more than `CHUNK_SIZE`, the function will return
fn update_client(file: &mut File, sock: &mut TcpStream, offset: &mut Offset) -> Result<(usize, i64)> {
    let len = file.metadata()?.len();
    let wanted = len as i64 - *offset;  // How many bytes the client wants
    let cnt = match wanted {            // How many bytes the client will get
        x if x <= 0 => return Ok((0, wanted)),
        x if x <= CHUNK_SIZE as i64=> x,
        _ => CHUNK_SIZE as i64,
    };
    info!("Sending {} bytes from offset {}", cnt, offset);
    let n = sendfile(sock.as_raw_fd(), file.as_raw_fd(), Some(offset), cnt as usize)?;
    Ok((n, wanted))
}
