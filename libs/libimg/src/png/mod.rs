//! Clean-room PNG (W3C Third Edition §§4–6, 9–11 and 13), static image path.
//! Samples remain at file precision through tRNS comparison, then scale and
//! premultiply exactly once. Colour-management and other ancillary chunks are ignored.
use alloc::vec::Vec;
use crate::{
    crc::crc32_parts,
    inflate::{zlib_decompress, InflateError},
    work,
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngError {
    BadColourType,
    BadFilter,
    MissingPlte,
    BadPlte,
    ChunkOrder,
    BadCrc,
    BadChunkLength,
    UnknownCritical,
    BadPaletteIndex,
    BadHeader,
    BadSignature,
    Truncated,
    TooLarge,
    Bomb,
    OutOfMemory,
    Compressed(InflateError),
}
use PngError::*;
const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PASSES: [(u32, u32, u32, u32); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];
fn u32be(s: &[u8]) -> u32 {
    u32::from_be_bytes(s.try_into().unwrap())
}
fn u16be(s: &[u8]) -> u16 {
    u16::from_be_bytes(s.try_into().unwrap())
}
fn dimensions(w: u32, h: u32) -> Result<(), PngError> {
    if w == 0 || h == 0 {
        return Err(BadHeader);
    }
    if w > 16384 || h > 16384 || u64::from(w) * u64::from(h) > 16777216 {
        return Err(TooLarge);
    }
    Ok(())
}
struct Header {
    w: u32,
    h: u32,
    depth: u8,
    colour: u8,
    channels: usize,
    interlace: bool,
}
impl Header {
    fn parse(s: &[u8]) -> Result<Self, PngError> {
        if s.len() != 13 {
            return Err(BadHeader);
        }
        let (w, h, depth, colour) = (u32be(&s[..4]), u32be(&s[4..8]), s[8], s[9]);
        dimensions(w, h)?;
        let valid = match colour {
            0 => [1, 2, 4, 8, 16].contains(&depth),
            2 | 4 | 6 => [8, 16].contains(&depth),
            3 => [1, 2, 4, 8].contains(&depth),
            _ => false,
        };
        if !valid {
            return Err(BadColourType);
        }
        if s[10] != 0 || s[11] != 0 || s[12] > 1 {
            return Err(BadHeader);
        }
        Ok(Self {
            w,
            h,
            depth,
            colour,
            channels: match colour {
                0 | 3 => 1,
                2 => 3,
                4 => 2,
                _ => 4,
            },
            interlace: s[12] == 1,
        })
    }
    fn passes(&self) -> &[(u32, u32, u32, u32)] {
        if self.interlace {
            &PASSES
        } else {
            &[(0, 0, 1, 1)]
        }
    }
    fn row_bytes(&self, w: u32) -> usize {
        (w as usize * self.channels * self.depth as usize).div_ceil(8)
    }
    fn expected(&self) -> usize {
        let mut n = 0;
        for &(x, y, dx, dy) in self.passes() {
            work::step();
            let (w, h) = (span(self.w, x, dx), span(self.h, y, dy));
            if w > 0 && h > 0 {
                n += h as usize * (1 + self.row_bytes(w));
            }
        }
        n
    }
}
fn span(n: u32, start: u32, step: u32) -> u32 {
    n.saturating_sub(start).div_ceil(step)
}
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (a as i32, b as i32, c as i32);
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}
fn unfilter(row: &mut [u8], prior: &[u8], bpp: usize, filter: u8) -> Result<(), PngError> {
    if filter > 4 {
        return Err(BadFilter);
    }
    for i in 0..row.len() {
        work::step();
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prior[i];
        let c = if i >= bpp { prior[i - bpp] } else { 0 };
        let p = match filter {
            0 => 0,
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            _ => paeth(a, b, c),
        };
        row[i] = row[i].wrapping_add(p);
    }
    Ok(())
}
fn sample(row: &[u8], i: usize, d: u8) -> u16 {
    match d {
        16 => u16be(&row[2 * i..2 * i + 2]),
        8 => row[i] as u16,
        _ => {
            let bit = i * d as usize;
            ((row[bit / 8] >> (8 - d as usize - bit % 8)) & ((1 << d) - 1)) as u16
        }
    }
}
// PNG §11.3.1.1: high tRNS bits are masked before the comparison.
fn key(s: &[u8], depth: u8) -> u16 {
    u16be(s) & (u16::MAX >> (16 - depth))
}
fn scaled(v: u16, d: u8) -> u8 {
    if d == 16 {
        ((v as u32 * 255 + 32767) / 65535) as u8
    } else {
        (v as u32 * 255 / ((1u32 << d) - 1)) as u8
    }
}
fn premul(c: u8, a: u8) -> u8 {
    let v = c as u32 * a as u32 + 128;
    ((v + (v >> 8)) >> 8) as u8
}
fn zeroed(n: usize) -> Result<Vec<u8>, PngError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(n).map_err(|_| OutOfMemory)?;
    bytes.resize(n, 0);
    Ok(bytes)
}
fn pixels(
    header: &Header,
    mut raw: Vec<u8>,
    palette: &[u8],
    trns: &[u8],
) -> Result<Image, PngError> {
    let mut image = Image {
        width: header.w,
        height: header.h,
        pixels: zeroed(header.w as usize * header.h as usize * 4)?,
    };
    let mut offset = 0;
    let bpp = (header.channels * header.depth as usize).div_ceil(8).max(1);
    for &(x0, y0, dx, dy) in header.passes() {
        work::step();
        let (w, h) = (span(header.w, x0, dx), span(header.h, y0, dy));
        if w == 0 || h == 0 {
            continue;
        }
        let n = header.row_bytes(w);
        let mut prior = zeroed(n)?;
        for y in 0..h {
            work::step();
            let filter = raw[offset];
            offset += 1;
            let row = &mut raw[offset..offset + n];
            unfilter(row, &prior, bpp, filter)?;
            for x in 0..w {
                work::step();
                let i = x as usize * header.channels;
                let v = sample(row, i, header.depth);
                let d = header.depth;
                let (r, g, b, a) = match header.colour {
                    0 => {
                        let c = scaled(v, d);
                        (
                            c,
                            c,
                            c,
                            if trns.len() == 2 && v == key(trns, d) {
                                0
                            } else {
                                255
                            },
                        )
                    }
                    2 => {
                        let (g, b) = (sample(row, i + 1, d), sample(row, i + 2, d));
                        let a = if trns.len() == 6
                            && v == key(&trns[..2], d)
                            && g == key(&trns[2..4], d)
                            && b == key(&trns[4..6], d)
                        {
                            0
                        } else {
                            255
                        };
                        (scaled(v, d), scaled(g, d), scaled(b, d), a)
                    }
                    3 => {
                        let p = v as usize * 3;
                        let rgb = palette.get(p..p + 3).ok_or(BadPaletteIndex)?;
                        (
                            rgb[0],
                            rgb[1],
                            rgb[2],
                            trns.get(v as usize).copied().unwrap_or(255),
                        )
                    }
                    4 => {
                        let c = scaled(v, d);
                        (c, c, c, scaled(sample(row, i + 1, d), d))
                    }
                    _ => (
                        scaled(v, d),
                        scaled(sample(row, i + 1, d), d),
                        scaled(sample(row, i + 2, d), d),
                        scaled(sample(row, i + 3, d), d),
                    ),
                };
                let dest = (((y0 + y * dy) * header.w + x0 + x * dx) * 4) as usize;
                image.pixels[dest..dest + 4].copy_from_slice(&[
                    premul(b, a),
                    premul(g, a),
                    premul(r, a),
                    a,
                ]);
            }
            prior.copy_from_slice(row);
            offset += n;
        }
    }
    Ok(image)
}
pub fn decode(input: &[u8]) -> Result<Image, PngError> {
    if input.len() < 8 {
        return Err(Truncated);
    }
    if &input[..8] != SIGNATURE {
        return Err(BadSignature);
    }
    let mut offset = 8;
    let mut header = None;
    let mut palette: &[u8] = &[];
    let mut trns: &[u8] = &[];
    let mut has_trns = false;
    let mut data = Vec::new();
    let mut idat = false;
    let mut closed = false;
    let mut ended = false;
    while offset < input.len() {
        work::step();
        let prefix = input.get(offset..offset + 8).ok_or(Truncated)?;
        let n = u32be(&prefix[..4]) as usize;
        if n > 0x7fffffff {
            return Err(BadChunkLength);
        }
        let name = &prefix[4..8];
        let end = offset
            .checked_add(12)
            .and_then(|v| v.checked_add(n))
            .ok_or(BadChunkLength)?;
        let chunk = input.get(offset + 8..end).ok_or(Truncated)?;
        let payload = &chunk[..n];
        if crc32_parts(name, payload) != u32be(&chunk[n..]) {
            return Err(BadCrc);
        }
        if header.is_none() && name != b"IHDR" {
            return Err(ChunkOrder);
        }
        if idat && name != b"IDAT" {
            closed = true;
        }
        match name {
            b"IHDR" => {
                if header.is_some() {
                    return Err(ChunkOrder);
                }
                header = Some(Header::parse(payload)?);
            }
            b"PLTE" => {
                let h = header.as_ref().ok_or(ChunkOrder)?;
                if idat || !palette.is_empty() || has_trns {
                    return Err(ChunkOrder);
                }
                if h.colour == 0
                    || h.colour == 4
                    || n == 0
                    || n % 3 != 0
                    || n / 3 > 256
                    || (h.colour == 3 && n / 3 > (1usize << h.depth))
                {
                    return Err(BadPlte);
                }
                palette = payload;
            }
            b"tRNS" => {
                let h = header.as_ref().ok_or(ChunkOrder)?;
                if idat || has_trns {
                    return Err(ChunkOrder);
                }
                if h.colour == 3 && palette.is_empty() { return Err(ChunkOrder); }
                has_trns = true;
                let valid = match h.colour {
                    0 => n == 2,
                    2 => n == 6,
                    3 => n > 0 && n <= palette.len() / 3,
                    _ => false,
                };
                if valid { trns = payload; }
            }
            b"IDAT" => {
                if closed {
                    return Err(ChunkOrder);
                }
                let h = header.as_ref().ok_or(ChunkOrder)?;
                if h.colour == 3 && palette.is_empty() {
                    return Err(MissingPlte);
                }
                idat = true;
                data.try_reserve_exact(payload.len()).map_err(|_| OutOfMemory)?;
                data.extend_from_slice(payload);
            }
            b"IEND" => {
                if !idat || n != 0 {
                    return Err(ChunkOrder);
                }
                ended = true;
                break;
            }
            _ => {
                if name[0] & 32 == 0 {
                    return Err(UnknownCritical);
                }
            }
        }
        offset = end;
    }
    if !ended {
        return Err(Truncated);
    }
    let h = header.ok_or(ChunkOrder)?;
    let expected = h.expected();
    let raw = zlib_decompress(&data, expected).map_err(|e| match e {
        InflateError::OutputLimit => Bomb,
        InflateError::Truncated => Truncated,
        InflateError::OutOfMemory => OutOfMemory,
        _ => Compressed(e),
    })?;
    if raw.len() != expected {
        return Err(Truncated);
    }
    pixels(&h, raw, palette, trns)
}
