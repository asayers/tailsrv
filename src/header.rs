use crate::index::*;
use nom::*;
use std::{path::*, str};

named!(
    path<PathBuf>,
    map!(take_until!(" "), |x| Path::new(str::from_utf8(x).unwrap())
        .to_owned())
);

// TODO: Unit tests
named!(
    pub index<Index>,
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
