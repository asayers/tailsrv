use index::*;
use nom;
use std::path::*;
use std::str;

#[derive(Debug)]
pub enum Header {
    List,
    Stream { path: PathBuf, index: Index },
    Stats,
}

// TODO: Unit tests
named!(pub header<Header>, alt!(list_header | stream_header | stats_header));
named!(
    list_header<Header>,
    do_parse!(tag!("list") >> (Header::List))
);
named!(
    stats_header<Header>,
    do_parse!(tag!("stats") >> (Header::Stats))
);
named!(
    stream_header<Header>,
    do_parse!(
        tag!("stream ")
            >> path: path
            >> tag!(" from ")
            >> index: index
            >> (Header::Stream {
                path: path,
                index: index
            })
    )
);
named!(
    path<PathBuf>,
    map!(take_until!(" "), |x| Path::new(str::from_utf8(x).unwrap())
        .to_owned())
);

// TODO: Unit tests
named!(
    index<Index>,
    alt!(byte_idx | line_idx | seqnum_idx | start_idx | end_idx)
);
named!(
    byte_idx<Index>,
    do_parse!(tag!("byte ") >> bytes: natural >> (Index::Byte(bytes as i64)))
);
named!(
    line_idx<Index>,
    do_parse!(tag!("line ") >> lines: natural >> (Index::Line(lines as i64)))
);
named!(
    seqnum_idx<Index>,
    do_parse!(tag!("seqnum ") >> seqnum: natural >> (Index::SeqNum(seqnum)))
);
named!(start_idx<Index>, do_parse!(tag!("start") >> (Index::Start)));
named!(end_idx<Index>, do_parse!(tag!("end") >> (Index::End)));
named!(
    natural<usize>,
    flat_map!(recognize!(nom::digit), parse_to!(usize))
);
