//! Clean-room RFC 1951 §§3.1–3.3 and RFC 1950 §2 decoder.
//! The incremental parser retains at most a symbol's bits, its two canonical
//! alphabets and the caller's output. Matches read the last 32 KiB of that output.
use crate::work;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    BadStoredLength,
    BadBlockType,
    CodeOversubscribed,
    CodeIncomplete,
    RepeatWithoutPrevious,
    BadSymbol,
    DistanceTooFar,
    Truncated,
    OutputLimit,
    BadZlibHeader,
    PresetDictionary,
    BadMethod,
    WindowTooLarge,
    Adler32Mismatch,
}
use InflateError::*;
#[derive(Clone, Copy, Default)]
struct Bits {
    value: u64,
    count: u8,
}
impl Bits {
    fn read(&mut self, n: u8) -> Result<u32, InflateError> {
        if self.count < n {
            return Err(Truncated);
        }
        let v = self.value & ((1u64 << n) - 1);
        self.value >>= n;
        self.count -= n;
        Ok(v as u32)
    }
}
struct Huffman {
    counts: [u16; 16],
    first: [u16; 16],
    offset: [u16; 16],
    symbols: [u16; 288],
    empty: bool,
}
impl Huffman {
    fn empty() -> Self {
        Self {
            counts: [0; 16],
            first: [0; 16],
            offset: [0; 16],
            symbols: [0; 288],
            empty: true,
        }
    }
    fn build(lengths: &[u8], single: bool, empty: bool) -> Result<Self, InflateError> {
        let mut h = Self::empty();
        let mut total = 0;
        for &n in lengths {
            work::step();
            if n > 15 {
                return Err(BadSymbol);
            }
            if n != 0 {
                h.counts[n as usize] += 1;
                total += 1;
            }
        }
        if total == 0 {
            return if empty { Ok(h) } else { Err(CodeIncomplete) };
        }
        let mut space = 1i32;
        let mut offset = 0;
        let mut code = 0;
        for n in 1..=15 {
            work::step();
            space = space * 2 - h.counts[n] as i32;
            if space < 0 {
                return Err(CodeOversubscribed);
            }
            code = (code + h.counts[n - 1]) << 1;
            h.first[n] = code;
            h.offset[n] = offset;
            offset += h.counts[n];
        }
        if space != 0 && !(single && total == 1 && h.counts[1] == 1) {
            return Err(CodeIncomplete);
        }
        let mut next = h.offset;
        for (s, &n) in lengths.iter().enumerate() {
            work::step();
            if n != 0 {
                let i = &mut next[n as usize];
                h.symbols[*i as usize] = s as u16;
                *i += 1;
            }
        }
        h.empty = false;
        Ok(h)
    }
    fn symbol(&self, bits: &mut Bits) -> Result<u16, InflateError> {
        if self.empty {
            return Err(BadSymbol);
        }
        let mut code = 0;
        for n in 1..=15 {
            work::step();
            code = (code << 1) | bits.read(1)? as u16;
            let first = self.first[n];
            if code >= first && code - first < self.counts[n] {
                return Ok(self.symbols[(self.offset[n] + code - first) as usize]);
            }
        }
        Err(BadSymbol)
    }
}
#[derive(Clone, Copy)]
enum State {
    Header,
    StoredHeader,
    Stored(usize),
    DynamicCounts,
    CodeLengths(usize),
    Lengths(usize),
    Codes,
    Done,
}
/// Raw DEFLATE decoder. `feed` processes its bytes immediately; `finish` checks BFINAL.
pub struct Inflater {
    bits: Bits,
    state: State,
    last: bool,
    output: Vec<u8>,
    limit: usize,
    error: Option<InflateError>,
    lit: Huffman,
    dist: Huffman,
    cl: Huffman,
    hlit: usize,
    hdist: usize,
    hclen: usize,
    cl_lengths: [u8; 19],
    lengths: [u8; 320],
}
impl Inflater {
    pub fn new(limit: usize) -> Self {
        Self {
            bits: Bits::default(),
            state: State::Header,
            last: false,
            output: Vec::new(),
            limit,
            error: None,
            lit: Huffman::empty(),
            dist: Huffman::empty(),
            cl: Huffman::empty(),
            hlit: 0,
            hdist: 0,
            hclen: 0,
            cl_lengths: [0; 19],
            lengths: [0; 320],
        }
    }
    fn byte(&mut self, b: u8) -> Result<(), InflateError> {
        if self.output.len() == self.limit {
            return Err(OutputLimit);
        }
        self.output.push(b);
        Ok(())
    }
    fn end_block(&mut self) {
        self.state = if self.last {
            State::Done
        } else {
            State::Header
        };
    }
    fn step(&mut self) -> Result<bool, InflateError> {
        match self.state {
            State::Header => {
                let header = self.bits.read(3)?;
                self.last = header & 1 != 0;
                match (header >> 1) & 3 {
                    0 => {
                        let skip = self.bits.count % 8;
                        self.bits.read(skip)?;
                        self.state = State::StoredHeader;
                    }
                    1 => {
                        let mut l = [8; 288];
                        l[144..256].fill(9);
                        l[256..280].fill(7);
                        self.lit = Huffman::build(&l, true, false)?;
                        self.dist = Huffman::build(&[5; 32], true, false)?;
                        self.state = State::Codes;
                    }
                    2 => self.state = State::DynamicCounts,
                    _ => return Err(BadBlockType),
                }
            }
            State::StoredHeader => {
                let n = self.bits.read(16)?;
                let inverse = self.bits.read(16)?;
                if n ^ inverse != 65535 {
                    return Err(BadStoredLength);
                }
                self.state = State::Stored(n as usize);
            }
            State::Stored(0) => self.end_block(),
            State::Stored(n) => {
                let b = self.bits.read(8)? as u8;
                self.byte(b)?;
                self.state = State::Stored(n - 1);
            }
            State::DynamicCounts => {
                let hlit = self.bits.read(5)? as usize + 257;
                let hdist = self.bits.read(5)? as usize + 1;
                let hclen = self.bits.read(4)? as usize + 4;
                if hlit > 286 {
                    return Err(BadSymbol);
                }
                self.hlit = hlit;
                self.hdist = hdist;
                self.hclen = hclen;
                self.cl_lengths = [0; 19];
                self.state = State::CodeLengths(0);
            }
            State::CodeLengths(i) => {
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                self.cl_lengths[ORDER[i]] = self.bits.read(3)? as u8;
                if i + 1 == self.hclen {
                    self.cl = Huffman::build(&self.cl_lengths, false, false)?;
                    self.state = State::Lengths(0);
                } else {
                    self.state = State::CodeLengths(i + 1);
                }
            }
            State::Lengths(i) => {
                let symbol = self.cl.symbol(&mut self.bits)?;
                let (n, v) = match symbol {
                    0..=15 => (1, symbol as u8),
                    16 => {
                        if i == 0 {
                            return Err(RepeatWithoutPrevious);
                        }
                        (self.bits.read(2)? as usize + 3, self.lengths[i - 1])
                    }
                    17 => (self.bits.read(3)? as usize + 3, 0),
                    18 => (self.bits.read(7)? as usize + 11, 0),
                    _ => return Err(BadSymbol),
                };
                if i + n > self.hlit + self.hdist {
                    return Err(BadSymbol);
                }
                for j in i..i + n {
                    work::step();
                    self.lengths[j] = v;
                }
                if i + n == self.hlit + self.hdist {
                    self.lit = Huffman::build(&self.lengths[..self.hlit], true, false)?;
                    if self.lengths[256] == 0 {
                        return Err(BadSymbol);
                    }
                    self.dist = Huffman::build(
                        &self.lengths[self.hlit..self.hlit + self.hdist],
                        true,
                        true,
                    )?;
                    self.state = State::Codes;
                } else {
                    self.state = State::Lengths(i + n);
                }
            }
            State::Codes => {
                let s = self.lit.symbol(&mut self.bits)?;
                match s {
                    0..=255 => self.byte(s as u8)?,
                    256 => self.end_block(),
                    257..=285 => {
                        const LEN: [usize; 29] = [
                            3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51,
                            59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
                        ];
                        const EXTRA: [u8; 29] = [
                            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4,
                            5, 5, 5, 5, 0,
                        ];
                        let i = s as usize - 257;
                        let n = LEN[i] + self.bits.read(EXTRA[i])? as usize;
                        let d = self.dist.symbol(&mut self.bits)? as usize;
                        if d >= 30 {
                            return Err(BadSymbol);
                        }
                        let extra = if d < 4 { 0 } else { (d / 2 - 1) as u8 };
                        let base = if d < 4 {
                            d + 1
                        } else {
                            ((2 + (d & 1)) << extra) + 1
                        };
                        let distance = base + self.bits.read(extra)? as usize;
                        if distance > self.output.len() {
                            return Err(DistanceTooFar);
                        }
                        for _ in 0..n {
                            work::step();
                            let b = self.output[self.output.len() - distance];
                            self.byte(b)?;
                        }
                    }
                    _ => return Err(BadSymbol),
                }
            }
            State::Done => return Ok(false),
        }
        Ok(true)
    }
    fn pump(&mut self) -> Result<(), InflateError> {
        loop {
            work::step();
            let saved = self.bits;
            match self.step() {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(Truncated) => {
                    self.bits = saved;
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
    }
    pub fn feed(&mut self, input: &[u8]) -> Result<(), InflateError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        for &b in input {
            work::step();
            if matches!(self.state, State::Done) {
                break;
            }
            self.bits.value |= (b as u64) << self.bits.count;
            self.bits.count += 8;
            if let Err(e) = self.pump() {
                self.error = Some(e);
                return Err(e);
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<Vec<u8>, InflateError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        self.pump()?;
        if matches!(self.state, State::Done) {
            Ok(self.output)
        } else {
            Err(Truncated)
        }
    }
}
pub fn inflate(input: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
    let mut i = Inflater::new(limit);
    i.feed(input)?;
    i.finish()
}
pub fn adler32(input: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &v in input {
        work::step();
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}
pub fn zlib_decompress(input: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
    if input.len() < 2 {
        return Err(Truncated);
    }
    let (cmf, flg) = (input[0], input[1]);
    if cmf & 15 != 8 {
        return Err(BadMethod);
    }
    if cmf >> 4 > 7 {
        return Err(WindowTooLarge);
    }
    if (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
        return Err(BadZlibHeader);
    }
    if flg & 32 != 0 {
        return Err(PresetDictionary);
    }
    if input.len() < 6 {
        return Err(Truncated);
    }
    // Preserve the exact compressed endpoint, rather than treating trailing data
    // as a checksum: RFC 1950 permits bytes following the zlib stream.
    let mut i = Inflater::new(limit);
    let mut end = 2;
    for &b in &input[2..] {
        work::step();
        i.feed(&[b])?;
        end += 1;
        if matches!(i.state, State::Done) {
            break;
        }
    }
    let output = i.finish()?;
    let checksum = input.get(end..end + 4).ok_or(Truncated)?;
    if adler32(&output) != u32::from_be_bytes(checksum.try_into().unwrap()) {
        return Err(Adler32Mismatch);
    }
    Ok(output)
}
