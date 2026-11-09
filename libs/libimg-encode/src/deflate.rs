//! Clean-room RFC 1951 §3.2.4 stored-block writer and RFC 1950 §2 wrapper.
use alloc::vec::Vec;
use libimg::{inflate::adler32, work};
use alloc::vec;
pub fn deflate(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + input.len().div_ceil(65535) * 5 + 5);
    if input.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 255, 255]);
        return out;
    }
    let count = input.len().div_ceil(65535);
    for (i, block) in input.chunks(65535).enumerate() {
        work::step();
        out.push(u8::from(i + 1 == count));
        let n = block.len() as u16;
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(block);
    }
    out
}
pub fn zlib_compress(input: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    out.extend(deflate(input));
    out.extend_from_slice(&adler32(input).to_be_bytes());
    out
}

// RFC 1951 sections 3.1.1, 3.2.5 and 3.2.6. This bounded search targets
// repeated pixel channels; the stored encoder remains the fallback for noise.
struct Bits { bytes: Vec<u8>, pending: u32, count: u32 }
impl Bits {
    fn new() -> Self { Self { bytes: Vec::new(), pending: 0, count: 0 } }
    fn put(&mut self, value: u32, bits: u32) {
        self.pending |= value << self.count;
        self.count += bits;
        while self.count >= 8 {
            self.bytes.push(self.pending as u8);
            self.pending >>= 8;
            self.count -= 8;
        }
    }
    fn symbol(&mut self, symbol: u32) {
        let (code, bits) = match symbol {
            0..=143 => (0x30 + symbol, 8),
            144..=255 => (0x190 + symbol - 144, 9),
            256..=279 => (symbol - 256, 7),
            _ => (0xc0 + symbol - 280, 8),
        };
        self.put(code.reverse_bits() >> (32 - bits), bits);
    }
    fn finish(mut self) -> Vec<u8> {
        if self.count != 0 { self.bytes.push(self.pending as u8); }
        self.bytes
    }
}
pub fn zlib_compact(input: &[u8]) -> Vec<u8> {
    const BASE: [usize; 29] = [3,4,5,6,7,8,9,10,11,13,15,17,19,23,27,31,35,43,51,59,67,83,99,115,131,163,195,227,258];
    const EXTRA: [u32; 29] = [0,0,0,0,0,0,0,0,1,1,1,1,2,2,2,2,3,3,3,3,4,4,4,4,5,5,5,5,0];
    let mut out = Bits::new();
    out.put(3, 3); // Final block, fixed Huffman trees.
    let mut at = 0;
    while at < input.len() {
        work::step();
        let mut best = (0, 0u32);
        for distance in 1..=4usize {
            if at < distance { continue; }
            let mut len = 0;
            while len < 258 && at + len < input.len()
                && input[at + len] == input[at + len - distance] { len += 1; }
            if len >= 3 && len > best.0 { best = (len, distance as u32); }
        }
        if best.0 >= 3 {
            let index = BASE.iter().rposition(|&base| base <= best.0).unwrap();
            out.symbol(257 + index as u32);
            out.put((best.0 - BASE[index]) as u32, EXTRA[index]);
            // Distances 1..4 have five-bit codes 0..3 and no extra bits.
            out.put((best.1 - 1).reverse_bits() >> 27, 5);
            at += best.0;
        } else {
            out.symbol(input[at] as u32);
            at += 1;
        }
    }
    out.symbol(256);
    let compressed = out.finish();
    let stored = deflate(input);
    let payload = if compressed.len() < stored.len() { compressed } else { stored };
    let mut zlib = vec![0x78, 0x01];
    zlib.extend(payload);
    zlib.extend_from_slice(&adler32(input).to_be_bytes());
    zlib
}
