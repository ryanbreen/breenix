use super::kit::*;

pub fn click_raises() -> CaseResult {
    let (mut d, mut a, b) = twin()?;
    d.focus(&mut a)?;
    check(
        a.info()?.z > b.info()?.z,
        "click did not raise the target by stable window id",
    )?;
    content(&mut d, &a)
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

pub fn raise_stable_id() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    let c = Client::start(41)?;
    placed(&c)?;
    d.client_ready(&c)?;
    b.send(EXIT, 0, 0)?;
    until(1000, "middle stable id removed", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    d.focus(&mut a)?;
    check(
        a.info()?.z > c.info()?.z,
        "raising surviving id addressed an old index",
    )?;
    content(&mut d, &a)
}
pub fn appbar_order() -> CaseResult {
    let (mut d, mut a, b) = twin()?;
    d.focus(&mut a)?;
    appbar(&d, b.id)?;
    until(1000, "creation-order second appbar target", || focused(&b))?;
    check(
        a.info()?.z < b.info()?.z,
        "second appbar entry followed stack order",
    )?;
    content(&mut d, &b)
}
pub fn minimize_no_input() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    minimize(&d, &b)?;
    b.clear()?;
    a.clear()?;
    v.batch(&[(1, 30, 1), (1, 30, 0)], 100)?;
    let es = stream.read_from(3, &[v.id])?;
    a.await_event("eligible survivor receives key", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    let w = b.info()?;
    d.click(w.x + 20, w.y + 20)?;
    b.snapshot()?;
    check(
        !b.events
            .iter()
            .any(|e| matches!(e.kind, kind::KEY_PRESS | kind::POINTER_BUTTON)),
        "minimized client received input",
    )?;
    let rows = ledger()?;
    check(
        !rows.iter().any(|r| {
            r.first().is_some_and(|s| s == &es[0].serial.to_string())
                && r.get(2).is_some_and(|s| s == &b.id.to_string())
        }),
        "key disposition targeted minimized client",
    )
}
pub fn minimize_close() -> CaseResult {
    let (mut d, a, mut b) = twin()?;
    minimize(&d, &b)?;
    let before = listed()?.len();
    b.send(EXIT, 0, 0)?;
    until(1000, "minimized owner removed", || {
        Ok(listed()?.len() + 1 == before)
    })?;
    appbar(&d, a.id)?;
    until(1000, "remaining appbar entry eligible", || focused(&a))?;
    content(&mut d, &a)
}
pub fn fkeys_skip_minimized() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    minimize(&d, &a)?;
    d.focus(&mut b)?;
    a.clear()?;
    b.clear()?;
    // A key behind F1 on the same keyboard is a positive routing barrier.
    hostkeys("f1,a")?;
    b.await_event("key following F1 reaches the eligible client", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    a.snapshot()?;
    check(
        !focused(&a)? && focused(&b)? && !a.events.iter().any(|e| e.kind == kind::FOCUS_GAINED),
        "function key selected minimized window",
    )
}
pub fn chromeless_top() -> CaseResult {
    let mut d = Desktop::start()?;
    let c = Client::start_mode(77, flags::CHROMELESS | flags::IGNORE_CLOSE)?;
    d.client_ready(&c)?;
    let mut normal = Client::start(11)?;
    placed(&normal)?;
    d.client_ready(&normal)?;
    d.focus(&mut normal)?;
    check(
        c.info()?.z > normal.info()?.z,
        "normal window raised above chromeless layer",
    )?;
    content(&mut d, &c)
}
pub fn dimmer_only_with_chromeless() -> CaseResult {
    let mut d = Desktop::start()?;
    parked(&d)?;
    let before = Screen::read()?;
    let x = d.width as i32 - 100;
    let y = 100;
    let original = before.pixel(x, y)?;
    let mut c = Client::start_mode(77, flags::CHROMELESS | flags::IGNORE_CLOSE)?;
    d.client_ready(&c)?;
    let dim = d.shown(&c)?.pixel(x, y)?;
    let half = 0xff000000
        | (((original >> 16) & 255) / 2) << 16
        | (((original >> 8) & 255) / 2) << 8
        | (original & 255) / 2;
    check(
        dim == half,
        "chromeless dimmer does not halve desktop channels",
    )?;
    c.send(EXIT, 0, 0)?;
    until(1000, "chromeless exit removes dimmer", || {
        Ok(!listed()?.iter().any(|w| w.id == c.id))
    })?;
    until(1000, "undimmed desktop repainted", || {
        Ok(Screen::read()?.pixel(x, y)? == original)
    })?;
    Ok(())
}
pub fn launcher_toggle() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let _d = Desktop::start()?;
    let mut v = Virtual::create()?;
    let mut n = 0;
    for expected in [true, false] {
        for stamp in [n, n + 200_000_000] {
            v.batch(&[(1, 125, 1), (1, 125, 0)], stamp)?;
            clock.step(stamp / 1_000_000 + 1, stamp)?;
            measure::acknowledged(stamp / 1_000_000 + 1)?;
        }
        until(2000, "launcher toggle visibility", || {
            Ok(listed()?.iter().any(|w| w.title.first() == Some(&1)) == expected)
        })?;
        n += 1_000_000_000;
    }
    Ok(())
}
pub fn close_focus_next() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    a.clear()?;
    b.send(EXIT, 0, 0)?;
    a.await_event("top close focuses next stack id", |e| {
        e.kind == kind::FOCUS_GAINED
    })?;
    check(focused(&a)?, "next visible stack id not focused")?;
    content(&mut d, &a)
}
pub fn new_on_top() -> CaseResult {
    let (mut d, a) = one()?;
    let mut b = Client::start(23)?;
    placed(&b)?;
    d.client_ready(&b)?;
    b.await_event("new top focused", |e| e.kind == kind::FOCUS_GAINED)?;
    check(
        b.info()?.z > a.info()?.z && focused(&b)? && !focused(&a)?,
        "new window did not become focused top",
    )?;
    content(&mut d, &b)
}
fn model(steps: usize, seed: u32) -> CaseResult {
    let mut d = Desktop::start()?;
    let mut clients = Vec::new();
    let mut stack = Vec::<u64>::new();
    let mut minimized = Vec::<u64>::new();
    let mut random = seed;
    for seed in [11, 23, 41] {
        let c = Client::start(seed)?;
        placed(&c)?;
        d.client_ready(&c)?;
        stack.push(c.id);
        clients.push(c);
    }
    for step in 0..steps {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        let index = random as usize % clients.len();
        let id = clients[index].id;
        match (random >> 8) % 4 {
            0 => {
                if !minimized.contains(&id) {
                    d.focus(&mut clients[index])?;
                    stack.retain(|&n| n != id);
                    stack.push(id);
                }
            }
            1 => {
                if !minimized.contains(&id) {
                    d.focus(&mut clients[index])?;
                    stack.retain(|&n| n != id);
                    stack.push(id);
                    minimize(&d, &clients[index])?;
                    minimized.push(id);
                }
            }
            2 => {
                if minimized.contains(&id) {
                    appbar(&d, id)?;
                    until(1000, "model restore", || {
                        Ok(clients[index].info()?.flags & win_flags::MINIMIZED == 0)
                    })?;
                    minimized.retain(|&n| n != id);
                    stack.retain(|&n| n != id);
                    stack.push(id);
                }
            }
            _ => {
                if clients.len() > 1 {
                    clients[index].send(EXIT, 0, 0)?;
                    until(1000, "model close", || {
                        Ok(!listed()?.iter().any(|w| w.id == id))
                    })?;
                    clients.remove(index);
                    stack.retain(|&n| n != id);
                    minimized.retain(|&n| n != id);
                } else {
                    let c = Client::start(60 + step as u32)?;
                    placed(&c)?;
                    d.client_ready(&c)?;
                    stack.push(c.id);
                    clients.push(c);
                }
            }
        }
        let mut registry = listed()?;
        registry.sort_by_key(|w| w.z);
        check(
            registry.iter().map(|w| w.id).collect::<Vec<_>>() == stack,
            &format!("registry differs from operation model at step {step}"),
        )?;
        check(
            registry
                .iter()
                .all(|w| (w.flags & win_flags::MINIMIZED != 0) == minimized.contains(&w.id)),
            "registry visibility differs from model",
        )?;
        if let Some(id) = stack.iter().rev().find(|id| !minimized.contains(id)) {
            let c = clients
                .iter_mut()
                .find(|c| c.id == *id)
                .ok_or("model lost client")?;
            check(focused(c)?, "model top eligible id not focused")?;
            content(&mut d, c)?;
            c.clear()?;
            key(c, "a", 30)?;
        }
    }
    Ok(())
}
pub fn registry_agrees() -> CaseResult {
    model(16, 0x6f37a201)
}
pub fn model_sequence() -> CaseResult {
    model(50, 0xe1800001)
}
pub fn minimize_all_restore_all() -> CaseResult {
    let (mut d, a, b) = twin()?;
    let original = [a.info()?.id, b.info()?.id];
    let before = d.shown(&b)?;
    minimize(&d, &b)?;
    minimize(&d, &a)?;
    appbar(&d, a.id)?;
    appbar(&d, b.id)?;
    until(1000, "restore both", || {
        Ok(a.info()?.flags & win_flags::MINIMIZED == 0
            && b.info()?.flags & win_flags::MINIMIZED == 0)
    })?;
    let mut actual = listed()?;
    actual.sort_by_key(|w| w.z);
    check(
        actual.iter().map(|w| w.id).collect::<Vec<_>>() == original,
        "restore-all changed original stack",
    )?;
    parked(&d)?;
    let after = d.shown(&b)?;
    let aw = a.info()?;
    let bw = b.info()?;
    for y in aw.y..bw.y + bw.height as i32 {
        for x in aw.x..bw.x + bw.width as i32 {
            check(
                before.pixel(x, y)? == after.pixel(x, y)?,
                "restore-all did not reproduce original window scene",
            )?;
        }
    }
    Ok(())
}
pub fn titlebar_drag_raises() -> CaseResult {
    let (mut d, a, b) = twin()?;
    let w = a.info()?;
    drag(&d, (w.x + 30, w.y - 16 * d.scale), (w.x + 70, w.y + 20))?;
    check(
        a.info()?.z > b.info()?.z,
        "titlebar drag did not raise lower stable id",
    )?;
    content(&mut d, &a)
}
