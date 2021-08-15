use crate::tracker::Tracker;
use crate::FILE_LENGTH;
use log::*;
use once_cell::sync::OnceCell;
use std::{
    ops::Neg,
    str::FromStr,
    sync::{atomic::Ordering, Mutex},
};
use thiserror::*;

#[derive(Debug)]
pub enum Index {
    End,
    Byte(i64),
    Idx(i64),
}

// TODO: Unit tests
impl FromStr for Index {
    type Err = Error;
    fn from_str(s: &str) -> Result<Index> {
        let mut tokens = s.split(' ').map(|x| x.trim());
        let mut token = || tokens.next().ok_or(Error::NotEnoughTokens);
        let first = token()?;
        if let Ok(x) = first.parse::<i64>() {
            return Ok(Index::Idx(x));
        }
        match first {
            "" | "end" => Ok(Index::End),
            "byte" => Ok(Index::Byte(token()?.parse()?)),
            _ => Err(Error::UnknownIndex),
        }
    }
}

pub static TRACKERS: OnceCell<Mutex<Tracker>> = OnceCell::new();

/// Resolves an index to a byte offset.
///
/// `None` means that the index refers to a position beyond the end of the file and we don't have
/// enough information to resolve it yet.
// TODO: Unit tests
pub fn resolve_index(idx: Index) -> Result<Option<usize>> {
    Ok(match idx {
        Index::End => Some(FILE_LENGTH.load(Ordering::SeqCst) as usize),
        Index::Byte(x) if x >= 0 => Some(x as usize),
        Index::Byte(x) => Some(FILE_LENGTH.load(Ordering::SeqCst) as usize - (x.neg() as usize)),
        Index::Idx(x) => TRACKERS.get().unwrap().lock().unwrap().lookup(x),
    })
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unknown index")]
    UnknownIndex,
    #[error("Expected another token")]
    NotEnoughTokens,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Int(#[from] std::num::ParseIntError),
}
