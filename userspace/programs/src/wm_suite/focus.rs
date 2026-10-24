use super::kit::*;

#[path = "../bwm/input_limits.rs"]
mod input_limits;

pub fn initial_focus() -> CaseResult {
    let (_d, mut c) = one()?;
    c.await_event("initial FocusGained", |e| e.kind == kind::FOCUS_GAINED)?;
    check(
        c.events
            .iter()
            .filter(|e| e.kind == kind::FOCUS_GAINED)
            .count()
            == 1,
        "initial focus was duplicated",
    )?;
    check(
        c.info()?.flags & win_flags::FOCUSED != 0,
        "focused window is not published as focused in LIST",
    )
}
pub fn focus_pair() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    b.clear()?;
    d.focus(&mut a)?;
    a.snapshot()?;
    b.snapshot()?;
    check(
        a.events
            .iter()
            .filter(|e| e.kind == kind::FOCUS_GAINED)
            .count()
            == 1
            && b.events
                .iter()
                .filter(|e| e.kind == kind::FOCUS_LOST)
                .count()
                == 1,
        "focus change omitted, duplicated or mistargeted the lost/gained pair",
    )
}
pub fn keys_focused() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut a)?;
    a.clear()?;
    b.clear()?;
    key(&mut a, "a", 30)?;
    b.snapshot()?;
    check(
        b.events.iter().all(|e| e.kind != kind::KEY_PRESS),
        "typed key reached an unfocused client",
    )
}
pub fn text_exact() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    hostkeys("shift-h,e,l,l,o,comma,space,shift-w,o,r,l,d,shift-1")?;
    until(2000, "typed text", || {
        c.snapshot()?;
        Ok(c.events
            .iter()
            .filter(|e| e.kind == kind::TEXT)
            .map(|e| e.text_len as usize)
            .sum::<usize>()
            >= 13)
    })?;
    let text: Vec<u8> = c
        .events
        .iter()
        .filter(|e| e.kind == kind::TEXT)
        .flat_map(|e| e.text[..e.text_len as usize].iter().copied())
        .collect();
    check(
        text == b"Hello, World!",
        &format!("typed text differs byte for byte: {text:?}"),
    )
}
pub fn releases() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    key(&mut c, "a", 30)?;
    c.await_event("physical A release routed through bwm", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30
    })?;
    Ok(())
}
pub fn ctrl_c() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let e = key(&mut c, "ctrl-c", 46)?;
    check(
        e.modifiers & abi::modifier::CTRL != 0 && e.modifiers & abi::modifier::SUPER == 0,
        "Ctrl+C lost Ctrl or became Super+C",
    )
}
pub fn navigation() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    for (chord, code) in [
        ("home", 102),
        ("end", 107),
        ("delete", 111),
        ("pageup", 104),
        ("pagedown", 109),
        ("left", 105),
        ("right", 106),
        ("up", 103),
        ("down", 108),
    ] {
        key(&mut c, chord, code)?;
    }
    Ok(())
}
pub fn escape() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    key(&mut c, "esc", 1)?;
    Ok(())
}
pub fn cycle_next() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    b.clear()?;
    hostkeys("alt-tab")?;
    a.await_event("Alt+Tab focuses next eligible client", |e| {
        e.kind == kind::FOCUS_GAINED
    })?;
    hostkeys("alt-tab")?;
    b.await_event("Alt+Tab wraps to the other client", |e| {
        e.kind == kind::FOCUS_GAINED
    })?;
    Ok(())
}

fn pair(a: &mut Client, b: &mut Client) -> CaseResult {
    a.await_event("old focus lost", |e| e.kind == kind::FOCUS_LOST)?;
    b.await_event("new focus gained", |e| e.kind == kind::FOCUS_GAINED)?;
    check(
        a.events
            .iter()
            .filter(|e| e.kind == kind::FOCUS_LOST)
            .count()
            == 1
            && b.events
                .iter()
                .filter(|e| e.kind == kind::FOCUS_GAINED)
                .count()
                == 1,
        "focus transition did not contain exactly one lost and gained pair",
    )
}
pub fn pair_appbar() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    b.clear()?;
    d.click(30 * d.scale, d.height as i32 - 18 * d.scale)?;
    pair(&mut b, &mut a)
}
pub fn pair_cycle() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    b.clear()?;
    hostkeys("alt-tab")?;
    pair(&mut b, &mut a)
}
pub fn focus_after_close() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    b.send(EXIT, 0, 0)?;
    a.await_event("focus successor after close", |e| {
        e.kind == kind::FOCUS_GAINED
    })?;
    check(
        a.info()?.flags & win_flags::FOCUSED != 0,
        "successor focus absent from registry",
    )
}
pub fn cycle_prev() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    let mut c = Client::start(47)?;
    placed(&c)?;
    d.client_ready(&c)?;
    d.focus(&mut c)?;
    for target in [&mut b, &mut a, &mut c] {
        target.clear()?;
        hostkeys("alt-shift-tab")?;
        target.await_event("reverse cycle focus order", |e| {
            e.kind == kind::FOCUS_GAINED
        })?;
        check(
            target.info()?.flags & win_flags::FOCUSED != 0,
            "reverse-cycle target not published focused",
        )?;
    }
    Ok(())
}
pub fn cycle_skips_minimized() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    let mut c = Client::start(47)?;
    placed(&c)?;
    d.client_ready(&c)?;
    minimize(&d, &b)?;
    d.focus(&mut c)?;
    b.clear()?;
    for index in [0, 1, 0] {
        let target = if index == 0 { &mut a } else { &mut c };
        target.clear()?;
        hostkeys("alt-tab")?;
        target.await_event("cycle skips minimized middle window", |e| {
            e.kind == kind::FOCUS_GAINED
        })?;
    }
    b.snapshot()?;
    check(
        !b.events.iter().any(|e| e.kind == kind::FOCUS_GAINED),
        "cycle focused minimized window",
    )
}
fn release_owner(no_new_release: bool) -> CaseResult {
    let mut v = Virtual::create()?;
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut a)?;
    a.clear()?;
    b.clear()?;
    v.batch(&[(1, 30, 1)], 100)?;
    a.await_event("A press before focus moves", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    d.focus(&mut b)?;
    v.batch(&[(1, 30, 0)], 200)?;
    a.await_event("A release follows original owner", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30
    })?;
    b.snapshot()?;
    if no_new_release {
        check(
            !b.events
                .iter()
                .any(|e| e.kind == kind::KEY_RELEASE && e.keycode == 30),
            "release leaked to new focus",
        )?;
    }
    Ok(())
}
pub fn release_follows_press() -> CaseResult {
    release_owner(false)
}
pub fn focus_change_mid_press() -> CaseResult {
    release_owner(true)
}
fn repeats(c: &mut Client) -> Result<usize, String> {
    c.snapshot()?;
    Ok(c.events
        .iter()
        .filter(|e| e.kind == kind::KEY_PRESS && e.flags & abi::EVENT_REPEAT != 0)
        .count())
}
fn held() -> Result<(measure::Clock, Desktop, Client, Virtual), String> {
    let clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let mut v = Virtual::create()?;
    v.batch(&[(1, 30, 1)], 0)?;
    c.await_event("initial held key press", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    Ok((clock, d, c, v))
}
pub fn repeat_delay() -> CaseResult {
    let (mut clock, _d, mut c, _v) = held()?;
    clock.step(1, 499_999_999)?;
    measure::acknowledged(1)?;
    check(repeats(&mut c)? == 0, "repeat arrived before 500 ms")?;
    clock.step(2, 500_000_000)?;
    measure::acknowledged(2)?;
    check(
        repeats(&mut c)? == 1,
        "repeat missing at inclusive 500 ms deadline",
    )
}
pub fn repeat_rate() -> CaseResult {
    let (mut clock, _d, mut c, _v) = held()?;
    for n in 1..=20 {
        clock.step(n, n * 50_000_000)?;
        measure::acknowledged(n)?;
    }
    check(
        repeats(&mut c)? == 11,
        "held key did not produce exactly eleven repeats through 1000 ms",
    )
}
pub fn repeat_cancel() -> CaseResult {
    for remove in [false, true] {
        let (mut clock, _d, mut c, mut v) = held()?;
        clock.step(1, 500_000_000)?;
        measure::acknowledged(1)?;
        check(
            repeats(&mut c)? == 1,
            "positive repeat before cancellation missing",
        )?;
        if !remove {
            v.batch(&[(1, 30, 0)], 500_000_000)?;
        }
        drop(v);
        c.await_event("release after cancel or device removal", |e| {
            e.kind == kind::KEY_RELEASE && e.keycode == 30
        })?;
        let count = repeats(&mut c)?;
        clock.step(2, 1_000_000_000)?;
        measure::acknowledged(2)?;
        check(
            repeats(&mut c)? == count,
            "repeat continued after release or device removal",
        )?;
    }
    Ok(())
}
pub fn repeat_frozen_clock() -> CaseResult {
    let (_clock, _d, mut c, _v) = held()?;
    let before = now();
    time::sleep_ms(650).map_err(|e| e.to_string())?;
    check(repeats(&mut c)? == 0, "frozen UI clock produced repeats")?;
    check(
        now() - before < 1_500_000_000,
        "frozen-clock check missed its real deadline",
    )
}
pub fn super_return() -> CaseResult {
    let (mut d, c, w) = launch()?;
    let screen = d.shown(&c)?;
    check(
        screen.contains(w.id, w.commit_id) && w.owner != c.child.pid as u32,
        "configured Super+Return did not launch an independent committed owner",
    )
}
pub fn alt_q() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    hostkeys("alt-q")?;
    c.await_event("Alt+Q close notice", |e| e.kind == kind::CLOSE_REQUESTED)?;
    check(
        c.child.status()?.is_none(),
        "Alt+Q killed ignoring client before escalation",
    )
}
pub fn hotkey_consumed() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    v.batch(&[(1, 56, 1), (1, 16, 1), (1, 16, 0), (1, 56, 0)], 100)?;
    let es = stream.read_from(5, &[v.id])?;
    let serial = es
        .iter()
        .find(|e| e.code == 16 && e.value == 1)
        .ok_or("Q source event missing")?
        .serial;
    c.await_event("consumed Alt+Q action", |e| e.kind == kind::CLOSE_REQUESTED)?;
    disposition(serial, "wm-handled", None)?;
    check(
        !c.events
            .iter()
            .any(|e| e.kind == kind::KEY_PRESS && e.keycode == 16),
        "consumed hotkey leaked Q to client",
    )?;
    v.batch(&[(1, 16, 1), (1, 16, 0)], 200)?;
    c.await_event("unmodified Q still delivered", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 16
    })?;
    Ok(())
}
pub fn super_double_tap() -> CaseResult {
    for (interval, opens) in [(400_000_000, true), (400_000_001, false)] {
        let mut clock = measure::Clock::new()?;
        let _d = Desktop::start()?;
        let mut v = Virtual::create()?;
        let before = listed()?.len();
        v.batch(&[(1, 125, 1), (1, 125, 0)], 0)?;
        clock.step(1, interval)?;
        measure::acknowledged(1)?;
        v.batch(&[(1, 125, 1), (1, 125, 0)], interval)?;
        clock.step(2, interval + 1)?;
        measure::acknowledged(2)?;
        // W30: the epoch acknowledges the launch action, not the asynchronous
        // child's REGISTER. Establish that prerequisite inside the existing
        // case deadline before comparing the unchanged window count.
        if opens {
            until(2000, "double-tap child registered its window", || {
                Ok(listed()?.len() > before)
            })?;
        }
        let windows = listed()?;
        check(
            (windows.len() > before) == opens,
            "Super double tap violated inclusive 400 ms boundary",
        )?;
    }
    Ok(())
}
pub fn super_single_tap() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let _d = Desktop::start()?;
    let mut v = Virtual::create()?;
    let before = listed()?;
    v.batch(&[(1, 125, 1), (1, 125, 0)], 0)?;
    clock.step(1, 500_000_000)?;
    measure::acknowledged(1)?;
    check(
        listed()?.len() == before.len(),
        "single Super tap opened launcher",
    )
}
pub fn hotkeys_config() -> CaseResult {
    let _config = FileOverride::new(
        "/etc/hotkeys.conf",
        b"super+k = exec /usr/libexec/breenix/wm-client_test\n",
    )?;
    let _d = Desktop::start()?;
    let before = listed()?;
    hostkeys("super-k")?;
    until(5000, "custom hotkeys.conf launch", || {
        Ok(listed()?
            .iter()
            .any(|w| !before.iter().any(|old| old.id == w.id) && w.commit_id != 0))
    })?;
    Ok(())
}
pub fn no_focus_no_delivery() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (_d, mut c) = one()?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    c.await_event("positive delivery before no focus", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    c.send(EXIT, 0, 0)?;
    until(1000, "last window removed", || Ok(listed()?.is_empty()))?;
    let _ = stream.read_from(3, &[v.id])?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 200)?;
    let es = stream.read_from(3, &[v.id])?;
    disposition(es[0].serial, "rejected", Some("no-focus"))
}
pub fn focus_new_window() -> CaseResult {
    let (mut d, mut a) = one()?;
    a.clear()?;
    let mut b = Client::start(47)?;
    placed(&b)?;
    d.client_ready(&b)?;
    pair(&mut a, &mut b)?;
    check(
        b.info()?.flags & win_flags::FOCUSED != 0,
        "new window did not take focus",
    )
}
pub fn routing_after_exit() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut a)?;
    b.send(EXIT, 0, 0)?;
    until(1000, "other owner exited", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    a.clear()?;
    key(&mut a, "a", 30)?;
    content(&mut d, &a)
}
pub fn lost_then_delivered() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (_d, mut c) = one()?;
    c.command(SET_FLAGS, flags::IGNORE_CLOSE | flags::NEVER_READ, 0)?;
    let mut sources = Vec::new();
    // W28: each pair produces at least two client events; exceed both
    // retention bounds by a complete additional client ring.
    let pairs = (input_limits::MAX_PENDING + 2 * abi::limits::EVENT_RING) as u64;
    for n in 1..=pairs {
        v.batch(&[(1, 30, 1), (1, 30, 0)], n)?;
        sources.extend(
            stream
                .read_from(3, &[v.id])?
                .into_iter()
                .filter(|e| e.kind == 1)
                .map(|e| e.serial),
        );
    }
    // W30: the observation must not allocate a serial string for every cell
    // of a source-by-ledger cross product. Source serials are already sorted.
    let rows = std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
    let lost = rows.lines().find_map(|row| {
        let mut fields = row.split_whitespace();
        let serial = fields.next()?.parse::<u64>().ok()?;
        (fields.next() == Some("lost") && sources.binary_search(&serial).is_ok()).then_some(serial)
    })
        .ok_or("full client ring had no lost source disposition")?;
    c.command(SET_FLAGS, flags::IGNORE_CLOSE, 0)?;
    c.clear()?;
    v.batch(&[(1, 48, 1), (1, 48, 0)], pairs + 1)?;
    let later = stream.read_from(3, &[v.id])?;
    c.await_event("delivery after overflow", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 48
    })?;
    disposition(lost, "lost", None)?;
    disposition(later[0].serial, "delivered", None)
}
