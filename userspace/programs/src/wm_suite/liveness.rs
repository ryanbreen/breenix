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
    let mut fds = [io::PollFd {
        fd: scene.as_raw_fd(),
        events: io::poll_events::POLLIN,
        revents: 0,
    }];
    // Drain any publication completing after the descriptor was created.
    if io::poll(&mut fds, 0)? != 0 {
        let mut changes = [0; 8];
        check(
            io::read(Fd::from_raw(scene.as_raw_fd() as u64), &mut changes)? == 8,
            "scene descriptor did not return its change total",
        )?;
    }
    const TIMEOUT_MS: i32 = 100;
    let begin = now();
    let ready = io::poll(&mut fds, TIMEOUT_MS)?;
    let elapsed = now() - begin;
    value(
        "wait-time",
        (elapsed / 1000) as i64,
        "us",
        Some((100000, 200000)),
    );
    check(
        ready == 0 && elapsed >= 100_000_000 && elapsed <= 200_000_000,
        "idle scene poll returned before its timeout or over 100 ms late",
    )?;
    c.send(PAINT, 23, 0)?;
    let begin = now();
    let ready = io::poll(&mut fds, TIMEOUT_MS)?;
    let elapsed = now() - begin;
    check(
        ready == 1 && fds[0].revents & io::poll_events::POLLIN != 0 && elapsed < 100_000_000,
        "client commit did not wake scene poll before its timeout",
    )?;
    c.receive()?;
    let mut changes = [0; 8];
    check(
        io::read(Fd::from_raw(scene.as_raw_fd() as u64), &mut changes)? == 8
            && u64::from_le_bytes(changes) > 0,
        "scene wake omitted its change total",
    )
}

pub fn deadline_wake() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, mut c) = one()?;
    measure::acknowledged(0)?;
    let before = status()?;
    check(
        before.next_deadline_ns > 0,
        "bwm did not publish its next clock deadline",
    )?;
    clock.step(1, before.next_deadline_ns - 1)?;
    c.command(STEP, 1, 0)?;
    measure::acknowledged(1)?;
    let early = status()?;
    check(
        early.composites == before.composites,
        "clock deadline fired early",
    )?;
    clock.step(2, before.next_deadline_ns)?;
    c.command(STEP, 2, 0)?;
    measure::acknowledged(2)?;
    let due = status()?;
    check(
        due.composites > early.composites && due.presented != 0,
        "clock deadline did not wake and present without input",
    )?;
    check(
        Screen::read()?.serial() >= due.presented,
        "deadline frame lacks native scanout completion",
    )?;
    content(&mut d, &c)
}

pub fn slow_client() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, mut a, mut b) = twin()?;
    b.command(SET_SLOW, 10, 0)?;
    b.command(SET_FLAGS, flags::IGNORE_CLOSE | flags::SLOW_COMMIT, 0)?;
    measure::acknowledged(0)?;
    for epoch in 1..=30 {
        clock.step(epoch, epoch * 16_000_000)?;
        a.command(STEP, epoch as u32, 1)?;
        b.command(STEP, epoch as u32, 1)?;
        measure::acknowledged(epoch)?;
        measure::completed_commit(&a, a.frame_commit)?;
        check(
            a.frame == epoch + 1,
            "fast client's frame did not advance each epoch",
        )?;
        if epoch % 10 == 0 {
            check(
                b.frame == epoch / 10 + 1,
                "slow client omitted its tenth-epoch frame",
            )?;
            measure::completed_commit(&b, b.frame_commit)?;
        } else {
            check(
                b.frame == if epoch < 10 { 0 } else { epoch / 10 + 1 },
                "slow client painted before its tenth epoch",
            )?;
        }
    }
    let screen = d.shown(&b)?;
    screen.client(&b)
}

pub fn stopped_client() -> CaseResult {
    let mut clock = measure::Clock::new()?;
    let (mut d, mut a, b) = twin()?;
    b.child.freeze()?;
    let stale = b.info()?.commit_id;
    measure::acknowledged(0)?;
    // Only a participates in stepping; b is deliberately excluded after SIGSTOP.
    for epoch in 1..=10 {
        clock.step(epoch, epoch * 16_000_000)?;
        a.command(STEP, epoch as u32, 1)?;
        measure::acknowledged(epoch)?;
        measure::completed_commit(&a, a.frame_commit)?;
        check(
            b.info()?.commit_id == stale,
            "stopped excluded client advanced",
        )?;
    }
    d.shown(&b)?.client(&b)
}

pub fn stress_io() -> CaseResult {
    use std::io::{Read, Write};
    let mut clock = measure::Clock::new()?;
    let mut stream = Stream::open()?;
    let mut input = Virtual::create()?;
    let (mut d, mut c) = one()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let mut tx = std::net::TcpStream::connect(listener.local_addr().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let (mut rx, _) = listener.accept().map_err(|e| e.to_string())?;
    rx.set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    let path = format!("/tmp/wm-io-{}", std::process::id());
    let _file = FileOverride::new(&path, b"")?;
    let mut disk = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut last = 0;
    for epoch in 1..=100 {
        let block = [epoch as u8; 4096];
        disk.write_all(&block).map_err(|e| e.to_string())?;
        disk.sync_data().map_err(|e| e.to_string())?;
        tx.write_all(&block).map_err(|e| e.to_string())?;
        let mut got = [0; 4096];
        rx.read_exact(&mut got).map_err(|e| e.to_string())?;
        check(got == block, "loopback traffic corrupted under animation")?;
        c.clear()?;
        input.batch(&[(1, 30, 1), (1, 30, 0)], epoch)?;
        let events = stream.read_from(3, &[input.id])?;
        clock.step(epoch, epoch * 16_000_000)?;
        c.command(STEP, epoch as u32, 1)?;
        c.await_event("input during disk and network traffic", |e| {
            e.kind == kind::KEY_PRESS
        })?;
        for event in events.iter().filter(|e| e.kind == 1) {
            check(event.serial > last, "input order regressed under I/O")?;
            last = event.serial;
            disposition(event.serial, "delivered", None)?;
        }
        measure::acknowledged(epoch)?;
        measure::completed_commit(&c, c.handling_commit)?;
    }
    content(&mut d, &c)
}

pub fn progress_under_load() -> CaseResult {
    measure::receipts()?;
    let (mut d, mut a, mut b) = twin()?;
    let mut c = Client::start(41)?;
    d.client_ready(&c)?;
    fn soft_lockups() -> Result<u64, String> {
        let text = std::fs::read_to_string("/proc/trace/counters").map_err(|e| e.to_string())?;
        text.lines()
            .find_map(|line| {
                let words: Vec<_> = line
                    .split(|c: char| c.is_whitespace() || c == '=' || c == ':')
                    .filter(|s| !s.is_empty())
                    .collect();
                (words.first().copied() == Some("soft_lockups"))
                    .then(|| words.last()?.parse().ok())
                    .flatten()
            })
            .ok_or("soft-lockup counter is absent".into())
    }
    let lockups = soft_lockups()?;
    for c in [&mut a, &mut b, &mut c] {
        c.command(ANIMATE, 1, 0)?;
    }
    let begin = now();
    let mut previous = measure::receipts()?
        .iter()
        .map(|r| r.serial)
        .max()
        .unwrap_or(0);
    for tick in 1..=50 {
        let end = begin + tick * 100_000_000;
        if now() < end {
            time::sleep_ms((end - now()).div_ceil(1_000_000))?;
        }
        let next = measure::receipts()?
            .iter()
            .map(|r| r.serial)
            .max()
            .unwrap_or(0);
        check(
            next > previous,
            "no completed composite in a 100 ms load window",
        )?;
        previous = next;
    }
    check(
        soft_lockups()? == lockups,
        "composition increased the soft-lockup counter",
    )?;
    for c in [&mut a, &mut b, &mut c] {
        c.command(ANIMATE, 0, 0)?;
    }
    d.shown(&c)?.client(&c)
}

pub fn cursor_shape_idle() -> CaseResult {
    super::composition::cursor_stationary_shape()
}
