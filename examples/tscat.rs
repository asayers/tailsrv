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
    /// The file to save the stream to
    #[bpaf(short, long)]
    out: Option<PathBuf>,
    /// How often to ping the server to check for a dead connection
    #[bpaf(fallback(5))]
    heartbeat_secs: u64,
    /// The remote tailsrv to connect to
    #[bpaf(positional)]
    addr: SocketAddr,
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

fn main() -> Result<()> {
    let opts = opts().run();
    if let Some(path) = &opts.out {
        // Open the file in append mode, creating it if it doesn't already
        // exist.
        let file = File::options().append(true).create(true).open(path)?;
        // Take an exclusive lock on the file, and exit if it's already locked.
        // This prevents two tscats from writing to the same file.
        let mut file = RwLock::new(file);
        let mut file = file.try_write()?;
        let file = &mut file as &mut File;
        // We assume that this point that we're the only process writing to
        // the file, so we can read its length and not worry about TOCTOU.
        let len = file.seek(SeekFrom::End(0))?;
        mirror(opts.addr, len, file, opts.heartbeat_secs)
    } else {
        let stdout = std::io::stdout().lock();
        mirror(opts.addr, 0, stdout, opts.heartbeat_secs)
    }
}

fn mirror(
    addr: SocketAddr,
    start_from: u64,
    mut out: impl Write,
    heartbeat_secs: u64,
) -> Result<()> {
    let mut conn = TcpStream::connect(addr)?;
    conn.set_keepalive(Some(Duration::from_secs(heartbeat_secs)))?;
    writeln!(conn, "{start_from}")?;
    std::io::copy(&mut conn, &mut out)?;
    Ok(())
}
