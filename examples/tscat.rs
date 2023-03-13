use clap::Parser;
use std::fs::File;
use std::io::prelude::*;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;

#[derive(Parser)]
struct Opts {
    /// The remote tailsrv to connect to
    addr: SocketAddr,
    /// The file to save the stream to
    #[clap(short, long)]
    out: Option<PathBuf>,
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

fn main() -> Result<()> {
    let opts = Opts::parse();
    tracing_subscriber::fmt::init();

    if let Some(path) = &opts.out {
        // Open the file in append mode, creating it if it doesn't already
        // exist.
        let file = File::options().append(true).create(true).open(path)?;
        mirror(opts.addr, file)
    } else {
        let stdout = std::io::stdout().lock();
        mirror(opts.addr, stdout)
    }
}

fn mirror(addr: SocketAddr, mut out: impl Write) -> Result<()> {
    let mut conn = TcpStream::connect(addr)?;
    writeln!(conn, "0")?;

    std::io::copy(&mut conn, &mut out)?;
    Ok(())
}
