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
// Each SPH begins 80 bytes before a 128-byte-aligned instruction stream.
pub const CONSTANT_COLOUR_FRAGMENT_OFFSET: usize = 0x2b0;
pub const DROP_COLOUR_VERTEX_OFFSET: usize = 0x3b0;
pub const CODE_REGION_SIZE: usize = 0x480;

pub const CONSTANT_COLOUR_FRAGMENT_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/constant-colour-fragment.sph");
pub const CONSTANT_COLOUR_FRAGMENT_CODE: &[u8; 48] =
    include_bytes!("../../../../assets/kepler/constant-colour-fragment.bin");
pub const DROP_COLOUR_VERTEX_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/drop-colour-vertex.sph");
pub const DROP_COLOUR_VERTEX_CODE: &[u8; 88] =
    include_bytes!("../../../../assets/kepler/drop-colour-vertex.bin");
pub const GRADIENT_FRAGMENT_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/gradient-fragment.sph");
pub const GRADIENT_FRAGMENT_CODE: &[u8; 152] =
    include_bytes!("../../../../assets/kepler/gradient-fragment.bin");

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
pub const GRADIENT_FRAGMENT_GPRS: u32 = asset_gprs(include_str!("../../../../assets/kepler/gradient-fragment.json"));

// Basic catalogue and executable admission probes; G5 committed assets.
pub const PIPELINE_SOLID_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-solid.sph");
pub const PIPELINE_SOLID_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-solid.bin");
pub const PIPELINE_SOLID_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-solid.json"));
pub const PIPELINE_COPY_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-copy.sph");
pub const PIPELINE_COPY_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-copy.bin");
pub const PIPELINE_COPY_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-copy.json"));
pub const PIPELINE_COPY_SCALED_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-copy_scaled.sph");
pub const PIPELINE_COPY_SCALED_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-copy_scaled.bin");
pub const PIPELINE_COPY_SCALED_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-copy_scaled.json"));
pub const PIPELINE_NEAREST_SCALED_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-nearest_scaled.sph");
pub const PIPELINE_NEAREST_SCALED_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-nearest_scaled.bin");
pub const PIPELINE_NEAREST_SCALED_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-nearest_scaled.json"));
pub const PIPELINE_BOX_FILTER_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-box_filter.sph");
pub const PIPELINE_BOX_FILTER_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-box_filter.bin");
pub const PIPELINE_BOX_FILTER_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-box_filter.json"));
pub const PROBE_INTEGER_OPS_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-integer-ops.sph");
pub const PROBE_INTEGER_OPS_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-integer-ops.bin");
pub const PROBE_INTEGER_OPS_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-integer-ops.json"));
pub const PROBE_TEXEL_FETCH_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-texel-fetch.sph");
pub const PROBE_TEXEL_FETCH_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-texel-fetch.bin");
pub const PROBE_TEXEL_FETCH_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-texel-fetch.json"));

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

// Shared catalogue programs reserved for milestone 8.3.
pub const PIPELINE_OVER_OFFSET: usize = 0x2530;
pub const PIPELINE_OVER_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-over.sph");
pub const PIPELINE_OVER_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-over.bin");
pub const PIPELINE_OVER_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-over.json"));
pub const PIPELINE_OVER_STRAIGHT_OFFSET: usize = 0x2930;
pub const PIPELINE_OVER_STRAIGHT_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-over_straight.sph");
pub const PIPELINE_OVER_STRAIGHT_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-over_straight.bin");
pub const PIPELINE_OVER_STRAIGHT_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-over_straight.json"));
pub const PIPELINE_COVERAGE_OFFSET: usize = 0x2d30;
pub const PIPELINE_COVERAGE_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-coverage.sph");
pub const PIPELINE_COVERAGE_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-coverage.bin");
pub const PIPELINE_COVERAGE_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-coverage.json"));
pub const PIPELINE_BITMAP_TEXT_OFFSET: usize = 0x3130;
pub const PIPELINE_BITMAP_TEXT_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-bitmap_text.sph");
pub const PIPELINE_BITMAP_TEXT_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-bitmap_text.bin");
pub const PIPELINE_BITMAP_TEXT_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-bitmap_text.json"));
pub const PROBE_INT_00_OFFSET: usize = 0x3930;
pub const PROBE_INT_00_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-00.sph");
pub const PROBE_INT_00_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-00.bin");
pub const PROBE_INT_00_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-00.json"));
pub const PROBE_INT_01_OFFSET: usize = 0x3a30;
pub const PROBE_INT_01_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-01.sph");
pub const PROBE_INT_01_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-01.bin");
pub const PROBE_INT_01_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-01.json"));
pub const PROBE_INT_02_OFFSET: usize = 0x3b30;
pub const PROBE_INT_02_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-02.sph");
pub const PROBE_INT_02_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-02.bin");
pub const PROBE_INT_02_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-02.json"));
pub const PROBE_INT_03_OFFSET: usize = 0x3c30;
pub const PROBE_INT_03_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-03.sph");
pub const PROBE_INT_03_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-03.bin");
pub const PROBE_INT_03_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-03.json"));
pub const PROBE_INT_04_OFFSET: usize = 0x3d30;
pub const PROBE_INT_04_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-04.sph");
pub const PROBE_INT_04_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-04.bin");
pub const PROBE_INT_04_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-04.json"));
pub const PROBE_INT_05_OFFSET: usize = 0x3e30;
pub const PROBE_INT_05_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-05.sph");
pub const PROBE_INT_05_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-05.bin");
pub const PROBE_INT_05_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-05.json"));
pub const PROBE_INT_06_OFFSET: usize = 0x3fb0;
pub const PROBE_INT_06_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-06.sph");
pub const PROBE_INT_06_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-06.bin");
pub const PROBE_INT_06_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-06.json"));
pub const PROBE_INT_07_OFFSET: usize = 0x40b0;
pub const PROBE_INT_07_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-07.sph");
pub const PROBE_INT_07_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-07.bin");
pub const PROBE_INT_07_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-07.json"));
pub const PROBE_INT_08_OFFSET: usize = 0x41b0;
pub const PROBE_INT_08_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-08.sph");
pub const PROBE_INT_08_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-08.bin");
pub const PROBE_INT_08_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-08.json"));
pub const PROBE_INT_09_OFFSET: usize = 0x42b0;
pub const PROBE_INT_09_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-09.sph");
pub const PROBE_INT_09_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-09.bin");
pub const PROBE_INT_09_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-09.json"));
pub const PROBE_INT_10_OFFSET: usize = 0x44b0;
pub const PROBE_INT_10_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-10.sph");
pub const PROBE_INT_10_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-10.bin");
pub const PROBE_INT_10_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-10.json"));
pub const PROBE_INT_11_OFFSET: usize = 0x47b0;
pub const PROBE_INT_11_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-11.sph");
pub const PROBE_INT_11_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-11.bin");
pub const PROBE_INT_11_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-11.json"));
pub const PROBE_INT_18_OFFSET: usize = 0x4cb0;
pub const PROBE_INT_18_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-18.sph");
pub const PROBE_INT_18_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-18.bin");
pub const PROBE_INT_18_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-18.json"));
pub const PROBE_INT_19_OFFSET: usize = 0x4db0;
pub const PROBE_INT_19_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-19.sph");
pub const PROBE_INT_19_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-19.bin");
pub const PROBE_INT_19_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-19.json"));
pub const PROBE_INT_12_OFFSET: usize = 0x4f30;
pub const PROBE_INT_12_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-12.sph");
pub const PROBE_INT_12_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-12.bin");
pub const PROBE_INT_12_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-12.json"));
pub const PROBE_INT_13_OFFSET: usize = 0x63b0;
pub const PROBE_INT_13_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-13.sph");
pub const PROBE_INT_13_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-13.bin");
pub const PROBE_INT_13_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-13.json"));
pub const PROBE_INT_14_OFFSET: usize = 0x7830;
pub const PROBE_INT_14_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-14.sph");
pub const PROBE_INT_14_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-14.bin");
pub const PROBE_INT_14_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-14.json"));
pub const PROBE_INT_15_OFFSET: usize = 0x91b0;
pub const PROBE_INT_15_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-15.sph");
pub const PROBE_INT_15_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-15.bin");
pub const PROBE_INT_15_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-15.json"));
pub const PROBE_INT_16_OFFSET: usize = 0x9430;
pub const PROBE_INT_16_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-16.sph");
pub const PROBE_INT_16_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-16.bin");
pub const PROBE_INT_16_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-16.json"));
pub const PROBE_INT_17_OFFSET: usize = 0x9730;
pub const PROBE_INT_17_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-17.sph");
pub const PROBE_INT_17_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-17.bin");
pub const PROBE_INT_17_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-17.json"));
pub const PROBE_INT_20_OFFSET: usize = 0x9930;
pub const PROBE_INT_20_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-20.sph");
pub const PROBE_INT_20_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-20.bin");
pub const PROBE_INT_20_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-20.json"));
pub const PROBE_INT_21_OFFSET: usize = 0x9eb0;
pub const PROBE_INT_21_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-21.sph");
pub const PROBE_INT_21_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-21.bin");
pub const PROBE_INT_21_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-21.json"));
pub const PROBE_INT_22_OFFSET: usize = 0xa6b0;
pub const PROBE_INT_22_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-22.sph");
pub const PROBE_INT_22_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-22.bin");
pub const PROBE_INT_22_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-22.json"));
pub const PROBE_INT_23_OFFSET: usize = 0xaeb0;
pub const PROBE_INT_23_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-23.sph");
pub const PROBE_INT_23_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-23.bin");
pub const PROBE_INT_23_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-23.json"));
pub const PROBE_INT_24_OFFSET: usize = 0xe2b0;
pub const PROBE_INT_24_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/probe-int-24.sph");
pub const PROBE_INT_24_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/probe-int-24.bin");
pub const PROBE_INT_24_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/probe-int-24.json"));

// Shared shape fragments reserved for milestone 8.4; admission follows separately.
pub const PIPELINE_COVERAGE_SPANS_OFFSET: usize = 0xed30;
pub const PIPELINE_COVERAGE_SPANS_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-coverage_spans.sph");
pub const PIPELINE_COVERAGE_SPANS_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-coverage_spans.bin");
pub const PIPELINE_COVERAGE_SPANS_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-coverage_spans.json"));
pub const PIPELINE_LINE_OFFSET: usize = 0x129b0;
pub const PIPELINE_LINE_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-line.sph");
pub const PIPELINE_LINE_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-line.bin");
pub const PIPELINE_LINE_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-line.json"));
pub const PIPELINE_CIRCLE_FILL_OFFSET: usize = 0x13430;
pub const PIPELINE_CIRCLE_FILL_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-circle_fill.sph");
pub const PIPELINE_CIRCLE_FILL_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-circle_fill.bin");
pub const PIPELINE_CIRCLE_FILL_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-circle_fill.json"));
pub const PIPELINE_CIRCLE_OUTLINE_OFFSET: usize = 0x139b0;
pub const PIPELINE_CIRCLE_OUTLINE_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-circle_outline.sph");
pub const PIPELINE_CIRCLE_OUTLINE_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-circle_outline.bin");
pub const PIPELINE_CIRCLE_OUTLINE_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-circle_outline.json"));
pub const PIPELINE_ROUNDED_RECT_OFFSET: usize = 0x14230;
pub const PIPELINE_ROUNDED_RECT_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-rounded_rect.sph");
pub const PIPELINE_ROUNDED_RECT_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-rounded_rect.bin");
pub const PIPELINE_ROUNDED_RECT_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-rounded_rect.json"));
pub const PIPELINE_GRADIENT_V_OFFSET: usize = 0x14a30;
pub const PIPELINE_GRADIENT_V_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-gradient_v.sph");
pub const PIPELINE_GRADIENT_V_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-gradient_v.bin");
pub const PIPELINE_GRADIENT_V_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-gradient_v.json"));
pub const PIPELINE_TRIANGLE_OFFSET: usize = 0x161b0;
pub const PIPELINE_TRIANGLE_HEADER: &[u8; 80] =
    include_bytes!("../../../../assets/kepler/pipeline-triangle.sph");
pub const PIPELINE_TRIANGLE_CODE: &[u8] =
    include_bytes!("../../../../assets/kepler/pipeline-triangle.bin");
pub const PIPELINE_TRIANGLE_GPRS: u32 =
    asset_gprs(include_str!("../../../../assets/kepler/pipeline-triangle.json"));

// Rounded to the code allocation unit after the final shape program.
pub const CATALOGUE_CODE_SIZE: usize = 0x17e00;

pub const SHAPE_PROGRAMS: &[(usize, &[u8; 80], &[u8], u32)] = &[
    (PIPELINE_COVERAGE_SPANS_OFFSET, PIPELINE_COVERAGE_SPANS_HEADER, PIPELINE_COVERAGE_SPANS_CODE, PIPELINE_COVERAGE_SPANS_GPRS),
    (PIPELINE_LINE_OFFSET, PIPELINE_LINE_HEADER, PIPELINE_LINE_CODE, PIPELINE_LINE_GPRS),
    (PIPELINE_CIRCLE_FILL_OFFSET, PIPELINE_CIRCLE_FILL_HEADER, PIPELINE_CIRCLE_FILL_CODE, PIPELINE_CIRCLE_FILL_GPRS),
    (PIPELINE_CIRCLE_OUTLINE_OFFSET, PIPELINE_CIRCLE_OUTLINE_HEADER, PIPELINE_CIRCLE_OUTLINE_CODE, PIPELINE_CIRCLE_OUTLINE_GPRS),
    (PIPELINE_ROUNDED_RECT_OFFSET, PIPELINE_ROUNDED_RECT_HEADER, PIPELINE_ROUNDED_RECT_CODE, PIPELINE_ROUNDED_RECT_GPRS),
    (PIPELINE_GRADIENT_V_OFFSET, PIPELINE_GRADIENT_V_HEADER, PIPELINE_GRADIENT_V_CODE, PIPELINE_GRADIENT_V_GPRS),
    (PIPELINE_TRIANGLE_OFFSET, PIPELINE_TRIANGLE_HEADER, PIPELINE_TRIANGLE_CODE, PIPELINE_TRIANGLE_GPRS),
];
