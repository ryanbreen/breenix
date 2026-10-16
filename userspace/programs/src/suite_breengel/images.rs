//! inflate, bmp, png-decode and png-encode (contract sections 7 and 11 to 13).

use crate::*;

pub(crate) const INFLATE: &[Case] = &[
    case("stored", "A stored block decodes to its bytes", inflate_stored),
    case("stored-length", "A stored block whose LEN and NLEN disagree is rejected", inflate_stored_length),
    case("fixed-huffman", "A fixed-Huffman block decodes to the literal expected bytes", inflate_fixed_huffman),
    case("dynamic-huffman", "A dynamic-Huffman block decodes to the literal expected bytes", inflate_dynamic_huffman),
    case("block-sequence", "Stored, fixed and dynamic blocks in one stream decode, ending at the final block", inflate_block_sequence),
    case("block-type-invalid", "Block type 3 is rejected", inflate_block_type_invalid),
    case("code-oversubscribed", "An over-subscribed code-length set is rejected", inflate_code_oversubscribed),
    case("code-incomplete", "An incomplete code-length code is rejected, and so is an incomplete literal/length or distance code other than a single one-bit code", inflate_code_incomplete),
    case("code-single", "A literal/length code holding only end-of-block (code 256, one bit) decodes, and a distance code of one one-bit code decodes; the recheck's empty dynamic block decodes to empty output, as the independent decoder recorded", inflate_code_single),
    case("no-distance-codes", "A block whose distance code lengths are all zero (RFC 1951 §3.2.7: no distance codes used) decodes its literals, and a length symbol in that block is rejected", inflate_no_distance_codes),
    case("repeat-first", "A repeat code with no previous length is rejected", inflate_repeat_first),
    case("symbols-invalid", "Literal/length symbols 286 and 287 and distance symbols 30 and 31 are rejected", inflate_symbols_invalid),
    case("distance-too-far", "A distance reaching before the start of the output is rejected", inflate_distance_too_far),
    case("distance-max", "A distance of 32,768 within the window decodes", inflate_distance_max),
    case("length-258", "A match of length 258 decodes", inflate_length_258),
    case("truncated", "A stream cut off at any byte is rejected", inflate_truncated),
    case("zlib-header", "A bad check value, a preset dictionary, a method other than 8 and a window above 32 KiB are each rejected", inflate_zlib_header),
    case("adler32", "A stream whose Adler-32 is wrong is rejected", inflate_adler32),
    case("output-limit", "Output past the caller's limit is rejected without writing past it", inflate_output_limit),
    case("byte-split", "A stream fed in pieces split at every byte position decodes identically", inflate_byte_split),
    case("independent-fixtures", "Twelve fixtures made by an independent encoder at authoring time decode to their recorded bytes", inflate_independent_fixtures),
];

pub(crate) const BMP: &[Case] = &[
    case("decode-fixtures", "Independent BMP fixtures decode to their literal pixels", bmp_decode_fixtures),
    case("orientation", "Bottom-up and top-down (negative height) BMPs decode to the same pixels", bmp_orientation),
    case("padding", "Widths 1 to 4 decode correctly across row padding", bmp_padding),
    case("encode-bytes", "encode_bmp_24 writes the literal expected header and rows", bmp_encode_bytes),
    case("roundtrip", "Encoding then decoding returns the original pixels", bmp_roundtrip),
    case("truncated", "A BMP shorter than its header claims is rejected", bmp_truncated),
    case("size-overflow", "Dimensions whose row or image size overflows are rejected before any allocation", bmp_size_overflow),
    case("huge", "Dimensions beyond section 12.3's limits are rejected before any allocation", bmp_huge),
    case("unsupported", "An unsupported depth or compression is rejected with its error", bmp_unsupported),
];

pub(crate) const PNG_DECODE: &[Case] = &[
    case("gray-depths", "Grayscale at depths 1, 2, 4, 8 and 16 decodes exactly", png_decode_gray_depths),
    case("gray-alpha", "Grayscale with alpha at depths 8 and 16 decodes exactly", png_decode_gray_alpha),
    case("rgb", "RGB at depths 8 and 16 decodes exactly", png_decode_rgb),
    case("rgba", "RGBA at depths 8 and 16 decodes to premultiplied pixels exactly", png_decode_rgba),
    case("palette-depths", "Palette images at depths 1, 2, 4 and 8 decode exactly", png_decode_palette_depths),
    case("trns-gray", "A grayscale tRNS key colour becomes transparent", png_decode_trns_gray),
    case("trns-rgb", "An RGB tRNS key colour becomes transparent", png_decode_trns_rgb),
    case("trns-palette", "Palette tRNS alphas apply per entry", png_decode_trns_palette),
    case("invalid-combos", "Every illegal colour type and depth pair is rejected with its error", png_decode_invalid_combos),
    case("rounding-16bit", "16-bit samples convert to 8 bits by the contract's formula", png_decode_rounding_16bit),
    case("filters", "Rows using each of the five filter types decode exactly, at every bytes-per-pixel width", png_decode_filters),
    case("filter-invalid", "A filter type above 4 is rejected", png_decode_filter_invalid),
    case("interlace", "An Adam7 image decodes to the same pixels as its non-interlaced twin", png_decode_interlace),
    case("interlace-small", "Images from 1x1 to 9x9, where some passes are empty, decode correctly", png_decode_interlace_small),
    case("multi-idat", "Image data split across many IDAT chunks, including empty ones, decodes identically", png_decode_multi_idat),
    case("crc", "A chunk with a bad CRC is rejected", png_decode_crc),
    case("chunk-order", "IDAT before IHDR, PLTE after IDAT and a missing IEND are each rejected", png_decode_chunk_order),
    case("plte-required", "A palette image without PLTE is rejected", png_decode_plte_required),
    case("plte-size", "A PLTE whose length is not a multiple of 3 or exceeds the depth's entries is rejected", png_decode_plte_size),
    case("palette-index", "A pixel index past the palette's end is rejected", png_decode_palette_index),
    case("critical-unknown", "An unknown critical chunk is rejected", png_decode_critical_unknown),
    case("ancillary-ignored", "gAMA, iCCP, sRGB and tEXt are ignored, as the contract states (no colour management)", png_decode_ancillary_ignored),
    case("ihdr-invalid", "Zero width or height and invalid compression, filter or interlace methods are rejected", png_decode_ihdr_invalid),
    case("chunk-length", "A chunk length above 2³¹ - 1 is rejected", png_decode_chunk_length),
    case("huge", "Dimensions beyond section 12.3's limits are rejected before inflating", png_decode_huge),
    case("bomb", "Data that inflates past the image's expected size is rejected", png_decode_bomb),
    case("truncated", "A PNG cut off inside IDAT is rejected", png_decode_truncated),
    case("signature", "A bad signature is rejected", png_decode_signature),
    case("independent-fixtures", "The public PngSuite fixtures decode to the pixels an independent decoder recorded at authoring time", png_decode_independent_fixtures),
    case("blit", "A decoded RGBA image blitted into a surface gives the literal blended values", png_decode_blit),
    case("from-disk", "A pinned PNG read from the ext2 disk decodes to its recorded pixels", png_decode_from_disk),
];

pub(crate) const PNG_ENCODE: &[Case] = &[
    case("decodes", "An encoded frame decodes to the same pixels", png_encode_decodes),
    case("stable", "Encoding the same frame twice gives identical bytes", png_encode_stable),
    case("format", "The output is the documented format: RGB or RGBA at depth 8, filter 0, stored or fixed-Huffman blocks", png_encode_format),
    case("chunk-crc", "Every chunk's CRC matches the literal table's value", png_encode_chunk_crc),
    case("zlib-adler", "The zlib stream's Adler-32 is correct", png_encode_zlib_adler),
    case("sizes", "Frames of 1x1, odd widths and 1280x800 encode and decode", png_encode_sizes),
    case("limits", "A frame beyond the dimension limit is refused", png_encode_limits),
];

// ---------------------------------------------------------------------------------------
// inflate: clean-room inflate (contract section 11)
// ---------------------------------------------------------------------------------------

fn no_inflater() -> CaseResult {
    missing("libimg::inflate (inflate, zlib_decompress and the streaming Inflater)", "11")
}

fn inflate_stored() -> CaseResult { no_inflater() }
fn inflate_stored_length() -> CaseResult { no_inflater() }
fn inflate_fixed_huffman() -> CaseResult { no_inflater() }
fn inflate_dynamic_huffman() -> CaseResult { no_inflater() }
fn inflate_block_sequence() -> CaseResult { no_inflater() }
fn inflate_block_type_invalid() -> CaseResult { no_inflater() }
fn inflate_code_oversubscribed() -> CaseResult { no_inflater() }
fn inflate_code_incomplete() -> CaseResult { no_inflater() }
fn inflate_code_single() -> CaseResult { no_inflater() }
fn inflate_no_distance_codes() -> CaseResult { no_inflater() }
fn inflate_repeat_first() -> CaseResult { no_inflater() }
fn inflate_symbols_invalid() -> CaseResult { no_inflater() }
fn inflate_distance_too_far() -> CaseResult { no_inflater() }
fn inflate_distance_max() -> CaseResult { no_inflater() }
fn inflate_length_258() -> CaseResult { no_inflater() }
fn inflate_truncated() -> CaseResult { no_inflater() }
fn inflate_zlib_header() -> CaseResult { no_inflater() }
fn inflate_adler32() -> CaseResult { no_inflater() }
fn inflate_output_limit() -> CaseResult { no_inflater() }
fn inflate_byte_split() -> CaseResult { no_inflater() }

fn inflate_independent_fixtures() -> CaseResult {
    match std::fs::metadata("/usr/local/test/share/breengel/inflate") {
        Ok(_) => no_inflater(),
        Err(_) => missing("libimg::inflate and its fixtures (/usr/local/test/share/breengel/inflate)", "11"),
    }
}

// ---------------------------------------------------------------------------------------
// bmp: BMP images (contract section 12)
// ---------------------------------------------------------------------------------------

/// A 24-bit BMP written here from the format's description: headers, then rows padded to
/// four bytes, bottom-up unless `top_down`. `rows` are RGB, top row first.
fn bmp_fixture(width: u32, rows: &[Vec<[u8; 3]>], top_down: bool) -> Vec<u8> {
    let height = rows.len() as u32;
    let pad = (4 - (width as usize * 3) % 4) % 4;
    let data_len = (width as usize * 3 + pad) * height as usize;
    let mut v = Vec::new();
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&((54 + data_len) as u32).to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&(if top_down { -(height as i32) } else { height as i32 }).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    v.extend_from_slice(&[0; 16]);
    let order: Vec<&Vec<[u8; 3]>> = if top_down { rows.iter().collect() } else { rows.iter().rev().collect() };
    for row in order {
        for &[r, g, b] in row {
            v.extend_from_slice(&[b, g, r]);
        }
        v.extend(std::iter::repeat_n(0u8, pad));
    }
    v
}

fn flat(rows: &[Vec<[u8; 3]>]) -> Vec<u8> {
    rows.iter().flatten().flatten().copied().collect()
}

fn fixture_rows(width: u32, height: u32, seed: u64) -> Vec<Vec<[u8; 3]>> {
    let mut rng = Seeded(seed | 1);
    (0..height).map(|_| (0..width).map(|_| [rng.next() as u8, rng.next() as u8, rng.next() as u8]).collect()).collect()
}

fn decodes_to(section: &str, what: &str, bytes: &[u8], width: u32, rows: &[Vec<[u8; 3]>]) -> CaseResult {
    match libimg::bmp::decode_bmp_24(bytes) {
        Some((w, h, rgb)) if (w, h) == (width, rows.len() as u32) && rgb == flat(rows) => Ok(()),
        Some((w, h, rgb)) => at(section, format!("{what} decoded to {w}x{h} with {} differing bytes; the fixture is {width}x{}",
            rgb.iter().zip(flat(rows)).filter(|(a, b)| *a != b).count(), rows.len())),
        None => at(section, format!("{what} was rejected; it is a valid 24-bit BMP")),
    }
}

fn bmp_decode_fixtures() -> CaseResult {
    let rows = vec![vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]], vec![[10, 20, 30], [40, 50, 60], [70, 80, 90]]];
    decodes_to("12", "a 3x2 fixture", &bmp_fixture(3, &rows, false), 3, &rows)?;
    let rows = vec![vec![[1, 2, 3], [4, 5, 6]], vec![[7, 8, 9], [10, 11, 12]]];
    decodes_to("12", "the contract's 2x2 file", &BMP_2X2, 2, &rows)
}

fn bmp_orientation() -> CaseResult {
    let rows = fixture_rows(5, 4, 0xB0B);
    decodes_to("12", "a bottom-up 5x4 BMP", &bmp_fixture(5, &rows, false), 5, &rows)?;
    decodes_to("12", "the same pixels top-down (negative height)", &bmp_fixture(5, &rows, true), 5, &rows)
}

fn bmp_padding() -> CaseResult {
    for width in 1..=4 {
        let rows = fixture_rows(width, 3, width as u64 * 77);
        decodes_to("12", &format!("a {width}x3 BMP"), &bmp_fixture(width, &rows, false), width, &rows)?;
    }
    Ok(())
}

/// The contract's literal 70-byte file for the 2x2 image (section 12).
const BMP_2X2: [u8; 70] = [
    0x42, 0x4D, 0x46, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x36, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x02, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x08, 0x07, 0x0C, 0x0B, 0x0A,
    0x00, 0x00, 0x03, 0x02, 0x01, 0x06, 0x05, 0x04, 0x00, 0x00,
];

fn bmp_encode_bytes() -> CaseResult {
    let got = libimg::bmp::encode_bmp_24(2, 2, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    match got.iter().zip(BMP_2X2.iter()).position(|(a, b)| a != b) {
        _ if got.len() != BMP_2X2.len() => at("12", format!("encode_bmp_24 of the 2x2 image wrote {} bytes; the contract's file is 70", got.len())),
        Some(i) => at("12", format!("encode_bmp_24 of the 2x2 image differs at byte {i}: {:#04x}, the contract has {:#04x}", got[i], BMP_2X2[i])),
        None => Ok(()),
    }
}

fn bmp_roundtrip() -> CaseResult {
    for (w, h) in [(1u32, 1u32), (5, 3), (7, 7), (64, 2)] {
        let rows = fixture_rows(w, h, (w * 31 + h) as u64);
        let rgb = flat(&rows);
        let bytes = libimg::bmp::encode_bmp_24(w, h, &rgb);
        decodes_to("12", &format!("an encoded {w}x{h} image"), &bytes, w, &rows)?;
    }
    Ok(())
}

fn bmp_truncated() -> CaseResult {
    let rows = fixture_rows(3, 3, 0x7A7);
    let full = bmp_fixture(3, &rows, false);
    let len = full.len();
    decodes_to("12", "the whole 3x3 file", &full, 3, &rows)?;
    step("12", isolated("decoding every proper prefix of a 3x3 BMP", 5_000, move || {
        for cut in 0..len {
            if let Some((w, h, _)) = libimg::bmp::decode_bmp_24(&full[..cut]) {
                return Err(format!("the first {cut} of {len} bytes decoded as {w}x{h}; the contract rejects a truncated BMP"));
            }
        }
        Ok(())
    }))
}

/// A BMP header claiming `width` × `height` (height may be negative), followed by `data` bytes.
fn bmp_header(width: u32, height: i32, bpp: u16, compression: u32, data: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&((54 + data) as u32).to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&height.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&bpp.to_le_bytes());
    v.extend_from_slice(&compression.to_le_bytes());
    v.extend_from_slice(&(data as u32).to_le_bytes());
    v.extend_from_slice(&[0; 16]);
    v.extend(std::iter::repeat_n(0x33u8, data));
    v
}

/// The file is rejected, without a crash and without allocating anything.
fn rejected_before_allocating(section: &str, what: &str, bytes: Vec<u8>) -> CaseResult {
    let what = what.to_string();
    step(section, isolated(&what.clone(), 5_000, move || {
        let before = ALLOC_CALLS.load(Relaxed);
        let decoded = libimg::bmp::decode_bmp_24(&bytes).map(|(w, h, rgb)| (w, h, rgb.len()));
        let allocated = ALLOC_CALLS.load(Relaxed) - before;
        match decoded {
            Some((w, h, n)) => Err(format!("{what} decoded as {w}x{h} ({n} bytes); the contract rejects it")),
            None if allocated > 0 => Err(format!("{what} was rejected, but only after {allocated} allocations; the contract checks before allocating")),
            None => Ok(()),
        }
    }))
}

fn bmp_size_overflow() -> CaseResult {
    rejected_before_allocating("7, 12", "a BMP 1,431,655,766 pixels wide (its row size overflows 32 bits)", bmp_header(0x5555_5556, 1, 24, 0, 64))?;
    rejected_before_allocating("7, 12", "a BMP of 2^30 x 2^30 pixels", bmp_header(0x4000_0000, 0x4000_0000, 24, 0, 64))?;
    rejected_before_allocating("7, 12", "a BMP 2^31 - 1 rows tall, top-down", bmp_header(4, -i32::MAX, 24, 0, 64))
}

fn bmp_huge() -> CaseResult {
    rejected_before_allocating("7, 12", "a complete BMP 20,000 x 1 pixels", bmp_header(20_000, 1, 24, 0, 60_000))?;
    rejected_before_allocating("7, 12", "a complete BMP 1 x 16,385 pixels", bmp_header(1, 16_385, 24, 0, 4 * 16_385))?;
    rejected_before_allocating("7, 12", "a BMP of 4,097 x 4,097 pixels (16,785,409 pixels)", bmp_header(4_097, 4_097, 24, 0, 1_024))
}

fn bmp_unsupported() -> CaseResult {
    for (bpp, compression) in [(8u16, 0u32), (32, 0), (24, 1), (16, 3)] {
        if let Some((w, h, _)) = libimg::bmp::decode_bmp_24(&bmp_header(2, 2, bpp, compression, 64)) {
            return at("12", format!("a {bpp}-bit BMP with compression {compression} decoded as {w}x{h}; the contract does not support it"));
        }
    }
    missing("libimg::bmp::decode_bmp returning BmpError::Unsupported (every unsupported file is rejected, without a reason)", "12")
}

// ---------------------------------------------------------------------------------------
// png-decode and png-encode: PNG images (contract section 13)
// ---------------------------------------------------------------------------------------

fn no_png_decoder() -> CaseResult {
    missing("libimg::png::decode", "13")
}

fn no_png_encoder() -> CaseResult {
    missing("libimg::png::encode", "13")
}

fn png_decode_gray_depths() -> CaseResult { no_png_decoder() }
fn png_decode_gray_alpha() -> CaseResult { no_png_decoder() }
fn png_decode_rgb() -> CaseResult { no_png_decoder() }
fn png_decode_rgba() -> CaseResult { no_png_decoder() }
fn png_decode_palette_depths() -> CaseResult { no_png_decoder() }
fn png_decode_trns_gray() -> CaseResult { no_png_decoder() }
fn png_decode_trns_rgb() -> CaseResult { no_png_decoder() }
fn png_decode_trns_palette() -> CaseResult { no_png_decoder() }
fn png_decode_invalid_combos() -> CaseResult { no_png_decoder() }
fn png_decode_rounding_16bit() -> CaseResult { no_png_decoder() }
fn png_decode_filters() -> CaseResult { no_png_decoder() }
fn png_decode_filter_invalid() -> CaseResult { no_png_decoder() }
fn png_decode_interlace() -> CaseResult { no_png_decoder() }
fn png_decode_interlace_small() -> CaseResult { no_png_decoder() }
fn png_decode_multi_idat() -> CaseResult { no_png_decoder() }
fn png_decode_crc() -> CaseResult { no_png_decoder() }
fn png_decode_chunk_order() -> CaseResult { no_png_decoder() }
fn png_decode_plte_required() -> CaseResult { no_png_decoder() }
fn png_decode_plte_size() -> CaseResult { no_png_decoder() }
fn png_decode_palette_index() -> CaseResult { no_png_decoder() }
fn png_decode_critical_unknown() -> CaseResult { no_png_decoder() }
fn png_decode_ancillary_ignored() -> CaseResult { no_png_decoder() }
fn png_decode_ihdr_invalid() -> CaseResult { no_png_decoder() }
fn png_decode_chunk_length() -> CaseResult { no_png_decoder() }
fn png_decode_huge() -> CaseResult { no_png_decoder() }
fn png_decode_bomb() -> CaseResult { no_png_decoder() }
fn png_decode_truncated() -> CaseResult { no_png_decoder() }
fn png_decode_signature() -> CaseResult { no_png_decoder() }

fn png_decode_independent_fixtures() -> CaseResult {
    match std::fs::metadata("/usr/local/test/share/breengel/png") {
        Ok(_) => no_png_decoder(),
        Err(_) => missing("libimg::png::decode and the PngSuite fixtures (/usr/local/test/share/breengel/png)", "13"),
    }
}

fn png_decode_blit() -> CaseResult {
    missing("libimg::png::decode and libgfx blit_over", "5.8, 13")
}

fn png_decode_from_disk() -> CaseResult {
    png_decode_independent_fixtures()
}

fn png_encode_decodes() -> CaseResult { no_png_encoder() }
fn png_encode_stable() -> CaseResult { no_png_encoder() }
fn png_encode_format() -> CaseResult { no_png_encoder() }
fn png_encode_chunk_crc() -> CaseResult { no_png_encoder() }
fn png_encode_zlib_adler() -> CaseResult { no_png_encoder() }
fn png_encode_sizes() -> CaseResult { no_png_encoder() }
fn png_encode_limits() -> CaseResult { no_png_encoder() }
