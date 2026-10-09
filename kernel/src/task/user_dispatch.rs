//! User-thread dispatches per logical CPU, on every architecture.
//!
//! Each arch's context-switch path counts a dispatch once the switch to a user
//! (non-kernel) thread has committed: x86-64 after `switch_to_thread` has
//! installed the thread as the CPU's current one; ARM64 at an EL0 return frame,
//! at a kernel context restored for a user thread inside a syscall, and at a
//! ret-based kernel resume of a user thread. The userspace completion report
//! prints the counts, and the scheduler milestone's "user work ran on every
//! online CPU" stage is printed from them.

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::task::scheduler::MAX_CPUS;

static USER_DISPATCHES: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Count a dispatch of a user thread on logical CPU `cpu`. One relaxed add.
#[inline]
pub fn note(cpu: usize) {
    if let Some(count) = USER_DISPATCHES.get(cpu) {
        count.fetch_add(1, Ordering::Relaxed);
    }
}

fn online_cpus() -> impl Iterator<Item = usize> {
    (0..MAX_CPUS).filter(|&cpu| crate::arch_impl::current::smp::is_cpu_online(cpu))
}

/// How many CPUs are online, and how many of them dispatched at least one user
/// thread.
pub fn coverage() -> (usize, usize) {
    online_cpus().fold((0, 0), |(online, ran), cpu| {
        let dispatched = USER_DISPATCHES[cpu].load(Ordering::Relaxed) > 0;
        (online + 1, ran + usize::from(dispatched))
    })
}

/// The online CPUs' counts, formatted as ` cpu0=N cpu1=M ...`.
pub struct PerCpu;

impl fmt::Display for PerCpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for cpu in online_cpus() {
            write!(f, " cpu{}={}", cpu, USER_DISPATCHES[cpu].load(Ordering::Relaxed))?;
        }
        Ok(())
    }
}
