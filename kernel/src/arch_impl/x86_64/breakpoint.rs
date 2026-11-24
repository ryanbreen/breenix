use core::sync::atomic::{AtomicU64, Ordering};

use crate::signal::constants::{SIGTRAP, TRAP_BRKPT};

pub(crate) const fn from_userspace(cs: u64) -> bool {
    cs & 3 == 3
}

/// The thread's INT3, handed to the signal return path without PROCESS_MANAGER.
#[derive(Default)]
pub(crate) struct PendingBreakpoint {
    next_rip: AtomicU64,
}

impl PendingBreakpoint {
    pub(crate) fn record(&self, next_rip: u64) {
        self.next_rip.store(next_rip, Ordering::Release);
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.next_rip.load(Ordering::Acquire) != 0
    }

    pub(crate) fn take(&self) -> Option<(u32, i32, u64)> {
        // Intel SDM Vol. 3A, #BP: the saved RIP follows the one-byte INT3.
        let address = self.next_rip.swap(0, Ordering::AcqRel).checked_sub(1)?;
        Some((SIGTRAP, TRAP_BRKPT, address))
    }
}
