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
