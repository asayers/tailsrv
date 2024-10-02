use bpaf::{Bpaf, Parser};
use fd_lock::RwLock;
use net2::TcpStreamExt;
use std::fs::File;
use std::io::{prelude::*, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Bpaf)]
struct Opts {
    /// How often to ping the server to check for a dead connection
    #[bpaf(fallback(5))]
    heartbeat_secs: u64,
    /// The remote tailsrv to connect to
    #[bpaf(positional("ADDR"))]
    addr: SocketAddr,
    /// The file to save the stream to
    #[bpaf(positional("PATH"))]
    file: PathBuf,
}

fn main() -> std::io::Result<()> {
    let opts = opts().run();
    // Open the file in append mode, creating it if it doesn't already
    // exist.
    let file = File::options().append(true).create(true).open(opts.file)?;
    // Take an exclusive lock on the file, and exit if it's already locked.
    // This prevents two tscats from writing to the same file.
    let mut file = RwLock::new(file);
    let mut file = file.try_write()?;
    // We assume that this point that we're the only process writing to
    // the file, so we can read its length and not worry about TOCTOU.
    let len = file.seek(SeekFrom::End(0))?;
    let mut conn = TcpStream::connect(opts.addr)?;
    // Use TCP keepalive to detect dead connections
    let keepalive = Duration::from_secs(opts.heartbeat_secs);
    conn.set_keepalive(Some(keepalive))?;
    // Use the current length as the "start from" offset
    writeln!(conn, "{len}")?;
    // Append the stream to the file
    std::io::copy(&mut conn, &mut file as &mut File)?;
    Ok(())
}
