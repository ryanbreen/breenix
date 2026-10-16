//! Independent live checks of the production runner, contract §23.8.
use super::*;
/// Live showcase checks exercise the production renderer and v2 scanout, with one-shot
/// faults consumed by the same code used during the real runner's between-case draw.
pub fn showcase_check(name: &str) -> CaseResult {
    let mut s = SHOW
        .lock()
        .unwrap()
        .clone()
        .ok_or("no runner stage evidence")?;
    if name == "quiet-window" {
        check(
            s.quiet(),
            "a runner composite overlaps an earlier measured case",
        )?;
        for faulty in [true, false] {
            if faulty {
                crate::test_control::arm(14, process::getpid()?.raw() as u64, 0)?;
            }
            let pid = process::getpid()?.raw() as u64;
            let before = kernel_serial(pid).map_err(crate::suite::CaseError::Fail)?;
            let bits = fault_bits();
            tick_show(&mut s, bits).map_err(crate::suite::CaseError::Fail)?;
            let after = kernel_serial(pid).map_err(crate::suite::CaseError::Fail)?;
            let found = after != before;
            check(
                found == faulty,
                "runner in-window draw detection disagrees with the composite records",
            )?;
            crate::test_control::disarm_all()?;
        }
        return Ok(());
    }
    if name == "judged-frame-shown" {
        let pixels = std::vec![0,0,0,255];
        let hash = libpixcheck::sha256(&pixels);
        let line = std::format!("sha256={}", hash.iter().map(|b| std::format!("{b:02x}")).collect::<String>());
        let mut frame = JudgedFrame { width:1,height:1,pixels,sha256:hash,expected:None,bound:0 };
        verify_source(&line,&frame).map_err(crate::suite::CaseError::Fail)?;
        frame.pixels[0] ^= 255;
        check(verify_source(&line,&frame).is_err(), "wrong source selected at record() was accepted")?;
    }
    let point = match name {
        "judged-frame-shown" => Some(13),
        "serial-agreement" => Some(12),
        "boundary-notice" => None,
        _ => return Err("unknown live stage check".into()),
    };
    for faulty in if point.is_some() {
        &[true, false][..]
    } else {
        &[false][..]
    } {
        if *faulty {
            crate::test_control::arm(point.unwrap(), process::getpid()?.raw() as u64, 0)?;
        }
        s.draw(fault_bits())
            .map_err(crate::suite::CaseError::Fail)?;
        let (pixels, rec) = snapshot().map_err(crate::suite::CaseError::Fail)?;
        let mut mismatch = false;
        match name {
            "judged-frame-shown" => {
                let mut n = 0;
                for (i, e) in s.entries.iter().enumerate() {
                    if let Some(p) = &e.picture {
                        n += 1;
                        let hash: String = p
                            .source_hash
                            .iter()
                            .map(|b| std::format!("{b:02x}"))
                            .collect();
                        check(
                            e.line.contains(&std::format!(" sha256={hash}")),
                            "tile snapshot hash differs from its stored CASE bytes",
                        )?;
                        check(
                            libpixcheck::sha256(&p.pixels) == p.thumb_hash,
                            "thumbnail snapshot changed after verification",
                        )?;
                        let slot = i + s.groups.iter().filter(|v| v.2 <= i).count();
                        let (left, top) = (18 + 40 * (slot % 20), 114 + 28 * (slot / 20));
                        for y in 0..p.height {
                            for x in 0..p.width {
                                let o = ((top + y) * 1280 + left + x) * 4;
                                let r = (y * p.width + x) * 4;
                                mismatch |= pixels[o..o + 4] != p.pixels[r..r + 4];
                            }
                        }
                    }
                }
                check(n > 0, "no earlier judged pictures to check")?;
            }
            "serial-agreement" => {
                for (i, e) in s.entries.iter().enumerate() {
                    if e.line.is_empty() {
                        continue;
                    }
                    let status = match e.stamp {
                        Stamp::Pass => "PASS",
                        Stamp::Fail => "FAIL",
                        Stamp::NotReached => "SKIP",
                        _ => return Err("CASE bytes have a pending tile".into()),
                    };
                    check(
                        e.line.starts_with(&std::format!(
                            "SUITE {} CASE {} {status}",
                            s.suite,
                            e.id
                        )),
                        "tile id or verdict differs from its CASE bytes",
                    )?;
                    if e.stamp != Stamp::NotReached {
                        check(
                            e.line
                                .split_whitespace()
                                .any(|v| v == std::format!("ms={}", e.key)),
                            "tile key differs from its CASE milliseconds",
                        )?;
                    }
                    let slot = i + s.groups.iter().filter(|v| v.2 <= i).count();
                    let (left, top) = (16 + 40 * (slot % 20), 112 + 28 * (slot / 20));
                    let words: Vec<_> = e.line.split_whitespace().collect();
                    let colour = match words.get(4).copied() {
                        Some("PASS") => libgfx::color::Color::rgb(26, 112, 77),
                        Some("FAIL") => libgfx::color::Color::rgb(145, 50, 58),
                        Some("SKIP") => libgfx::color::Color::rgb(57, 67, 80),
                        _ => return Err("invalid stored CASE verdict".into()),
                    };
                    let mut expected = FrameBuf::new_filled(
                        38,
                        26,
                        Format::BGRX,
                        libgfx::color::Color::rgb(12, 19, 31),
                    )
                    .map_err(|e| e.to_string())?;
                    libgfx::shapes::draw_rect(&mut expected, 0, 0, 38, 26, colour);
                    libgfx::shapes::fill_rect(
                        &mut expected,
                        2,
                        2,
                        16,
                        12,
                        libgfx::color::Color::rgb(18, 29, 44),
                    );
                    if let Some(p) = &e.picture {
                        for y in 0..p.height {
                            for x in 0..p.width {
                                let o = (y * p.width + x) * 4;
                                expected.put_pixel(
                                    x + 2,
                                    y + 2,
                                    libgfx::color::Color::rgb(
                                        p.pixels[o + 2],
                                        p.pixels[o + 1],
                                        p.pixels[o],
                                    ),
                                );
                            }
                        }
                    }
                    expected.push_clip(libgfx::framebuf::Rect::new(20, 2, 18, 7));
                    libgfx::font::draw_text(
                        &mut expected,
                        std::format!("{}", i + 1).as_bytes(),
                        20,
                        2,
                        libgfx::color::Color::WHITE,
                        1,
                    );
                    expected.pop_clip();
                    libgfx::shapes::fill_rect(&mut expected, 1, 15, 36, 10, colour);
                    let number = words
                        .iter()
                        .find_map(|w| w.strip_prefix("ms="))
                        .unwrap_or("0");
                    let n = number
                        .parse::<u64>()
                        .map_err(|_| "invalid CASE milliseconds")?;
                    let key = if n > 99999 { ">" } else { number };
                    expected.push_clip(libgfx::framebuf::Rect::new(2, 17, 34, 7));
                    libgfx::font::draw_text(
                        &mut expected,
                        key.as_bytes(),
                        2,
                        17,
                        libgfx::color::Color::WHITE,
                        1,
                    );
                    expected.pop_clip();
                    for y in 0..26 {
                        for x in 0..38 {
                            let o = ((top + y) * 1280 + left + x) * 4;
                            let p = expected.get_pixel(x, y);
                            mismatch |= pixels[o..o + 4] != [p.b, p.g, p.r, 255];
                        }
                    }
                }
            }
            "boundary-notice" => {
                let notice = match (rec.backend, rec.flags, rec.boundary) {
                    (1, 1, 1) => "host copy and window outside guest view",
                    (2, 0, 2) => "host scanning outside guest view",
                    (3, 1, 1) => "picture not host-verified",
                    (4, 0, 3) => "host window outside guest view",
                    _ => return Err("invalid observation boundary combination".into()),
                };
                let mut expected = FrameBuf::new_filled(
                    1280,
                    800,
                    Format::BGRX,
                    libgfx::color::Color::rgb(12, 19, 31),
                )
                .map_err(|e| e.to_string())?;
                libgfx::font::draw_text(
                    &mut expected,
                    notice.as_bytes(),
                    24,
                    776,
                    libgfx::color::Color::rgb(161, 183, 204),
                    1,
                );
                for y in 776..783 {
                    for x in 24..1280 {
                        let o = (y * 1280 + x) * 4;
                        let p = expected.get_pixel(x, y);
                        mismatch |= pixels[o..o + 4] != [p.b, p.g, p.r, 255];
                    }
                }
            }
            _ => unreachable!(),
        }
        check(
            mismatch == *faulty,
            "stage scanout fault detection or clean agreement failed",
        )?;
        if let Some(p) = point {
            if *faulty {
                check(
                    crate::test_control::fired(p)? > 0,
                    "stage fault never fired",
                )?;
            }
        }
        crate::test_control::disarm_all()?;
    }
    Ok(())
}
