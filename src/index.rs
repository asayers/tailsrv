use std::{
    convert::TryFrom,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

/// Records the locations of all newlines in a file.
pub struct DenseIndex {
    delim: u8,
    offset: usize,
    newlines: Vec<usize>,
    file: BufReader<File>,
}

impl DenseIndex {
    pub fn new(path: &Path, delim: u8) -> std::io::Result<DenseIndex> {
        let mut ret = DenseIndex {
            delim,
            offset: 0,
            file: BufReader::new(File::open(path)?),
            newlines: vec![],
        };
        ret.update()?;
        Ok(ret)
    }

    /// `len` is the length of the line _without_ newline.
    pub fn push_line(&mut self, len: usize) {
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
                self.newlines.push(self.offset + x);
                self.offset += x + 1;
                self.file.consume(x + 1);
            } else {
                let x = buf.len();
                self.offset += x;
                self.file.consume(x);
            }
        }
    }

    /// The offset of the byte after the `n`th delimiter.
    pub fn lookup(&self, n: i64) -> Option<usize> {
        let len = i64::try_from(self.len()).unwrap();
        if n == 0 {
            Some(0)
        } else if n > len {
            None
        } else if n < -len {
            None
        } else if n < 0 {
            let n = usize::try_from(len + n).unwrap();
            Some(self.newlines[n - 1] + 1)
        } else {
            let n = usize::try_from(n).unwrap();
            Some(self.newlines[n - 1] + 1)
        }
    }

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
//         let lines = DenseIndex::from_file(f.path()).unwrap();
//         assert_eq!(lines.len(), 3);
//         // line2range never includes the newline char, hence the non-contiguous
//         // ranges
//         assert_eq!(lines.line2range(0), 0..7);
//         assert_eq!(lines.line2range(1), 8..11);
//         assert_eq!(lines.line2range(2), 12..15);
//         assert_eq!(s.len(), 16);
//     }
// }
