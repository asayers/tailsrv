use clap::Parser;
use fd_lock::RwLock;
use std::fs::File;
use std::io::{prelude::*, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use tracing::*;

#[derive(Parser)]
struct Opts {
    /// The remote tailsrv to connect to
    addr: SocketAddr,
    /// The file to save the stream to
    #[clap(short, long)]
    out: Option<PathBuf>,
    /// Send traces to journald instead of the terminal.
    #[cfg(feature = "tracing-journald")]
    #[clap(long)]
    journald: bool,
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

fn main() -> Result<()> {
    let opts = Opts::parse();
    tailsrv::log_init(
        #[cfg(feature = "tracing-journald")]
        opts.journald,
    );

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
        mirror(opts.addr, len, file)
    } else {
        let stdout = std::io::stdout().lock();
        mirror(opts.addr, 0, stdout)
    }
}

fn mirror(addr: SocketAddr, start_from: u64, mut out: impl Write) -> Result<()> {
    let mut conn = TcpStream::connect(addr)?;
    if start_from != 0 {
        info!("Starting from byte {start_from}");
    }
    writeln!(conn, "{start_from}")?;

    std::io::copy(&mut conn, &mut out)?;
    Ok(())
}
