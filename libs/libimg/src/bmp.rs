//! BMP 24-bit uncompressed encoding and decoding.


/// Encode a top-down RGB pixel buffer as a 24-bit uncompressed BMP file.
///
/// `rgb_data` must be `width * height * 3` bytes: rows top-to-bottom, pixels
/// left-to-right, 3 bytes per pixel (R, G, B).
///
/// Returns the complete BMP file as a byte vector.
pub fn encode_bmp_24(width: u32, height: u32, rgb_data: &[u8]) -> Vec<u8> {
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
        let src_off = row * row_bytes;
        for col in 0..width as usize {
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

/// Decode a 24-bit uncompressed BMP file into a top-down RGB pixel buffer.
///
/// Returns `(width, height, rgb_data)` where `rgb_data` is `width * height * 3`
/// bytes: rows top-to-bottom, pixels left-to-right, 3 bytes per pixel (R, G, B).
///
/// Returns `None` if the data is not a valid 24-bit uncompressed BMP.
pub fn decode_bmp_24(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    // Need at least 54 bytes for headers
    if data.len() < 54 {
        return None;
    }

    // Check BM signature
    if data[0] != b'B' || data[1] != b'M' {
        return None;
    }

    let pixel_offset = read_u32_le(data, 10) as usize;
    let width = read_u32_le(data, 18);
    let height_raw = read_u32_le(data, 22) as i32;
    let bits_per_pixel = read_u16_le(data, 28);
    let compression = read_u32_le(data, 30);

    // Only support 24-bit uncompressed
    if bits_per_pixel != 24 || compression != 0 {
        return None;
    }

    let height = height_raw.unsigned_abs();
    let bottom_up = height_raw > 0;

    let row_bytes = (width * 3) as usize;
    let padding = (4 - (row_bytes % 4)) % 4;
    let padded_row = row_bytes + padding;

    let pixel_data = data.get(pixel_offset..)?;
    if pixel_data.len() < padded_row * height as usize {
        return None;
    }

    let mut rgb = vec![0u8; row_bytes * height as usize];

    for row in 0..height as usize {
        // BMP bottom-up: first row in file is bottom of image
        let src_row = if bottom_up { (height as usize) - 1 - row } else { row };
        let src_off = src_row * padded_row;
        let dst_off = row * row_bytes;

        for col in 0..width as usize {
            let si = src_off + col * 3;
            let di = dst_off + col * 3;
            rgb[di] = pixel_data[si + 2];     // R (from BGR)
            rgb[di + 1] = pixel_data[si + 1]; // G
            rgb[di + 2] = pixel_data[si];     // B
        }
    }

    Some((width, height, rgb))
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
