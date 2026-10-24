use super::kit::*;

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

pub fn death_releases() -> CaseResult {
    let stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    v.batch(&[(1, 42, 1), (1, 30, 1), (1, 0x110, 1)], 100)?;
    c.await_event("held key before compositor death", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 30
    })?;
    let old = status()?.claim_epoch;
    d.wm.stop()?;
    let _claim = InputClaim::new()?;
    stream.grab(&[v.id])?;
    drop(_claim);
    let mut next = Desktop::start()?;
    let s = status()?;
    check(
        s.claim_epoch > old,
        "new compositor reused dead claim epoch",
    )?;
    next.focus(&mut c)?;
    c.clear()?;
    v.batch(&[(1, 48, 1), (1, 48, 0)], 200)?;
    let e = c.await_event("fresh key after grab death reset", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 48
    })?;
    check(e.modifiers == 0, "dead compositor retained held modifier")?;
    c.require_alive()?;
    content(&mut next, &c)
}
pub fn blocked_acquire() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.create_window(47, 240, 160, 2)?;
    d.client_ready(&c)?;
    content(&mut d, &c)?;
    let fired = libbreenix::test_control::fired(abi::tc_point::COMPOSITE_STALL)
        .map_err(|e| e.to_string())?;
    libbreenix::test_control::arm(abi::tc_point::COMPOSITE_STALL, c.id, 0)
        .map_err(|e| e.to_string())?;
    c.command(PAINT, 48, 0)?;
    until(
        1000,
        "compositor pinned the two-buffer client's front",
        || {
            Ok(
                libbreenix::test_control::fired(abi::tc_point::COMPOSITE_STALL)
                    .map_err(|e| e.to_string())?
                    == fired + 1,
            )
        },
    )?;
    c.command(PAINT, 49, 0)?;
    c.send(PAINT, 50, 0)?;
    time::sleep_ms(20).map_err(|e| e.to_string())?;
    check(
        !c.response_ready()?,
        "ACQUIRE returned while both buffers were retained",
    )?;
    let start = now();
    d.wm.stop()?;
    c.finish_command()?;
    check(
        now() - start <= 250_000_000,
        "blocked ACQUIRE did not return within 250 ms of compositor death",
    )?;
    c.require_alive()?;
    let _ = libbreenix::test_control::disarm_all();
    let mut next = Desktop::start()?;
    content(&mut next, &c)
}
pub fn blocked_wait() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.clear()?;
    c.send(WAIT_EVENTS, 0, 0)?;
    until(1000, "client sleeping inside blocking READ_EVENTS", || {
        let text = std::fs::read_to_string(format!("/proc/{}/status", c.child.pid))
            .map_err(|e| e.to_string())?;
        Ok(text
            .lines()
            .any(|l| l.starts_with("State:") && l.contains('S')))
    })?;
    check(
        !c.response_ready()?,
        "blocking event wait returned before compositor death",
    )?;
    let start = now();
    d.wm.stop()?;
    c.finish_command()?;
    check(
        now() - start <= 250_000_000,
        "blocked event wait did not return within 250 ms of compositor death",
    )?;
    check(
        c.events.iter().any(|e| e.kind == kind::COMPOSITOR_CHANGED),
        "event wait woke without compositor-death notification",
    )?;
    c.require_alive()?;
    Ok(())
}
pub fn death_at_composite() -> CaseResult {
    let (mut d, mut c) = one()?;
    let before = libbreenix::test_control::fired(abi::tc_point::COMPOSITE_STALL)
        .map_err(|e| e.to_string())?;
    libbreenix::test_control::arm(abi::tc_point::COMPOSITE_STALL, c.id, 0)
        .map_err(|e| e.to_string())?;
    c.command(PAINT, 47, 0)?;
    until(1000, "real composite entered planted pause", || {
        Ok(
            libbreenix::test_control::fired(abi::tc_point::COMPOSITE_STALL)
                .map_err(|e| e.to_string())?
                == before + 1,
        )
    })?;
    d.wm.stop()?;
    let _ = libbreenix::test_control::disarm_all();
    c.require_alive()?;
    let mut next = Desktop::start()?;
    c.command(PAINT, 48, 0)?;
    content(&mut next, &c)
}
pub fn death_mid_configure() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.command(REFUSE, 1, 0)?;
    let old = c.info()?;
    drag(
        &d,
        (old.x + old.width as i32 + 1, old.y + 70),
        (old.x + old.width as i32 + 61, old.y + 70),
    )?;
    c.await_event("request pending at compositor death", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    d.wm.stop()?;
    c.require_alive()?;
    check(
        (c.info()?.width, c.info()?.height) == (old.width, old.height),
        "pending resize changed owner size at death",
    )?;
    let mut next = Desktop::start()?;
    content(&mut next, &c)?;
    let got = c.info()?;
    check(
        (got.width, got.height) == (old.width, old.height),
        "restart adopted unaccepted requested size",
    )
}
pub fn death_mid_capture() -> CaseResult {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    let w = c.info()?;
    d.pointer(w.x + 40, w.y + 40, 1, 0)?;
    c.await_event("capture before death", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 1
    })?;
    d.wm.stop()?;
    c.require_alive()?;
    let next = Desktop::start()?;
    c.clear()?;
    next.pointer(w.x + w.width as i32 + 100, w.y + 40, 1, 0)?;
    next.pointer(w.x + w.width as i32 + 100, w.y + 40, 0, 0)?;
    // Wait for a later motion on the same device before inspecting release delivery.
    next.pointer(w.x + 60, w.y + 60, 0, 0)?;
    c.await_event("post-restart pointer motion after outside release", |e| {
        e.kind == kind::POINTER_MOVE && e.x == 60 && e.y == 60
    })?;
    check(
        !c.events
            .iter()
            .any(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 0),
        "restart inherited dead compositor capture",
    )?;
    next.click(w.x + 60, w.y + 60)?;
    c.await_event("fresh capture releases after restart", |e| {
        e.kind == kind::POINTER_BUTTON && e.pressed == 0
    })?;
    check(
        c.events
            .iter()
            .filter(|e| e.kind == kind::POINTER_BUTTON && e.pressed == 0)
            .count()
            == 1,
        "fresh capture must release exactly once",
    )
}
pub fn focus_restored() -> CaseResult {
    let (mut d, mut a, b) = twin()?;
    d.focus(&mut a)?;
    d.wm.stop()?;
    a.require_alive()?;
    let mut next = Desktop::start()?;
    until(1000, "restart restores stable focus id", || {
        Ok(focused(&a)? && !focused(&b)?)
    })?;
    a.clear()?;
    key(&mut a, "a", 30)?;
    content(&mut next, &a)
}
pub fn clipboard_reset() -> CaseResult {
    let (mut d, mut c) = one()?;
    clip_ok(&mut c, true, b"old claim text")?;
    d.wm.stop()?;
    c.require_alive()?;
    let _next = Desktop::start()?;
    let (code, _, text) = c.clipboard(false, &[])?;
    check(
        code == 0 && text.is_empty(),
        "new claim retained old clipboard",
    )
}
pub fn supervisor_gives_up() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let mut d = Desktop::launch(true)?;
    for n in 0..5 {
        let pid = children(d.wm.pid, "bwm")?
            .first()
            .copied()
            .ok_or("supervisor stopped before five failures")?;
        signal::kill(pid, signal::SIGKILL)?;
        clock.step(n + 1, (n + 1) * 1_000_000_000)?;
        if n < 4 {
            until(2000, "bounded supervisor restarts before limit", || {
                Ok(children(d.wm.pid, "bwm")?.iter().any(|p| *p != pid))
            })?;
        }
    }
    until(2000, "supervisor stops at five failures", || {
        Ok(children(d.wm.pid, "bwm")?.is_empty())
    })?;
    clock.step(6, 6_000_000_000)?;
    time::sleep_ms(250).map_err(|e| e.to_string())?;
    check(
        children(d.wm.pid, "bwm")?.is_empty(),
        "crash-loop supervisor kept restarting after limit",
    )?;
    check(
        d.output().contains("[init] bwm gave up: 5 failures in 10 s; stopped"),
        "supervisor stopped bwm without reporting the give-up limit",
    )
}
pub fn unexpected_crash_fails() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.command(SET_FLAGS, flags::IGNORE_CLOSE | flags::CRASH_AFTER, 1)?;
    c.clear()?;
    d.wm.stop()?;
    until(
        1000,
        "planted app crash occurs during compositor restart",
        || {
            let text = std::fs::read_to_string(format!("/proc/{}/status", c.child.pid))
                .map_err(|e| e.to_string())?;
            Ok(text
                .lines()
                .any(|l| l.starts_with("State:") && l.contains('Z')))
        },
    )?;
    c.collect_events()?;
    check(
        c.logged
            .iter()
            .any(|e| e.event.kind == kind::COMPOSITOR_CHANGED),
        "crash fixture did not reach compositor-death event",
    )?;
    let diagnostic = c
        .require_alive()
        .err()
        .ok_or("recovery scenario accepted unexpected app crash")?;
    check(
        diagnostic.contains("unexpected client exit") && diagnostic.contains("wait status 11"),
        "recovery failure did not identify unexpected app SIGSEGV",
    )
}
