//! The timing-jitter source: how many counter ticks a fixed memory walk takes.
//!
//! A walk is `STEPS` dependent read-modify-write steps at data-dependent places
//! in a 1 MiB buffer, without data-dependent branches, so each walk runs the
//! same instruction sequence and its time varies with how the machine runs it:
//! caches, TLBs, memory, interrupts and the hypervisor. A clock that counts
//! instructions, or does not move, gives each walk the same tick count or one
//! of the two counts its phase allows, and the health tests (`health.rs`) fail
//! such a clock (tools/random-host-tests/src/health_checks.rs).
//!
//! The raw sample is the low byte of a walk's tick count. Each window's tick
//! counts are hashed into the pool; `health::Credit` decides what it earns.

use alloc::vec::Vec;

use super::blake2s::Blake2s;
use super::erase;
use super::health::{HealthTests, WindowVerdict};

/// Words in the walk's buffer (1 MiB), more than a first-level cache holds.
const WORDS: usize = 1 << 17;
/// Steps in one walk: about 20 us, some 450 ticks of a 24 MHz counter, on the
/// machines this was measured on.
const STEPS: u64 = 4096;

/// The counter, read once the instructions before it have completed (ISB,
/// LFENCE). The asm is not `nomem`, so the compiler keeps the walk's memory
/// accesses on their side.
#[cfg(target_arch = "aarch64")]
fn ticks() -> u64 {
    let value: u64;
    unsafe { core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) value, options(nostack)) };
    value
}

#[cfg(target_arch = "x86_64")]
fn ticks() -> u64 {
    let low: u32;
    let high: u32;
    unsafe { core::arch::asm!("lfence", "rdtsc", out("eax") low, out("edx") high, options(nostack)) };
    (u64::from(high) << 32) | u64::from(low)
}

pub struct Jitter {
    scratch: Vec<u64>,
    walk: u64,
    health: HealthTests,
}

impl Jitter {
    /// Returns no source when the buffer cannot be allocated.
    pub fn new() -> Option<Self> {
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(WORDS).ok()?;
        scratch.resize(WORDS, 0);
        Some(Self { scratch, walk: ticks(), health: HealthTests::new() })
    }

    fn sample(&mut self) -> u64 {
        let mask = self.scratch.len() - 1;
        let start = ticks();
        for i in 0..STEPS {
            let j = (self.walk ^ i.wrapping_mul(0x9e37_79b9_7f4a_7c15)) as usize & mask;
            let value = self.scratch[j].wrapping_add(self.walk).rotate_left(9);
            self.scratch[j] = value;
            self.walk ^= value.wrapping_add(i);
        }
        let elapsed = ticks().wrapping_sub(start);
        self.walk ^= elapsed;
        elapsed
    }

    /// Take one window of samples. `digest` receives a hash of the window's
    /// tick counts, for the pool; the verdict says whether it passed.
    pub fn window(&mut self, digest: &mut [u8; 32]) -> WindowVerdict {
        let mut hash = Blake2s::new(32, &[]);
        loop {
            let elapsed = self.sample();
            hash.update(&elapsed.to_le_bytes());
            if let Some(verdict) = self.health.feed(elapsed as u8) {
                hash.update(&self.walk.to_le_bytes());
                hash.finalize(digest);
                return verdict;
            }
        }
    }
}

impl Drop for Jitter {
    fn drop(&mut self) {
        erase::wipe(&mut self.scratch);
        erase::wipe(core::slice::from_mut(&mut self.walk));
    }
}
