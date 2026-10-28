/*
 * Copyright 2010 Christoph Bumiller
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */
//! Committed diagnostic programs in the Kepler graphics code layout.
// Mesa nvc0_program.c:766-805, 813-856: 64-byte allocation units,
// 80-byte SPH and 128-byte instruction alignment. The segment base must
// itself be aligned to 256 bytes, matching these program offsets.

pub const VERTEX_OFFSET: usize = 0x30;
pub const FRAGMENT_OFFSET: usize = 0x1b0;
pub const SEGMENT_SIZE: usize = 0x280;

// Read the committed 7.1 metadata at compile time; no container toolchain runs.
const fn asset_gprs(json: &str) -> u32 {
    let bytes = json.as_bytes();
    let key = b"\"gprs\":";
    let mut i = 0;
    while i + key.len() < bytes.len() {
        let mut j = 0;
        while j < key.len() && bytes[i + j] == key[j] { j += 1; }
        if j == key.len() {
            i += j;
            while bytes[i] == b' ' { i += 1; }
            let mut value = 0;
            while bytes[i] >= b'0' && bytes[i] <= b'9' {
                value = value * 10 + (bytes[i] - b'0') as u32;
                i += 1;
            }
            assert!(value >= 4 && value <= 255);
            return value;
        }
        i += 1;
    }
    panic!("Kepler asset has no gprs field");
}
pub const VERTEX_GPRS: u32 = asset_gprs(include_str!("../../../../assets/kepler/diagnostic-triangle-vertex.json"));
pub const FRAGMENT_GPRS: u32 = asset_gprs(include_str!("../../../../assets/kepler/diagnostic-triangle-fragment.json"));

const VERTEX_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/diagnostic-triangle-vertex.sph");
const VERTEX_CODE: &[u8; 160] =
    include_bytes!("../../../../assets/kepler/diagnostic-triangle-vertex.bin");
const FRAGMENT_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/diagnostic-triangle-fragment.sph");
const FRAGMENT_CODE: &[u8; 48] =
    include_bytes!("../../../../assets/kepler/diagnostic-triangle-fragment.bin");

/// Construct the upload bytes, including zero-filled allocation padding.
pub fn triangle_segment() -> [u8; SEGMENT_SIZE] {
    let mut segment = [0; SEGMENT_SIZE];
    for (offset, header, code) in [
        (VERTEX_OFFSET, &VERTEX_HEADER[..], &VERTEX_CODE[..]),
        (FRAGMENT_OFFSET, &FRAGMENT_HEADER[..], &FRAGMENT_CODE[..]),
    ] {
        segment[offset..offset + header.len()].copy_from_slice(header);
        segment[offset + header.len()..offset + header.len() + code.len()].copy_from_slice(code);
    }
    segment
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadCase { Withheld, AfterWithheld, Flipped, AfterFlipped }

pub trait UploadIo {
    type Error;
    fn poison(&mut self, byte: u8) -> Result<(), Self::Error>;
    fn upload(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
    fn readback(&mut self, case: UploadCase, expected: &[u8]) -> Result<(), Self::Error>;
}

/// Exercise both negative controls and their subsequent clean uploads.
pub fn exercise_uploads<I: UploadIo>(io: &mut I) -> Result<(), I::Error> {
    let clean = triangle_segment();
    let poison = [0x55; SEGMENT_SIZE];
    io.poison(0x55)?;
    io.readback(UploadCase::Withheld, &poison)?;
    io.upload(&clean)?;
    io.readback(UploadCase::AfterWithheld, &clean)?;
    let mut flipped = clean;
    flipped[0] ^= 1;
    io.upload(&flipped)?;
    io.readback(UploadCase::Flipped, &flipped)?;
    io.upload(&clean)?;
    io.readback(UploadCase::AfterFlipped, &clean)
}
