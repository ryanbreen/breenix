//! E24 coordinator 531(w): fixed, mount-wide mutation observations.
//! Collection updates atomic counters and monotonic timestamps.
//! Times include preemption and device waits; nested publication spans are
//! inclusive, so phase totals must not be added as if they were CPU time.
//! E24 coordinator 583(x10): max_ns is the mount-lifetime maximum completed
//! span, including time descheduled with exclusion held; it is not a case delta.
use alloc::string::String;
use core::fmt::{self, Write};
use core::sync::atomic::{fence, AtomicU64, Ordering};
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
/// One refused reservation (E24 coordinator 621(y2)). Grants N are those
/// admission charges beyond the open generation's own, so they include a
/// frozen generation's grants until its install retires them.
pub(super) struct Refusal {
    pub generation: u64,
    /// Upper bound on the publications this refusal can wait for, or
    /// `generation` when there is no release to wait for.
    pub release_generation: u64,
    pub free: u64,
    pub reserve: u64,
    /// PENDING blocks and the oldest released_generation, from the installed view.
    pub pending: Option<(u64, u64)>,
    pub grants_next: u64,
    /// Charged grants beyond the open generation and the denied request.
    /// An estimate error has no ledger decision.
    pub accounting: Option<(u64, u64)>,
}
/// Receipts per installed generation, and the ring's size.
const REFUSALS_PER_PUBLICATION: u64 = 8;
const REFUSAL_RING: usize = 32;
const REFUSAL_FIELDS: usize = 10;
const UNREADABLE: u64 = u64::MAX;
struct RefusalSlot {
    // 0 while empty or being written; otherwise the receipt's sequence.
    seq: AtomicU64,
    fields: [AtomicU64; REFUSAL_FIELDS],
}
/// Written only by a holder of the mount's mutation lock; read without it.
struct Refusals {
    window: AtomicU64,
    in_window: AtomicU64,
    recorded: AtomicU64,
    slots: [RefusalSlot; REFUSAL_RING],
}
impl Refusals {
    const fn new() -> Self {
        Self {
            window: AtomicU64::new(0),
            in_window: AtomicU64::new(0),
            recorded: AtomicU64::new(0),
            slots: [const { RefusalSlot { seq: AtomicU64::new(0), fields: [const { AtomicU64::new(0) }; REFUSAL_FIELDS] } }; REFUSAL_RING],
        }
    }
}
pub(super) struct Costs {
    slots: [Slot; OPS.len() * PHASES.len()],
    refusals: Refusals,
    write_bytes: AtomicU64,
}
impl Costs {
    pub fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; OPS.len() * PHASES.len()],
            refusals: Refusals::new(),
            write_bytes: AtomicU64::new(0),
        }
    }
    pub fn record_write(&self, bytes: usize) {
        self.write_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    pub fn write_bytes(&self) -> u64 {
        self.write_bytes.load(Ordering::Relaxed)
    }
    /// Whether a refusal under installed `generation` gets a receipt: the
    /// first REFUSALS_PER_PUBLICATION per generation. Mount lock held.
    pub fn refusal_wanted(&self, generation: u64) -> bool {
        let refusals = &self.refusals;
        if refusals.window.load(Ordering::Relaxed) != generation {
            refusals.window.store(generation, Ordering::Relaxed);
            refusals.in_window.store(0, Ordering::Relaxed);
        }
        let taken = refusals.in_window.load(Ordering::Relaxed);
        if taken >= REFUSALS_PER_PUBLICATION {
            return false;
        }
        refusals.in_window.store(taken + 1, Ordering::Relaxed);
        true
    }
    /// Record a receipt that refusal_wanted admitted. Mount lock held.
    pub fn refusal(&self, op: Op, refusal: &Refusal) {
        let refusals = &self.refusals;
        let seq = refusals.recorded.load(Ordering::Relaxed) + 1;
        let slot = &refusals.slots[(seq as usize - 1) % REFUSAL_RING];
        let (pending, oldest) = refusal.pending.unwrap_or((UNREADABLE, UNREADABLE));
        let (grants_n, request) = refusal.accounting.unwrap_or((0, 0));
        let values = [op as u64, refusal.generation, refusal.release_generation, refusal.free,
            refusal.reserve, pending, oldest, grants_n, refusal.grants_next, request];
        // Seqlock: a reader that sees the same nonzero seq before and after
        // its loads has read one receipt whole.
        slot.seq.store(0, Ordering::Relaxed);
        fence(Ordering::Release);
        for (field, value) in slot.fields.iter().zip(values) {
            field.store(value, Ordering::Relaxed);
        }
        slot.seq.store(seq, Ordering::Release);
        refusals.recorded.store(seq, Ordering::Relaxed);
    }
    fn report_refusals(&self, mount: usize, root: u64, text: &mut String) -> fmt::Result {
        let mut kept = [(0u64, [0u64; REFUSAL_FIELDS]); REFUSAL_RING];
        let mut count = 0;
        for slot in &self.refusals.slots {
            let seq = slot.seq.load(Ordering::Acquire);
            if seq == 0 {
                continue;
            }
            let values = core::array::from_fn(|i| slot.fields[i].load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if slot.seq.load(Ordering::Relaxed) == seq {
                kept[count] = (seq, values);
                count += 1;
            }
        }
        let kept = &mut kept[..count];
        kept.sort_unstable_by_key(|(seq, _)| *seq);
        for (seq, v) in kept.iter() {
            write!(text, "admission_refusal mount={mount} root={root} seq={seq} op={} generation={} release_generation={} free={} reserve={} pending={} pending_oldest={}",
                OPS.get(v[0] as usize).copied().unwrap_or("unknown"), v[1], v[2], v[3], v[4],
                NumberOr(v[5]), NumberOr(v[6]))?;
            // A zero-block request cannot be denied by the ledger.
            if v[9] != 0 {
                writeln!(text, " grants_n={} grants_next={} request={}", v[7], v[8], v[9])?;
            } else {
                writeln!(text, " grants_n=unavailable grants_next={} request=unavailable reason=estimate", v[8])?;
            }
        }
        Ok(())
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
        self.report_refusals(mount, root, text)
    }
}
/// A receipt field that prints `unreadable` for a failed PENDING read.
struct NumberOr(u64);
impl fmt::Display for NumberOr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == UNREADABLE { f.write_str("unreadable") } else { write!(f, "{}", self.0) }
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
