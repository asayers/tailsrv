use bpaf::{Bpaf, Parser};
use net2::TcpStreamExt;
use std::io::prelude::*;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

#[derive(Bpaf)]
struct Opts {
    /// How often to ping the server to check for a dead connection
    #[bpaf(fallback(5))]
    heartbeat_secs: u64,
    /// The remote tailsrv to connect to
    #[bpaf(positional("ADDR"))]
    addr: SocketAddr,
}

fn main() -> std::io::Result<()> {
    let opts = opts().run();
    let mut conn = TcpStream::connect(opts.addr)?;
    // Use TCP keepalive to detect dead connections
    let keepalive = Duration::from_secs(opts.heartbeat_secs);
    conn.set_keepalive(Some(keepalive))?;
    // Start from the beginning
    writeln!(conn, "0")?;
    // Copy the stream to stdout
    let mut stdout = std::io::stdout().lock();
    std::io::copy(&mut conn, &mut stdout)?;
    Ok(())
}
