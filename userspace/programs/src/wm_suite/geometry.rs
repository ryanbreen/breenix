use super::kit::*;

pub fn drag_move() -> CaseResult {
    let (mut d, c) = one()?;
    let w = c.info()?;
    let p = (w.x + 50, w.y - 16 * d.scale);
    d.client_ready(&c)?;
    d.pointer(p.0, p.1, 0, 0)?;
    time::sleep_ms(150)?;
    d.pointer(p.0, p.1, 1, 0)?;
    time::sleep_ms(150)?;
    d.pointer(p.0 + 1, p.1 + 1, 1, 0)?;
    let started = until(1000, "drag has consumed its press and first motion", || {
        Ok((c.info()?.x, c.info()?.y) == (w.x + 1, w.y + 1))
    });
    if started.is_err() {
        let a = c.info()?;
        value("drag-dx", (a.x - w.x) as i64, "px", None);
        value("drag-dy", (a.y - w.y) as i64, "px", None);
    }
    started?;
    finish_paused_gesture(&d, (p.0 + 90, p.1 + 70))?;
    until(1000, "titlebar drag published geometry", || {
        Ok(c.info()?.x == w.x + 90 && c.info()?.y == w.y + 70)
    })?;
    content(&mut d, &c)
}
// Retain the final held position across a release and later park while the
// compositor is stopped; the first geometry change acknowledges the grab.
fn finish_paused_gesture(d: &Desktop, to: (i32, i32)) -> Result<(), String> {
    d.wm.freeze()?;
    d.pointer(to.0, to.1, 1, 0)?;
    d.pointer(to.0, to.1, 0, 0)?;
    d.pointer(d.width as i32 - 40, d.height as i32 - 100, 0, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())?;
    d.wm.signal(signal::SIGCONT)
}

pub fn resize_right() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.client_ready(&c)?;
    let w = c.info()?;
    let from = (w.x + w.width as i32 + 1, w.y + 80);
    d.pointer(from.0, from.1, 0, 0)?;
    time::sleep_ms(150)?;
    d.pointer(from.0, from.1, 1, 0)?;
    time::sleep_ms(150)?;
    d.pointer(from.0 + 1, from.1, 1, 0)?;
    until(1000, "resize has consumed its press and first motion", || {
        Ok(c.info()?.width == w.width + 1)
    })?;
    finish_paused_gesture(&d, (from.0 + 80, from.1))?;
    c.await_event("owner-mediated ResizeRequested", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    until(1000, "owner resized buffers published", || {
        c.snapshot()?;
        Ok((c.width, c.height) == (w.width + 80, w.height))
    })?;
    let after = c.info()?;
    check(
        after.x == w.x && after.y == w.y,
        "right/bottom resize moved content origin",
    )?;
    content(&mut d, &c)
}
pub fn resize_bottom() -> CaseResult {
    resize_case(true)
}
pub fn resize_refused() -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    c.clear()?;
    let target = (w.width + 800, w.height);
    // Window backing uses a 256-byte row pitch (also a valid multiple of
    // VMware's packed pitch here) and three page-rounded buffers.
    let bytes =
        ((target.0 as u64 * 4).next_multiple_of(256) * target.1 as u64).next_multiple_of(4096) * 3;
    {
        let _limit = AddressLimit::new(c.child.pid, bytes)?;
        c.command(RESIZE, target.0, target.1)?;
        check(
            (c.info()?.width, c.info()?.height) == target,
            "same owner resize did not succeed one page above the refusal limit",
        )?;
    }
    c.command(RESIZE, w.width, w.height)?;
    content(&mut d, &c)?;
    c.clear()?;
    {
        let _limit = AddressLimit::new(c.child.pid, bytes - 4096)?;
        drag(
            &d,
            (w.x + w.width as i32 + 1, w.y + 70),
            (w.x + target.0 as i32 + 1, w.y + 70),
        )?;
        let request = c.await_event("constrained owner receives resize request", |e| {
            e.kind == kind::RESIZE_REQUESTED
        })?;
        check(
            (request.width, request.height) == target,
            "constrained resize differs from the successful precondition",
        )?;
        c.await_event("RLIMIT_AS refusal reports ResizeFailed", |e| {
            e.kind == kind::RESIZE_FAILED && e.width == w.width && e.height == w.height
        })?;
        check(
            (c.info()?.x, c.info()?.y, c.info()?.width, c.info()?.height)
                == (w.x, w.y, w.width, w.height),
            "failed allocation changed retained frame geometry",
        )?;
        content(&mut d, &c)?;
    }
    // An owner ignoring ResizeRequested is a separate retained-frame path.
    c.command(REFUSE, 1, 0)?;
    c.clear()?;
    drag(
        &d,
        (w.x + w.width as i32 + 1, w.y + 70),
        (w.x + w.width as i32 + 61, w.y + 70),
    )?;
    c.await_event("ignoring owner receives ResizeRequested", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    c.command(REFUSE, 0, 0)?;
    check(
        (c.info()?.width, c.info()?.height) == (w.width, w.height),
        "ignored ResizeRequested changed owner geometry",
    )?;
    content(&mut d, &c)?;
    c.command(RESIZE, target.0, target.1)?;
    content(&mut d, &c)
}

pub fn resize_cross() -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    drag(
        &d,
        (w.x + w.width as i32 + 1, w.y + 80),
        (w.x - 20, w.y + 80),
    )?;
    let requested = c.await_event("crossing resize request", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    check(
        requested.width == ((120 - 4) * d.scale) as u32,
        "crossing resize requested a huge or non-minimum width",
    )?;
    c.snapshot()?;
    check(
        c.width == ((120 - 4) * d.scale) as u32,
        "crossing right edge did not clamp to minimum content width",
    )?;
    content(&mut d, &c)
}
pub fn client_resize() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.command(RESIZE, 360, 220)?;
    content(&mut d, &c)?;
    let w = c.info()?;
    check(
        (w.width, w.height) == (360, 220),
        "client-initiated resize was not reconciled",
    )
}

pub fn drag_top_clamp() -> CaseResult {
    let (mut d, c) = one()?;
    let w = c.info()?;
    drag(&d, (w.x + 50, w.y - 16 * d.scale), (w.x + 50, 0))?;
    let after = c.info()?;
    check(
        after.y == (28 + 34) * d.scale,
        "drag did not stop at taskbar's lower edge",
    )?;
    content(&mut d, &c)
}
pub fn drag_keeps_titlebar() -> CaseResult {
    let (mut d, c) = one()?;
    let w = c.info()?;
    drag(&d, (w.x + 50, w.y - 16 * d.scale), (0, w.y + 50))?;
    let a = c.info()?;
    check(
        a.x + a.width as i32 + 2 * d.scale >= 32 * d.scale,
        "left drag lost minimum titlebar strip",
    )?;
    drag(
        &d,
        (a.x + 50, a.y - 16 * d.scale),
        (d.width as i32 - 1, a.y + 50),
    )?;
    let b = c.info()?;
    check(
        b.x - 2 * d.scale <= d.width as i32 - 32 * d.scale,
        "right drag lost minimum titlebar strip",
    )?;
    content(&mut d, &c)
}
fn anchored_resize(left: bool, top: bool) -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    let x = if left {
        w.x - 2 * d.scale
    } else {
        w.x + w.width as i32 + 1
    };
    let y = if top {
        w.y - 34 * d.scale
    } else {
        w.y + w.height as i32 + 1
    };
    let from = if left && !top {
        (x, w.y + 70)
    } else if top && !left {
        (w.x + 70, y)
    } else {
        (x, y)
    };
    drag(
        &d,
        from,
        (
            from.0 - if left { 30 } else { 0 },
            from.1 - if top { 20 } else { 0 },
        ),
    )?;
    c.await_event("anchored owner resize", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    c.snapshot()?;
    let a = c.info()?;
    check(
        (!left || a.x + a.width as i32 == w.x + w.width as i32)
            && (!top || a.y + a.height as i32 == w.y + w.height as i32),
        "anchored resize moved its fixed opposite edge",
    )?;
    check(
        (!left || a.width == w.width + 30) && (!top || a.height == w.height + 20),
        "anchored resize returned wrong extent",
    )?;
    content(&mut d, &c)
}
pub fn resize_left() -> CaseResult {
    anchored_resize(true, false)
}
pub fn resize_top() -> CaseResult {
    anchored_resize(false, true)
}
pub fn resize_corners() -> CaseResult {
    let mut d = Desktop::start()?;
    let mut c = Client::start(11)?;
    placed(&c)?;
    d.client_ready(&c)?;
    for (left, top) in [(true, true), (false, true), (true, false), (false, false)] {
        c.clear()?;
        let w = c.info()?;
        let x = if left {
            w.x - 2 * d.scale
        } else {
            w.x + w.width as i32 + 1
        };
        let y = if top {
            w.y - 34 * d.scale
        } else {
            w.y + w.height as i32 + 1
        };
        drag(
            &d,
            (x, y),
            (
                x + if left { -30 } else { 30 },
                y + if top { -20 } else { 20 },
            ),
        )?;
        c.await_event("corner ResizeRequested", |e| {
            e.kind == kind::RESIZE_REQUESTED
        })?;
        c.snapshot()?;
        let a = c.info()?;
        check(
            a.width == w.width + 30 && a.height == w.height + 20,
            "corner failed to resize both axes",
        )?;
        check(
            (if left { a.x + a.width as i32 } else { a.x })
                == (if left { w.x + w.width as i32 } else { w.x })
                && (if top { a.y + a.height as i32 } else { a.y })
                    == (if top { w.y + w.height as i32 } else { w.y }),
            "corner moved opposite anchor",
        )?;
        content(&mut d, &c)?;
    }
    Ok(())
}
pub fn resize_minimum() -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    drag(
        &d,
        (w.x + w.width as i32 + 1, w.y + w.height as i32 + 1),
        (w.x + 1, w.y + 1),
    )?;
    c.await_event("minimum resize request", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    c.snapshot()?;
    check(
        (c.width, c.height) == ((116 * d.scale) as u32, (80 * d.scale) as u32),
        "resize did not clamp to literal content minimum",
    )?;
    content(&mut d, &c)
}
pub fn resize_maximum() -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    drag(
        &d,
        (w.x + w.width as i32 + 1, w.y + w.height as i32 + 1),
        (d.width as i32 - 1, d.height as i32 - 1),
    )?;
    c.await_event("maximum resize request", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    c.snapshot()?;
    let a = c.info()?;
    check(
        a.width <= 4096
            && a.height <= 4096
            && a.x + a.width as i32 + 2 * d.scale <= d.width as i32
            && a.y + a.height as i32 + 2 * d.scale <= d.height as i32 - 36 * d.scale,
        "resize escaped screen or buffer maximum",
    )?;
    content(&mut d, &c)
}
pub fn configure_ack() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.command(REFUSE, 1, 0)?;
    let old = c.info()?;
    drag(
        &d,
        (old.x + old.width as i32 + 1, old.y + 70),
        (old.x + old.width as i32 + 61, old.y + 70),
    )?;
    let request = c.await_event("pending ResizeRequested", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    let pending = c.info()?;
    check(
        (pending.x, pending.y, pending.width, pending.height)
            == (old.x, old.y, old.width, old.height),
        "frame changed before LIST reported owner size",
    )?;
    content(&mut d, &c)?;
    c.command(RESIZE, request.width, request.height)?;
    content(&mut d, &c)?;
    let answered = c.info()?;
    check(
        answered.width == request.width && answered.height == request.height,
        "frame ignored owner's LIST answer",
    )
}
pub fn buffer_matches() -> CaseResult {
    let (mut d, mut c) = one()?;
    for (width, height) in [(360, 220), (180, 96), (300, 200)] {
        c.command(RESIZE, width, height)?;
        content(&mut d, &c)?;
        let w = c.info()?;
        check(
            (w.width, w.height) == (c.width, c.height),
            "registry content and owner buffer disagree after resize",
        )?;
    }
    Ok(())
}
pub fn coalesce() -> CaseResult {
    let (d, mut c) = one()?;
    c.command(REFUSE, 1, 0)?;
    c.clear()?;
    let w = c.info()?;
    let x = w.x + w.width as i32 + 1;
    let y = w.y + 70;
    d.pointer(x, y, 0, 0)?;
    time::sleep_ms(100).map_err(|e| e.to_string())?;
    d.pointer(x, y, 1, 0)?;
    time::sleep_ms(100).map_err(|e| e.to_string())?;
    for n in 1..=60 {
        d.pointer(x + n, y, 1, 0)?;
        time::sleep_ms(15).map_err(|e| e.to_string())?;
    }
    d.pointer(x + 60, y, 0, 0)?;
    // Wait for the final delivered motion after the 60-step gesture.
    d.pointer(w.x + 10, w.y + 10, 0, 0)?;
    c.await_event("resize gesture's final input delivered", |e| {
        e.kind == kind::POINTER_MOVE && e.x == 10 && e.y == 10
    })?;
    c.snapshot()?;
    check(
        c.events
            .iter()
            .filter(|e| e.kind == kind::RESIZE_REQUESTED)
            .count()
            == 1,
        "bwm emitted multiple outstanding ResizeRequested records before LIST changed",
    )
}
fn exit_gesture(resize: bool) -> CaseResult {
    let (mut d, a, mut b) = twin()?;
    let w = b.info()?;
    let p = if resize {
        (w.x + w.width as i32 + 1, w.y + 70)
    } else {
        (w.x + 50, w.y - 16 * d.scale)
    };
    d.pointer(p.0, p.1, 0, 0)?;
    time::sleep_ms(100).map_err(|e| e.to_string())?;
    d.pointer(p.0, p.1, 1, 0)?;
    time::sleep_ms(100).map_err(|e| e.to_string())?;
    let original = a.info()?;
    b.send(EXIT, 0, 0)?;
    until(1000, "gesture owner removed", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    d.pointer(p.0 + 80, p.1 + 70, 1, 0)?;
    d.pointer(p.0 + 80, p.1 + 70, 0, 0)?;
    let after = a.info()?;
    check(
        (after.x, after.y, after.width, after.height)
            == (original.x, original.y, original.width, original.height),
        "dead gesture mutated surviving window",
    )?;
    content(&mut d, &a)
}
pub fn exit_during_drag() -> CaseResult {
    exit_gesture(false)
}
pub fn exit_during_resize() -> CaseResult {
    exit_gesture(true)
}
pub fn client_title() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.title(b"renamed window 0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLM")?;
    let w = c.info()?;
    check(
        &w.title[..w.title_len as usize]
            == b"renamed window 0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLM",
        "client title truncated in registry",
    )?;
    title_pixels(&mut d, &c)
}
pub fn remembered_geometry() -> CaseResult {
    let (mut d, mut c) = one()?;
    let old = c.info()?;
    drag(
        &d,
        (old.x + 50, old.y - 16 * d.scale),
        (old.x + 130, old.y + 40),
    )?;
    let placed = c.info()?;
    c.send(EXIT, 0, 0)?;
    until(1000, "first titled window removed", || {
        Ok(!listed()?.iter().any(|w| w.id == c.id))
    })?;
    let replacement = Client::start(11)?;
    until(1000, "remembered geometry restored", || {
        let a = replacement.info()?;
        Ok((a.x, a.y, a.width, a.height) == (placed.x, placed.y, placed.width, placed.height))
    })?;
    content(&mut d, &replacement)
}
pub fn expose_during_drag() -> CaseResult {
    let (mut d, a, b) = twin()?;
    let w = b.info()?;
    for n in 1..=4 {
        let current = b.info()?;
        drag(
            &d,
            (current.x + 50, current.y - 16 * d.scale),
            (current.x + 130, current.y + 50),
        )?;
        check(
            b.info()?.x == w.x + 80 * n,
            "drag sequence departed from requested delta",
        )?;
        parked(&d)?;
        let s = d.shown(&b)?;
        s.client(&b)?;
        let aw = a.info()?;
        check(
            s.pixel(aw.x + 10, aw.y + 10)? == pattern(a.seed, 0),
            "drag left stale top-window pixels in exposed lower content",
        )?;
    }
    check(b.info()?.x > w.x, "drag sequence did not move window")
}
pub fn resize_repaints() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.command(RESIZE, 400, 280)?;
    content(&mut d, &c)?;
    let old = c.info()?;
    c.command(RESIZE, 180, 96)?;
    content(&mut d, &c)?;
    let s = d.shown(&c)?;
    let x = old.x + 390;
    let y = old.y + 270;
    check(
        s.pixel(x, y)? == background(x, y, d.height as i32, d.scale),
        "shrinking window left old client pixels on desktop",
    )
}
pub fn geometry_registry() -> CaseResult {
    let (mut d, mut c) = one()?;
    let mut expected = c.info()?;
    for n in 0..8 {
        let p = (expected.x + 50, expected.y - 16 * d.scale);
        drag(&d, p, (p.0 + 10, p.1 + 8))?;
        expected.x += 10;
        expected.y += 8;
        let actual = c.info()?;
        check(
            (actual.x, actual.y) == (expected.x, expected.y),
            "registry move differs from literal delta model",
        )?;
        let dims = (240 + n * 3, 160 + n * 2);
        c.command(RESIZE, dims.0, dims.1)?;
        let actual = c.info()?;
        check(
            (actual.width, actual.height) == dims,
            "registry resize differs from owner's requested dimensions",
        )?;
        expected = actual;
        content(&mut d, &c)?;
    }
    Ok(())
}
pub fn chromeless_not_resizable() -> CaseResult {
    let mut d = Desktop::start()?;
    let mut c = Client::start_mode(77, flags::CHROMELESS | flags::IGNORE_CLOSE)?;
    d.client_ready(&c)?;
    let old = c.info()?;
    c.clear()?;
    drag(
        &d,
        (old.x + old.width as i32 - 1, old.y + old.height as i32 / 2),
        (old.x + old.width as i32 + 60, old.y + old.height as i32 / 2),
    )?;
    c.snapshot()?;
    let a = c.info()?;
    check(
        !c.events.iter().any(|e| e.kind == kind::RESIZE_REQUESTED)
            && (a.width, a.height) == (old.width, old.height),
        "chromeless edge started resize",
    )?;
    content(&mut d, &c)
}
