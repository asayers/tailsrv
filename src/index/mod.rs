mod line;
#[cfg(feature = "prefixed")]
mod prefixed;

use self::line::*;
#[cfg(feature = "prefixed")]
use self::prefixed::*;
use std::fs::File;
use std::ops::Neg;
use std::{fmt, io};
use thiserror::*;

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
        #[cfg(feature = "prefixed")]
        Index::SeqNum(x) => seqbyte(file, x),
        #[cfg(not(feature = "prefixed"))]
        Index::SeqNum(_) => return Err(Error::PrefixedNotEnabled),
        Index::Start => Some(0),
        Index::End => Some(file.metadata()?.len() as usize),
    })
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Client not found")]
    ClientNotFound,
    #[error("File not watched")]
    FileNotWatched,
    #[error("Line-prefixed support not enabled")]
    PrefixedNotEnabled,
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Nix(#[from] nix::Error),
    #[error("{0}")]
    Fmt(#[from] fmt::Error),
}
