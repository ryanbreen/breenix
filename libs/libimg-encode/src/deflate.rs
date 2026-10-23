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
