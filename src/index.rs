use memchr::Memchr;
use memmap::*;
use nom;
use std::fs::File;
use std::ops::Neg;
use types::*;

#[derive(Debug)]
pub enum Index {
    Byte(i64),
    Line(i64),
    Start,
    End,
}

// TODO: Unit tests
named!(pub index<Index>, alt!( byte_idx | line_idx | start_idx | end_idx ));
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
pub fn resolve_index(file: &File, idx: Index) -> Result<Option<usize>> {
    Ok(match idx {
        Index::Byte(x) if x >= 0 => Some(x as usize),
        Index::Byte(x) => Some(file.metadata()?.len() as usize - (x.neg() as usize)),
        Index::Line(x) if x >= 0 => linebyte(file, x as usize),
        Index::Line(x) => rlinebyte(file, x.neg() as usize),
        Index::Start => Some(0),
        Index::End => Some(file.metadata()?.len() as usize),
    })
}

/// Returns the byte-offset of the start of the `cnt`th line within the given file. Line- and
/// byte-counts are both 0-indexed.
///
/// ```
/// assert_eq!(linebyte("test_data/file.txt", 0), Some(0));
/// assert_eq!(linebyte("test_data/file.txt", 1), Some(4));
/// assert_eq!(linebyte("test_data/file.txt", 2), Some(11));
/// assert_eq!(linebyte("test_data/file.txt", 3), None);
/// ```
fn linebyte(file: &File, cnt: usize) -> Option<usize> {
    if cnt == 0 { return Some(0); }
    let mmap = Mmap::open(&file, Protection::Read).unwrap();
    let buf = unsafe { mmap.as_slice() };
    Memchr::new(b'\n', buf).nth(cnt - 1)
}

fn rlinebyte(file: &File, cnt: usize) -> Option<usize> {
    if cnt == 0 { return Some(0); }
    let mmap = Mmap::open(&file, Protection::Read).unwrap();
    let buf = unsafe { mmap.as_slice() };
    Memchr::new(b'\n', buf).rev().nth(cnt - 1)
}
