use bpaf::{Bpaf, Parser};
use net2::TcpStreamExt;
use std::io::{prelude::*, BufReader};
use std::thread::JoinHandle;
use std::{
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Bpaf)]
struct Opts {
    #[bpaf(short, long)]
    jobs: usize,
    /// How often to ping the server to check for a dead connection
    #[bpaf(fallback(5))]
    heartbeat_secs: u64,
    /// The remote tailsrv to connect to
    #[bpaf(positional("ADDR"))]
    addr: SocketAddr,
}

fn main() -> std::io::Result<()> {
    let opts = opts().run();
    let mut tails: Vec<Arc<Mutex<String>>> = vec![];
    let mut ts: Vec<JoinHandle<_>> = vec![];
    for _ in 0..opts.jobs {
        tails.push(Arc::new(Mutex::new(String::new())));
        let tail = tails.last().unwrap().clone();
        ts.push(std::thread::spawn(move || {
            let mut conn = TcpStream::connect(opts.addr)?;
            // Use TCP keepalive to detect dead connections
            let keepalive = Duration::from_secs(opts.heartbeat_secs);
            conn.set_keepalive(Some(keepalive))?;
            // Start from the beginning
            writeln!(conn, "0")?;
            let mut buf = String::new();
            let mut conn = BufReader::new(conn);
            loop {
                buf.clear();
                let n = conn.read_line(&mut buf)?;
                if n == 0 {
                    return std::io::Result::Ok(());
                }
                std::mem::swap(&mut *tail.lock().unwrap(), &mut buf);
            }
        }));
    }

    let mut term = liveterm::TermPrinter::new(std::io::stdout().lock());
    loop {
        use std::fmt::Write;
        term.clear()?;
        term.buf.clear();
        let reference = tails
            .first()
            .map(|x| x.lock().unwrap().clone())
            .unwrap_or_default();
        let mut n = 0;
        for (i, tail) in tails.iter().enumerate() {
            let tail = tail.lock().unwrap();
            if *tail == reference {
                n += 1;
            } else {
                writeln!(&mut term.buf, "#{i}: {}", tail.trim()).unwrap();
            }
        }
        writeln!(&mut term.buf, "{n} others: {}", reference.trim()).unwrap();
        let any_alive = ts.iter().any(|t| !t.is_finished());
        if any_alive {
            term.print()?;
        } else {
            return term.print_all();
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
