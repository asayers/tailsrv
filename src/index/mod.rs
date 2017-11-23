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

/// Resolves an index to a byte offset.
///
/// `None` means that the index refers to a position beyond the end of the file and we don't have
/// enough information to resolve it yet.
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
