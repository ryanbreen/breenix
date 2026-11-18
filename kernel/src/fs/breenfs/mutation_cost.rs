//! E24 coordinator 531(w): fixed, mount-wide mutation observations.
//! Collection updates atomic counters and monotonic timestamps.
//! Times include preemption and device waits; nested publication spans are
//! inclusive, so phase totals must not be added as if they were CPU time.
//! E24 coordinator 583(x10): max_ns is the mount-lifetime maximum completed
//! span, including time descheduled with exclusion held; it is not a case delta.
use alloc::string::String;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use crate::block::{BlockDevice, BlockError};
#[cfg(not(test))]
use crate::block::HardwareDisk;

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
    Foreground,
}
#[derive(Clone, Copy)]
pub(super) enum Phase {
    Operation,
    Reservation,
    Edit,
    Publication,
    Wait,
    PrepareFiles,
    NextFilesView,
    WriteCached,
    DeviceFlush,
    MountExclusion,
    RetainOpen,
    ApplyTouches,
    DrainDirty,
    PrepareReap,
    Cleanup,
    Completion,
    MountExcluded,
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
    "foreground",
];
const PHASES: &[&str] = &[
    "operation",
    "reservation",
    "inode-edit",
    "publication",
    "wait",
    "prepare-files",
    "next-files-view",
    "write-cached",
    "device-flush",
    "mount-exclusion",
    "retain-open",
    "apply-touches",
    "drain-dirty",
    "prepare-reap",
    "cleanup",
    "completion",
    "mount-excluded",
];
struct Slot {
    started: AtomicU64,
    completed: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    last_start_ns: AtomicU64,
    last_end_ns: AtomicU64,
}
impl Slot {
    const fn new() -> Self {
        Self {
            started: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
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
        #[cfg(test)]
        super::super::preempt::context::observe(super::super::preempt::context::Event::Phase {
            op: OPS[op as usize], phase: PHASES[phase as usize], begin: true,
        });
        Span { slot, start, #[cfg(test)] names: (OPS[op as usize], PHASES[phase as usize]) }
    }
    pub fn device<'a>(&'a self, device: &'a dyn BlockDevice) -> PublicationDevice<'a> {
        PublicationDevice { device, costs: self }
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
            writeln!(text, "mutation_cost mount={mount} root={root} op={} phase={} started={started} completed={completed} total_ns={} last_start_ns={} last_end_ns={} max_ns={}",
                OPS[i / PHASES.len()], PHASES[i % PHASES.len()], slot.total_ns.load(Ordering::Relaxed),
                slot.last_start_ns.load(Ordering::Relaxed), slot.last_end_ns.load(Ordering::Relaxed),
                slot.max_ns.load(Ordering::Relaxed))?;
        }
        Ok(())
    }
}
/// Publication-local view: retain backend identity and native batch calls.
/// Only the synchronous device flush call is timed, after cache admission.
pub(super) struct PublicationDevice<'a> {
    device: &'a dyn BlockDevice,
    costs: &'a Costs,
}
impl BlockDevice for PublicationDevice<'_> {
    #[cfg(not(test))]
    fn is_read_only(&self) -> bool { self.device.is_read_only() }
    #[cfg(not(test))]
    fn hardware_disk(&self) -> Option<HardwareDisk> { self.device.hardware_disk() }
    fn identity(&self) -> usize { self.device.identity() }
    fn block_size(&self) -> usize { self.device.block_size() }
    fn num_blocks(&self) -> u64 { self.device.num_blocks() }
    fn read_block(&self, block: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.device.read_block(block, out)
    }
    fn read_blocks(&self, block: u64, count: usize, out: &mut [u8]) -> Result<(), BlockError> {
        self.device.read_blocks(block, count, out)
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.device.write_block(block, bytes)
    }
    fn write_blocks(&self, block: u64, count: usize, bytes: &[u8]) -> Result<(), BlockError> {
        self.device.write_blocks(block, count, bytes)
    }
    fn flush(&self) -> Result<(), BlockError> {
        let _flush = self.costs.span(Op::Commit, Phase::DeviceFlush);
        self.device.flush()
    }
}
pub(super) struct Span<'a> {
    slot: &'a Slot,
    start: u64,
    #[cfg(test)]
    names: (&'static str, &'static str),
}
impl Drop for Span<'_> {
    fn drop(&mut self) {
        #[cfg(test)]
        super::super::preempt::context::observe(super::super::preempt::context::Event::Phase {
            op: self.names.0, phase: self.names.1, begin: false,
        });
        let end = now();
        self.slot
            .total_ns
            .fetch_add(end.saturating_sub(self.start), Ordering::Relaxed);
        self.slot.max_ns.fetch_max(end.saturating_sub(self.start), Ordering::Relaxed);
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
