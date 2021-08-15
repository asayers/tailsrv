#[cfg(feature = "prefixed")]
mod prefixed;

#[cfg(feature = "prefixed")]
use self::prefixed::*;
use crate::tracker::Tracker;
use log::*;
use once_cell::sync::OnceCell;
use std::{convert::TryFrom, fs::File, ops::Neg, str::FromStr, sync::Mutex};
use thiserror::*;

#[derive(Debug)]
pub enum Index {
    Start,
    End,
    Byte(i64),
    Line(i64),
    Zero(i64),
    SeqNum(usize),
}

// TODO: Unit tests
impl FromStr for Index {
    type Err = Error;
    fn from_str(s: &str) -> Result<Index> {
        info!("Parsing {}", s);
        let mut tokens = s.split(' ').map(|x| x.trim());
        let mut token = || tokens.next().ok_or(Error::NotEnoughTokens);
        match token()? {
            "" | "start" => Ok(Index::Start),
            "end" => Ok(Index::End),
            "byte" => Ok(Index::Byte(token()?.parse()?)),
            "line" => Ok(Index::Line(token()?.parse()?)),
            "zero" => Ok(Index::Zero(token()?.parse()?)),
            "seqnum" => Ok(Index::SeqNum(token()?.parse()?)),
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
pub fn resolve_index(zero_terminated: bool, file: &mut File, idx: Index) -> Result<Option<usize>> {
    Ok(match idx {
        Index::Start => Some(0),
        Index::End => Some(file.metadata()?.len() as usize),
        Index::Byte(x) if x >= 0 => Some(x as usize),
        Index::Byte(x) => Some(file.metadata()?.len() as usize - (x.neg() as usize)),
        Index::Line(x) => {
            if zero_terminated {
                panic!()
            }
            if x < 0 {
                todo!()
            }
            Some(
                TRACKERS
                    .get()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .lookup(usize::try_from(x).unwrap()),
            )
        }
        Index::Zero(x) => {
            if !zero_terminated {
                panic!()
            }
            if x < 0 {
                todo!()
            }
            Some(
                TRACKERS
                    .get()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .lookup(usize::try_from(x).unwrap()),
            )
        }
        #[cfg(feature = "prefixed")]
        Index::SeqNum(x) => seqbyte(file, x),
        #[cfg(not(feature = "prefixed"))]
        Index::SeqNum(_) => return Err(Error::PrefixedNotEnabled),
    })
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unknown index")]
    UnknownIndex,
    #[error("Line-prefixed support not enabled")]
    PrefixedNotEnabled,
    #[error("Expected another token")]
    NotEnoughTokens,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Int(#[from] std::num::ParseIntError),
}
