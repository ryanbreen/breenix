//! Independent PM-timer sampling, separated from port I/O for host execution.

pub const PM_HZ: u64 = 3_579_545;

#[derive(Clone, Copy)]
pub struct Sample {
    pub ticks: u32,
    pub before: u64,
    pub after: u64,
}

impl Sample {
    fn midpoint(self) -> u64 {
        self.before + (self.after - self.before) / 2
    }
}

/// Take a full PM second, retrying collections interrupted by vCPU stalls.
/// Safety bounds use the 100 MHz plausibility floor, never the rate under test:
/// a 25M-cycle gap is at most 250 ms, well below the 24-bit PM wrap period.
/// Endpoint uncertainty is limited to 100 ppm of the measured span, leaving
/// the 1000 ppm agreement assertion unchanged.
pub fn measure(mut sample: impl FnMut() -> Sample, mask: u32) -> Option<u64> {
    const MAX_GAP: u64 = 25_000_000;
    const TIMEOUT: u64 = 5 * 10_000_000_000;
    for _ in 0..8 {
        let start = sample();
        if start.after - start.before > MAX_GAP {
            continue;
        }
        let mut previous = start;
        loop {
            let end = sample();
            // Include the complete read bracket, not just the midpoints: a
            // stall inside a read can hide one or more PM counter wraps too.
            if end.after - previous.before > MAX_GAP
                || end.after - start.before > TIMEOUT
            {
                break;
            }
            let elapsed = u64::from(end.ticks.wrapping_sub(start.ticks) & mask);
            if elapsed >= PM_HZ {
                let span = end.midpoint() - start.midpoint();
                let widths = (start.after - start.before) + (end.after - end.before);
                if u128::from(widths) * 10_000 > u128::from(span) {
                    break;
                }
                return Some((u128::from(span) * u128::from(PM_HZ)
                    / u128::from(elapsed)) as u64);
            }
            previous = end;
            core::hint::spin_loop();
        }
    }
    None
}
