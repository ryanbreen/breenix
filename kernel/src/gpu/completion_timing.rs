//! Sleeping completion probes for interrupt-free devices (W23).

/// Long-running work keeps W22's measured CPU margin.
pub(super) const POLL_NS: u64 = 1_500_000;
/// Absolute offsets from submission, not delays added after each wake.
const EARLY_NS: [u64; 4] = [100_000, 200_000, 400_000, 800_000];

pub(super) fn next_probe(sent: u64, now: u64) -> u64 {
    let age = now.saturating_sub(sent);
    if let Some(offset) = EARLY_NS.into_iter().find(|&offset| offset > age) {
        return sent.saturating_add(offset);
    }
    let steps = (age - EARLY_NS[3]) / POLL_NS + 1;
    sent.saturating_add(EARLY_NS[3]).saturating_add(steps.saturating_mul(POLL_NS))
}

/// Five caller checks including the initial one. Thereafter only kgpu polls.
pub(super) fn caller_probe(start: u64, checks: usize, now: u64) -> Option<u64> {
    EARLY_NS.get(checks.saturating_sub(1)).map(|&offset| start.saturating_add(offset).max(now))
}
