//! BMP 24-bit uncompressed encoding and decoding.
use alloc::vec::Vec;

/// Encode a top-down RGB pixel buffer as a 24-bit uncompressed BMP file.
///
/// `rgb_data` must be `width * height * 3` bytes: rows top-to-bottom, pixels
/// left-to-right, 3 bytes per pixel (R, G, B).
///
/// Returns the complete BMP file, or an empty vector for invalid dimensions
/// or an RGB buffer whose length does not match. Callers must check before saving.
pub fn encode_bmp_24(width: u32, height: u32, rgb_data: &[u8]) -> Vec<u8> {
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 16777216
        || rgb_data.len() != width as usize * height as usize * 3
    {
        return Vec::new();
    }
    let row_bytes = (width * 3) as usize;
    let padding = (4 - (row_bytes % 4)) % 4;
    let padded_row = row_bytes + padding;
    let pixel_data_size = padded_row * height as usize;
    let file_size = 14 + 40 + pixel_data_size;

    let mut buf = vec![0u8; file_size];
    let mut o = 0;

    // -- BITMAPFILEHEADER (14 bytes) --
    buf[o] = b'B';
    buf[o + 1] = b'M';
    o += 2;
    write_u32_le(&mut buf, o, file_size as u32);
    o += 4;
    // reserved
    o += 4;
    // pixel data offset
    write_u32_le(&mut buf, o, 54);
    o += 4;

    // -- BITMAPINFOHEADER (40 bytes) --
    write_u32_le(&mut buf, o, 40); // header size
    o += 4;
    write_u32_le(&mut buf, o, width); // width
    o += 4;
    write_u32_le(&mut buf, o, height); // height (positive = bottom-up)
    o += 4;
    write_u16_le(&mut buf, o, 1); // planes
    o += 2;
    write_u16_le(&mut buf, o, 24); // bits per pixel
    o += 2;
    // compression (0 = BI_RGB)
    o += 4;
    write_u32_le(&mut buf, o, pixel_data_size as u32); // image size
    o += 4;
    // x/y pixels per meter, colors used, colors important (all 0)
    o += 16;

    // -- Pixel data (bottom-up, BGR) --
    for row in (0..height as usize).rev() {
        crate::work::step();
        let src_off = row * row_bytes;
        for col in 0..width as usize {
            crate::work::step();
            let si = src_off + col * 3;
            let r = rgb_data[si];
            let g = rgb_data[si + 1];
            let b = rgb_data[si + 2];
            buf[o] = b; // BGR order
            buf[o + 1] = g;
            buf[o + 2] = r;
            o += 3;
        }
        // Pad row to 4-byte boundary
        o += padding;
    }

    buf
}

/// A decoded BMP with top-down, straight RGB rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bmp {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmpError {
    Truncated,
    BadSignature,
    BadHeader,
    Unsupported { bpp: u16, compression: u32 },
    TooLarge,
    SizeOverflow,
}

/// Decode supported 24-bit BMP headers and return top-down RGB rows.
/// Returns `None` on rejection; use [`decode_bmp`] to retain the reason.
pub fn decode_bmp_24(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    decode_bmp(data).ok().map(|b| (b.width, b.height, b.rgb))
}
/// Decode a 24-bit uncompressed BMP, checking sizes before allocation.
/// Returns the specific header, format, size or truncation error in [`BmpError`].
pub fn decode_bmp(data: &[u8]) -> Result<Bmp, BmpError> {
    use BmpError::*;
    if data.len() < 18 {
        return Err(Truncated);
    }
    if &data[..2] != b"BM" {
        return Err(BadSignature);
    }
    // BITMAPFILEHEADER.bfSize is a file-size claim (contract §12).
    if read_u32_le(data, 2) as usize > data.len() {
        return Err(Truncated);
    }
    let offset = read_u32_le(data, 10) as usize;
    let dib = read_u32_le(data, 14) as usize;
    if dib != 12 && dib < 40 { return Err(BadHeader); }
    let header_end = 14usize.checked_add(dib).ok_or(SizeOverflow)?;
    if header_end > data.len() { return Err(Truncated); }
    if offset < header_end { return Err(BadHeader); }
    // Microsoft's published CORE header uses unsigned WORD dimensions and has
    // no compression field; INFO and extended headers use signed LONG height.
    let (width, raw_height, planes, bpp, compression) = if dib == 12 {
        (read_u16_le(data, 18) as u32, read_u16_le(data, 20) as i32,
            read_u16_le(data, 22), read_u16_le(data, 24), 0)
    } else {
        (read_u32_le(data, 18), read_u32_le(data, 22) as i32,
            read_u16_le(data, 26), read_u16_le(data, 28), read_u32_le(data, 30))
    };
    let height = raw_height.unsigned_abs();
    if bpp != 24 || compression != 0 || planes != 1 {
        return Err(Unsupported { bpp, compression });
    }
    if width == 0 || height == 0 { return Err(BadHeader); }
    // BMP sizes are 32-bit fields, even on 64-bit hosts. Check them before
    // both the dimension refusal and any allocation.
    let row = width.checked_mul(3).ok_or(SizeOverflow)?;
    let padded = row.checked_add(3).ok_or(SizeOverflow)? & !3;
    let size = padded.checked_mul(height).ok_or(SizeOverflow)?;
    if width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 16777216
    {
        return Err(TooLarge);
    }
    let end = offset.checked_add(size as usize).ok_or(SizeOverflow)?;
    let pixels = data.get(offset..end).ok_or(Truncated)?;
    let mut rgb = vec![0; row as usize * height as usize];
    for y in 0..height as usize {
        crate::work::step();
        let src = if raw_height > 0 {
            height as usize - 1 - y
        } else {
            y
        };
        for x in 0..width as usize {
            crate::work::step();
            let si = src * padded as usize + x * 3;
            let di = y * row as usize + x * 3;
            rgb[di..di + 3].copy_from_slice(&[pixels[si + 2], pixels[si + 1], pixels[si]]);
        }
    }
    Ok(Bmp { width, height, rgb })
}

fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    buf[offset] as u32
        | (buf[offset + 1] as u32) << 8
        | (buf[offset + 2] as u32) << 16
        | (buf[offset + 3] as u32) << 24
}

fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    buf[offset] as u16 | (buf[offset + 1] as u16) << 8
}

fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset] = val as u8;
    buf[offset + 1] = (val >> 8) as u8;
    buf[offset + 2] = (val >> 16) as u8;
    buf[offset + 3] = (val >> 24) as u8;
}

fn write_u16_le(buf: &mut [u8], offset: usize, val: u16) {
    buf[offset] = val as u8;
    buf[offset + 1] = (val >> 8) as u8;
}
