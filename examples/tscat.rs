use clap::Parser;
use std::io::prelude::*;
use std::net::{SocketAddr, TcpStream};

#[derive(Parser)]
struct Opts {
    /// The remote tailsrv to connect to
    addr: SocketAddr,
}

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

fn main() -> Result<()> {
    let opts = Opts::parse();
    tracing_subscriber::fmt::init();

    let mut out = std::io::stdout().lock();

    let mut conn = TcpStream::connect(opts.addr)?;
    writeln!(conn, "0")?;

    std::io::copy(&mut conn, &mut out)?;
    Ok(())
}
