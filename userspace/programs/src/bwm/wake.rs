//! Calculate descriptor wait bounds from outstanding work.

// Display ownership has no readiness descriptor; retry recovery at this bound.
pub const DISPLAY_WAIT_MS: i32 = 100;

#[derive(Clone, Copy, Default)]
pub struct Deadlines {
    pub closing: Option<u64>,
    pub clipboard: Option<u64>,
    pub repeat: Option<u64>,
    pub launch: Option<u64>,
    pub check_after_ms: Option<u64>,
    pub wall: Option<u64>,
    pub presentation: Option<u64>,
}

pub fn timeout(dirty: bool, display_lost: bool, now: u64, monotonic_ns: u64, deadlines: Deadlines) -> i32 {
    // A lost display leaves the frame dirty; recovery must still back off.
    if display_lost { return DISPLAY_WAIT_MS; }
    if dirty { return 0; }
    let ui = [deadlines.closing, deadlines.clipboard, deadlines.repeat, deadlines.launch, deadlines.wall]
        .into_iter().flatten().map(|d| d.saturating_sub(now));
    let check = deadlines.check_after_ms.map(|d| d.saturating_mul(1_000_000).saturating_sub(monotonic_ns));
    ui.chain(check).chain(deadlines.presentation.map(|d| d.saturating_sub(monotonic_ns))).min().map_or(-1, |ns| {
        (ns.saturating_add(999_999) / 1_000_000).min(i32::MAX as u64) as i32
    })
}
