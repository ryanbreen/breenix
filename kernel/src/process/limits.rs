//! Per-process Linux resource limits. All updates are serialized by PROCESS_MANAGER.
use super::{Process, ProcessId, ProcessManager};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const CPU: usize = 0;
pub const FSIZE: usize = 1;
pub const DATA: usize = 2;
pub const STACK: usize = 3;
pub const CORE: usize = 4;
pub const NPROC: usize = 6;
pub const NOFILE: usize = 7;
pub const AS: usize = 9;
pub const COUNT: usize = 16;
pub const INFINITY: u64 = u64::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rlimit {
    pub soft: u64,
    pub hard: u64,
}

fn defaults() -> [Rlimit; COUNT] {
    let mut limits = [Rlimit {
        soft: INFINITY,
        hard: INFINITY,
    }; COUNT];
    limits[STACK].soft = 8 << 20;
    limits[NOFILE] = Rlimit {
        soft: crate::ipc::fd::INITIAL_FDS as u64,
        hard: crate::ipc::MAX_FDS as u64,
    };
    limits[CORE].soft = 0;
    limits
}

/// Count the user's unreaped tasks under the same lock that publishes a fork.
/// Zombies retain their charge until wait reaps them; the iterator excludes tombstones.
pub fn fork_allowed(manager: &ProcessManager, pid: ProcessId) -> bool {
    let Some(parent) = manager.get_process(pid) else {
        return false;
    };
    parent.uid == 0
        || (manager
            .iter_processes()
            .filter(|(_, p)| p.uid == parent.uid)
            .count() as u64)
            < parent.limits.get(NPROC).soft
}

impl Process {
    pub fn mapped_bytes(&self) -> u64 {
        self.image_size
            .saturating_add(self.heap_end.saturating_sub(self.heap_start))
            .saturating_add(self.user_stack_top.saturating_sub(self.user_stack_bottom))
            .saturating_add(self.vmas.iter().map(|v| v.size()).sum::<u64>())
    }

    pub fn data_bytes(&self) -> u64 {
        self.image_data_size
            .saturating_add(self.heap_end.saturating_sub(self.heap_start))
            .saturating_add(
                self.vmas
                    .iter()
                    .filter(|v| {
                        v.flags.contains(crate::memory::vma::MmapFlags::PRIVATE)
                            && v.prot.contains(crate::memory::vma::Protection::WRITE)
                    })
                    .map(|v| v.size())
                    .sum::<u64>(),
            )
    }

    /// Consume scheduler-produced CPU signals without taking its lock.
    pub fn check_cpu_limit(&mut self) {
        let pending = self.limits.pending.fetch_and(!1, Ordering::AcqRel);
        if pending & 1 != 0 {
            self.signals.set_pending(crate::signal::constants::SIGXCPU);
        }
        if pending & 2 != 0 {
            self.signals.set_pending(crate::signal::constants::SIGKILL);
        }
    }
}

/// Shared by CLONE_VM siblings, independently copied by fork and spawn.
/// PM serializes limit reads and updates; the scheduler reads CPU thresholds
/// atomically and charges time without acquiring PM or walking thread rows.
pub struct Limits {
    values: [(AtomicU64, AtomicU64); COUNT],
    ticks: AtomicU64,
    next: AtomicU64,
    pending: AtomicU32,
}

impl Limits {
    pub fn new() -> Arc<Self> {
        Self::from_values(defaults())
    }

    fn from_values(values: [Rlimit; COUNT]) -> Arc<Self> {
        Arc::new(Self {
            values: values.map(|v| (AtomicU64::new(v.soft), AtomicU64::new(v.hard))),
            ticks: AtomicU64::new(0),
            next: AtomicU64::new(0),
            pending: AtomicU32::new(0),
        })
    }

    pub fn inherit(&self) -> Arc<Self> {
        Self::from_values(core::array::from_fn(|i| self.get(i)))
    }

    pub fn get(&self, resource: usize) -> Rlimit {
        Rlimit {
            soft: self.values[resource].0.load(Ordering::Acquire),
            hard: self.values[resource].1.load(Ordering::Acquire),
        }
    }

    pub fn set(&self, resource: usize, value: Rlimit) {
        self.values[resource].1.store(value.hard, Ordering::Release);
        self.values[resource].0.store(value.soft, Ordering::Release);
        if resource == CPU {
            self.next.store(0, Ordering::Release);
            self.charge_cpu(0);
        }
    }

    /// Charge one execution interval. Exited threads' time remains in the group.
    pub fn charge_cpu(&self, ticks: u64) {
        let total = self
            .ticks
            .fetch_add(ticks, Ordering::Relaxed)
            .saturating_add(ticks);
        let limit = self.get(CPU);
        if limit.soft == INFINITY && limit.hard == INFINITY {
            return;
        }
        let seconds = total.saturating_mul(crate::time::timer::MS_PER_TICK) / 1000;
        if seconds >= limit.hard.max(1) {
            self.pending.fetch_or(2, Ordering::Release);
        } else if seconds >= limit.soft.max(1) {
            let next = self.next.load(Ordering::Relaxed);
            if seconds >= next
                && self
                    .next
                    .compare_exchange(
                        next,
                        seconds.saturating_add(1),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                self.pending.fetch_or(1, Ordering::Release);
            }
        }
    }
}
