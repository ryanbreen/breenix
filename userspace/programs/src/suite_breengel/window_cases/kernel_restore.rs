//! Restoration oracle: exact static pixels and a complete serial-derived live heading.
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

/// Independent reconstruction from producer timestamps and raw suite records.
pub struct Heading {
    pub prefix: String,
    case: String,
    skipped: usize,
    started_ms: u64,
    done_ms: Option<u64>,
}

impl Heading {
    pub fn read(serial: &str) -> Option<Self> {
        let (mut total, mut passed, mut failed, mut skipped) = (None, 0, 0, 0);
        let (mut started_ms, mut done_ms, mut case) = (0, None, "waiting");
        for line in serial.lines() {
            let (stamp, record) = line.split_once('|')?;
            let ms = stamp.parse::<u64>().ok()?;
            let fields: Vec<_> = record.split_whitespace().collect();
            if fields.get(..3) == Some(&["SUITE", "breengel", "START"]) {
                total = fields.iter().find_map(|s| s.strip_prefix("cases=")?.parse::<usize>().ok());
                (passed, failed, skipped) = (0, 0, 0);
                (started_ms, done_ms, case) = (ms, None, "waiting");
            } else if total.is_some() && fields.get(..3) == Some(&["SUITE", "breengel", "CASE"]) {
                match fields.get(4).copied() {
                    Some("PASS") => passed += 1,
                    Some("FAIL") => failed += 1,
                    Some("SKIP") => skipped += 1,
                    _ => return None,
                }
                case = fields.get(3)?;
            } else if total.is_some() && fields.get(..3) == Some(&["SUITE", "breengel", "DONE"]) {
                done_ms = Some(ms);
            }
        }
        let total = total?;
        let pending = total.checked_sub(passed + failed + skipped)?;
        Some(Self {
            prefix: format!("breengel: {passed} passed · {failed} failed · {pending} pending of {total}"),
            case: case.into(), skipped, started_ms, done_ms,
        })
    }

    fn text(&self, seconds: u64) -> String {
        let mut heading = format!("{} · {}:{:02} · case: {}", self.prefix, seconds / 60, seconds % 60, self.case);
        if self.skipped > 0 { heading.push_str(&format!(" · {} skipped", self.skipped)); }
        heading
    }
}

pub fn compare(before: &[u8], after: &[u8], width: usize, height: usize, heading: &Heading, samples: [(u64, u64); 2]) -> Result<(), String> {
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
    // Reconstruct the entire band, including blank pixels after the clipped text.
    // Panel refreshes use a 250 ms interval; allow 50 ms for tick resolution
    // and sampling around that publication. A delayed scanner cannot reset START.
    let glyph_width = get_raster_width(FontWeight::Regular, font);
    for (pixels, (first, last)) in [before, after].into_iter().zip(samples) {
        if first > last { return Err("screen sample times are reversed".into()); }
        let elapsed = |now: u64| heading.done_ms.unwrap_or(now).saturating_sub(heading.started_ms) / 1000;
        let low = elapsed(first.saturating_sub(300));
        let high = elapsed(last);
        let matches = (low..=high).any(|seconds| {
            let glyphs: Vec<_> = heading.text(seconds).chars()
                .map(|ch| get_raster(ch, FontWeight::Regular, font)).collect();
            (0..rows).all(|dy| (0..span).all(|dx| {
                let (gx, gy) = (dx / scale, dy / scale);
                let coverage = glyphs.get(gx / glyph_width).and_then(|g| g.as_ref())
                    .and_then(|g| g.raster().get(gy))
                    .and_then(|row| row.get(gx % glyph_width)).copied().unwrap_or(0);
                let a = u32::from(coverage);
                let rgb: [u8;3] = core::array::from_fn(|c| {
                    (([214,225,244][c] * a + [12,19,31][c] * (255-a) + 127) / 255) as u8
                });
                let at = ((top+dy)*width+left+dx)*4;
                pixels[at..at+4] == [rgb[2],rgb[1],rgb[0],255]
            }))
        });
        if !matches {
            return Err(format!("kernel heading differs from captured serial or elapsed interval {low}..={high}: {}", heading.prefix));
        }
    }
    Ok(())
}
