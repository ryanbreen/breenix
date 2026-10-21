//! PNG encoding for capture/export consumers without GPU upload authority.
#![no_std]
extern crate alloc;
pub mod deflate;
use alloc::vec::Vec;
use libimg::{crc::crc32_parts, work};
use libimg::png::{Image, PngError, PngError::*};
const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
fn dimensions(w:u32,h:u32)->Result<(),PngError> {
    if w==0 || h==0 {return Err(BadHeader);}
    if w>16384 || h>16384 || u64::from(w)*u64::from(h)>16777216 {return Err(TooLarge);}
    Ok(())
}
fn write_chunk(out: &mut Vec<u8>, name: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32_parts(name, payload).to_be_bytes());
}
fn straight(c: u8, a: u8) -> u8 {
    if a == 0 {
        0
    } else {
        ((c as u32 * 255 + a as u32 / 2) / a as u32) as u8
    }
}
/// Encode premultiplied BGRA as depth-8 RGB or RGBA, filter zero, one IDAT.
/// Returns TooLarge before inspecting pixels or allocating for invalid dimensions.
pub fn encode(image: &Image) -> Result<Vec<u8>, PngError> {
    dimensions(image.width, image.height)?;
    if image.pixels.len() != image.width as usize * image.height as usize * 4 {
        return Err(BadHeader);
    }
    let mut alpha = false;
    for p in image.pixels.chunks_exact(4) {
        work::step();
        if p[0] > p[3] || p[1] > p[3] || p[2] > p[3] {
            return Err(BadHeader);
        }
        alpha |= p[3] != 255;
    }
    let channels = if alpha { 4 } else { 3 };
    let mut raw = Vec::with_capacity(image.height as usize * (1 + image.width as usize * channels));
    for row in image.pixels.chunks_exact(image.width as usize * 4) {
        work::step();
        raw.push(0);
        for p in row.chunks_exact(4) {
            work::step();
            raw.extend_from_slice(&[
                straight(p[2], p[3]),
                straight(p[1], p[3]),
                straight(p[0], p[3]),
            ]);
            if alpha {
                raw.push(p[3]);
            }
        }
    }
    let mut out = SIGNATURE.to_vec();
    let mut header = [0; 13];
    header[..4].copy_from_slice(&image.width.to_be_bytes());
    header[4..8].copy_from_slice(&image.height.to_be_bytes());
    header[8] = 8;
    header[9] = if alpha { 6 } else { 2 };
    write_chunk(&mut out, b"IHDR", &header);
    write_chunk(&mut out, b"IDAT", &crate::deflate::zlib_compress(&raw));
    write_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}
