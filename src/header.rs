use nom;
use std::path::*;

#[derive(Debug)]
pub enum Header {
    List,
    Stream{ path: PathBuf, offset: i64 },
    Stats,
}

named!(pub header<&str,Header>, alt!(list_header | stream_header | stats_header));
named!(list_header<&str,Header>, do_parse!(
    tag!("list") >>
    (Header::List)
));
named!(stats_header<&str,Header>, do_parse!(
    tag!("stats") >>
    (Header::Stats)
));
named!(stream_header<&str,Header>, do_parse!(
    tag!("stream ") >>
    path: path >>
    tag!(" from byte ") >>
    offset: natural >>
    (Header::Stream{ path: path, offset: offset as i64 })
));
named!(natural<&str, usize>, flat_map!(recognize!(nom::digit), parse_to!(usize)));
named!(path<&str, PathBuf>, map!(take_until!(" "), |x| PathBuf::from(x)));
