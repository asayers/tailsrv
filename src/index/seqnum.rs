use fs2::FileExt;
use integer_encoding::VarInt;
use memmap::MmapOptions;
use std::fs::File;
use index::cache::*;

type SeqNum = usize;
type ByteOffset = usize;

// FIXME: Let's not mmap log files...
pub fn seqbyte(file: &File, mut target: SeqNum, cache: Option<IndexCache>) -> Option<usize> {
    file.lock_exclusive().expect("Lock file to resolve index"); // Try to make mmaping safer
    let mut offset: usize = 0;
    let foo = cache.map(|c| c.lookup(target));
    if let Some((pred_seqnum, pred_offset)) = foo {
        offset = pred_offset;
        target -= pred_seqnum;
    }
    let mmap = unsafe { MmapOptions::new().offset(offset).map(&file).unwrap() };
    let (ret, new_entries) = build_index(&mmap, target);
    if let Some(c) = cache { c.append(new_entries); }
    ret
}

const PAGE_SIZE: usize = 4 * 1024;

pub fn build_index(logfile: &[u8], target: SeqNum) -> (Option<ByteOffset>, Vec<(SeqNum, ByteOffset)>) {
    let mut pairs = vec![];
    let mut last_written_byte = 0;
    let mut byte = 0;
    for seqno in 0..target {
        if byte > logfile.len() { return (None, pairs); }
        let (len, n) = usize::decode_var(&logfile[byte..]);
        byte += len + n;
        if byte/PAGE_SIZE > last_written_byte/PAGE_SIZE {
            // let's write it!
            last_written_byte = byte;
            pairs.push((seqno, byte));
        }
    }
    (Some(byte),pairs)
}
