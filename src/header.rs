use index::*;
use std::path::*;
use std::str;

#[derive(Debug)]
pub enum Header {
    List,
    Stream{ path: PathBuf, index: Index },
    Stats,
}

named!(pub header<Header>, alt!(list_header | stream_header | stats_header));
named!(list_header<Header>, do_parse!(
    tag!("list") >>
    (Header::List)
));
named!(stats_header<Header>, do_parse!(
    tag!("stats") >>
    (Header::Stats)
));
named!(stream_header<Header>, do_parse!(
    tag!("stream ") >>
    path: path >>
    tag!(" from ") >>
    index: index >>
    (Header::Stream{ path: path, index: index })
));
named!(path<PathBuf>, map!(take_until!(" "), |x| Path::new(str::from_utf8(x).unwrap()).to_owned()));
