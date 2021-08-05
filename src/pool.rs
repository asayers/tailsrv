use crate::types::*;
use inotify::*;
use log::*;
use std::convert::TryFrom;
use std::fmt::{self, Debug};
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use tokio::net::TcpStream;
use tokio::sync::watch;

pub async fn client_task(
    sock: TcpStream,
    initial_offset: Offset,
    file: std::os::unix::io::RawFd,
    mut file_len: watch::Receiver<FileLength>,
) {
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
        let wanted = i64::try_from(*file_len.borrow()).unwrap() - offset;
        if wanted <= 0 {
            // We're all caught-up.  Wait for new data to be written
            // to the file before continuing.
            info!("Waiting for changes");
            match file_len.changed().await {
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
            nix::sys::sendfile::sendfile(sock.as_raw_fd(), file, Some(&mut offset), cnt as usize)
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

/// Keeps track of which clients are interested in which files.
///
/// tailsrv sends data to clients in response to two kinds of event:
///
/// - A watched file was modified;
/// - A client became writable;
pub struct WatcherPool {
    files: Map<FileId, (File, watch::Sender<FileLength>, watch::Receiver<FileLength>)>,
    pub inotify: Inotify,
    buf: Vec<u8>,
}

impl Debug for WatcherPool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.files.fmt(f)
    }
}

impl WatcherPool {
    pub fn new() -> WatcherPool {
        WatcherPool {
            files: Map::default(),
            inotify: Inotify::init().unwrap(),
            buf: vec![0; 4096],
        }
    }

    pub fn update_all(&mut self) -> Result<()> {
        for ev in self.inotify.read_events(&mut self.buf).unwrap() {
            if ev.mask.contains(EventMask::DELETE_SELF) || ev.mask.contains(EventMask::MOVE_SELF) {
                info!("Deregistering file {:?}", ev.wd);
                self.files.remove(&ev.wd).ok_or(Error::FileNotWatched)?;
                self.inotify.rm_watch(ev.wd).unwrap(); // TODO: does this happen automatically?
            } else if ev.mask.contains(EventMask::MODIFY) {
                let (file, tx, _) = self.files.get(&ev.wd).ok_or(Error::FileNotWatched)?;
                let file_len = file.metadata()?.len();
                info!("{:?}: File length is now {}", ev.wd, file_len);
                tx.send(file_len).unwrap();
            }
        }
        Ok(())
    }

    pub fn register_client(
        &mut self,
        path: &Path,
    ) -> Result<(std::os::unix::io::RawFd, watch::Receiver<FileLength>)> {
        info!("Registering client");
        let fid = self
            .inotify
            .add_watch(
                &path,
                WatchMask::MODIFY | WatchMask::DELETE_SELF | WatchMask::MOVE_SELF,
            )
            .unwrap();
        let (file, _, rx) = self.files.entry(fid).or_insert_with(|| {
            let file = File::open(path).unwrap();
            let file_len = file.metadata().unwrap().len();
            let (tx, rx) = watch::channel(file_len);
            (file, tx, rx)
        });
        Ok((file.as_raw_fd(), rx.clone()))
    }
}
