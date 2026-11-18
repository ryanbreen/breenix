//! E24 coordinator 531(w): fixed, mount-wide mutation observations.
//! Collection updates atomic counters and monotonic timestamps.
//! Times include preemption and device waits; nested publication spans are
//! inclusive, so phase totals must not be added as if they were CPU time.
use alloc::string::String;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub(super) enum Op {
    Create,
    Link,
    Unlink,
    Rmdir,
    Rename,
    Setattr,
    Write,
    Resize,
    Writeback,
    Fsync,
    Commit,
    Xattr,
}
#[derive(Clone, Copy)]
pub(super) enum Phase {
    Operation,
    Reservation,
    Edit,
    Publication,
    Wait,
}
const OPS: &[&str] = &[
    "create",
    "link",
    "unlink",
    "rmdir",
    "rename",
    "setattr",
    "write",
    "resize",
    "writeback",
    "fsync",
    "commit",
    "xattr",
];
const PHASES: &[&str] = &[
    "operation",
    "reservation",
    "inode-edit",
    "publication",
    "wait",
];
struct Slot {
    started: AtomicU64,
    completed: AtomicU64,
    total_ns: AtomicU64,
    last_start_ns: AtomicU64,
    last_end_ns: AtomicU64,
}
impl Slot {
    const fn new() -> Self {
        Self {
            started: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            last_start_ns: AtomicU64::new(0),
            last_end_ns: AtomicU64::new(0),
        }
    }
}
pub(super) struct Costs {
    slots: [Slot; OPS.len() * PHASES.len()],
}
impl Costs {
    pub fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; OPS.len() * PHASES.len()],
        }
    }
    pub fn span(&self, op: Op, phase: Phase) -> Span<'_> {
        let slot = &self.slots[op as usize * PHASES.len() + phase as usize];
        let start = now();
        slot.last_start_ns.fetch_max(start, Ordering::Relaxed);
        slot.started.fetch_add(1, Ordering::Release);
        Span { slot, start }
    }
    /// Read-only transport through the existing filesystem state report.
    /// Concurrent slots are sampled independently; no mutation lock is taken.
    pub fn report(&self, mount: usize, root: u64, text: &mut String) -> fmt::Result {
        for (i, slot) in self.slots.iter().enumerate() {
            let completed = slot.completed.load(Ordering::Acquire);
            let started = slot.started.load(Ordering::Acquire);
            if started == 0 {
                continue;
            }
            writeln!(text, "mutation_cost mount={mount} root={root} op={} phase={} started={started} completed={completed} total_ns={} last_start_ns={} last_end_ns={}",
                OPS[i / PHASES.len()], PHASES[i % PHASES.len()], slot.total_ns.load(Ordering::Relaxed),
                slot.last_start_ns.load(Ordering::Relaxed), slot.last_end_ns.load(Ordering::Relaxed))?;
        }
        Ok(())
    }
}
pub(super) struct Span<'a> {
    slot: &'a Slot,
    start: u64,
}
impl Drop for Span<'_> {
    fn drop(&mut self) {
        let end = now();
        self.slot
            .total_ns
            .fetch_add(end.saturating_sub(self.start), Ordering::Relaxed);
        self.slot.last_end_ns.fetch_max(end, Ordering::Relaxed);
        self.slot.completed.fetch_add(1, Ordering::Release);
    }
}
fn now() -> u64 {
    #[cfg(not(test))]
    {
        super::monotonic_ns()
    }
    #[cfg(test)]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_nanos() as u64
            + 1
    }
}
