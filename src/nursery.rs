use header::*;
use mio::net::*;
use nom;
use std::io::{self, BufReader, BufRead};
use types::*;

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
pub struct Nursery(Map<ClientId, BufReader<TcpStream>>);

impl Nursery {
    pub fn new() -> Nursery {
        Nursery(Map::new())
    }

    /// Add a client to the nursery.
    pub fn register(&mut self, cid: ClientId, sock: TcpStream) -> Result<()> {
        info!("Registering client {}", cid);
        self.0.insert(cid, BufReader::new(sock));
        Ok(())
    }

    /// Remove a client from the nursery.
    pub fn graduate(&mut self, cid: ClientId) -> Result<TcpStream> {
        info!("Graduating client {}", cid);
        let sock = self.0.remove(&cid).ok_or(ErrorKind::ClientNotFound)?.into_inner();
        Ok(sock)
    }

    /// Try to read a header from cid's socket
    pub fn try_read_header(&mut self, cid: ClientId) -> Result<Option<Header>> {
        info!("Trying to read a header from client {}", cid);
        let rdr = self.0.get_mut(&cid).ok_or(ErrorKind::ClientNotFound)?;
        match rdr.fill_buf() {
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => bail!(e),
            Ok(buf) => match header(buf) {
                nom::IResult::Done(_, x) => Ok(Some(x)), // Leave the data in the buffer, it's fine
                nom::IResult::Error(e) => bail!(e),
                nom::IResult::Incomplete{..} => Ok(None), // FIXME: data gets removed somehow
            }
        }
    }
}
