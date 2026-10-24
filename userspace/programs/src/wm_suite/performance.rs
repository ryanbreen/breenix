use super::kit::*;

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
