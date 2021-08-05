use error_chain::*;
use inotify::WatchDescriptor;
use std::{
    collections::{hash_map::*, hash_set::*},
    fmt, io,
};

pub type Map<K, V> = HashMap<K, V, RandomState>;
pub type Set<K> = HashSet<K, RandomState>;
pub type FileId = WatchDescriptor;
pub type ClientId = usize;
pub type Offset = i64;

error_chain! {
    foreign_links {
        Io(io::Error);
        Nix(nix::Error);
        Nom(nom::ErrorKind);
        Ignore(ignore::Error);
        Fmt(fmt::Error);
    }
    errors {
        NoonesInterested
        AlreadyConnected
        ClientNotFound
        IllegalFile
        FileNotWatched
    }
}
