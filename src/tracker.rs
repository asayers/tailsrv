use std::{
    fs::File,
    io::{BufRead, BufReader},
    ops::Range,
    path::Path,
};

/// Records the locations of all newlines in a file.
pub struct Tracker {
    delim: u8,
    offset: u64,
    newlines: Vec<u64>,
    file: BufReader<File>,
}

impl Tracker {
    pub fn new(path: &Path, delim: u8) -> std::io::Result<Tracker> {
        let mut ret = Tracker {
            delim,
            offset: 0,
            file: BufReader::new(File::open(path)?),
            newlines: vec![],
        };
        ret.update()?;
        Ok(ret)
    }

    /// `len` is the length of the line _without_ newline.
    pub fn push_line(&mut self, len: u64) {
        self.newlines.push(self.offset + len);
        self.offset += len + 1;
    }

    /// Reads the file, starting at EOF the last time this function was
    /// called, up to the current EOF, adding line-break offsets to `newlines`.
    pub fn update(&mut self) -> std::io::Result<()> {
        loop {
            let buf = self.file.fill_buf()?;
            if buf.is_empty() {
                return Ok(());
            }
            if let Some(x) = memchr::memchr(self.delim, buf) {
                self.newlines.push(self.offset + x as u64);
                self.offset += x as u64 + 1;
                self.file.consume(x + 1);
            } else {
                let x = buf.len();
                self.offset += x as u64;
                self.file.consume(x);
            }
        }
    }

    /// Gives a byte-range which doesn't include the newline
    pub fn line2range(&self, line: usize) -> Range<u64> {
        let lhs = if line == 0 {
            0
        } else {
            self.newlines[line - 1] as u64 + 1
        };
        let rhs = self.newlines[line] as u64;
        lhs..rhs
    }

    // pub fn line2pos(&self, mut line: usize) -> csv::Position {
    //     line += 1;
    //     let mut pos = csv::Position::new();
    //     pos.set_line(line as u64)
    //         .set_byte(self.line2range(line).start)
    //         .set_record(0);
    //     pos
    // }

    pub fn len(&self) -> usize {
        self.newlines.len()
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use std::io::{BufReader, Cursor, Write};
//     use tempfile::*;

//     #[test]
//     fn test() {
//         let mut f = NamedTempFile::new().unwrap();
//         let s = b"foo,bar\n1,2\n3,4\n";
//         f.write_all(s).unwrap();
//         let lines = Tracker::from_file(f.path()).unwrap();
//         assert_eq!(lines.len(), 3);
//         // line2range never includes the newline char, hence the non-contiguous
//         // ranges
//         assert_eq!(lines.line2range(0), 0..7);
//         assert_eq!(lines.line2range(1), 8..11);
//         assert_eq!(lines.line2range(2), 12..15);
//         assert_eq!(s.len(), 16);
//     }
// }
