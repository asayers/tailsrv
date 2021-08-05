use ignore::{Walk, WalkBuilder};
use same_file::*;
use std::fmt::Write;
use std::path::*;
use types::*;

// TODO: Sort
fn valid_files() -> Walk {
    WalkBuilder::new(".")
        .git_global(false) // Parsing git-related files is surprising
        .git_ignore(false) // behaviour in the context of tailsrv, so
        .git_exclude(false) // let's not read those files.
        .ignore(true) // However, we *should* read generic ".ignore" files...
        .hidden(true) // and ignore dotfiles (so clients can't read the .ignore files)
        .parents(false) // Don't search the parent directory for .ignore files.
        .build()
}

pub fn file_is_valid(path: &Path) -> bool {
    for entry in valid_files() {
        match entry {
            Err(e) => warn!("{}", e),
            Ok(ref entry) => {
                if entry.file_type().map(|x| x.is_file()).unwrap_or(false)
                    && is_same_file(path, entry.path()).unwrap_or(false)
                {
                    return true;
                }
            }
        }
    }
    false
}

pub fn list_files() -> Result<String> {
    let mut buf = String::new();
    for entry in valid_files() {
        match entry {
            Err(e) => warn!("{}", e),
            Ok(ref entry) if entry.file_type().map(|x| x.is_file()).unwrap_or(false) => {
                writeln!(buf, "{}", entry.path().display())?
            }
            _ => {}
        }
    }
    Ok(buf)
}
