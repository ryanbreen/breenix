use super::kit::*;

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
    super::startup::claims_display()
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

pub fn close_one_of_two() -> CaseResult {
    let mut d = Desktop::start()?;
    let mut c = Client::start_mode(11, flags::TWO_WINDOWS)?;
    let first = c.id;
    let second = c.second;
    d.client_ready(&c)?;
    let w = c.info()?;
    d.click(w.x + w.width as i32 - 15 * d.scale, w.y - 16 * d.scale)?;
    until(1000, "one-of-two close removes only target", || {
        Ok(!listed()?.iter().any(|w| w.id == first))
    })?;
    check(
        c.child.status()?.is_none() && listed()?.iter().any(|w| w.id == second),
        "closing one window killed the shared owner or its sibling",
    )?;
    c.snapshot()?;
    check(c.id == second, "client did not retain its other window")?;
    content(&mut d, &c)
}
fn visible_launch_error(d: &mut Desktop, needle: &str) -> CaseResult {
    let mut error = None;
    until(1000, "visible launch error", || {
        error = listed()?.into_iter().find(|w| {
            String::from_utf8_lossy(&w.title[..w.title_len as usize])
                .to_lowercase()
                .contains(needle)
        });
        Ok(error.as_ref().is_some_and(|w| w.commit_id != 0))
    })?;
    let w = error.ok_or("launch error window absent")?;
    let s = Screen::read()?;
    check(
        s.contains(w.id, w.commit_id),
        "launch error title exists without a composited error frame",
    )?;
    check(d.wm.status()?.is_none(), "launch failure terminated bwm")
}
pub fn launch_missing() -> CaseResult {
    let _config = FileOverride::new(
        "/etc/hotkeys.conf",
        b"super+k = exec /bin/wm-suite-missing-program\n",
    )?;
    let mut d = Desktop::start()?;
    let before = children(d.wm.pid, "")?;
    hostkeys("super-k")?;
    visible_launch_error(&mut d, "missing")?;
    check(
        children(d.wm.pid, "")? == before,
        "missing launch left an unreaped child",
    )
}
pub fn launch_fork_fails() -> CaseResult {
    let _config = FileOverride::new(
        "/etc/hotkeys.conf",
        b"super+k = exec /usr/libexec/breenix/wm-client_test\n",
    )?;
    let mut d = Desktop::start()?;
    let before = libbreenix::test_control::fired(abi::tc_point::LAUNCH_FORK_FAIL)
        .map_err(|e| e.to_string())?;
    libbreenix::test_control::arm(abi::tc_point::LAUNCH_FORK_FAIL, d.wm.pid as u64, 0)
        .map_err(|e| e.to_string())?;
    hostkeys("super-k")?;
    until(1000, "launch fork failure point fired", || {
        Ok(
            libbreenix::test_control::fired(abi::tc_point::LAUNCH_FORK_FAIL)
                .map_err(|e| e.to_string())?
                == before + 1,
        )
    })?;
    let _ = libbreenix::test_control::disarm_all();
    visible_launch_error(&mut d, "fork")?;
    let before = listed()?;
    hostkeys("super-k")?;
    until(5000, "launch after one-shot fork failure", || {
        Ok(listed()?
            .iter()
            .any(|w| !before.iter().any(|b| b.id == w.id) && w.commit_id != 0))
    })?;
    Ok(())
}
pub fn launch_close_50() -> CaseResult {
    let _config = FileOverride::new(
        "/etc/hotkeys.conf",
        b"super+k = exec /usr/libexec/breenix/wm-client_test\n",
    )?;
    let d = Desktop::start()?;
    let baseline = listed()?;
    let processes = children(d.wm.pid, "")?;
    let memory = rss(d.wm.pid)?;
    for n in 0..50 {
        hostkeys("super-k")?;
        let mut launched = None;
        until(5000, "cycle launch committed", || {
            launched = listed()?
                .into_iter()
                .find(|w| !baseline.iter().any(|b| b.id == w.id) && w.commit_id != 0);
            Ok(launched.is_some())
        })?;
        let w = launched.ok_or("launched client missing")?;
        let s = Screen::read()?;
        check(
            s.contains(w.id, w.commit_id),
            &format!("cycle {n} launch not on device scanout"),
        )?;
        d.click(w.x + w.width as i32 - 15 * d.scale, w.y - 16 * d.scale)?;
        until(2000, "cycle close reaped and removed", || {
            Ok(listed()?.len() == baseline.len() && children(d.wm.pid, "")? == processes)
        })?;
    }
    check(
        rss(d.wm.pid)? <= memory + 64,
        "fifty launches retained more than 64 KiB resident memory",
    )
}
pub fn read_other_denied() -> CaseResult {
    let (_d, c) = one()?;
    let id = c.id;
    child_check(|| {
        if unsafe { libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::SETUID, 1000) }
            as i64
            != 0
        {
            return 1;
        }
        let mut pixels = vec![0u8; 240 * 160 * 4];
        let mut a = abi::Capture::ZERO;
        a.id = id;
        a.dst = pixels.as_mut_ptr() as u64;
        a.dst_len = pixels.len() as u64;
        match window::call(abi::op::CAPTURE, &mut a) {
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EPERM)) => 0,
            _ => 2,
        }
    })
}
pub fn huge_window() -> CaseResult {
    let (mut d, mut c) = one()?;
    c.create_window(73, 4096, 4096, 2)?;
    d.client_ready(&c)?;
    let w = c.info()?;
    check(
        w.x - 2 * d.scale < d.width as i32 - 32 * d.scale
            && w.x + w.width as i32 + 2 * d.scale >= 32 * d.scale
            && w.y - 34 * d.scale >= 28 * d.scale,
        "huge window lost its reachable titlebar",
    )?;
    visible_content(&mut d, &c)?;
    key(&mut c, "a", 30)?;
    Ok(())
}
pub fn commit_flood() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    a.command(ANIMATE, 1, 0)?;
    d.focus(&mut b)?;
    b.clear()?;
    let start = now();
    key(&mut b, "a", 30)?;
    check(
        now() - start <= 100_000_000,
        "commit flood delayed another owner's key beyond 100 ms",
    )?;
    a.command(ANIMATE, 0, 0)?;
    content(&mut d, &b)
}
pub fn title_flood() -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut b)?;
    for n in 0..10_000 {
        a.title(format!("flood title {n:05}").as_bytes())?;
        if n % 1000 == 999 {
            b.clear()?;
            key(&mut b, "a", 30)?;
        }
    }
    check(
        a.info()?.title.starts_with(b"flood title 09999"),
        "final title lost after flood",
    )?;
    content(&mut d, &b)
}
pub fn never_reads() -> CaseResult {
    let mut stream = Stream::open()?;
    let mut v = Virtual::create()?;
    let (mut d, mut a, mut b) = twin()?;
    d.focus(&mut a)?;
    a.command(SET_FLAGS, flags::IGNORE_CLOSE | flags::NEVER_READ, 0)?;
    let mut sources = Vec::new();
    for n in 1..=300 {
        v.batch(&[(1, 30, 1), (1, 30, 0)], n)?;
        sources.extend(
            stream
                .read_from(3, &[v.id])?
                .into_iter()
                .filter(|e| e.kind == 1)
                .map(|e| e.serial),
        );
    }
    let rows = ledger()?;
    check(
        sources.iter().any(|serial| {
            rows.iter().any(|r| {
                r.first().is_some_and(|s| s == &serial.to_string())
                    && r.get(1).is_some_and(|s| s == "lost")
                    && r.get(2).is_some_and(|s| s == &a.id.to_string())
            })
        }),
        "unread client ring lost no recorded event",
    )?;
    d.focus(&mut b)?;
    b.clear()?;
    v.batch(&[(1, 48, 1), (1, 48, 0)], 301)?;
    let es = stream.read_from(3, &[v.id])?;
    b.await_event("healthy peer survives unread owner", |e| {
        e.kind == kind::KEY_PRESS && e.keycode == 48
    })?;
    disposition(es[0].serial, "delivered", None)
}
pub fn normal_exit_gives_back() -> CaseResult {
    let before = Screen::read()?;
    let mut d = Desktop::start()?;
    d.wm.signal(signal::SIGTERM)?;
    let mut ended = None;
    until(1000, "graceful bwm exit", || {
        ended = d.wm.status()?;
        Ok(ended.is_some())
    })?;
    check(ended == Some(0), "bwm did not exit normally on SIGTERM")?;
    let after = Screen::read()?;
    check(
        before.pixels == after.pixels,
        "graceful bwm exit did not restore prior display pixels",
    )
}
pub fn window_limit() -> CaseResult {
    let (mut d, mut c) = one()?;
    let first = c.id;
    let mut ids = vec![first];
    for n in 1..16 {
        c.create_window(11 + n, 240, 160, 2)?;
        ids.push(c.id);
    }
    let before = listed()?;
    let refusal = c
        .create_window(99, 240, 160, 2)
        .err()
        .ok_or("seventeenth window unexpectedly created")?;
    check(
        refusal.contains("EAGAIN") || refusal.contains("errno 11"),
        "seventeenth CREATE did not return EAGAIN",
    )?;
    let after = listed()?;
    check(
        ids.len() == 16
            && ids.iter().all(|id| after.iter().any(|w| w.id == *id))
            && before.len() == after.len(),
        "window-limit refusal changed the original sixteen",
    )?;
    c.select(0)?;
    d.focus(&mut c)?;
    key(&mut c, "a", 30)?;
    content(&mut d, &c)
}
