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

/// Midpoint frequency and enclosing frequency bounds, in Hz. Read latency
/// widens these bounds; it does not change the rate comparison's tolerance.
pub fn frequency(first: RtcEdge, last: RtcEdge) -> Result<(u64, u64, u64), String> {
    let seconds = (last.second - first.second).rem_euclid(86_400) as u64;
    if seconds == 0 { return Err(format!("two RTC seconds edges both read {}", last.second)); }
    let cycles = last.at.checked_sub(first.at)
        .ok_or("the counter went backwards between RTC edges")?;
    let hz = cycles / seconds;
    if hz == 0 { return Err("the counter did not advance between RTC edges".into()); }
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
