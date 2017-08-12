use header::*;
use mio::net::*;
use mio;
use nom;
use std::io::{self, BufReader, BufRead};
use types::*;
use token_box::*;

/// The Nursery is where newly connected clients go. They stay here until they've sent a Header.
///
/// We assume that all clients go through a two-stage lifecycle:
///
/// 1. *Reading*. At first we read some data from the client. We call this data the "header".
/// 2. *Writing*. After we've recieved a header we ignore everything the client sends us; now we're
///    the ones sending data.
///
/// Clearly this scheme doesn't work for complicated protocols which require multiple round-trips
/// to negotiate things. But for our purposes, it's sufficient.
#[derive(Debug)]
pub struct Nursery<'a> {
    poll: &'a mio::Poll,
    clients: Map<ClientId, BufReader<TcpStream>>,
    next_id: usize,
}

impl<'a> Nursery<'a> {
    pub fn new(poll: &'a mio::Poll) -> Nursery {
        Nursery {
            poll: poll,
            clients: Map::new(),
            next_id: 0,
        }
    }

    /// Create a new uninitialized client
    pub fn register(&mut self, sock: TcpStream) -> Result<()> {
        let cid = self.next_id;
        self.next_id += 1;
        info!("Registering client {}", cid);
        let token = to_token(TypedToken::NurseryToken(cid));
        self.poll.register(&sock, token,
            mio::Ready::readable() | mio::unix::UnixReady::hup(),
            mio::PollOpt::edge())?;
        self.clients.insert(cid, BufReader::new(sock));
        Ok(())
    }

    /// Remove a client (whether initialized or uninitialized).
    pub fn deregister(&mut self, cid: ClientId) -> Result<TcpStream> {
        info!("Deregistering client {}", cid);
        let sock = self.clients.remove(&cid).ok_or(ErrorKind::ClientNotFound)?.into_inner();
        self.poll.deregister(&sock)?;
        Ok(sock)
    }

    /// Remove a client from the nursery and watch for it becoming writable.
    pub fn graduate(&mut self, cid: ClientId) -> Result<TcpStream> {
        info!("Graduating client {}", cid);
        let sock = self.clients.remove(&cid).ok_or(ErrorKind::ClientNotFound)?.into_inner();
        let token = to_token(TypedToken::LibraryToken(cid));
        self.poll.reregister(&sock,
            token,
            mio::Ready::writable() | mio::unix::UnixReady::hup(),
            mio::PollOpt::edge())?;
        Ok(sock)
    }

    /// If the client is uninitialized, read a line from it.
    fn readln(&mut self, cid: ClientId) -> Result<Option<String>> {
        let rdr = self.clients.get_mut(&cid).ok_or(ErrorKind::ClientNotFound)?;
        let mut buf = String::new();
        match rdr.read_line(&mut buf) {
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => bail!(e),
            Ok(_) => Ok(Some(buf)),
        }
    }

    /// Try to read a header from cid's socket
    pub fn try_read_header(&mut self, cid: ClientId) -> Result<Option<Header>> {
        match self.readln(cid)? {
            None => Ok(None),
            Some(buf) => match header(&buf) {
                nom::IResult::Done(_, x) => Ok(Some(x)),
                nom::IResult::Error(e) => bail!(e),
                nom::IResult::Incomplete{..} => Ok(None),    // FIXME: data removed from buffer
            }
        }
    }
}
