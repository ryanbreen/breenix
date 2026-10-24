use super::kit::*;

pub fn chrome() -> CaseResult {
    let (mut d, c) = one()?;
    parked(&d)?;
    let s = d.shown(&c)?;
    let w = c.info()?;
    let k = d.scale;
    for (x, y, colour) in [
        (w.x - 2 * k, w.y, 0xff3c82ff),
        (w.x + 8 * k, w.y - 30 * k, 0xff2864dc),
        (w.x + w.width as i32 - 21 * k, w.y - 27 * k, 0xffc83232),
        (d.width as i32 / 2, 1, 0xff14161e),
        (1, d.height as i32 - 2, 0xff191c26),
    ] {
        check(
            s.pixel(x, y)? == colour,
            &format!(
                "chrome at {x},{y}: {:08x}, expected {colour:08x}",
                s.pixel(x, y)?
            ),
        )?;
    }
    Ok(())
}
pub fn overlap() -> CaseResult {
    let (mut d, a, b) = twin()?;
    parked(&d)?;
    let s = d.shown(&b)?;
    s.client(&b)?;
    let (a, b) = (a.info()?, b.info()?);
    check(
        a.x < b.x && b.x < a.x + a.width as i32 && a.y < b.y && b.y < a.y + a.height as i32,
        "fixture clients do not overlap",
    )?;
    check(a.z < b.z, "new client not above previous client")
}
pub fn client_update() -> CaseResult {
    let (mut d, mut c) = one()?;
    content(&mut d, &c)?;
    let before = c.commit;
    c.command(PAINT, 47, 0)?;
    check(c.commit > before, "paint did not commit a new frame")?;
    content(&mut d, &c)?;
    let shown = d.shown(&c)?;
    let generation = c.info()?.pixel_gen;
    c.command(STEP, 1, 0)?;
    check(
        c.info()?.pixel_gen == generation,
        "metadata-only commit changed pixels",
    )?;
    // B41: the previously completed frame already presents this generation.
    shown.client(&c)
}
pub fn expose_close() -> CaseResult {
    let (mut d, a, mut b) = twin()?;
    content(&mut d, &b)?;
    b.send(EXIT, 0, 0)?;
    until(1000, "exited client removed", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    content(&mut d, &a)
}
pub fn cascade() -> CaseResult {
    let (d, a, b) = twin()?;
    let (a, b) = (a.info()?, b.info()?);
    check(
        (b.x - a.x, b.y - a.y) == (50 * d.scale, 50 * d.scale),
        "new windows do not cascade by 50 logical pixels on each axis",
    )
}
pub fn frame_integrity() -> CaseResult {
    let (mut d, c) = one()?;
    parked(&d)?;
    let s = d.shown(&c)?;
    s.client(&c)?;
    // The check itself must find a wrong colour and a missing provenance entry.
    let mut wrong = Screen {
        pixels: s.pixels.clone(),
        record: s.record,
        width: s.width,
        height: s.height,
    };
    let w = c.info()?;
    let p = ((w.y as u32 * s.width + w.x as u32) * 4) as usize;
    wrong.pixels[p] ^= 1;
    check(
        wrong.client(&c).is_err(),
        "client-pattern oracle accepted a corrupted content pixel",
    )?;
    wrong.pixels.clone_from(&s.pixels);
    wrong.record[196..200].copy_from_slice(&0u32.to_le_bytes());
    check(
        wrong.client(&c).is_err(),
        "client-pattern oracle accepted missing commit provenance",
    )
}

pub fn client_position() -> CaseResult {
    let (mut d, c) = one()?;
    let w = c.info()?;
    check(
        (w.x, w.y) == (32 * d.scale, 72 * d.scale),
        "first content origin differs from the contract cascade and chrome",
    )?;
    content(&mut d, &c)
}
pub fn title_text() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.title(b"Window title 0123456789")?;
    time::sleep_ms(100)?;
    title_pixels(&mut d, &c)
}
pub fn shadow() -> CaseResult {
    let (mut d, c) = one()?;
    parked(&d)?;
    let s = d.shown(&c)?;
    let w = c.info()?;
    let k = d.scale;
    for y in w.y - 34 * k + 3 * k..w.y + w.height as i32 + 5 * k {
        for x in w.x + w.width as i32 + 2 * k..w.x + w.width as i32 + 5 * k {
            check(
                s.pixel(x, y)? == 0xff080a12,
                "drop shadow rectangle differs from contract",
            )?;
        }
    }
    Ok(())
}
pub fn background_pixels() -> CaseResult {
    let (mut d, c) = one()?;
    parked(&d)?;
    let s = d.shown(&c)?;
    super::scene::background_region(
        &s,
        d.scale,
        (
            d.width as i32 / 2,
            28 * d.scale,
            d.width as i32 / 3,
            d.height as i32 - 64 * d.scale,
        ),
    )
}
pub fn appbar_entries() -> CaseResult {
    let (mut d, a, b) = twin()?;
    parked(&d)?;
    let s = d.shown(&b)?;
    let mut x = 4 * d.scale;
    for (c, bg) in [(&a, 0xff282d3c), (&b, 0xff2864dc)] {
        let w = c.info()?;
        let width =
            (w.title_len as i32 * 9 * d.scale + 16 * d.scale).clamp(60 * d.scale, 180 * d.scale);
        check(
            s.pixel(x, d.height as i32 - 32 * d.scale)? == bg,
            "appbar entry missing or out of creation order",
        )?;
        x += width + 2 * d.scale;
    }
    check(
        s.pixel(x, d.height as i32 - 32 * d.scale)? == 0xff191c26,
        "unexpected extra appbar entry",
    )
}
pub fn chromeless() -> CaseResult {
    let mut d = Desktop::start()?;
    let c = Client::start_mode(23, flags::CHROMELESS | flags::IGNORE_CLOSE)?;
    d.client_ready(&c)?;
    parked(&d)?;
    let s = d.shown(&c)?;
    let w = c.info()?;
    check(
        (w.x, w.y) == ((d.width as i32 - w.width as i32) / 2, d.height as i32 / 5),
        "chromeless placement differs from contract",
    )?;
    s.client(&c)?;
    check(
        s.pixel(1, 1)? == 0xff0a0b0f && s.pixel(1, d.height as i32 - 2)? == 0xff0c0e13,
        "chromeless dimmer does not cover both bars",
    )?;
    let original = background(w.x - 1, w.y, d.height as i32, d.scale);
    let dimmed = 0xff000000 | ((original & 0x00fefefe) >> 1);
    check(
        s.pixel(w.x - 1, w.y)? == dimmed,
        "chromeless window has a frame or lacks the dimmer",
    )
}
pub fn sixteen_clients() -> CaseResult {
    let mut clients = Vec::new();
    // The 92-pixel content rectangles have 100-pixel cascade steps.
    // One device observation checks the sixteen complete patterns.
    for n in 0..16 {
        clients.push(Client::spawn_shape(
            11 + n,
            flags::UNREGISTERED | flags::IGNORE_CLOSE | flags::NEVER_READ,
            92,
            64,
            // Event reads are disabled for these 16 clients. Keep strict
            // overflow checks without mapping 8192 unused slots per child.
            64,
        )?);
    }
    for (index, c) in clients.iter_mut().enumerate() {
        c.initialize()?;
        c.receive()
            .map_err(|e| format!("initial paint of client {index}: {e}"))?;
    }
    let mut d = Desktop::start()?;
    for c in clients.iter_mut() {
        c.send(REGISTER, 0, 0)?;
    }
    for c in clients.iter_mut() {
        c.receive()?;
    }
    if let Some(status) = d.wm.status()? {
        return Err(format!("desktop exited during the sixteen-client fixture: {status}").into());
    }
    let mut final_screen = None;
    let mut last = String::from("no observation attempted");
    until(
        2000,
        "sixteen registered client commits composed",
        || match Screen::read() {
            Ok(s)
                if clients.iter().all(|c| {
                    let Ok(w) = c.info() else {
                        return false;
                    };
                    s.contains(c.id, c.frame_commit)
                        && w.x > 0
                        && w.y > 0
                        && s.pixel(w.x, w.y).ok() == Some(pattern(c.seed, 0))
                }) =>
            {
                final_screen = Some(s);
                Ok(true)
            }
            Ok(s) => {
                last = format!(
                    "composite {} contains {} entries",
                    s.serial(),
                    word(&s.record, 196)
                );
                Ok(false)
            }
            Err(e) => {
                last = e;
                Ok(false)
            }
        },
    )
    .map_err(|e| format!("{e}; last {last}"))?;
    let screen = final_screen.unwrap();
    for c in &clients {
        screen.client(c)?;
    }
    Ok(())
}
pub fn moving_client() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    let mut last = 0;
    for epoch in 1..=20 {
        clock.step(epoch, epoch * 16_000_000)?;
        c.command(STEP, epoch as u32, 1)?;
        measure::acknowledged(epoch)?;
        let r = measure::completed_commit(&c, c.frame_commit)?;
        check(
            r.commit > last,
            "animation frame was skipped or composed out of order",
        )?;
        last = r.commit;
        d.shown(&c)?.client(&c)?;
    }
    Ok(())
}
pub fn frozen_client() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, c) = one()?;
    c.child.freeze()?;
    let before = c.info()?.commit_id;
    for epoch in 1..=3 {
        clock.step(epoch, epoch * 1_000_000_000)?;
        measure::acknowledged(epoch)?;
    }
    check(
        c.info()?.commit_id == before,
        "hung client changed its last commit",
    )?;
    d.shown(&c)?.client(&c)
}
pub fn present_failure() -> CaseResult {
    use libbreenix::test_control as tc;
    let _clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    parked(&d)?;
    measure::acknowledged(0)?;
    let before = status()?.composites;
    let fired = tc::fired(abi::tc_point::BACKEND_ERROR)?;
    tc::arm(abi::tc_point::BACKEND_ERROR, 0, 0)?;
    c.command(PAINT, 47, 0)?;
    until(1000, "one composite backend fault", || {
        Ok(tc::fired(abi::tc_point::BACKEND_ERROR).map_err(|e| e.to_string())? == fired + 1)
    })?;
    tc::disarm_all()?;
    d.shown(&c)?.client(&c)?;
    check(
        d.output().contains("[bwm] GPU frame failed:"),
        "bwm did not report the forced present failure",
    )?;
    check(
        status()?.composites == before + 1,
        "failed present counted as a successful frame or retry was omitted",
    )
}
pub fn visible_only() -> CaseResult {
    let (mut d, a, b) = twin()?;
    minimize(&d, &b)?;
    let commit = a.barrier()?;
    let s = d.shown_after(&a, commit)?;
    check(
        !s.contains(b.id, 0),
        "minimized client remains in composite record",
    )?;
    s.client(&a)
}
pub fn expose_move() -> CaseResult {
    let (mut d, a, b) = twin()?;
    let w = b.info()?;
    drag(&d, (w.x + 40, w.y - 16 * d.scale), (w.x + 600, w.y + 400))?;
    d.shown(&a)?.client(&a)
}
pub fn partial_equals_full() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    measure::acknowledged(0)?;
    parked(&d)?;
    c.command(PAINT, 47, 0)?;
    let partial = d.shown(&c)?;
    d.wm.stop()?;
    let mut full = Desktop::start()?;
    parked(&full)?;
    clock.step(1, 0)?;
    c.command(STEP, 1, 0)?;
    measure::acknowledged(1)?;
    let repaired = full.shown(&c)?;
    check(
        partial.pixels == repaired.pixels,
        "partial redraw differs from the same scene fully recomposed after restart",
    )
}

pub fn outer_bounds() -> CaseResult {
    let (mut d, c) = one()?;
    parked(&d)?;
    let s = d.shown(&c)?;
    let w = c.info()?;
    let k = d.scale;
    for (x, y) in [
        (w.x - 2 * k - 1, w.y),
        (w.x + w.width as i32 + 5 * k, w.y),
        (w.x, w.y - 34 * k - 1),
        (w.x, w.y + w.height as i32 + 5 * k),
    ] {
        check(
            s.pixel(x, y)? == background(x, y, d.height as i32, k),
            "paint outside the contract outer bounds",
        )?;
    }
    s.client(&c)
}
pub fn resized_content() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.command(RESIZE, 400, 260)?;
    until(1000, "resized content reconciled", || {
        Ok(c.info()?.width == 400 && c.info()?.height == 260)
    })?;
    content(&mut d, &c)
}
pub fn cursor_over_window() -> CaseResult {
    let (mut d, c) = one()?;
    let w = c.info()?;
    parked(&d)?;
    let clean = d.shown(&c)?;
    d.pointer(w.x + 40, w.y + 40, 0, 0)?;
    time::sleep_ms(100)?;
    let under = d.shown(&c)?;
    check(under.pixels != clean.pixels, "cursor absent over client")?;
    parked(&d)?;
    d.shown(&c)?.client(&c)
}
fn sprite(shown: &Screen, clean: &Screen, x: i32, y: i32) -> Result<Vec<(i32, i32, u32)>, String> {
    let mut result = Vec::new();
    for dy in -32..64 {
        for dx in -32..64 {
            let (sx, sy) = (x + dx, y + dy);
            if sx < 0 || sy < 0 || sx >= shown.width as i32 || sy >= shown.height as i32 {
                continue;
            }
            let colour = shown.pixel(sx, sy)?;
            if colour != clean.pixel(sx, sy)? {
                result.push((dx, dy, colour));
            }
        }
    }
    if result.is_empty() {
        return Err("cursor sprite absent from device pixels".into());
    }
    Ok(result)
}
pub fn cursor_edge_shapes() -> CaseResult {
    let (mut d, c) = one()?;
    let w = c.info()?;
    parked(&d)?;
    let clean = d.shown(&c)?;
    let mut shapes = Vec::new();
    for (x, y) in [
        (w.x - 2 * d.scale, w.y + 80),
        (w.x + 80, w.y - 34 * d.scale),
        (w.x - 2 * d.scale, w.y - 34 * d.scale),
        (w.x + w.width as i32 + 1, w.y - 34 * d.scale),
    ] {
        d.pointer(x, y, 0, 0)?;
        time::sleep_ms(100)?;
        shapes.push(sprite(&d.shown(&c)?, &clean, x, y)?);
    }
    for i in 0..shapes.len() {
        for j in 0..i {
            check(
                shapes[i] != shapes[j],
                "different resize edges produced identical cursor sprites",
            )?;
        }
    }
    let extent = |s: &Vec<(i32, i32, u32)>| {
        (
            s.iter().map(|p| p.0).max().unwrap() - s.iter().map(|p| p.0).min().unwrap(),
            s.iter().map(|p| p.1).max().unwrap() - s.iter().map(|p| p.1).min().unwrap(),
        )
    };
    let ew = extent(&shapes[0]);
    let ns = extent(&shapes[1]);
    check(
        ew.0 > ew.1 && ns.1 > ns.0,
        "resize cursor orientation differs from its edge",
    )
}
pub fn cursor_stationary_shape() -> CaseResult {
    let _clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    measure::acknowledged(0)?;
    let w = c.info()?;
    parked(&d)?;
    let clean = d.shown(&c)?;
    let p = (w.x + 40, w.y + 40);
    d.pointer(p.0, p.1, 0, 0)?;
    time::sleep_ms(100)?;
    let arrow = sprite(&d.shown(&c)?, &clean, p.0, p.1)?;
    let (x, y) = (w.x + w.width as i32 + 1, w.y + 60);
    d.pointer(x, y, 0, 0)?;
    time::sleep_ms(100)?;
    let edge = sprite(&d.shown(&c)?, &clean, x, y)?;
    check(edge != arrow, "resize edge did not change the arrow sprite")?;
    c.command(RESIZE, w.width + 100, w.height)?;
    until(1000, "stationary pointer becomes content", || {
        Ok(c.info()?.width == w.width + 100)
    })?;
    let stationary = d.shown(&c)?;
    parked(&d)?;
    let repaired = d.shown(&c)?;
    check(
        sprite(&stationary, &repaired, x, y)? == arrow,
        "idle stationary pointer did not change back to the arrow",
    )
}
pub fn clock_test_config() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let _config = FileOverride::new(
        "/etc/bwm.conf",
        b"font=/usr/share/fonts/DejaVuSansMono.ttf\nclock=frozen\nfps=off\n",
    )?;
    let (mut d, mut c) = one()?;
    clock.step(1, 1_000_000_000)?;
    c.command(STEP, 1, 0)?;
    measure::acknowledged(1)?;
    let s = d.shown(&c)?;
    super::scene::clock(&s, &d, "00:00:01 UTC")
}
pub fn no_fps_overlay() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let _config = FileOverride::new("/etc/bwm.conf", b"clock=frozen\nfps=off\n")?;
    let (mut d, mut c) = one()?;
    let before = d.shown(&c)?;
    clock.step(1, 5_000_000_000)?;
    c.command(STEP, 1, 0)?;
    measure::acknowledged(1)?;
    let after = d.shown(&c)?;
    check(
        !d.output().contains("[bwm-fps]"),
        "test config emitted frame-rate output",
    )?;
    let end = (d.width as usize - 230) * 4;
    for y in 0..28 * d.scale as usize {
        let p = y * d.width as usize * 4;
        check(
            before.pixels[p..p + end] == after.pixels[p..p + end],
            "frame-rate text changed the frozen top bar",
        )?;
    }
    Ok(())
}

/// A fixed three-window W10 scene: one normal client, one small chromeless
/// client and one mode-sized chromeless client above both. The full-mode
/// pattern covers the desktop; the arrow at the last pixel is clipped to its tip.
pub fn scene_identical() -> CaseResult {
    let mut d = Desktop::start()?;
    check(
        (d.width, d.height, d.scale) == (2560, 1600, 2),
        "pinned scene requires the contract scale-2 mode",
    )?;
    let a = Client::start(11)?;
    d.client_ready(&a)?;
    let mut b = Client::start_mode(
        23,
        flags::CHROMELESS | flags::IGNORE_CLOSE | flags::NEVER_READ,
    )?;
    d.client_ready(&b)?;
    b.create_window(41, d.width, d.height, 3)?;
    d.client_ready(&b)?;
    let before = d.shown(&b)?.serial();
    d.pointer(d.width as i32 - 1, d.height as i32 - 1, 0, 0)?;
    let mut completed = None;
    until(2000, "fixed scene cursor commit completed", || {
        let s = Screen::read()?;
        if s.serial() > before
            && s.contains(b.id, b.frame_commit)
            && s.pixel(d.width as i32 - 1, d.height as i32 - 1)? == 0xff000000
        {
            completed = Some(s);
            Ok(true)
        } else {
            Ok(false)
        }
    })?;
    let s = completed.unwrap();
    check(
        listed()?.len() == 3 && b.info()?.x == 0 && b.info()?.y == 0,
        "fixed three-window scene has incorrect registry or placement",
    )?;
    for y in 0..d.height {
        for x in 0..d.width {
            let q = u32::from(x >= d.width / 2) + 2 * u32::from(y >= d.height / 2);
            let expected = if x == d.width - 1 && y == d.height - 1 {
                0xff000000
            } else {
                pattern(41, q)
            };
            check(
                s.pixel(x as i32, y as i32)? == expected,
                "fixed scene differs from its independent full-mode formula",
            )?;
        }
    }
    check(
        super::scene::hash(&s) == 0xaa15277dfb38ce7d,
        "fixed three-window scene differs from its pinned FNV-1a hash",
    )
}

pub fn damage_bounded() -> CaseResult {
    let _clock = measure::Clock::new()?;
    let (mut d, c) = one()?;
    measure::acknowledged(0)?;
    let w = c.info()?;
    let p = (w.x + 80, w.y - 16 * d.scale);
    d.pointer(p.0, p.1, 0, 0)?;
    time::sleep_ms(100)?;
    d.pointer(p.0, p.1, 1, 0)?;
    time::sleep_ms(100)?;
    let result = (|| -> CaseResult {
        d.shown(&c)?;
        let before = counter("composite_pixels")?;
        d.pointer(p.0 + 20, p.1, 1, 0)?;
        until(1000, "single-step titlebar move", || {
            Ok(c.info()?.x == w.x + 20)
        })?;
        let commit = c.barrier()?;
        d.shown_after(&c, commit)?;
        let after = counter("composite_pixels")?;
        let width = w.width as u64 + 7 * d.scale as u64;
        let height = w.height as u64 + 39 * d.scale as u64;
        // Both cursor footprints are inside the titlebar and therefore inside
        // this union. The scene-to-scanout copy is transport, not scene damage.
        check(
            after - before == (width + 20) * height,
            "move composed more or less than the exact old/new painted-bounds union",
        )
    })();
    let released = d.pointer(p.0 + 20, p.1, 0, 0);
    result?;
    released?;
    Ok(())
}
