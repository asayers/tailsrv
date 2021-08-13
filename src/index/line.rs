use fs2::FileExt;
use memchr::Memchr;
use memmap::Mmap;
use std::fs::File;

/// Returns the byte-offset of the start of the `cnt`th line within the given file. Line- and
/// byte-counts are both 0-indexed.
// FIXME: Let's not mmap log files...
pub fn linebyte(file: &File, cnt: usize, delim: u8) -> Option<usize> {
    if cnt == 0 {
        return Some(0);
    }
    file.lock_exclusive().expect("Lock file to resolve index"); // Try to make mmaping safer
    let mmap = unsafe { Mmap::map(file).unwrap() };
    Memchr::new(delim, &mmap).nth(cnt - 1)
}

// FIXME: Let's not mmap log files...
pub fn rlinebyte(file: &File, cnt: usize, delim: u8) -> Option<usize> {
    if cnt == 0 {
        return Some(0);
    }
    file.lock_exclusive().expect("Lock file to resolve index"); // Try to make mmaping safer
    let mmap = unsafe { Mmap::map(file).unwrap() };
    Memchr::new(delim, &mmap).rev().nth(cnt - 1)
}
