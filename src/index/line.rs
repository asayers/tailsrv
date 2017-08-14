use fs2::FileExt;
use memchr::Memchr;
use std::fs::File;
use memmap::*;

/// Returns the byte-offset of the start of the `cnt`th line within the given file. Line- and
/// byte-counts are both 0-indexed.
///
/// ```
/// assert_eq!(linebyte("test_data/file.txt", 0), Some(0));
/// assert_eq!(linebyte("test_data/file.txt", 1), Some(4));
/// assert_eq!(linebyte("test_data/file.txt", 2), Some(11));
/// assert_eq!(linebyte("test_data/file.txt", 3), None);
/// ```
pub fn linebyte(file: &File, cnt: usize) -> Option<usize> {
    if cnt == 0 { return Some(0); }
    file.lock_exclusive().expect("Lock file to resolve index"); // Try to make mmaping safer
    let mmap = Mmap::open(&file, Protection::Read).unwrap();
    let buf = unsafe { mmap.as_slice() };
    Memchr::new(b'\n', buf).nth(cnt - 1)
}

pub fn rlinebyte(file: &File, cnt: usize) -> Option<usize> {
    if cnt == 0 { return Some(0); }
    file.lock_exclusive().expect("Lock file to resolve index"); // Try to make mmaping safer
    let mmap = Mmap::open(&file, Protection::Read).unwrap();
    let buf = unsafe { mmap.as_slice() };
    Memchr::new(b'\n', buf).rev().nth(cnt - 1)
}
