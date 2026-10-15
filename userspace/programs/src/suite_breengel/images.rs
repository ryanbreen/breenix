//! inflate, bmp, png-decode and png-encode (contract sections 7 and 11 to 13).

use crate::*;
#[path = "../../../../libs/libimg/tests/checks.rs"]
mod checks;

fn picture_case(run: impl FnOnce() -> CaseResult) -> CaseResult {
    let _ = checks::take_judged_images();
    let result = run();
    let handoff = handoff_images(checks::take_judged_images());
    match (result, handoff) {
        (Err(primary), Err(secondary)) => fail(format!("{primary:?}; showcase: {secondary:?}")),
        (Err(primary), _) => Err(primary),
        (Ok(()), handoff) => handoff,
    }
}
fn image_check(name: &str) -> CaseResult {
    picture_case(|| {
        let run = checks::CHECKS.iter().find(|(id, _)| *id == name)
            .map(|(_, run)| *run).ok_or_else(|| format!("unknown image check {name}"))?;
        run().map_err(|e| format!("contract §§7,11–13: {e}"))?;
        Ok(())
    })
}

// An opaque, lossless view of the compared bytes: BGR alongside alpha in grey,
// with actual, reference and absolute difference rows for each fixture. No
// source is redrawn or composited against a background that could hide alpha.
fn handoff_images(images: Vec<checks::JudgedImage>) -> CaseResult {
    use libgfx::framebuf::Format;
    if images.is_empty() { return Ok(()); }
    let mut placements = Vec::new();
    let width = images.iter().map(|i| i.actual.width.max(i.expected.width) as usize * 2)
        .max().unwrap_or(1).max(1280);
    let (mut x, mut y, mut row_height) = (0, 0, 0);
    for image in &images {
        let w = image.actual.width.max(image.expected.width) as usize;
        let h = image.actual.height.max(image.expected.height) as usize;
        let tile_width = (w * 2).max(240);
        let tile_height = h * 3 + 48;
        if x + tile_width > width { x = 0; y += row_height; row_height = 0; }
        placements.push((x, y, w, h));
        x += tile_width;
        row_height = row_height.max(tile_height);
    }
    let mut frame = FrameBuf::new_filled(width, y + row_height, Format::BGRX, Color::rgb(12, 19, 31))
        .map_err(|e| format!("image showcase surface: {e:?}"))?;
    for (ordinal, (image, &(x0, y0, w, h))) in images.iter().zip(&placements).enumerate() {
        for (row, title) in ["ACTUAL BGR / ALPHA", "REFERENCE BGR / ALPHA", "DIFFERENCE BGR / ALPHA"].iter().enumerate() {
            let y0 = y0 + row * (h + 16);
            font5x7::draw_text(&mut frame, format!("{} {} {}", ordinal + 1, image.label, title).as_bytes(), x0, y0, Color::rgb(255, 255, 255), 1);
            for y in 0..h { for x in 0..w {
                let read = |i: &libimg::png::Image| -> [u8; 4] {
                    if x >= i.width as usize || y >= i.height as usize { return [0; 4]; }
                    let at = (y * i.width as usize + x) * 4;
                    i.pixels.get(at..at+4).and_then(|p| p.try_into().ok()).unwrap_or([0; 4])
                };
                let a = read(&image.actual);
                let r = read(&image.expected);
                let p = match row { 0 => a, 1 => r, _ => std::array::from_fn(|c| a[c].abs_diff(r[c])) };
                frame.put_pixel(x0 + x, y0 + 16 + y, Color::rgb(p[2], p[1], p[0]));
                frame.put_pixel(x0 + w + x, y0 + 16 + y, Color::rgb(p[3], p[3], p[3]));
            } }
        }
    }
    libbreenix::suite::judged(&frame)
}

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
    case(
        "decode-fixtures",
        "Independent BMP fixtures decode to their literal pixels",
        bmp_decode_fixtures,
    ),
    case(
        "orientation",
        "Bottom-up and top-down (negative height) BMPs decode to the same pixels",
        bmp_orientation,
    ),
    case(
        "padding",
        "Widths 1 to 4 decode correctly across row padding",
        bmp_padding,
    ),
    case(
        "encode-bytes",
        "encode_bmp_24 writes the literal expected header and rows",
        bmp_encode_bytes,
    ),
    case(
        "roundtrip",
        "Encoding then decoding returns the original pixels",
        bmp_roundtrip,
    ),
    case(
        "truncated",
        "A BMP shorter than its header claims is rejected",
        bmp_truncated,
    ),
    case(
        "size-overflow",
        "Dimensions whose row or image size overflows are rejected before any allocation",
        bmp_size_overflow,
    ),
    case(
        "huge",
        "Dimensions beyond section 12.3's limits are rejected before any allocation",
        bmp_huge,
    ),
    case(
        "unsupported",
        "An unsupported depth or compression is rejected with its error",
        bmp_unsupported,
    ),
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

fn inflate_stored() -> CaseResult {
    image_check("inflate_stored")
}
fn inflate_stored_length() -> CaseResult {
    image_check("inflate_stored_length")
}
fn inflate_fixed_huffman() -> CaseResult {
    image_check("inflate_fixed_huffman")
}
fn inflate_dynamic_huffman() -> CaseResult {
    image_check("inflate_dynamic_huffman")
}
fn inflate_block_sequence() -> CaseResult {
    image_check("inflate_block_sequence")
}
fn inflate_block_type_invalid() -> CaseResult {
    image_check("inflate_block_type_invalid")
}
fn inflate_code_oversubscribed() -> CaseResult {
    image_check("inflate_code_oversubscribed")
}
fn inflate_code_incomplete() -> CaseResult {
    image_check("inflate_code_incomplete")
}
fn inflate_code_single() -> CaseResult {
    image_check("inflate_code_single")
}
fn inflate_no_distance_codes() -> CaseResult {
    image_check("inflate_no_distance_codes")
}
fn inflate_repeat_first() -> CaseResult {
    image_check("inflate_repeat_first")
}
fn inflate_symbols_invalid() -> CaseResult {
    image_check("inflate_symbols_invalid")
}
fn inflate_distance_too_far() -> CaseResult {
    image_check("inflate_distance_too_far")
}
fn inflate_distance_max() -> CaseResult {
    image_check("inflate_distance_max")
}
fn inflate_length_258() -> CaseResult {
    image_check("inflate_length_258")
}
fn inflate_truncated() -> CaseResult {
    image_check("inflate_truncated")
}
fn inflate_zlib_header() -> CaseResult {
    image_check("inflate_zlib_header")
}
fn inflate_adler32() -> CaseResult {
    image_check("inflate_adler32")
}
fn inflate_output_limit() -> CaseResult {
    image_check("inflate_output_limit")
}
fn inflate_byte_split() -> CaseResult {
    image_check("inflate_byte_split")
}
fn inflate_independent_fixtures() -> CaseResult {
    image_check("inflate_independent_fixtures")
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
    v.extend_from_slice(
        &(if top_down {
            -(height as i32)
        } else {
            height as i32
        })
        .to_le_bytes(),
    );
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    v.extend_from_slice(&[0; 16]);
    let order: Vec<&Vec<[u8; 3]>> = if top_down {
        rows.iter().collect()
    } else {
        rows.iter().rev().collect()
    };
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
    (0..height)
        .map(|_| {
            (0..width)
                .map(|_| [rng.next() as u8, rng.next() as u8, rng.next() as u8])
                .collect()
        })
        .collect()
}

fn decodes_to(
    section: &str,
    what: &str,
    bytes: &[u8],
    width: u32,
    rows: &[Vec<[u8; 3]>],
) -> CaseResult {
    match libimg::bmp::decode_bmp_24(bytes) {
        Some((w, h, rgb)) => {
            let matches = (w, h) == (width, rows.len() as u32) && rgb == flat(rows);
            let pixels = rgb
                .chunks_exact(3)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect();
            checks::record_image(what.to_string(), libimg::png::Image { width: w, height: h, pixels },
                libimg::png::Image { width, height: rows.len() as u32,
                    pixels: flat(rows).chunks_exact(3).flat_map(|p| [p[2], p[1], p[0], 255]).collect() });
            if matches {
                Ok(())
            } else {
                at(section,format!("{what} decoded to {w}x{h} with {} differing bytes; the fixture is {width}x{}",rgb.iter().zip(flat(rows)).filter(|(a,b)|*a!=b).count(),rows.len()))
            }
        }
        None => at(
            section,
            format!("{what} was rejected; it is a valid 24-bit BMP"),
        ),
    }
}

fn bmp_decode_fixtures() -> CaseResult { picture_case(bmp_decode_fixtures_checks) }
fn bmp_decode_fixtures_checks() -> CaseResult {
    let rows = vec![
        vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]],
        vec![[10, 20, 30], [40, 50, 60], [70, 80, 90]],
    ];
    decodes_to(
        "12",
        "a 3x2 fixture",
        &bmp_fixture(3, &rows, false),
        3,
        &rows,
    )?;
    let rows = vec![vec![[1, 2, 3], [4, 5, 6]], vec![[7, 8, 9], [10, 11, 12]]];
    decodes_to("12", "the contract's 2x2 file", &BMP_2X2, 2, &rows)
}

fn bmp_orientation() -> CaseResult { picture_case(bmp_orientation_checks) }
fn bmp_orientation_checks() -> CaseResult {
    let rows = fixture_rows(5, 4, 0xB0B);
    decodes_to(
        "12",
        "a bottom-up 5x4 BMP",
        &bmp_fixture(5, &rows, false),
        5,
        &rows,
    )?;
    decodes_to(
        "12",
        "the same pixels top-down (negative height)",
        &bmp_fixture(5, &rows, true),
        5,
        &rows,
    )
}

fn bmp_padding() -> CaseResult { picture_case(bmp_padding_checks) }
fn bmp_padding_checks() -> CaseResult {
    for width in 1..=4 {
        let rows = fixture_rows(width, 3, width as u64 * 77);
        decodes_to(
            "12",
            &format!("a {width}x3 BMP"),
            &bmp_fixture(width, &rows, false),
            width,
            &rows,
        )?;
    }
    Ok(())
}

/// The contract's literal 70-byte file for the 2x2 image (section 12).
const BMP_2X2: [u8; 70] = [
    0x42, 0x4D, 0x46, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x36, 0x00, 0x00, 0x00, 0x28, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x18, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x08, 0x07, 0x0C, 0x0B, 0x0A, 0x00, 0x00, 0x03, 0x02,
    0x01, 0x06, 0x05, 0x04, 0x00, 0x00,
];

fn bmp_encode_bytes() -> CaseResult {
    let got = libimg::bmp::encode_bmp_24(2, 2, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    match got.iter().zip(BMP_2X2.iter()).position(|(a, b)| a != b) {
        _ if got.len() != BMP_2X2.len() => at("12", format!("encode_bmp_24 of the 2x2 image wrote {} bytes; the contract's file is 70", got.len())),
        Some(i) => at("12", format!("encode_bmp_24 of the 2x2 image differs at byte {i}: {:#04x}, the contract has {:#04x}", got[i], BMP_2X2[i])),
        None => Ok(()),
    }
}

fn bmp_roundtrip() -> CaseResult { picture_case(bmp_roundtrip_checks) }
fn bmp_roundtrip_checks() -> CaseResult {
    for (w, h) in [(1u32, 1u32), (5, 3), (7, 7), (64, 2)] {
        let rows = fixture_rows(w, h, (w * 31 + h) as u64);
        let rgb = flat(&rows);
        let bytes = libimg::bmp::encode_bmp_24(w, h, &rgb);
        decodes_to("12", &format!("an encoded {w}x{h} image"), &bytes, w, &rows)?;
    }
    Ok(())
}

fn bmp_truncated() -> CaseResult { picture_case(bmp_truncated_checks) }
fn bmp_truncated_checks() -> CaseResult {
    let rows = fixture_rows(3, 3, 0x7A7);
    let full = bmp_fixture(3, &rows, false);
    let len = full.len();
    decodes_to("12", "the whole 3x3 file", &full, 3, &rows)?;
    step(
        "12",
        isolated(
            "decoding every proper prefix of a 3x3 BMP",
            5_000,
            move || {
                for cut in 0..len {
                    if let Some((w, h, _)) = libimg::bmp::decode_bmp_24(&full[..cut]) {
                        return Err(format!("the first {cut} of {len} bytes decoded as {w}x{h}; the contract rejects a truncated BMP"));
                    }
                }
                checks::bmp_truncated_claim()
            },
        ),
    )
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
fn rejected_before_allocating(section: &str, what: &str, bytes: Vec<u8>, expected: libimg::bmp::BmpError) -> CaseResult {
    let what = what.to_string();
    step(
        section,
        isolated(&what.clone(), 5_000, move || {
            let before = ALLOC_CALLS.load(Relaxed);
            let decoded = libimg::bmp::decode_bmp(&bytes).map(|b| (b.width, b.height, b.rgb.len()));
            let allocated = ALLOC_CALLS.load(Relaxed) - before;
            match decoded {
            Ok((w, h, n)) => Err(format!("{what} decoded as {w}x{h} ({n} bytes); the contract rejects it")),
            Err(_) if allocated > 0 => Err(format!("{what} was rejected, but only after {allocated} allocations; the contract checks before allocating")),
            Err(error) if error == expected => Ok(()),
            Err(error) => Err(format!("{what}: got {error:?}, expected {expected:?}")),
        }
        }),
    )
}

fn bmp_size_overflow() -> CaseResult {
    rejected_before_allocating(
        "7, 12",
        "a BMP 1,431,655,766 pixels wide (its row size overflows 32 bits)",
        bmp_header(0x5555_5556, 1, 24, 0, 64),
        libimg::bmp::BmpError::SizeOverflow,
    )?;
    rejected_before_allocating(
        "7, 12",
        "a BMP of 2^30 x 2^30 pixels",
        bmp_header(0x4000_0000, 0x4000_0000, 24, 0, 64),
        libimg::bmp::BmpError::SizeOverflow,
    )?;
    rejected_before_allocating(
        "7, 12",
        "a BMP 2^31 - 1 rows tall, top-down",
        bmp_header(4, -i32::MAX, 24, 0, 64),
        libimg::bmp::BmpError::SizeOverflow,
    )
}

fn bmp_huge() -> CaseResult {
    rejected_before_allocating(
        "7, 12",
        "a complete BMP 20,000 x 1 pixels",
        bmp_header(20_000, 1, 24, 0, 60_000),
        libimg::bmp::BmpError::TooLarge,
    )?;
    rejected_before_allocating(
        "7, 12",
        "a complete BMP 1 x 16,385 pixels",
        bmp_header(1, 16_385, 24, 0, 4 * 16_385),
        libimg::bmp::BmpError::TooLarge,
    )?;
    rejected_before_allocating(
        "7, 12",
        "a BMP of 4,097 x 4,097 pixels (16,785,409 pixels)",
        bmp_header(4_097, 4_097, 24, 0, 1_024),
        libimg::bmp::BmpError::TooLarge,
    )
}

fn bmp_unsupported() -> CaseResult {
    image_check("bmp_unsupported")
}

// ---------------------------------------------------------------------------------------
// png-decode and png-encode: PNG images (contract section 13)
// ---------------------------------------------------------------------------------------

fn png_decode_gray_depths() -> CaseResult {
    image_check("png_decode_gray_depths")
}
fn png_decode_gray_alpha() -> CaseResult {
    image_check("png_decode_gray_alpha")
}
fn png_decode_rgb() -> CaseResult {
    image_check("png_decode_rgb")
}
fn png_decode_rgba() -> CaseResult {
    image_check("png_decode_rgba")
}
fn png_decode_palette_depths() -> CaseResult {
    image_check("png_decode_palette_depths")
}
fn png_decode_trns_gray() -> CaseResult {
    image_check("png_decode_trns_gray")
}
fn png_decode_trns_rgb() -> CaseResult {
    image_check("png_decode_trns_rgb")
}
fn png_decode_trns_palette() -> CaseResult {
    image_check("png_decode_trns_palette")
}
fn png_decode_invalid_combos() -> CaseResult {
    image_check("png_decode_invalid_combos")
}
fn png_decode_rounding_16bit() -> CaseResult {
    image_check("png_decode_rounding_16bit")
}
fn png_decode_filters() -> CaseResult {
    image_check("png_decode_filters")
}
fn png_decode_filter_invalid() -> CaseResult {
    image_check("png_decode_filter_invalid")
}
fn png_decode_interlace() -> CaseResult {
    image_check("png_decode_interlace")
}
fn png_decode_interlace_small() -> CaseResult {
    image_check("png_decode_interlace_small")
}
fn png_decode_multi_idat() -> CaseResult {
    image_check("png_decode_multi_idat")
}
fn png_decode_crc() -> CaseResult {
    image_check("png_decode_crc")
}
fn png_decode_chunk_order() -> CaseResult {
    image_check("png_decode_chunk_order")
}
fn png_decode_plte_required() -> CaseResult {
    image_check("png_decode_plte_required")
}
fn png_decode_plte_size() -> CaseResult {
    image_check("png_decode_plte_size")
}
fn png_decode_palette_index() -> CaseResult {
    image_check("png_decode_palette_index")
}
fn png_decode_critical_unknown() -> CaseResult {
    image_check("png_decode_critical_unknown")
}
fn png_decode_ancillary_ignored() -> CaseResult {
    image_check("png_decode_ancillary_ignored")
}
fn png_decode_ihdr_invalid() -> CaseResult {
    image_check("png_decode_ihdr_invalid")
}
fn png_decode_chunk_length() -> CaseResult {
    image_check("png_decode_chunk_length")
}
fn png_decode_huge() -> CaseResult {
    image_check("png_decode_huge")?;
    for (w, h) in [(16385, 1), (1, 16385), (4097, 4097)] {
        let bytes = checks::pngfile(&checks::ihdr(w, h, 8, 0, 0), &[], &[0, 42]);
        let before = ALLOC_CALLS.load(Relaxed);
        let result = libimg::png::decode(&bytes);
        let allocations = ALLOC_CALLS.load(Relaxed) - before;
        if result != Err(libimg::png::PngError::TooLarge) || allocations != 0 {
            return at(
                "7,13",
                format!(
                    "oversized PNG {w}x{h}: result {:?}, {allocations} allocations before refusal",
                    result.map(|i| (i.width, i.height))
                ),
            );
        }
    }
    Ok(())
}
fn png_decode_bomb() -> CaseResult {
    image_check("png_decode_bomb")
}
fn png_decode_truncated() -> CaseResult {
    image_check("png_decode_truncated")
}
fn png_decode_signature() -> CaseResult {
    image_check("png_decode_signature")
}
fn png_decode_independent_fixtures() -> CaseResult {
    image_check("png_decode_independent_fixtures")
}
fn blit_decoded(image: &mut libimg::png::Image) -> Result<libimg::png::Image, String> {
    use libgfx::framebuf::Format;
    // SAFETY: the caller checked this decoded fixture is exactly 1x1/4 bytes;
    // its vector remains exclusively borrowed until this raw surface is dropped.
    let source = unsafe {
        FrameBuf::from_raw_format(
            image.pixels.as_mut_ptr(),
            1,
            1,
            4,
            Format::BGRA_PREMULTIPLIED,
        )
    }
    .map_err(|e| format!("PNG blit source: {e:?}"))?;
    let mut dest = FrameBuf::new_filled(3, 1, Format::BGRX, Color::rgb(200, 200, 200))
        .map_err(|e| format!("PNG blit destination: {e:?}"))?;
    libgfx::blit::blit_over(&mut dest, &source, 1, 0);
    // SAFETY: this owned 3x1 BGRX surface contains exactly twelve bytes;
    // blit has finished and no mutable access overlaps the snapshot.
    let pixels = unsafe { std::slice::from_raw_parts(dest.raw_ptr(), 12) }.to_vec();
    Ok(libimg::png::Image {
        width: 3,
        height: 1,
        pixels,
    })
}
fn png_decode_blit() -> CaseResult { picture_case(png_decode_blit_checks) }
fn png_decode_blit_checks() -> CaseResult {
    let bytes = checks::pngfile(&checks::ihdr(1, 1, 8, 6, 0), &[], &[0, 128, 0, 0, 128]);
    let mut image = libimg::png::decode(&bytes).map_err(|e| format!("PNG blend fixture: {e:?}"))?;
    if (image.width, image.height) != (1, 1) || image.pixels != [0, 0, 64, 128] {
        return at(
            "2,13",
            "decoded red-128 at alpha-128 must be premultiplied BGRA [0,0,64,128]",
        );
    }
    let actual = blit_decoded(&mut image)?;
    // §2 literal: source channel 64 at alpha 128 over channel 200 gives 164;
    // the two zero source channels give 100, and the opaque background stays opaque.
    let expected = [200, 200, 200, 255, 100, 100, 164, 255, 200, 200, 200, 255];
    let matches = (actual.width, actual.height) == (3, 1) && actual.pixels == expected;
    checks::record_image("source-over".into(), actual,
        libimg::png::Image { width: 3, height: 1, pixels: expected.to_vec() });
    if matches {
        Ok(())
    } else {
        at(
            "2,5.8,13",
            "PNG source-over must produce [100,100,164,255] only at the middle pixel",
        )
    }
}
fn png_decode_from_disk() -> CaseResult {
    image_check("png_decode_from_disk")
}
fn png_encode_decodes() -> CaseResult {
    image_check("png_encode_decodes")
}
fn png_encode_stable() -> CaseResult {
    image_check("png_encode_stable")
}
fn png_encode_format() -> CaseResult {
    image_check("png_encode_format")
}
fn png_encode_chunk_crc() -> CaseResult {
    image_check("png_encode_chunk_crc")
}
fn png_encode_zlib_adler() -> CaseResult {
    image_check("png_encode_zlib_adler")
}
fn png_encode_sizes() -> CaseResult {
    image_check("png_encode_sizes")
}
fn png_encode_limits() -> CaseResult {
    image_check("png_encode_limits")?;
    for (w, h) in [(16385, 1), (1, 16385), (4097, 4097)] {
        let image = libimg::png::Image {
            width: w,
            height: h,
            pixels: Vec::new(),
        };
        let before = ALLOC_CALLS.load(Relaxed);
        let result = libimg::png::encode(&image);
        let allocations = ALLOC_CALLS.load(Relaxed) - before;
        if result != Err(libimg::png::PngError::TooLarge) || allocations != 0 {
            return at("7,13",format!("oversized encode {w}x{h}: result {:?}, {allocations} allocations before refusal",result.map(|b|b.len())));
        }
    }
    Ok(())
}
