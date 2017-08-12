use ignore::{WalkBuilder,Walk};
use same_file::*;
use std::path::*;

pub fn file_is_valid(path: &Path) -> bool {
    valid_files().any(|entry| {
        is_same_file(entry.unwrap().path(), path).unwrap_or(false)
    })
}

// TODO: Regular files only
// TODO: Sort
pub fn valid_files() -> Walk {
    WalkBuilder::new(".")
        .git_global(false)   // Parsing git-related files is surprising
        .git_ignore(false)   // behaviour in the context of tailsrv, so
        .git_exclude(false)  // let's not read those files.
        .ignore(true)   // However, we *should* read generic ".ignore" files...
        .hidden(true)   // and ignore dotfiles (so clients can't read the .ignore files)
        .parents(false) // Don't search the parent directory for .ignore files.
        .build()
        // .filter(|e| {
        //     e.unwrap_or_else(return false)
        //         .file_type().unwrap_or_else(return false);
        //         .is_file()
        // })
}
