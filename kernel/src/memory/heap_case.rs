//! Failure-only, system-wide heap accounting for a suite case. Only the heap
//! lock owner writes a sample. The runner can read the last completed sample
//! even if another CPU is in the allocator when the case fails.

use core::sync::atomic::{AtomicU64, Ordering};

#[path = "../task/cpu_sample.rs"]
mod sample;
use sample::Sample;

use super::Heap;

pub(crate) const PR_HEAP_CASE: u64 = 0x4252_000a;

static NEXT: AtomicU64 = AtomicU64::new(1);
static ACTIVE: AtomicU64 = AtomicU64::new(0);
static PUBLISHED: AtomicU64 = AtomicU64::new(0);
static SAMPLES: [Sample<6>; 2] = [const { Sample::new() }; 2];
static NOTICE: crate::log_notice::Record = crate::log_notice::Record::new();

pub(super) struct LockWait {
    epoch: u64,
    since_ns: Option<u64>,
}

impl LockWait {
    pub(super) fn new() -> Self {
        Self {
            epoch: ACTIVE.load(Ordering::Acquire),
            since_ns: None,
        }
    }

    pub(super) fn active(&self) -> bool {
        self.epoch != 0
    }

    pub(super) fn contended(&mut self) {
        if self.active() && self.since_ns.is_none() {
            self.since_ns = crate::time::tsc::nanoseconds_since_base();
        }
    }

    pub(super) fn acquired(&mut self) {
        if let Some(start) = self.since_ns {
            self.since_ns =
                crate::time::tsc::nanoseconds_since_base().map(|end| end.saturating_sub(start));
        }
    }
}

pub(super) struct Counts {
    epoch: u64,
    walk_total: u64,
    walk_max: u32,
    wait_total_ns: u64,
    wait_max_ns: u64,
}

impl Counts {
    pub(super) const fn new() -> Self {
        Self {
            epoch: 0,
            walk_total: 0,
            walk_max: 0,
            wait_total_ns: 0,
            wait_max_ns: 0,
        }
    }

    pub(super) fn record(&mut self, heap: &Heap, wait: LockWait, walk: u32) {
        if !wait.active() || wait.epoch != ACTIVE.load(Ordering::Relaxed) {
            return;
        }
        if self.epoch != wait.epoch {
            *self = Self {
                epoch: wait.epoch,
                ..Self::new()
            };
        }
        let wait_ns = wait.since_ns.unwrap_or(0);
        self.walk_total = self.walk_total.saturating_add(u64::from(walk));
        self.walk_max = self.walk_max.max(walk);
        self.wait_total_ns = self.wait_total_ns.saturating_add(wait_ns);
        self.wait_max_ns = self.wait_max_ns.max(wait_ns);

        let (nodes, largest) = heap.free_shape();
        let index = (PUBLISHED.load(Ordering::Relaxed) ^ 1) as usize;
        SAMPLES[index].store([
            self.epoch,
            (u64::from(nodes) << 32) | u64::from(largest),
            self.walk_total,
            u64::from(self.walk_max),
            self.wait_total_ns,
            self.wait_max_ns,
        ]);
        PUBLISHED.store(index as u64, Ordering::Release);
    }
}

pub(crate) fn sys_heap_case(action: u64) -> crate::syscall::SyscallResult {
    use crate::syscall::{
        errno::{EBUSY, EINVAL, ENOSYS, EPERM},
        SyscallResult,
    };

    if !crate::test_control::enabled() {
        return SyscallResult::Err(ENOSYS as u64);
    }
    let root = crate::task::thread::current_thread_signals()
        .and_then(|signals| signals.mirror.identity())
        .is_some_and(|identity| identity.euid == 0);
    if !root {
        return SyscallResult::Err(EPERM as u64);
    }
    if action == 0 {
        let epoch = NEXT.fetch_add(1, Ordering::Relaxed);
        return match ACTIVE.compare_exchange(0, epoch, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => SyscallResult::Ok(epoch),
            Err(_) => SyscallResult::Err(EBUSY as u64),
        };
    }

    let epoch = action >> 1;
    if epoch == 0
        || ACTIVE
            .compare_exchange(epoch, 0, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
    {
        return SyscallResult::Err(EINVAL as u64);
    }
    if action & 1 != 0 {
        report(epoch);
    }
    SyscallResult::Ok(0)
}

fn report(epoch: u64) {
    let latest = PUBLISHED.load(Ordering::Acquire) as usize;
    let sample = [latest, latest ^ 1]
        .into_iter()
        .find_map(|index| SAMPLES[index].load().filter(|row| row[0] == epoch));
    if let Some([_, shape, walk_total, walk_max, wait_total, wait_max]) = sample {
        let nodes = shape >> 32;
        let largest = shape as u32;
        NOTICE.post_again(format_args!(
            "KERNEL_HEAP_CASE epoch={epoch} free_nodes={nodes} largest_free_bytes={largest} alloc_walk_max={walk_max} alloc_walk_total={walk_total} lock_wait_ns={wait_total} lock_wait_max_ns={wait_max} scope=kernel"
        ));
    } else {
        NOTICE.post_again(format_args!(
            "KERNEL_HEAP_CASE epoch={epoch} sample=unavailable scope=kernel"
        ));
    }
}
