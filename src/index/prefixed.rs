use fs2::FileExt;
use integer_encoding::VarInt;
use memmap::{Mmap, Protection};
use std::fs::File;

pub fn seqbyte(file: &File, seqno: usize) -> Option<usize> {
    file.lock_exclusive().expect("Lock file to resolve index"); // Try to make mmaping safer
    let mmap = Mmap::open(&file, Protection::Read).unwrap();
    let buf = unsafe { mmap.as_slice() };
    let mut byte = 0;
    for _ in 0..seqno {
        let (len, n) = usize::decode_var(&buf[byte..]);
        byte += len + n;
    }
    Some(byte)
}
