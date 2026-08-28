//! Mutation-site execution coverage for the measured harness window.
//!
//! A mutation can only be called a trial when the region it changes executes
//! while the harness is measuring. Counts therefore remain closed through boot
//! and rendezvous setup, open at the driver's timed-loop edge, and close before
//! teardown. The site fast path is one relaxed flag load, a forward
//! predicted-not-taken branch out of the closed window, and one relaxed RMW.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// One coverage site per entry in `mutations::REGISTER`, in register order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum MutSite {
    BlockDeparture,
    CpuIdentity,
    ReclaimBracket,
    PendingNext,
    FutexSection,
    MaskedLock,
}

impl MutSite {
    pub const ALL: [Self; 6] = [
        Self::BlockDeparture,
        Self::CpuIdentity,
        Self::ReclaimBracket,
        Self::PendingNext,
        Self::FutexSection,
        Self::MaskedLock,
    ];

    pub const COUNT: usize = Self::ALL.len();

    pub const fn name(self) -> &'static str {
        match self {
            Self::BlockDeparture => "block_departure",
            Self::CpuIdentity => "cpu_identity",
            Self::ReclaimBracket => "reclaim_bracket",
            Self::PendingNext => "pending_next",
            Self::FutexSection => "futex_section",
            Self::MaskedLock => "masked_lock",
        }
    }
}

/// Sites counted at a harness call site rather than inside the registered host.
///
/// `CpuIdentity` lives in the permanently prohibited AArch64 context-switch
/// file, so neither `proof_cover!` nor a direct `crate::proof::` call may be
/// placed there. The adversarial peer counts a completed
/// `scheduler::schedule()` call in `proof/quiesce.rs` instead. On AArch64 that
/// wrapper calls `run_deferred_reclamation()` and then `schedule_from_kernel()`;
/// the mutated identity read is unconditional at the latter's entry. Each
/// harness-side increment is therefore an exact lower bound on executions of
/// the mutation site without weakening the prohibited-file ratchet.
pub const HARNESS_SIDE: &[MutSite] = &[MutSite::CpuIdentity];

static COUNTS: [AtomicU64; MutSite::COUNT] = [const { AtomicU64::new(0) }; MutSite::COUNT];
static WINDOW_OPEN: AtomicBool = AtomicBool::new(false);

/// Count one execution, but only inside the measured window.
#[inline(always)]
pub fn note(site: MutSite) {
    if !WINDOW_OPEN.load(Ordering::Relaxed) {
        return;
    }
    COUNTS[site as usize].fetch_add(1, Ordering::Relaxed);
}

pub fn open_window() {
    for count in &COUNTS {
        count.store(0, Ordering::Relaxed);
    }
    WINDOW_OPEN.store(true, Ordering::Relaxed);
}

pub fn close_window() {
    WINDOW_OPEN.store(false, Ordering::Relaxed);
}

pub fn counts() -> [u64; MutSite::COUNT] {
    core::array::from_fn(|index| COUNTS[index].load(Ordering::Relaxed))
}

/// Allocation-free formatting for the bracketed RUN record.
pub struct DisplayCounts<'a>(&'a [u64; MutSite::COUNT]);

impl fmt::Display for DisplayCounts<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, site) in MutSite::ALL.iter().enumerate() {
            if index != 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{}={}", site.name(), self.0[index])?;
        }
        Ok(())
    }
}

pub fn display_counts(counts: &[u64; MutSite::COUNT]) -> DisplayCounts<'_> {
    DisplayCounts(counts)
}
