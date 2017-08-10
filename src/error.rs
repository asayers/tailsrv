use ignore;
use nix;
use nom;
use std::io;

error_chain! {
    foreign_links {
        Io(io::Error);
        Nix(nix::Error);
        Nom(nom::ErrorKind);
        Ignore(ignore::Error);
    }
    errors {
        BookmarkMissing
        NoonesInterested
        HeaderNotEnoughBytes
        HeaderTooSlow
        IllegalFile
    }
}
