use inotify::WatchDescriptor;
use std::{
    collections::{hash_map::*, hash_set::*},
    fmt, io,
};
use thiserror::*;

pub type Map<K, V> = HashMap<K, V, RandomState>;
pub type Set<K> = HashSet<K, RandomState>;
pub type FileId = WatchDescriptor;
pub type ClientId = usize;
pub type Offset = i64; /* bytes */
pub type FileLength = u64; /* bytes */

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
