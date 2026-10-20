//! Elapsed milliseconds for the manager's progress lines: the time-stamp counter, calibrated once against the boot
//! services' `Stall` (UEFI 2.10 section 7.5).
use metal_efi::uefi::BootServices;

pub struct Clock {
    start: u64,
    per_ms: u64,
}

fn counter() -> u64 {
    // SAFETY: reading the time-stamp counter has no side effect.
    unsafe { core::arch::x86_64::_rdtsc() }
}

impl Clock {
    pub fn new(boot: &BootServices) -> Self {
        let before = counter();
        // SAFETY: a 20 ms busy wait in boot services.
        unsafe { (boot.stall)(20_000) };
        let per_ms = ((counter() - before) / 20).max(1);
        Self { start: counter(), per_ms }
    }

    pub fn ms(&self) -> u64 {
        (counter() - self.start) / self.per_ms
    }
}
