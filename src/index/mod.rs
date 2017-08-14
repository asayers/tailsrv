use nom;
use std::fs::File;
use std::ops::Neg;
use types::*;

mod line;     use self::line::*;
mod prefixed; use self::prefixed::*;

#[derive(Debug)]
pub enum Index {
    Byte(i64),
    Line(i64),
    SeqNum(usize),
    Start,
    End,
}

// TODO: Unit tests
named!(pub index<Index>, alt!( byte_idx | line_idx | seqnum_idx | start_idx | end_idx ));
named!(byte_idx<Index>, do_parse!(
    tag!("byte ") >>
    bytes: natural >>
    (Index::Byte(bytes as i64))
));
named!(line_idx<Index>, do_parse!(
    tag!("line ") >>
    lines: natural >>
    (Index::Line(lines as i64))
));
named!(seqnum_idx<Index>, do_parse!(
    tag!("seqnum ") >>
    seqnum: natural >>
    (Index::SeqNum(seqnum))
));
named!(start_idx<Index>, do_parse!(
    tag!("start") >>
    (Index::Start)
));
named!(end_idx<Index>, do_parse!(
    tag!("end") >>
    (Index::End)
));
named!(natural<usize>, flat_map!(recognize!(nom::digit), parse_to!(usize)));

/// Resolves an index to a byte offset. `None` means that the index refers to a position beyond the
/// end of the file and we don't have enough information to resolve it yet.
// TODO: Unit tests
pub fn resolve_index(file: &mut File, idx: Index) -> Result<Option<usize>> {
    Ok(match idx {
        Index::Byte(x) if x >= 0 => Some(x as usize),
        Index::Byte(x) => Some(file.metadata()?.len() as usize - (x.neg() as usize)),
        Index::Line(x) if x >= 0 => linebyte(file, x as usize),
        Index::Line(x) => rlinebyte(file, x.neg() as usize),
        Index::SeqNum(x) => seqbyte(file, x),
        Index::Start => Some(0),
        Index::End => Some(file.metadata()?.len() as usize),
    })
}
