//! B36a's restoration oracle: exact static pixels and serial-derived heading counts.
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

pub fn counts(serial: &str) -> Option<String> {
    let (mut total, mut passed, mut failed, mut skipped) = (None, 0, 0, 0);
    for line in serial.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.get(..3) == Some(&["SUITE", "breengel", "START"]) {
            total = fields.iter().find_map(|s| s.strip_prefix("cases=")?.parse::<usize>().ok());
            (passed, failed, skipped) = (0, 0, 0);
        } else if total.is_some() && fields.get(..3) == Some(&["SUITE", "breengel", "CASE"]) {
            match fields.get(4).copied() {
                Some("PASS") => passed += 1,
                Some("FAIL") => failed += 1,
                Some("SKIP") => skipped += 1,
                _ => (),
            }
        }
    }
    let total = total?;
    let pending = total.checked_sub(passed + failed + skipped)?;
    Some(format!("breengel: {passed} passed · {failed} failed · {pending} pending of {total}"))
}

pub fn compare(before: &[u8], after: &[u8], width: usize, height: usize, prefix: &str) -> Result<(), String> {
    if before.len() != width * height * 4 || after.len() != before.len() {
        return Err("kernel screen capture has the wrong extent".into());
    }
    // Independent B36 geometry, including the console contract's native raster rule.
    let (left, top, span, rows, font, scale) = if width % 1280 == 0 && width / 1280 > 0 && height == 800 * (width / 1280) {
        let scale = width / 1280;
        (16 * scale, 712 * scale, 1248 * scale, 24 * scale, RasterHeight::Size20, scale)
    } else {
        let (glyph, line, font) = [(14,34,RasterHeight::Size32), (11,26,RasterHeight::Size24),
                                  (9,22,RasterHeight::Size20), (7,18,RasterHeight::Size16)]
            .into_iter().find(|&(g,l,_)| width/g >= 128 && height/l >= 36)
            .unwrap_or((7,18,RasterHeight::Size16));
        if width <= 4 * glyph || height < 2 * line + 48 { return Err("mode cannot hold the heading".into()); }
        (2 * glyph, height - (2 * line + 48) + 8, width - 4 * glyph, line, font, 1)
    };
    for i in 0..width * height {
        let (x,y) = (i % width, i / width);
        if (left..left+span).contains(&x) && (top..top+rows).contains(&y) { continue; }
        if before[i*4..i*4+4] != after[i*4..i*4+4] {
            return Err(format!("kernel's restored screen differs at ({x}, {y}) outside the live heading"));
        }
    }
    // Compare the known counts to font samples, independently of the kernel painter.
    // Both captures must contain the current counts; the elapsed field may advance.
    let glyph_width = get_raster_width(FontWeight::Regular, font);
    for pixels in [before, after] {
        for (col,ch) in prefix.chars().enumerate() {
            let glyph = get_raster(ch, FontWeight::Regular, font).ok_or("heading reference glyph absent")?;
            for (dy,row) in glyph.raster().iter().enumerate() {
                for (dx,&coverage) in row.iter().enumerate() {
                    let a = u32::from(coverage);
                    let rgb: [u8;3] = core::array::from_fn(|c| {
                        (([214,225,244][c] * a + [12,19,31][c] * (255-a) + 127) / 255) as u8
                    });
                    for sy in 0..scale { for sx in 0..scale {
                        let (x,y) = (left+(col*glyph_width+dx)*scale+sx, top+dy*scale+sy);
                        if x >= left+span || y >= top+rows { continue; }
                        let at = (y*width+x)*4;
                        if pixels[at..at+4] != [rgb[2],rgb[1],rgb[0],255] {
                            return Err(format!("kernel heading counts differ from serial at ({x}, {y}); expected {prefix}"));
                        }
                    }}
                }
            }
        }
    }
    Ok(())
}
