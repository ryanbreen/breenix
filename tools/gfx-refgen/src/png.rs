//! A minimal PNG writer, written from the PNG specification (Third Edition, sections 5,
//! 11.2.1 and 11.2.2) and RFC 1950/1951: 8-bit RGB, filter 0 on every row, one IDAT of
//! stored deflate blocks in a zlib stream. No compression is attempted.

use libgfx::framebuf::FrameBuf;

/// CRC-32 over `parts` (PNG specification, annex D: polynomial 0xEDB88320, reflected,
/// initial and final XOR 0xFFFFFFFF).
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &b in *part {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
    }
    !crc
}

/// Adler-32 (RFC 1950 section 8).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// A zlib stream (RFC 1950: CMF 0x78, FLG 0x01) of stored blocks (RFC 1951 section 3.2.4).
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(65_535).collect() };
    for (i, chunk) in chunks.iter().enumerate() {
        out.push(if i + 1 == chunks.len() { 1 } else { 0 });
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// The surface's colours as an 8-bit RGB PNG.
pub fn encode_rgb(fb: &FrameBuf) -> Vec<u8> {
    let mut raw = Vec::with_capacity(fb.height * (1 + 3 * fb.width));
    for y in 0..fb.height {
        raw.push(0);
        for x in 0..fb.width {
            let c = fb.get_pixel(x, y);
            raw.extend_from_slice(&[c.r, c.g, c.b]);
        }
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(fb.width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(fb.height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}
