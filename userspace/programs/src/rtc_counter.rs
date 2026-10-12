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
