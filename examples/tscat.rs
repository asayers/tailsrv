use bpaf::{Bpaf, Parser};
use fd_lock::RwLock;
use std::fs::File;
use std::io::{prelude::*, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;
use tracing::*;

#[derive(Bpaf)]
struct Opts {
    /// The file to save the stream to
    #[bpaf(short, long)]
    out: Option<PathBuf>,
    /// Send traces to journald instead of the terminal.
    #[cfg(feature = "tracing-journald")]
    journald: bool,
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
    if start_from != 0 {
        info!("Starting from byte {start_from}");
    }
    writeln!(conn, "{start_from}")?;

    {
        let mut conn = conn.try_clone()?;
        std::thread::spawn(move || loop {
            // Send a newline charater back to tailsrv.  Tailsrv discards
            // anything sent to it by a client, so this newline will be thrown
            // away.  The purpose of this is to detect a dead TCP connection.
            if let Err(e) = writeln!(conn) {
                error!("{e}");
                // panic!() kills only the current thread.  This takes down
                // both threads.
                std::process::exit(1);
            }
            std::thread::sleep(Duration::from_secs(heartbeat_secs));
        });
    }

    std::io::copy(&mut conn, &mut out)?;
    Ok(())
}
