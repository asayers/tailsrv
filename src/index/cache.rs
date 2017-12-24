use byteorder::*;
use fs2::FileExt;
use memmap::Mmap;
use std::cmp::*;
use std::fs::{OpenOptions, File};
use std::path::Path;

// Index cache
// ===========
//
// An index cache is a map from Index -> usize (byte offset). It caches the output of the
// `resolve_index` function.
//
// It is updated semi-lazily. By this I mean, until a client performs a seek, the index cache is
// not written; however, when a client performs a seek, the cache is updated up the the point the
// client requested.
//
// The cache is written sparsely. (ie. not every (Index, usize) pair will be written.) The
// principal is that only one pair should be written per page of log file, and it should correspond
// to the first message in that page. This means that when the index cache is up-to-date, a client
// should be able to query the cache and get the offset of a message which may precede the target,
// but which must be in the same page as the target.



/// The sequence number of a message. The first message in the file has `SeqNum(0)`.
type SeqNum = usize;
type ByteOffset = usize;

/// A mapping from a message's sequence number to its byte offset in the log file.
///
/// A `IndexCache` index is always backed by a file. The file is a cache, and may be empty or
/// contain only a prefix of the messages. The file must be stored on a filesystem capable of
/// backing memory maps (ie. be careful with NFS).
#[derive(Debug)]
pub struct IndexCache(File);

// /// We assume that the given index file correctly maps sequence numbers to offsets into the given
// /// log file, up to a certain message, but that the log may contain new data which was appended to
// /// it since the index was last written. This function brings the index up-to-date by starting
// /// where the index leaves off and, from there, jumping through the log file to find the offsets of
// /// subsequent messages. These offsets are written back to the index file.
// fn update_index(log_path: &Path, idx_file: &mut File) {
//     let mut log_file = File::open(log_path).unwrap();
//     let last_offset = match idx_file.seek(SeekFrom::End(-8)) {
//         Ok(_) => idx_file.read_u64::<BigEndian>().expect("Read last entry in index file"),
//         Err(_) => 0,
//     };
//     let new_data = log_file.metadata().expect("Query log file metadata").len() - last_offset;
//     if new_data < 8 {
//         info!("The index file is already up-to-date");
//     } else {
//         info!("The log file has grown by {} bytes since the index was last written. Updating...", new_data);
//         log_file.seek(SeekFrom::Start(last_offset)).expect("Seek to last offset");
//         // let log_file = Mmap::open_with_offset(&log_file, Protection::Read, last_offset as usize, new_data as usize).unwrap();
//         // let log_file = unsafe { log_file.as_slice() };
//         // let mut log_file = Cursor::new(log_file);
//         loop {
//             if let Ok(len) = log_file.read_u64::<BigEndian>() {
//             if let Ok(offset) = log_file.seek(SeekFrom::Current(len as i64 - 8)) {
//                 idx_file.write_u64::<BigEndian>(offset).expect("Write entry to index file");
//             } else {
//                 break;
//             } } else { break; }
//         }
//     }
// }

impl IndexCache {
    /// Load an index from a file. The file is created if it doesn't exist already.
    pub fn open(path: &Path) -> IndexCache {
        let mut file = OpenOptions::new()
            .read(true).append(true).create(true)
            .open(path).expect("Open index file");
        file.lock_exclusive().expect("Lock index file"); // Try to make mmaping safer
        IndexCache(file)
    }

    pub fn append(&mut self, new_entries: Vec<(SeqNum, ByteOffset)>) {
        for (seqnum, offset) in new_entries {
            self.0.write_u64::<BigEndian>(seqnum as u64);
            self.0.write_u64::<BigEndian>(offset as u64);
        }
    }

    pub fn lookup(&self, msg: SeqNum) -> (SeqNum, ByteOffset) {
        // This is unsafe if the index file is modified concurrently. We make an effort to prevent
        // this by taking a flock. (See `load`).
        let mmap = unsafe { Mmap::map(&self.0).unwrap() };
        assert!(mmap.len() % 16 == 0);
        let get_pair = |i: usize| {
            let seqnum = BigEndian::read_u64(&mmap[(i*16)  ..(i*16)+8 ]);
            let offset = BigEndian::read_u64(&mmap[(i*16)+8..(i*16)+16]);
            (seqnum as usize, offset as usize)
        };

        debug!("binsearch start: {:?}", msg);
        let mut lower = 0;
        let mut upper = (mmap.len() / 16) - 1;

        // First just check if the target is outside the index
        let mut current = get_pair(upper);
        if current.0 <= msg { return current; }

        // If not, it's time to binary search!
        while upper - lower > 1 {
            let mid = (lower + upper) / 2;
            let current = get_pair(mid);
            debug!("binsearch step: {:?}", current);
            match current.0.cmp(&msg) {
                Ordering::Less  => lower = mid,
                Ordering::Equal => { lower = mid; upper = mid },
                Ordering::Greater => upper = mid,
            }
        }
        debug!("binsearch done: {:?}", current);
        current
    }
}

struct Indexer<'a> {
    file: &'a File,
    cache: IndexCache,
    build_index: fn(&[u8], SeqNum) -> (Option<ByteOffset>, Vec<(SeqNum, ByteOffset)>),
}

impl<'a> Indexer<'a> {
    fn lookup(target: SeqNum) -> Option<ByteOffset> {

    }
}
