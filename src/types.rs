use ignore;
use inotify::WatchDescriptor;
use nix;
use nom;
use std::collections::hash_map::*;
use std::collections::hash_set::*;
use std::io;
use std::fmt;

pub type Map<K,V> = HashMap<K, V, RandomState>;
pub type Set<K> = HashSet<K, RandomState>;
pub type FileId = WatchDescriptor;
pub type ClientId = usize;
pub type Offset = i64;  // A byte offset into a file

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
        IllegalFile
        ConnectionNotFound
        ClientNotFound
        FileNotWatched
    }
}
