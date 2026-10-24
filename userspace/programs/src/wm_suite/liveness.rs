use super::kit::*;

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

/// W14: compositor waits use the v2 scene descriptor on ARM64.
pub fn compositor_wait_timeout() -> CaseResult {
    use libbreenix::{io, types::Fd};
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut c = Client::start_mode(11, flags::IGNORE_CLOSE | flags::NEVER_READ)?;
    let _claim = InputClaim::new()?;
    let raw = window::call(abi::op::EVENT_FD, &mut abi::WinId { id: 0 })?;
    // SAFETY: EVENT_FD returns a new descriptor owned by this file.
    let scene = unsafe { std::fs::File::from_raw_fd(raw as i32) };
    let mut fds = [io::PollFd { fd: scene.as_raw_fd(), events: io::poll_events::POLLIN, revents: 0 }];
    // Drain any publication completing after the descriptor was created.
    if io::poll(&mut fds, 0)? != 0 {
        let mut changes = [0; 8];
        check(io::read(Fd::from_raw(scene.as_raw_fd() as u64), &mut changes)? == 8,
              "scene descriptor did not return its change total")?;
    }
    const TIMEOUT_MS: i32 = 100;
    let begin = now();
    let ready = io::poll(&mut fds, TIMEOUT_MS)?;
    let elapsed = now() - begin;
    value("wait-time", (elapsed / 1000) as i64, "us", Some((100000, 200000)));
    check(ready == 0 && elapsed >= 100_000_000 && elapsed <= 200_000_000,
          "idle scene poll returned before its timeout or over 100 ms late")?;
    c.send(PAINT, 23, 0)?;
    let begin = now();
    let ready = io::poll(&mut fds, TIMEOUT_MS)?;
    let elapsed = now() - begin;
    check(ready == 1 && fds[0].revents & io::poll_events::POLLIN != 0 && elapsed < 100_000_000,
          "client commit did not wake scene poll before its timeout")?;
    c.receive()?;
    let mut changes = [0; 8];
    check(io::read(Fd::from_raw(scene.as_raw_fd() as u64), &mut changes)? == 8
          && u64::from_le_bytes(changes) > 0, "scene wake omitted its change total")
}
