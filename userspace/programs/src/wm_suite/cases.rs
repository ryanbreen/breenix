use super::{desktop::*, input::*, measure, protocol::*};
use breenix_suite::suite::{check, hostkeys, value, CaseResult};
use libbreenix::{signal, time, window};
use window::abi::{self, event_kind as kind, win_flags, Plain};

fn placed(c: &Client) -> Result<(), String> {
    until(2000, "bwm assigned registered client geometry", || {
        let w = c.info()?;
        Ok(w.x > 0 && w.y > 0)
    })
}

fn one() -> Result<(Desktop, Client), String> {
    let d = Desktop::start()?;
    let c = Client::start(11)?;
    placed(&c)?;
    Ok((d, c))
}
fn twin() -> Result<(Desktop, Client, Client), String> {
    let d = Desktop::start()?;
    let a = Client::start(11)?;
    placed(&a)?;
    let b = Client::start(23)?;
    placed(&b)?;
    Ok((d, a, b))
}
fn key(c: &mut Client, chords: &str, code: u32) -> Result<abi::WindowEvent, String> {
    hostkeys(chords)?;
    c.await_event(
        &format!("bwm routing of {chords}, Linux keycode {code}"),
        |e| e.kind == kind::KEY_PRESS && e.keycode == code,
    )
}
fn parked(d: &Desktop) -> Result<(), String> {
    d.pointer(d.width as i32 - 40, d.height as i32 - 100, 0, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())
}
fn content(d: &mut Desktop, c: &Client) -> CaseResult {
    parked(d)?;
    let commit = c.barrier()?;
    d.shown_after(c, commit)?.client(c)
}

pub fn virtual_keyboard() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    let (_d, mut c) = one()?;
    device.batch(&[(1, 30, 1), (1, 30, 0)], 1_000_000)?;
    let events = stream.read_from(3, &[device.id])?;
    check(
        events
            .iter()
            .any(|e| e.kind == 1 && e.code == 30 && e.value == 1),
        "core omitted virtual A press",
    )?;
    check(
        events
            .iter()
            .any(|e| e.kind == 1 && e.code == 30 && e.value == 0),
        "core omitted virtual A release",
    )?;
    let press = events
        .iter()
        .find(|e| e.kind == 1 && e.code == 30 && e.value == 1)
        .unwrap()
        .serial;
    let release = events
        .iter()
        .find(|e| e.kind == 1 && e.code == 30 && e.value == 0)
        .unwrap()
        .serial;
    c.await_event("virtual A press through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30 && e.serial == press
    })?;
    c.await_event("virtual A release through bwm", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30 && e.serial == release
    })?;
    Ok(())
}
pub fn core_serials() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut a = Virtual::create()?;
    let mut b = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    b.batch(&[(1, 48, 1), (1, 48, 0)], 200)?;
    let events = stream.read_from(6, &[a.id, b.id])?;
    check(
        events
            .windows(2)
            .all(|v| v[0].serial > 0 && v[0].serial < v[1].serial),
        "core serials repeat or decrease across devices",
    )?;
    check(
        events
            .iter()
            .filter(|e| e.kind == 1)
            .all(|e| e.device != 0 && e.flags == 0),
        "core omitted device identity or set reserved flags",
    )?;
    c.await_event("first device A routed through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    c.await_event("second device B routed through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 48
    })?;
    Ok(())
}
pub fn core_timestamps() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 30, 1), (1, 30, 0)], 123_456_789)?;
    check(
        stream
            .read_from(3, &[a.id])?
            .iter()
            .all(|e| e.time == 123_456_789),
        "time=injector was replaced with a real clock",
    )?;
    check(
        a.batch(&[(1, 48, 1)], 123_456_788)
            .err()
            .is_some_and(|e| e.starts_with("uinput batch errno 22:")),
        "core accepted decreasing injector time",
    )?;
    c.await_event("injector-time A through bwm", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30 && e.time_ns == 123_456_789
    })?;
    Ok(())
}
pub fn core_modifiers() -> CaseResult {
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 29, 1), (1, 30, 1), (1, 30, 0), (1, 29, 0)], 100)?;
    let e = c.await_event("virtual Ctrl+A", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    check(
        e.modifiers & abi::modifier::CTRL != 0 && e.modifiers & abi::modifier::SUPER == 0,
        "Ctrl was lost or conflated with Super",
    )
}
pub fn core_text() -> CaseResult {
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    let text = "héλ🙂".as_bytes();
    a.text(text, 100)?;
    let e = c.await_event("whole virtual EV_TEXT through bwm", |e| {
        e.kind == kind::TEXT
    })?;
    check(
        e.text_len as usize == text.len() && &e.text[..text.len()] == text,
        "EV_TEXT was split, translated or truncated",
    )?;
    check(
        a.text(&[0xff], 200)
            .err()
            .is_some_and(|e| e.starts_with("EV_TEXT batch errno 22:")),
        "invalid UTF-8 EV_TEXT was accepted",
    )
}
pub fn core_removal() -> CaseResult {
    let mut a = Virtual::create()?;
    let (_d, mut c) = one()?;
    a.batch(&[(1, 30, 1)], 100)?;
    c.await_event("held virtual A", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    drop(a);
    c.await_event("release on virtual device removal", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30
    })?;
    Ok(())
}
pub fn core_ledger() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    let (_d, mut c) = one()?;
    device.batch(&[(1, 30, 1), (1, 30, 0), (3, 0, 0), (3, 1, 0)], 100)?;
    let events = stream.read_from(5, &[device.id])?;
    c.await_event("ledger script key reaches actual client", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    c.await_event("ledger script release reaches actual client", |e| {
        e.kind == kind::KEY_RELEASE && e.keycode == 30
    })?;
    let serials = events.iter().map(|e| e.serial).collect::<Vec<_>>();
    until(
        2000,
        "exactly one terminal disposition per injected serial",
        || {
            let text = std::fs::read_to_string("/proc/breenix/input-ledger")
                .map_err(|e| format!("input dispositions ledger is absent: {e}"))?;
            for serial in &serials {
                let rows = text
                    .lines()
                    .map(|s| s.split_whitespace().collect::<Vec<_>>())
                    .filter(|v| v.first().and_then(|s| s.parse::<u64>().ok()) == Some(*serial))
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    return Ok(false);
                }
                if rows.len() != 1 {
                    return Err(format!(
                        "input serial {serial} has {} dispositions",
                        rows.len()
                    ));
                }
                let row = &rows[0];
                if row.len() < 3 {
                    return Err("malformed disposition row".into());
                }
                if ![
                    "wm-handled",
                    "delivered",
                    "rejected",
                    "client-rejected",
                    "lost",
                ]
                .contains(&row[1])
                {
                    return Err("unknown terminal disposition".into());
                }
                if row[1] == "lost" {
                    return Err(format!(
                        "input serial {serial} was lost in a non-overflow script"
                    ));
                }
                if row[1] == "delivered" {
                    let id = row[2].parse::<u64>().map_err(|e| e.to_string())?;
                    if id != c.id {
                        return Err(format!("serial {serial} was delivered to the wrong client"));
                    }
                    let (_, meta) = window::read_meta(c.id).map_err(|e| e.to_string())?;
                    if meta.processed_serial < *serial {
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        },
    )?;
    Ok(())
}
pub fn core_capabilities() -> CaseResult {
    let mut device = Virtual::create()?;
    let (_d, mut c) = one()?;
    device.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    c.await_event("positive declared key before capability refusal", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    check(
        device
            .batch(&[(1, 999, 1)], 200)
            .err()
            .is_some_and(|e| e.starts_with("uinput batch errno 22:")),
        "undeclared capability did not return EINVAL",
    )?;
    device.batch(&[(1, 48, 1), (1, 48, 0)], 300)?;
    c.await_event("valid key still routes after capability refusal", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 48
    })?;
    Ok(())
}
pub fn virtual_pointer() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut device = Virtual::create()?;
    let (d, mut c) = one()?;
    let w = c.info()?;
    let x = w.x + 40;
    let y = w.y + 50;
    device.batch(
        &[
            (
                3,
                0,
                ((x as u64 * 32767 + (d.width - 2) as u64) / (d.width - 1) as u64) as i32,
            ),
            (
                3,
                1,
                ((y as u64 * 32767 + (d.height - 2) as u64) / (d.height - 1) as u64) as i32,
            ),
            (1, 0x110, 1),
        ],
        100,
    )?;
    check(
        stream
            .read_from(4, &[device.id])?
            .iter()
            .any(|e| e.kind == 3),
        "virtual pointer axes absent from core stream",
    )?;
    c.await_event("virtual pointer maps exactly into client content", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1 && (e.x, e.y) == (40, 50)
    })?;
    device.batch(&[(1, 0x110, 0)], 200)?;
    c.await_event("virtual pointer release", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0
    })?;
    Ok(())
}
pub fn keyboard_completion() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    replay(1, 0, 0)?;
    check(
        stream
            .read(2)?
            .iter()
            .any(|e| e.kind == 1 && e.code == 30 && e.value == 1),
        "bound keyboard completion did not report A",
    )?;
    c.await_event("bound keyboard completion wakes bwm and delivers A", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    Ok(())
}
pub fn pointer_completion() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    {
        let w = c.info()?;
        replay(2, w.x + 40, w.y + 50)?;
    };
    check(
        stream.read(4)?.iter().any(|e| e.kind == 3),
        "bound pointer completion did not report absolute axes",
    )?;
    c.await_event("bound pointer completion wakes bwm", |e| {
        e.kind == kind::POINTER_MOVE
    })?;
    Ok(())
}
pub fn replay_faults() -> CaseResult {
    let mut stream = Stream::open()?;
    let (_d, mut c) = one()?;
    replay(1, 0, 0)?;
    c.await_event("positive keyboard completion before planted fault", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    stream.read(2)?;
    c.clear()?;
    replay(0x101, 0, 0)?; // same production completion, suppress its report once
    check(
        stream.read(1).is_err(),
        "report-suppression fault still published input",
    )?;
    c.snapshot()?;
    check(
        c.events.iter().all(|e| e.kind != kind::KEY_PRESS),
        "report-suppression fault still reached a client",
    )
}

pub fn claims_display() -> CaseResult {
    let (mut d, c) = one()?;
    d.shown(&c)?;
    let result = window::claim_compositor();
    check(
        matches!(
            result,
            Err(libbreenix::error::Error::Os(
                libbreenix::errno::Errno::EBUSY
            ))
        ),
        &format!("bwm did not retain the compositor claim: {result:?}"),
    )
}
pub fn desktop_control() -> CaseResult {
    let d = Desktop::start()?;
    let s = Screen::read()?;
    check(
        s.serial() != 0 && s.pixel(d.width as i32 / 2, 1)? == 0xff14161e,
        "completed desktop control lacks the literal taskbar pixel",
    )
}

pub fn first_client() -> CaseResult {
    let (mut d, c) = one()?;
    content(&mut d, &c)
}
pub fn existing_windows() -> CaseResult {
    let c = Client::start(11)?;
    c.child.freeze()?;
    let mut d = Desktop::start()?;
    d.shown(&c)?.client(&c)
}
pub fn independent_clients() -> CaseResult {
    let (mut d, a, b) = twin()?;
    check(
        a.child.pid != b.child.pid
            && a.info()?.owner as i32 == a.child.pid
            && b.info()?.owner as i32 == b.child.pid,
        "clients are not independent window owners",
    )?;
    content(&mut d, &b)
}
pub fn supervisor_starts() -> CaseResult {
    let mut d = Desktop::launch(true)?;
    let c = Client::start(11)?;
    content(&mut d, &c)?;
    check(
        !children(d.wm.pid, "bwm")?.is_empty(),
        "init desktop did not retain a real bwm child",
    )
}

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
    content(&mut d, &c)
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

pub fn click_raises() -> CaseResult {
    let (mut d, mut a, b) = twin()?;
    d.focus(&mut a)?;
    check(
        a.info()?.z > b.info()?.z,
        "click did not raise the target by stable window id",
    )?;
    content(&mut d, &a)
}
fn minimize(d: &Desktop, c: &Client) -> Result<(), String> {
    let w = c.info()?;
    d.click(w.x + w.width as i32 - 39 * d.scale, w.y - 16 * d.scale)?;
    until(1000, "minimized visibility published", || {
        Ok(c.info()?.flags & win_flags::MINIMIZED != 0)
    })
}
pub fn minimize_hides() -> CaseResult {
    let (mut d, a, b) = twin()?;
    content(&mut d, &b)?;
    minimize(&d, &b)?;
    let commit = a.barrier()?;
    let s = d.shown_after(&a, commit)?;
    s.client(&a)?;
    check(
        !s.contains(b.id, 0),
        "minimized client's content remains in the completed composite",
    )
}
pub fn restore() -> CaseResult {
    let (mut d, mut c) = one()?;
    minimize(&d, &c)?;
    d.click(80 * d.scale, d.height as i32 - 18 * d.scale)?;
    until(1000, "appbar restore", || {
        Ok(c.info()?.flags & win_flags::VISIBLE != 0)
    })?;
    content(&mut d, &c)?;
    c.clear()?;
    key(&mut c, "a", 30)?;
    Ok(())
}
pub fn all_minimized() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    b.clear()?;
    key(&mut b, "a", 30)?;
    check(
        b.info()?.flags & win_flags::FOCUSED != 0,
        "positive focused client is not published focused",
    )?;
    minimize(&d, &b)?;
    // Minimize the remaining visible client using its own chrome.
    for w in listed()?
        .into_iter()
        .filter(|w| w.flags & win_flags::MINIMIZED == 0)
    {
        d.click(w.x + w.width as i32 - 39 * d.scale, w.y - 16 * d.scale)?;
    }
    until(1000, "both clients minimized with no focus", || {
        Ok([a.info()?, b.info()?]
            .iter()
            .all(|w| w.flags & win_flags::MINIMIZED != 0 && w.flags & win_flags::FOCUSED == 0))
    })?;
    check(
        listed()?.iter().all(|w| w.flags & win_flags::FOCUSED == 0),
        "all-minimized desktop retained a focused window",
    )?;
    a.clear()?;
    b.clear()?;
    hostkeys("a")?;
    time::sleep_ms(250)?;
    a.snapshot()?;
    b.snapshot()?;
    check(
        a.events
            .iter()
            .chain(&b.events)
            .all(|e| e.kind != kind::KEY_PRESS),
        "a minimized window received typed input",
    )
}
pub fn exit_restacks() -> CaseResult {
    let (mut d, a, mut b) = twin()?;
    let c = Client::start(31)?;
    d.shown(&c)?;
    let az = a.info()?.z;
    let cz = c.info()?.z;
    b.send(EXIT, 0, 0)?;
    until(1000, "middle exit", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    check(
        a.info()?.z < c.info()?.z && az < cz,
        "survivors changed order after another owner exited",
    )?;
    content(&mut d, &c)
}

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
    hostkeys("shift-h,e,l,l,o,space,shift-w,o,r,l,d")?;
    until(2000, "typed text", || {
        c.snapshot()?;
        Ok(c.events
            .iter()
            .filter(|e| e.kind == kind::KEY_PRESS)
            .map(|e| e.text_len as usize)
            .sum::<usize>()
            >= 11)
    })?;
    let text: Vec<u8> = c
        .events
        .iter()
        .filter(|e| e.kind == kind::KEY_PRESS)
        .flat_map(|e| e.text[..e.text_len as usize].iter().copied())
        .collect();
    check(
        text == b"Hello World",
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

fn drag(d: &Desktop, from: (i32, i32), to: (i32, i32)) -> Result<(), String> {
    d.pointer(from.0, from.1, 0, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())?;
    d.pointer(from.0, from.1, 1, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())?;
    d.pointer(to.0, to.1, 1, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())?;
    d.pointer(to.0, to.1, 0, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())
}
pub fn drag_move() -> CaseResult {
    let (mut d, c) = one()?;
    let w = c.info()?;
    let p = (w.x + 50, w.y - 16 * d.scale);
    drag(&d, p, (p.0 + 90, p.1 + 70))?;
    until(1000, "titlebar drag published geometry", || {
        Ok(c.info()?.x == w.x + 90 && c.info()?.y == w.y + 70)
    })?;
    content(&mut d, &c)
}
fn resize_case(bottom: bool) -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    let from = if bottom {
        (w.x + 80, w.y + w.height as i32 + 1)
    } else {
        (w.x + w.width as i32 + 1, w.y + 80)
    };
    let to = if bottom {
        (from.0, from.1 + 60)
    } else {
        (from.0 + 80, from.1)
    };
    drag(&d, from, to)?;
    c.await_event("owner-mediated ResizeRequested", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    until(1000, "owner resized buffers published", || {
        c.snapshot()?;
        Ok((c.width, c.height)
            == if bottom {
                (w.width, w.height + 60)
            } else {
                (w.width + 80, w.height)
            })
    })?;
    let after = c.info()?;
    check(
        after.x == w.x && after.y == w.y,
        "right/bottom resize moved content origin",
    )?;
    content(&mut d, &c)
}
pub fn resize_right() -> CaseResult {
    resize_case(false)
}
pub fn resize_bottom() -> CaseResult {
    resize_case(true)
}
pub fn resize_refused() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.command(REFUSE, 1, 0)?;
    let w = c.info()?;
    drag(
        &d,
        (w.x + w.width as i32 + 1, w.y + 80),
        (w.x + w.width as i32 + 81, w.y + 80),
    )?;
    c.await_event("refusing owner receives resize request", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    check(
        c.info()?.width == w.width && c.info()?.height == w.height,
        "ignored resize changed the owner buffer size",
    )?;
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

pub fn close_request() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    let w = c.info()?;
    d.click(w.x + w.width as i32 - 15 * d.scale, w.y - 16 * d.scale)?;
    c.await_event("first close delivers CloseRequested", |e| {
        e.kind == kind::CLOSE_REQUESTED
    })?;
    check(
        c.child.status()?.is_none(),
        "first close sent a terminating signal",
    )?;
    content(&mut d, &c)
}
fn disappear(sig: Option<i32>, exec: bool) -> CaseResult {
    let (mut d, a, mut b) = twin()?;
    content(&mut d, &b)?;
    if let Some(sig) = sig {
        b.child.signal(sig)?;
    } else {
        b.send(if exec { EXEC } else { EXIT }, 0, 0)?;
    }
    let mut status = None;
    until(1000, "client completed its exit, exec or crash", || {
        status = b.child.status()?;
        Ok(status.is_some())
    })?;
    let status = status.unwrap();
    if let Some(sig) = sig {
        check(
            status & 127 == sig,
            &format!("client did not die from expected signal {sig}: wait status {status}"),
        )?;
    } else {
        check(
            status == 0,
            &format!(
                "client {} failed: wait status {status}",
                if exec {
                    "exec of /bin/true"
                } else {
                    "normal exit"
                }
            ),
        )?;
    }
    until(1000, "dead/exec client window removed", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    content(&mut d, &a)
}
pub fn client_exit() -> CaseResult {
    disappear(None, false)
}
pub fn client_crash() -> CaseResult {
    disappear(Some(signal::SIGSEGV), false)
}
pub fn client_exec() -> CaseResult {
    disappear(None, true)
}
pub fn close_unresponsive() -> CaseResult {
    let (d, mut c) = one()?;
    let w = c.info()?;
    d.click(w.x + w.width as i32 - 15 * d.scale, w.y - 16 * d.scale)?;
    c.await_event("ignored first close", |e| e.kind == kind::CLOSE_REQUESTED)?;
    d.click(w.x + w.width as i32 - 15 * d.scale, w.y - 16 * d.scale)?;
    let mut status = None;
    until(1000, "second close terminates unresponsive owner", || {
        status = c.child.status()?;
        Ok(status.is_some())
    })?;
    check(
        status.unwrap() & 127 == signal::SIGTERM,
        "second close did not send SIGTERM",
    )
}
pub fn compositor_steal() -> CaseResult {
    claims_display()
}
pub fn inject_denied() -> CaseResult {
    let (_d, c) = one()?;
    let mut e = abi::WindowEvent::ZERO;
    e.kind = kind::KEY_PRESS;
    e.serial = 1;
    check(
        matches!(
            window::write_events(c.id, &[e]),
            Err(libbreenix::error::Error::Os(
                libbreenix::errno::Errno::EPERM
            ))
        ),
        "non-compositor injected input while bwm held the claim",
    )
}
fn launch() -> Result<(Desktop, Client, abi::WinInfo), String> {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    let before = listed()?.iter().map(|w| w.id).collect::<Vec<_>>();
    hostkeys("super-enter")?;
    let mut launched = None;
    until(
        5000,
        "Super+Return launches a real GUI client and its first commit",
        || {
            launched = listed()?
                .into_iter()
                .find(|w| !before.contains(&w.id) && w.commit_id > 0);
            Ok(launched.is_some())
        },
    )?;
    let w = launched.unwrap();
    until(2000, "launched client first commit composed by bwm", || {
        Ok(Screen::read().is_ok_and(|s| s.contains(w.id, w.commit_id)))
    })?;
    Ok((d, c, w))
}
pub fn launch_ack() -> CaseResult {
    let (_d, _c, w) = launch()?;
    check(
        w.owner != 0 && w.width > 0 && w.height > 0,
        "launch produced no real owned client surface",
    )
}
pub fn launch_reaped() -> CaseResult {
    let (d, _c, w) = launch()?;
    signal::kill(w.owner as i32, signal::SIGTERM)?;
    until(1000, "bwm reaps its launched child", || {
        Ok(!std::path::Path::new(&format!("/proc/{}/status", w.owner)).exists())
    })?;
    check(
        children(d.wm.pid, "bterm")?.is_empty(),
        "launched terminal remains a child after exit",
    )
}

pub fn clients_notified() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.clear()?;
    d.wm.stop()?;
    let e = c.await_event("client notified when real bwm dies", |e| {
        e.kind == kind::COMPOSITOR_CHANGED
    })?;
    check(e.epoch > 0, "compositor-death notice omitted its epoch")
}
pub fn restart_adopts() -> CaseResult {
    let (mut d, c) = one()?;
    content(&mut d, &c)?;
    c.child.freeze()?;
    d.wm.stop()?;
    let mut next = Desktop::start()?;
    next.shown(&c)?.client(&c)
}
pub fn interaction_restart() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    c.clear()?;
    key(&mut c, "a", 30)?;
    d.wm.stop()?;
    let mut next = Desktop::start()?;
    next.focus(&mut c)?;
    c.clear()?;
    key(&mut c, "b", 48)?;
    content(&mut next, &c)
}
pub fn stack_restored() -> CaseResult {
    let (mut d, mut a, b) = twin()?;
    d.focus(&mut a)?;
    minimize(&d, &a)?;
    let before = listed()?;
    d.wm.stop()?;
    let mut next = Desktop::start()?;
    next.shown(&b)?;
    let after = listed()?;
    check(
        before.iter().all(|w| {
            after.iter().any(|v| {
                v.id == w.id
                    && (
                        v.x,
                        v.y,
                        v.z,
                        v.flags & (win_flags::VISIBLE | win_flags::MINIMIZED),
                    ) == (
                        w.x,
                        w.y,
                        w.z,
                        w.flags & (win_flags::VISIBLE | win_flags::MINIMIZED),
                    )
            })
        }),
        "restarted bwm discarded published geometry, stacking or minimization",
    )
}
pub fn supervisor_restarts() -> CaseResult {
    let mut d = Desktop::launch(true)?;
    let c = Client::start(11)?;
    d.shown(&c)?;
    let old = children(d.wm.pid, "bwm")?
        .first()
        .copied()
        .ok_or("desktop supervisor has no bwm child")?;
    signal::kill(old, signal::SIGKILL)?;
    until(2000, "production desktop supervisor restarts bwm", || {
        Ok(children(d.wm.pid, "bwm")?.iter().any(|p| *p != old))
    })?;
    content(&mut d, &c)
}
pub fn children(parent: i32, name: &str) -> Result<Vec<i32>, String> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir("/proc")
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let wanted = status.lines().any(|s| {
            s.strip_prefix("PPid:")
                .is_some_and(|s| s.trim() == parent.to_string())
        });
        let named = status
            .lines()
            .any(|s| s.strip_prefix("Name:").is_some_and(|s| s.trim() == name));
        if wanted && named {
            result.push(pid);
        }
    }
    Ok(result)
}

fn clip_ok(c: &mut Client, set: bool, text: &[u8]) -> Result<(u64, Vec<u8>), String> {
    let (status, serial, text) = c.clipboard(set, text)?;
    if status != 0 {
        return Err(format!("bwm clipboard {} failed: errno {}; requires /run/bwm/clipboard and a real window-owning peer",if set{"SET"}else{"GET"},-status));
    }
    if serial == 0 {
        return Err("bwm clipboard omitted its change serial".into());
    }
    Ok((serial, text))
}
fn clip_roundtrip(text: &[u8]) -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.shown(&b)?;
    let (serial, _) = clip_ok(&mut a, true, text)?;
    let (got, bytes) = clip_ok(&mut b, false, &[])?;
    check(
        got == serial && bytes == text,
        "clipboard did not transfer exact bytes and serial between independent clients",
    )
}
pub fn clip_cross() -> CaseResult {
    clip_roundtrip(b"from the first program")
}
pub fn clip_utf8() -> CaseResult {
    clip_roundtrip("héλ🙂\n".as_bytes())
}
pub fn clip_large() -> CaseResult {
    clip_roundtrip(&vec![b'x'; 1024 * 1024])
}
pub fn clip_nul() -> CaseResult {
    clip_roundtrip(b"one\0two\0three")
}
pub fn clip_replace() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    let (first, _) = clip_ok(&mut a, true, b"first")?;
    let (second, _) = clip_ok(&mut b, true, b"second")?;
    let (got, text) = clip_ok(&mut a, false, &[])?;
    check(
        second > first && got == second && text == b"second",
        "clipboard replacement did not advance serial and replace exact text",
    )
}
pub fn clip_owner_exit() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    let (serial, _) = clip_ok(&mut a, true, b"retained")?;
    a.send(EXIT, 0, 0)?;
    until(1000, "clipboard owner exited", || {
        Ok(!listed()?.iter().any(|w| w.id == a.id))
    })?;
    let (got, text) = clip_ok(&mut b, false, &[])?;
    check(
        got == serial && text == b"retained",
        "clipboard text vanished or changed on owner exit",
    )
}

pub fn idle_composites() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    measure::acknowledged(0)?;
    let before = d.shown(&c)?.serial();
    for epoch in 1..=100 {
        clock.step(epoch, 0)?;
        c.command(STEP, epoch as u32, 0)?;
        measure::acknowledged(epoch)?;
    }
    let after = Screen::read()?.serial();
    value(
        "idle-composites",
        after.saturating_sub(before) as i64,
        "",
        Some((0, 0)),
    );
    check(after == before, "idle desktop composed with no visual work")
}
pub fn hung_client() -> CaseResult {
    let (mut d, mut a, b) = twin()?;
    b.child.signal(signal::SIGSTOP)?;
    d.focus(&mut a)?;
    a.clear()?;
    key(&mut a, "a", 30)?;
    content(&mut d, &a)
}
pub fn wake_registry() -> CaseResult {
    let (mut d, a) = one()?;
    parked(&d)?;
    let mut b = Client::unregistered(23)?;
    d.shown(&a)?;
    time::sleep_ms(200)?;
    b.command(REGISTER, 0, 0)?;
    d.shown(&b)?.client(&b)
}
pub fn stress_clients() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let mut device = Virtual::create()?;
    let mut d = Desktop::start()?;
    let mut clients = (0..8)
        .map(|n| Client::start(11 + n))
        .collect::<Result<Vec<_>, _>>()?;
    measure::acknowledged(0)?;
    let mut receipt_serial = 0;
    for n in 1..=300 {
        device.batch(&[(1, 30, 1), (1, 30, 0)], n as u64 * 16_000_000)?;
        receipt_serial = receipt_serial.max(measure::epoch(&mut clock, &mut clients, n, true)?);
        clients[7].await_event("stress epoch input processed", |e| {
            e.kind == kind::KEY_PRESS && e.keycode == 30
        })?;
        clients[7].clear()?;
    }
    measure::observed(&mut d, &clients.iter().collect::<Vec<_>>(), receipt_serial)?;
    Ok(())
}
pub fn memory_bounded() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let mut d = Desktop::start()?;
    let mut clients = (0..8)
        .map(|n| Client::start(11 + n))
        .collect::<Result<Vec<_>, _>>()?;
    measure::acknowledged(0)?;
    let mut receipt_serial = 0;
    for n in 1..=60 {
        receipt_serial = receipt_serial.max(measure::epoch(&mut clock, &mut clients, n, true)?);
    }
    let before = rss(d.wm.pid)?;
    for n in 61..=660 {
        receipt_serial = receipt_serial.max(measure::epoch(&mut clock, &mut clients, n, true)?);
    }
    measure::observed(&mut d, &clients.iter().collect::<Vec<_>>(), receipt_serial)?;
    let after = rss(d.wm.pid)?;
    value(
        "resident-growth",
        after - before,
        "kb",
        Some((i64::MIN, 64)),
    );
    check(
        after <= before + 64,
        "bwm resident memory grew more than 64 KiB after warmup",
    )
}
fn rss(pid: i32) -> Result<i64, String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| e.to_string())?;
    text.lines()
        .find_map(|s| s.strip_prefix("VmRSS:"))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .ok_or("VmRSS is missing".into())
}

pub fn present_latency() -> CaseResult {
    measure::receipts()?;
    let (mut d, mut c) = one()?;
    c.command(RESIZE, 256, 256)?;
    d.shown(&c)?;
    let mut samples = Vec::new();
    let mut receipt_serial = 0;
    for n in 0..330 {
        c.command(PAINT, n, 0)?;
        let r = measure::completed_commit(&c, c.commit)?;
        receipt_serial = receipt_serial.max(r.serial);
        check(r.ns >= c.commit_ns, "composite receipt predates the commit")?;
        if n >= 30 {
            samples.push((r.ns - c.commit_ns) / 1000);
        }
    }
    measure::observed(&mut d, &[&c], receipt_serial)?;
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    let max = *samples.last().unwrap();
    value("present-median", median as i64, "us", Some((0, 16000)));
    value("present-max", max as i64, "us", Some((0, 33000)));
    check(
        median <= 16000 && max <= 33000,
        "commit-to-observed-composite latency exceeds median 16 ms or max 33 ms",
    )
}
pub fn frame_floor() -> CaseResult {
    measure::receipts()?;
    let (mut d, mut c) = one()?;
    c.command(ANIMATE, 1, 0)?;
    time::sleep_ms(1000)?;
    let begin = now();
    time::sleep_ms(5000)?;
    let end = now();
    c.command(ANIMATE, 0, 0)?;
    let receipts = measure::receipts()?;
    let mut generations = std::collections::BTreeSet::new();
    for r in receipts
        .into_iter()
        .filter(|r| r.window == c.id && r.ns >= begin && r.ns < end)
    {
        generations.insert(r.generation);
    }
    let frames = generations.len();
    let elapsed = end - begin;
    d.shown(&c)?;
    value(
        "composites-per-second",
        (frames as u64 * 1_000_000_000 / elapsed) as i64,
        "hz",
        Some((60, i64::MAX)),
    );
    check(
        frames as u64 * 1_000_000_000 >= 60 * elapsed,
        "one client did not receive 60 completed composites per second",
    )
}
fn input_latency(pointer: bool, animation: bool) -> CaseResult {
    measure::receipts()?;
    let mut device = Virtual::create()?;
    let mut d = Desktop::start()?;
    let mut animators = Vec::new();
    if animation {
        for n in 0..3 {
            let mut c = Client::start(60 + n)?;
            c.command(ANIMATE, 1, 0)?;
            animators.push(c);
        }
    }
    let mut c = Client::start(11)?;
    d.shown(&c)?;
    if animation {
        c.command(ANIMATE, 1, 0)?;
    }
    let mut samples = Vec::new();
    let mut receipt_serial = 0;
    let w = c.info()?;
    for n in 0..110 {
        c.clear()?;
        let begin = now();
        if pointer {
            let x = (w.x + 20 + (n % 100)) as u64;
            let y = (w.y + 40) as u64;
            device.batch(
                &[
                    (3, 0, (x * 32767 / (d.width - 1) as u64) as i32),
                    (3, 1, (y * 32767 / (d.height - 1) as u64) as i32),
                ],
                begin,
            )?;
        } else {
            device.batch(&[(1, 30, 1), (1, 30, 0)], begin)?;
        }
        c.await_event("timed input delivered through bwm", |e| {
            e.kind
                == if pointer {
                    kind::POINTER_MOVE
                } else {
                    kind::KEY_PRESS
                }
        })?;
        let r = measure::completed_commit(&c, c.handling_commit)?;
        receipt_serial = receipt_serial.max(r.serial);
        check(r.ns >= begin, "completion predates injected input")?;
        if n >= 10 {
            samples.push((r.ns - begin) / 1000);
        }
        let spent = (now() - begin) / 1_000_000;
        if spent < 50 {
            time::sleep_ms(50 - spent)?;
        }
    }
    measure::observed(&mut d, &[&c], receipt_serial)?;
    samples.sort_unstable();
    let median = samples[50];
    let max = *samples.last().unwrap();
    let (median_bound, max_bound) = if animation {
        (33000, 100000)
    } else {
        (20000, 50000)
    };
    value(
        "latency-median",
        median as i64,
        "us",
        Some((0, median_bound)),
    );
    value("latency-max", max as i64, "us", Some((0, max_bound)));
    check(
        median <= median_bound as u64 && max <= max_bound as u64,
        "input-to-composited-ack latency exceeds the GUI plan bound",
    )
}
pub fn typing_latency() -> CaseResult {
    input_latency(false, false)
}
pub fn pointer_latency() -> CaseResult {
    input_latency(true, false)
}
pub fn animation_latency() -> CaseResult {
    input_latency(false, true)
}
pub fn clipboard_latency() -> CaseResult {
    let (_d, mut a, mut b) = twin()?;
    let text = vec![b'x'; 65536];
    let mut maximum = 0;
    for n in 0..20 {
        let begin = now();
        let (serial, _) = clip_ok(&mut a, true, &text)?;
        let (got, bytes) = clip_ok(&mut b, false, &[])?;
        check(
            serial == got && text == bytes,
            "timed clipboard transfer changed bytes or serial",
        )?;
        if n >= 2 {
            maximum = maximum.max(now() - begin);
        }
    }
    value(
        "clipboard-max",
        (maximum / 1000) as i64,
        "us",
        Some((0, 50000)),
    );
    check(
        maximum <= 50_000_000,
        "64 KiB cross-process clipboard latency exceeded 50 ms",
    )
}
pub fn idle_cpu() -> CaseResult {
    let (mut d, a) = one()?;
    let b = Client::start(23)?;
    let c = Client::start(31)?;
    d.shown(&c)?;
    time::sleep_ms(1000)?;
    let pids = [d.wm.pid, a.child.pid, b.child.pid, c.child.pid];
    let before = pids
        .iter()
        .map(|p| cpu_ticks(*p))
        .collect::<Result<Vec<_>, _>>()?;
    let start = now();
    time::sleep_ms(5000)?;
    let elapsed = now() - start;
    let after = pids
        .iter()
        .map(|p| cpu_ticks(*p))
        .collect::<Result<Vec<_>, _>>()?;
    let used = after.iter().zip(before).map(|(a, b)| a - b).sum::<u64>();
    // proc stat ticks are USER_HZ=100 (the proc ABI); 1 tick is 10 ms of one CPU.
    value(
        "idle-cpu",
        (used * 1_000_000_000 / elapsed) as i64,
        "percent",
        Some((0, 2)),
    );
    check(
        used * 10_000_000 * 100 <= elapsed * 2,
        "idle desktop exceeded 2% of one CPU",
    )?;
    drop((a, b, c));
    Ok(())
}
fn cpu_ticks(pid: i32) -> Result<u64, String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    let tail = text
        .rsplit_once(')')
        .ok_or("malformed proc stat")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let user = tail
        .get(11)
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("proc stat utime missing")?;
    let sys = tail
        .get(12)
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("proc stat stime missing")?;
    Ok(user + sys)
}
