use super::kit::*;

pub fn local_coordinates() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let w = c.info()?;
    d.pointer(w.x + 73, w.y + 81, 0, 0)?;
    c.await_event("exact local pointer (73,81)", |e| {
        e.kind == kind::POINTER_MOVE && (e.x, e.y) == (73, 81)
    })?;
    Ok(())
}
pub fn capture() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let w = c.info()?;
    d.pointer(w.x + 40, w.y + 40, 1, 0)?;
    c.await_event("capture press", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    d.pointer(w.x + w.width as i32 + 100, w.y + 40, 1, 0)?;
    c.await_event("captured motion outside content", |e| {
        e.kind == kind::POINTER_MOVE && e.x > w.width as i32
    })?;
    d.pointer(w.x + w.width as i32 + 100, w.y + 40, 0, 0)?;
    c.await_event("captured release outside content", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0 && e.x > w.width as i32
    })?;
    Ok(())
}
pub fn right_middle() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let w = c.info()?;
    for (mask, button) in [(2, 0x111), (4, 0x112)] {
        d.pointer(w.x + 60, w.y + 60, mask, 0)?;
        c.await_event("right/middle press", |e| {
            e.kind == kind::POINTER_BUTTON && e.button == button && e.pressed == 1
        })?;
        d.pointer(w.x + 60, w.y + 60, 0, 0)?;
        c.await_event("right/middle release", |e| {
            e.kind == kind::POINTER_BUTTON && e.button == button && e.pressed == 0
        })?;
    }
    Ok(())
}
pub fn scroll() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    b.clear()?;
    let w = a.info()?;
    d.pointer(w.x + 20, w.y + 20, 0, 1)?;
    a.await_event("scroll goes to pointer target on idle desktop", |e| {
        e.kind == kind::SCROLL && e.scroll_y == 1
    })?;
    b.snapshot()?;
    check(
        b.events.iter().all(|e| e.kind != kind::SCROLL),
        "scroll incorrectly went to the focused client",
    )
}
pub fn outside_none() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    parked(&d)?;
    time::sleep_ms(100)?;
    c.snapshot()?;
    check(
        c.events.iter().all(|e| e.kind != kind::POINTER_MOVE),
        "motion outside content reached a client without a capture",
    )
}
pub fn press_order() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let w = c.info()?;
    d.pointer(w.x + 40, w.y + 40, 1, 0)?;
    let p = c.await_event("pointer press", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    d.pointer(w.x + 60, w.y + 60, 1, 0)?;
    let m = c.await_event("pointer motion", |e| {
        e.kind == kind::POINTER_MOVE && (e.x, e.y) == (60, 60)
    })?;
    d.pointer(w.x + 60, w.y + 60, 0, 0)?;
    let r = c.await_event("pointer release", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0
    })?;
    check(
        p.serial < m.serial && m.serial < r.serial,
        "press, move and release arrived out of serial order",
    )
}

pub fn capture_target_closes() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    let w = b.info()?;
    d.pointer(w.x + 40, w.y + 40, 1, 0)?;
    b.await_event("capture before owner close", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    b.send(EXIT, 0, 0)?;
    until(1000, "captured window gone", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    let p = a.info()?;
    d.pointer(p.x + 20, p.y + 20, 0, 0)?;
    d.pointer(p.x + 60, p.y + 60, 0, 0)?;
    a.await_event(
        "later pointer motion proves orphan release was processed",
        |e| e.kind == kind::POINTER_MOVE && e.x == 60 && e.y == 60,
    )?;
    check(
        !a.events
            .iter()
            .any(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 0),
        "orphaned capture released into different client",
    )?;
    d.click(p.x + 60, p.y + 60)?;
    a.await_event("fresh capture on surviving client releases", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0
    })?;
    check(
        a.events
            .iter()
            .filter(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 0)
            .count()
            == 1,
        "surviving client received an extra release",
    )
}
pub fn scroll_wakes() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    clock.step(1, 16_000_000)?;
    measure::acknowledged(1)?;
    let before = status()?.handled_serial;
    let w = c.info()?;
    d.pointer(w.x + 60, w.y + 60, 0, -1)?;
    let e = c.await_event("scroll wakes idle bwm", |e| {
        e.kind == kind::SCROLL && e.scroll_y == -1
    })?;
    until(1000, "scroll disposition acknowledged", || {
        Ok(status()?.handled_serial >= e.serial)
    })?;
    check(e.serial > before, "idle scroll reused prior handled serial")
}
pub fn fast_click() -> CaseResult {
    let mut v = Virtual::create()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let w = c.info()?;
    d.pointer(w.x + 40, w.y + 40, 0, 0)?;
    pause();
    v.batch(&[(1, 0x110, 1), (1, 0x110, 0)], 100)?;
    let p = c.await_event("same-batch press", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    let r = c.await_event("same-batch release", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0
    })?;
    check(p.serial < r.serial, "fast click lost or reordered an edge")
}
fn double_click(interval: u64, dx: i32, dy: i32, expected: u32) -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let mut v = Virtual::create()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let w = c.info()?;
    d.pointer(w.x + 60, w.y + 60, 0, 0)?;
    time::sleep_ms(100).map_err(|e| e.to_string())?;
    v.batch(&[(1, 0x110, 1), (1, 0x110, 0)], 1_000_000_000)?;
    c.await_event("first counted click", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    clock.step(1, 1_000_000_000 + interval)?;
    measure::acknowledged(1)?;
    v.batch(
        &[(2, 0, dx), (2, 1, dy), (1, 0x110, 1), (1, 0x110, 0)],
        1_000_000_000 + interval,
    )?;
    c.snapshot()?;
    until(1000, "two counted clicks", || {
        c.snapshot()?;
        Ok(c.events
            .iter()
            .filter(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 1)
            .count()
            >= 2)
    })?;
    let presses: Vec<_> = c
        .events
        .iter()
        .filter(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 1)
        .collect();
    check(
        presses.len() == 2 && presses[0].clicks == 1 && presses[1].clicks == expected,
        "double-click count violated interval or per-axis distance boundary",
    )
}
pub fn double_click_threshold() -> CaseResult {
    double_click(400_000_000, 0, 0, 2)?;
    double_click(400_000_001, 0, 0, 1)
}
pub fn double_click_distance() -> CaseResult {
    for (dx, dy, count) in [(4, 4, 2), (5, 0, 1), (0, 5, 1)] {
        double_click(200_000_000, dx, dy, count)?;
    }
    Ok(())
}
pub fn chrome_click() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (d, mut c) = one()?;
    let w = c.info()?;
    d.pointer(w.x + 30, w.y - 16 * d.scale, 0, 0)?;
    time::sleep_ms(100).map_err(|e| e.to_string())?;
    c.clear()?;
    v.batch(&[(1, 0x110, 1), (1, 0x110, 0)], 100)?;
    let es = stream.read_from(3, &[v.id])?;
    disposition(es[0].serial, "wm-handled", None)?;
    c.snapshot()?;
    check(
        !c.events.iter().any(|e| e.kind == kind::POINTER_BUTTON),
        "titlebar click leaked to client",
    )
}
pub fn enter_leave() -> CaseResult {
    let (d, mut a, mut b) = twin()?;
    let aw = a.info()?;
    let bw = b.info()?;
    d.pointer(aw.x + 20, aw.y + 20, 0, 0)?;
    a.await_event("pointer enters first content", |e| {
        e.kind == kind::POINTER_ENTER
    })?;
    a.clear()?;
    b.clear()?;
    d.pointer(bw.x + 20, bw.y + 20, 0, 0)?;
    let leave = a.await_event("pointer leaves first content", |e| {
        e.kind == kind::POINTER_LEAVE
    })?;
    let enter = b.await_event("pointer enters second content", |e| {
        e.kind == kind::POINTER_ENTER
    })?;
    check(
        leave.serial <= enter.serial,
        "crossing enter preceded leave",
    )
}
pub fn cursor_position() -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    for (x, y) in [(w.x + 30, w.y + 30), (w.x + 80, w.y + 90)] {
        d.pointer(x, y, 0, 0)?;
        // W30: a host send is transport, not a completed pointer frame. The
        // client's later publication puts this move before the observed frame.
        c.await_event("cursor-position move delivered", |e| {
            e.kind == kind::POINTER_MOVE && (e.x, e.y) == (x - w.x, y - w.y)
        })?;
        let commit = c.barrier()?;
        let s = d.shown_after(&c, commit)?;
        check(
            s.pixel(x, y)? == 0xff000000,
            "cursor tip missing at injected physical position",
        )?;
    }
    Ok(())
}
pub fn modifier_click() -> CaseResult {
    let mut v = Virtual::create()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    let w = c.info()?;
    d.pointer(w.x + 50, w.y + 50, 0, 0)?;
    c.clear()?;
    v.batch(&[(1, 42, 1), (1, 0x110, 1), (1, 0x110, 0), (1, 42, 0)], 100)?;
    let e = c.await_event("Shift pointer press", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    check(
        e.modifiers & abi::modifier::SHIFT != 0,
        "pointer press omitted held Shift",
    )
}
pub fn clamp() -> CaseResult {
    let mut v = Virtual::create()?;
    let (mut d, c) = one()?;
    let w = c.info()?;
    d.pointer(w.x + 50, w.y + 50, 0, 0)?;
    v.batch(&[(2, 0, 32767), (2, 1, 32767)], 100)?;
    until(1000, "clamped bottom-right cursor", || {
        Ok(Screen::read()?.pixel(d.width as i32 - 1, d.height as i32 - 1)? == 0xff000000)
    })?;
    v.batch(&[(2, 0, -32767), (2, 1, -32767)], 200)?;
    until(1000, "clamped top-left cursor", || {
        Ok(Screen::read()?.pixel(0, 0)? == 0xff000000)
    })?;
    d.shown(&c)?;
    Ok(())
}
pub fn minimized_none() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    let w = b.info()?;
    minimize(&d, &b)?;
    b.clear()?;
    d.pointer(w.x + 30, w.y + 30, 0, 0)?;
    d.click(w.x + 30, w.y + 30)?;
    b.snapshot()?;
    check(
        !b.events.iter().any(|e| {
            matches!(
                e.kind,
                kind::POINTER_MOVE | kind::POINTER_BUTTON | kind::POINTER_ENTER
            )
        }),
        "minimized client received pointer input",
    )?;
    d.focus(&mut a)?;
    key(&mut a, "a", 30)?;
    Ok(())
}
pub fn chromeless_click() -> CaseResult {
    let mut d = Desktop::start()?;
    let mut c = Client::start_mode(77, flags::CHROMELESS)?;
    d.client_ready(&c)?;
    let w = c.info()?;
    d.click(w.x + 40, w.y + 40)?;
    c.await_event("chromeless content click", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1 && e.x == 40 && e.y == 40
    })?;
    d.click(10, d.height as i32 / 2)?;
    until(1000, "outside click dismisses chromeless owner", || {
        Ok(!listed()?.iter().any(|w| w.id == c.id))
    })?;
    Ok(())
}
pub fn press_raises_delivers() -> CaseResult {
    let (mut d, mut a, b) = twin()?;
    a.clear()?;
    let w = a.info()?;
    d.click(w.x + 20, w.y + 20)?;
    a.await_event("lower content press delivered", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    a.snapshot()?;
    check(
        a.info()?.z > b.info()?.z
            && a.events
                .iter()
                .filter(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 1)
                .count()
                == 1,
        "press did not raise and deliver once",
    )?;
    content(&mut d, &a)
}
pub fn hit_test() -> CaseResult {
    let (_d, a, b) = twin()?;
    let w = b.info()?;
    let mut h = abi::HitTest::default();
    h.x = w.x + 20;
    h.y = w.y + 20;
    window::call(abi::op::HIT_TEST, &mut h).map_err(|e| e.to_string())?;
    check(
        h.id == b.id && h.id != a.id && h.scene_version == status()?.scene_version,
        "hit test did not name topmost overlapping content and current scene",
    )
}
pub fn stale_scene() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    let s = status()?;
    let w = c.info()?;
    let (_, meta) = window::read_meta(c.id).map_err(|e| e.to_string())?;
    c.clear()?;
    v.tagged(
        &d,
        c.id,
        s.scene_version.saturating_sub(1),
        meta.layout_version,
        71,
        w.x + 40,
        w.y + 40,
        1,
        100,
    )?;
    let es = stream.read_from(5, &[v.id])?;
    let press = es
        .iter()
        .find(|e| e.kind == 1 && e.code == 0x110)
        .ok_or("tagged press missing from core")?;
    disposition(press.serial, "rejected", Some("stale-scene"))?;
    c.snapshot()?;
    check(
        !c.events
            .iter()
            .any(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 1),
        "stale scene started gesture",
    )?;
    let current = status()?;
    v.tagged(
        &d,
        c.id,
        current.scene_version,
        meta.layout_version,
        72,
        w.x + 50,
        w.y + 50,
        1,
        200,
    )?;
    c.await_event("valid gesture starts capture", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    let other = Client::start(99)?;
    placed(&other)?;
    d.client_ready(&other)?;
    v.tagged(
        &d,
        c.id,
        current.scene_version,
        meta.layout_version,
        72,
        w.x + 60,
        w.y + 60,
        0,
        300,
    )?;
    c.await_event("captured stale-scene release still delivered", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0 && e.x == 60 && e.y == 60
    })?;
    Ok(())
}
