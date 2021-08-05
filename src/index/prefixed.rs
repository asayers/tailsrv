use fs2::FileExt;
use integer_encoding::VarInt;
use memmap::Mmap;
use std::fs::File;

// FIXME: Let's not mmap log files...
pub fn seqbyte(file: &File, seqno: usize) -> Option<usize> {
    file.lock_exclusive().expect("Lock file to resolve index"); // Try to make mmaping safer
    let mmap = unsafe { Mmap::map(file).unwrap() };
    let mut byte = 0;
    for _ in 0..seqno {
        let (len, n) = usize::decode_var(&mmap[byte..]);
        byte += len + n;
    }
    Some(byte)
}
