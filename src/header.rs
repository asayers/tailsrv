use error::*;
use nom;
use std::path::*;

#[derive(Debug)]
pub enum Header {
    List,
    Stream{ path: PathBuf, offset: i64 },
}

pub fn parse_header(buf: &str) -> Result<Header> {
    match header(buf) {
        nom::IResult::Done(_, x) => Ok(x),
        nom::IResult::Error(e) => Err(e.into()),
        nom::IResult::Incomplete{..} => Err(ErrorKind::HeaderNotEnoughBytes.into()),
    }
}

named!(header<&str,Header>, alt!(list_header | stream_header));
named!(list_header<&str,Header>, do_parse!(
    tag!("list") >>
    (Header::List)
));
named!(stream_header<&str,Header>, do_parse!(
    tag!("stream ") >>
    path: path >>
    tag!(" ") >>
    offset: natural >>
    (Header::Stream{ path: path, offset: offset as i64 })
));
named!(natural<&str, usize>, flat_map!(recognize!(nom::digit), parse_to!(usize)));
named!(path<&str, PathBuf>, map!(take_until!(" "), |x| PathBuf::from(x)));
