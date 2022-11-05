use memmap2::Mmap;

type LineNum = usize; // zero-indexed
type ByteOffset = usize; // zero-indexed

pub struct Cache {
    stride: usize,
    data: Vec<ByteOffset>,
}

impl Cache {
    pub fn new(initial_stride: usize, max_size: usize) -> Cache {
        let mut data = Vec::with_capacity(max_size);
        data.push(0);
        Cache {
            stride: initial_stride,
            data,
        }
    }

    pub fn lookup(&mut self, x: LineNum, mmap: Mmap) -> Option<ByteOffset> {
        let highest = self.highest_known_line();
        if x > highest {
            self.extend_to(x, &mmap);
            self.lookup_within(x, &mmap)
        } else {
            self.lookup_within(x, &mmap)
        }
    }

    fn highest_known_line(&self) -> LineNum {
        self.stride * self.data.len()
    }

    fn highest_known_byte(&self) -> ByteOffset {
        self.data.last().copied().unwrap_or(0)
    }

    fn get_lower_bound(&self, x: LineNum) -> (LineNum, ByteOffset) {
        let idx = x / self.stride;
        match self.data.get(idx) {
            Some(byte) => (idx, *byte),
            None => (self.highest_known_line(), self.highest_known_byte()),
        }
    }

    fn double_stride(&mut self) {
        self.stride *= 2;
        let mut n = 0;
        self.data.retain(|_| {
            n += 1;
            n % 2 == 0
        });
    }

    fn lookup_within(&self, x: LineNum, mmap: &Mmap) -> Option<ByteOffset> {
        let (start_line, start_byte) = self.get_lower_bound(x);
        if x == start_line {
            return Some(start_byte);
        } else {
            let newline_idx = x - start_line;
            let newline_pos = memchr::memchr_iter(b'\n', &mmap[start_byte..]).nth(newline_idx)?;
            Some(newline_pos + 1)
        }
    }

    fn extend_to(&mut self, x: LineNum, mmap: &Mmap) {
        let start_byte = self.highest_known_byte();
        let mut line = self.highest_known_line();
        for byte in memchr::memchr_iter(b'\n', &mmap[start_byte..]) {
            line += 1;
            if line % self.stride == 0 {
                self.data.push(byte);
                if self.data.len() == self.data.capacity() {
                    self.double_stride();
                }
            }
            if line > x {
                break;
            }
        }
    }
}
