//! Bound an RTC edge by the reads that observed its two seconds.

#[derive(Clone, Copy)]
pub struct RtcPoll {
    pub second: i64,
    pub before: u64,
    pub after: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct RtcEdge {
    pub second: i64,
    pub at: u64,
    pub width: u64,
}

/// The old value was sampled somewhere inside the previous ioctl, even if
/// its return was delayed past the edge. Its return alone is not a lower bound.
pub fn next_edge(
    mut previous: RtcPoll,
    mut read: impl FnMut() -> Result<RtcPoll, String>,
    mut timed_out: impl FnMut() -> bool,
) -> Result<RtcEdge, String> {
    if previous.after < previous.before {
        return Err("the counter went backwards during an RTC read".into());
    }
    loop {
        let current = read()?;
        if current.before < previous.after || current.after < current.before {
            return Err("the counter went backwards between or during RTC reads".into());
        }
        if current.second != previous.second {
            let width = current.after.checked_sub(previous.before)
                .ok_or("the counter went backwards across an RTC edge")?;
            return Ok(RtcEdge {
                second: current.second,
                at: previous.before + width / 2,
                width,
            });
        }
        if timed_out() {
            return Err("the RTC's seconds did not change within 1500 ms".into());
        }
        previous = current;
    }
}

/// RTC elapsed seconds and midpoint frequency, with shared validity checks.
fn midpoint(first: RtcEdge, last: RtcEdge) -> Result<(u64, u64), String> {
    let seconds = (last.second - first.second).rem_euclid(86_400) as u64;
    if seconds == 0 { return Err(format!("two RTC seconds edges both read {}", last.second)); }
    let cycles = last.at.checked_sub(first.at)
        .ok_or("the counter went backwards between RTC edges")?;
    let hz = cycles / seconds;
    if hz == 0 { return Err("the counter did not advance between RTC edges".into()); }
    Ok((seconds, hz))
}

/// Midpoint frequency and enclosing frequency bounds in Hz, rounded outward.
pub fn frequency(first: RtcEdge, last: RtcEdge) -> Result<(u64, u64, u64), String> {
    let (seconds, hz) = midpoint(first, last)?;
    // next_edge's midpoint rounds down, so an odd width has one extra
    // cycle above the midpoint. Keep that cycle in the enclosing bounds.
    let first_low = first.at.checked_sub(first.width / 2)
        .ok_or("invalid starting RTC edge bracket")?;
    let first_high = first.at.checked_add(first.width - first.width / 2)
        .ok_or("invalid starting RTC edge bracket")?;
    let last_low = last.at.checked_sub(last.width / 2)
        .ok_or("invalid ending RTC edge bracket")?;
    let last_high = last.at.checked_add(last.width - last.width / 2)
        .ok_or("invalid ending RTC edge bracket")?;
    let lower = last_low.checked_sub(first_high)
        .ok_or("RTC edge brackets overlap")? / seconds;
    let upper_cycles = last_high.checked_sub(first_low)
        .ok_or("the counter went backwards between RTC edge brackets")?;
    let upper = upper_cycles / seconds + u64::from(upper_cycles % seconds != 0);
    if lower == 0 { return Err("RTC edge brackets do not bound a positive counter frequency".into()); }
    Ok((hz, lower, upper))
}

/// The first edge bracketed within `max_width`, then the first such edge at
/// least `seconds` RTC seconds after it, passing over at most `pass_over` wider
/// edges in all. `source` is told when the edge it returns is expected (None
/// for the first), `per_second` counts later, so it can sleep until shortly
/// before then. An edge known only to a wider bracket is never measured from.
pub fn span_edges(
    seconds: i64,
    per_second: u64,
    max_width: u64,
    pass_over: u32,
    mut source: impl FnMut(Option<u64>) -> Result<RtcEdge, String>,
) -> Result<(RtcEdge, RtcEdge, u32), String> {
    let mut passed = 0;
    let mut first = source(None)?;
    while first.width > max_width {
        pass_over_edge(&first, &mut passed, pass_over, max_width)?;
        first = source(Some(first.at + per_second))?;
    }
    let mut last = source(Some(first.at + seconds as u64 * per_second))?;
    loop {
        if (last.second - first.second).rem_euclid(86_400) >= seconds {
            if last.width <= max_width {
                return Ok((first, last, passed));
            }
            pass_over_edge(&last, &mut passed, pass_over, max_width)?;
        }
        last = source(Some(last.at + per_second))?;
    }
}

fn pass_over_edge(edge: &RtcEdge, passed: &mut u32, pass_over: u32, max_width: u64) -> Result<(), String> {
    *passed += 1;
    if *passed > pass_over {
        return Err(format!("{passed} RTC seconds edges were bracketed by reads wider than {max_width} counts (the last {}), so when they happened is not known well enough to measure from", edge.width));
    }
    Ok(())
}

/// Parts per million by which counts per RTC second at the frequency bounds
/// differ from `per_second`, rounded outward, so a bound just outside a
/// tolerance is never truncated into it.
pub fn ppm_interval(lower: u64, upper: u64, per_second: u64) -> (i64, i64) {
    let ppm = |hz: u64, round_up: bool| {
        let numerator = (hz as i128 - per_second as i128) * 1_000_000;
        let denominator = per_second as i128;
        let floor = numerator.div_euclid(denominator);
        let value = floor + i128::from(round_up && numerator.rem_euclid(denominator) != 0);
        value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    };
    (ppm(lower, false), ppm(upper, true))
}

/// Prefer narrow edges, passing over at most two, then use complete brackets.
/// Extend an uncertain four-second sample up to sixteen seconds to resolve
/// slow reads; a still-wide result is returned for the strict drift check.
#[cfg(any(target_arch = "x86_64", test))]
pub fn select_edges(
    mut source: impl FnMut() -> Result<RtcEdge, String>,
) -> Result<(RtcEdge, RtcEdge, u64), String> {
    let mut first = source()?;
    let mut passed_over = 0;
    loop {
        let edge = source()?;
        let (seconds, hz) = midpoint(first, edge)?;
        let limit = hz / 500;
        if passed_over < 2 && (first.width > limit || (seconds >= 4 && edge.width > limit)) {
            passed_over += 1;
            if first.width > limit { first = edge; }
            continue;
        }
        if seconds < 4 { continue; }
        let (_, lower, upper) = frequency(first, edge)?;
        let spread = (hz - lower).max(upper - hz);
        if seconds < 16 && spread as u128 * 1_000_000 > hz as u128 * 500 { continue; }
        return Ok((first, edge, passed_over));
    }
}

/// Enclosing drift in ppm at the frequency bounds, rounded outward rather
/// than truncating a value just outside the tolerance into a passing value.
pub fn drift_interval(elapsed_ns: i64, cycles: u64, lower: u64, upper: u64) -> Result<(i64, i64), String> {
    if elapsed_ns <= 0 || cycles == 0 || lower == 0 || upper < lower {
        return Err("invalid counter drift interval".into());
    }
    let denominator = cycles as u128 * 1_000_000_000;
    let drift = |hz: u64, round_up: bool| {
        let numerator = elapsed_ns as u128 * hz as u128 * 1_000_000;
        let ratio = numerator / denominator + u128::from(round_up && numerator % denominator != 0);
        (ratio as i128 - 1_000_000).clamp(i64::MIN as i128, i64::MAX as i128) as i64
    };
    Ok((drift(lower, false), drift(upper, true)))
}
