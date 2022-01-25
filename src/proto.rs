use crate::FILE_LENGTH;
use log::*;
use once_cell::sync::OnceCell;
use std::{
    ops::Neg,
    str::FromStr,
    sync::atomic::Ordering,
};
use thiserror::*;

#[derive(Debug)]
pub enum Req {
    End,
    Byte(i64),
}

// TODO: Unit tests
impl FromStr for Req {
    type Err = Error;
    fn from_str(s: &str) -> Result<Req> {
        let mut tokens = s.split(' ').map(|x| x.trim());
        let mut token = || tokens.next().ok_or(Error::NotEnoughTokens);
        let first = token()?;
        if let Ok(x) = first.parse::<i64>() {
            return Ok(Req::Byte(x));
        }
        match first {
            "" | "end" => Ok(Req::End),
            "byte" => Ok(Req::Byte(token()?.parse()?)),
            _ => Err(Error::UnknownIndex),
        }
    }
}

/// Resolves an index to a byte offset.
// TODO: Unit tests
pub fn resolve_index(idx: Req) -> Result<usize> {
    Ok(match idx {
        Req::End => FILE_LENGTH.load(Ordering::SeqCst) as usize,
        Req::Byte(x) if x >= 0 => x as usize,
        Req::Byte(x) => FILE_LENGTH.load(Ordering::SeqCst) as usize - (x.neg() as usize),
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
