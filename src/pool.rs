use crate::types::*;
use log::*;
use std::convert::TryFrom;
use std::os::unix::io::AsRawFd;
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
