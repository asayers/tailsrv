use mio::net::TcpStream;
use nix::sys::sendfile::sendfile;
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
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
#[derive(Debug)]
pub struct Librarian {
    files: Map<FileId, (File, Set<ClientId>)>,
    clients: Map<ClientId, (TcpStream, FileId, Offset)>,
    dirty_clients: VecDeque<ClientId>,
    // mio: (Registration, SetReadiness),
}

/// The maximum number of bytes which will be `sendfile()`'d to a client before moving onto the
/// next waiting client.
///
/// A bigger size increases total throughput, but may allow a client who is reading a lot of data
/// to hurt reaction latency for other clients.
pub const CHUNK_SIZE: i64 = 1024 * 1024;

impl Librarian {
    pub fn new() -> Librarian {
        Librarian {
            files: Map::new(),
            clients: Map::new(),
            dirty_clients: VecDeque::new(),
            // registration: Registration::new2(),
        }
    }

    pub fn handle_all_dirty(&mut self) -> Result<()> {
        loop {
            if self.handle_next_dirty()?.is_none() {
                break;
            }
        }
        Ok(())
    }

    /// Send data to the next dirty client, sending up to `CHUNK_SIZE` bytes from the file it's
    /// interested in.
    ///
    /// If there is more than `CHUNK_SIZE` waiting to be sent to the client, it will be re-marked
    /// as dirty.
    pub fn handle_next_dirty(&mut self) -> Result<Option<usize>> {
        let cid = match self.dirty_clients.pop_front() {
            None => return Ok(None),
            Some(x) => x,
        };
        info!("Sending data to client {}", cid);
        let (ref mut sock, fid, ref mut offset) = *self.clients.get_mut(&cid)
            .ok_or(ErrorKind::ClientNotFound)?;
        let (ref mut file, _) = *self.files.get_mut(&fid)
            .ok_or(ErrorKind::FileNotWatched)?;
        let len = file.metadata()?.len();
        let cnt = match len as i64 - *offset {
            x if x <= 0 => return Ok(Some(0)),
            x if x <= CHUNK_SIZE => x,
            _ => { self.dirty_clients.push_back(cid); CHUNK_SIZE }
        };
        Ok(Some(sendfile(sock.as_raw_fd(), file.as_raw_fd(), Some(offset), cnt as usize)?))
    }

    /// Mark all the given file's watchers as dirty.
    pub fn file_modified(&mut self, fid: FileId) -> Result<()> {
        let (_, ref fclients) = *self.files.get(&fid).ok_or(ErrorKind::FileNotWatched)?;
        for &cid in fclients {
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

    pub fn register_client(&mut self, cid: ClientId, sock: TcpStream, fid: FileId, path: &Path,
                           offset: Offset) -> Result<()> {
        info!("Registering client {}", cid);
        self.clients.insert(cid, (sock, fid, offset));
        match self.files.entry(fid) {
            Entry::Occupied(x) => {
                x.into_mut().1.insert(cid);
            }
            Entry::Vacant(x) => {
                let file = File::open(path)?;
                let fclients = Set::from_iter(vec![cid]);
                x.insert((file, fclients));
            }
        }
        Ok(())
    }

    /// HUPs the sock, dereg's the file if empty. Remember to remove the watch.
    pub fn deregister_client(&mut self, cid: ClientId) -> Result<()> {
        info!("Deregistering client {}", cid);
        let (_, fid, _) = self.clients.remove(&cid).ok_or(ErrorKind::ClientNotFound)?;
        let noones_interested = {
            let (_, ref mut fclients) = *self.files.get_mut(&fid).ok_or(ErrorKind::FileNotWatched)?;
            fclients.remove(&cid);
            fclients.is_empty()
        };
        if noones_interested {
            self.deregister_file(fid)?;
        }
        Ok(())
    }

    /// Closes the file, dereg's all clients. Remember to remove the watch.
    pub fn deregister_file(&mut self, fid: FileId) -> Result<()> {
        info!("Deregistering file {:?}", fid);
        let (_, fclients) = self.files.remove(&fid).ok_or(ErrorKind::FileNotWatched)?;
        for cid in fclients {
            self.clients.remove(&cid);
        }
        Ok(())
    }
}

// impl Evented for Deadline {
//     fn register(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt)
//         -> io::Result<()>
//     {
//         self.mio.0.register(poll, token, interest, opts)
//     }

//     fn reregister(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt)
//         -> io::Result<()>
//     {
//         self.mio.0.reregister(poll, token, interest, opts)
//     }

//     fn deregister(&self, poll: &Poll) -> io::Result<()> {
//         self.mio.0.deregister(poll)
//     }
// }
