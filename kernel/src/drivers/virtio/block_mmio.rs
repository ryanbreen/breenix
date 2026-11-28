//! VirtIO Block Device Driver for ARM64 (MMIO Transport)
//!
//! Several request chains run at once on queue 0 (E4.1). Each chain slot owns
//! a header, a status byte and data frames allocated one by one; the shared
//! `blk_queue` engine builds and drains the split ring. A synchronous
//! multi-sector call keeps a bounded window of chains in flight and notifies
//! once per window; callers whose sector ranges overlap, when either writes,
//! wait for each other, and a flush waits for the requests in flight.

use super::blk_queue::{
    request_type, split_layout, split_queue_size, Admission, BlkQueue, ChainStorage, Desc,
    DmaMemory, Drain, Limits, RangeTable, RequestError, RingMemory, SplitLayout, SubmitError,
    MAX_CHAINS, MAX_SEGMENTS,
};
use super::mmio::{
    device_id, VirtioMmioDevice, VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_SIZE,
};
use crate::arch_impl::aarch64::cpu::{dsb_sy, Aarch64Cpu};
use crate::arch_impl::aarch64::gic;
use crate::arch_impl::traits::{CpuOps, InterruptController};
use crate::task::completion::Completion;
use crate::block::read_failure::VirtioReadFailure;
use crate::task::waitqueue::WaitQueueHead;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, AtomicU32, Ordering};

/// Capacity for block devices across the QEMU virt MMIO transport slots.
pub const MAX_BLOCK_DEVICES: usize = VIRTIO_MMIO_COUNT;

const VIRTIO_IRQ_BASE: u32 = 48;
/// A request waits for its completion in sleeps of this length, checking
/// between them that the device has not failed.
const BLOCK_MMIO_WAIT_SLICE_NS: u64 = 1_000_000_000;
/// A request outstanding this long is reported, at most once per interval.
const BLOCK_MMIO_SLOW_REPORT_INTERVAL_NS: u64 = 10_000_000_000;
/// A wait that cannot sleep (before the scheduler, or on the boot thread
/// before its timer runs) busy-polls the CPU, so it is bounded: past this the
/// request is abandoned, the queue is stopped so its DMA buffers are not
/// reused, and the boot carries on without the disk.
const BLOCK_MMIO_BOOTSTRAP_WAIT_LIMIT_NS: u64 = 30_000_000_000;
const BLOCK_MMIO_WEDGED_ERROR: &str = "Block MMIO device wedged after an abandoned request";
/// When a slow request was last reported, so reports stay rate-limited.
static LAST_SLOW_REPORT_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const NO_COMPLETED_DESC: u32 = u32::MAX;
const NO_COMPLETED_STATUS: u32 = u32::MAX;
/// Callers admitted to one device at once.
const MAX_ADMITTED: usize = 32;

/// Admission to a device: a caller holds its sector range from before its
/// first chain is built until its last result is taken. Overlapping ranges
/// wait for each other when either writes; a flush holds the whole device.
struct BlockMmioRequestGate {
    wedged: AtomicBool,
    ranges: spin::Mutex<RangeTable<MAX_ADMITTED>>,
    /// Callers waiting for a range or for a chain slot.
    waiters: WaitQueueHead,
}

struct BlockMmioRequestGuard<'a> {
    gate: &'a BlockMmioRequestGate,
    entry: usize,
    release_on_drop: bool,
    _custody: crate::task::thread::KillCustody,
}

impl BlockMmioRequestGate {
    const fn new() -> Self {
        Self {
            wedged: AtomicBool::new(false),
            ranges: spin::Mutex::new(RangeTable::new()),
            waiters: WaitQueueHead::new(),
        }
    }

    fn try_admit(&self, start: u64, end: u64, write: bool) -> Option<usize> {
        self.attempt(start, end, write, None, true)
    }

    /// One admission attempt. A flush (`flush` is its turn) that has to wait
    /// holds its turn; a caller that cannot sleep tries once, ahead of
    /// waiting flushes, because it cannot wait its turn.
    fn attempt(&self, start: u64, end: u64, write: bool, flush: Option<&mut bool>, can_sleep: bool) -> Option<usize> {
        // No interrupt path takes the table; masking keeps a holder from being
        // preempted by a thread that would then spin on it.
        crate::arch_without_interrupts(|| {
            let mut table = self.ranges.lock();
            let admission = match flush {
                _ if !can_sleep => table.try_admit_ahead(start, end, write),
                Some(queued) => table.try_admit_flush(queued),
                None => table.try_admit(start, end, write),
            };
            match admission {
                Admission::Admitted(entry) => Some(entry),
                Admission::Wait => None,
            }
        })
    }

    /// Admit the sectors `[start, end)`, waiting for conflicting requests.
    fn admit(&self, start: u64, end: u64, write: bool) -> Result<BlockMmioRequestGuard<'_>, &'static str> {
        self.admit_claim(start, end, write, false)
    }

    /// Admit a flush for the whole device, waiting its turn.
    fn admit_flush(&self) -> Result<BlockMmioRequestGuard<'_>, &'static str> {
        self.admit_claim(0, u64::MAX, true, true)
    }

    fn admit_claim(&self, start: u64, end: u64, write: bool, flush: bool) -> Result<BlockMmioRequestGuard<'_>, &'static str> {
        if self.wedged.load(Ordering::Acquire) {
            return Err(BLOCK_MMIO_WEDGED_ERROR);
        }

        // A pending SIGKILL may end syscall waits, but cannot discard a
        // queued device owner or its DMA receipt. Nest custody before the
        // wait and keep it until request ownership has ended (E24 520(r)).
        let custody = crate::task::thread::KillCustody::try_enter()
            .ok_or("Block MMIO request gate refused a kill-claimed thread")?;
        let mut queued = false;
        let entry = self.wait_for_admission(start, end, write, flush.then_some(&mut queued));
        if queued {
            // A flush that gave up: requests waiting behind it go ahead.
            crate::arch_without_interrupts(|| self.ranges.lock().cancel_flush(&mut queued));
            self.waiters.wake_up();
        }
        entry.map(|entry| BlockMmioRequestGuard { gate: self, entry, release_on_drop: true, _custody: custody })
    }

    fn wait_for_admission(&self, start: u64, end: u64, write: bool, mut flush: Option<&mut bool>) -> Result<usize, &'static str> {
        loop {
            let can_sleep = block_mmio_request_gate_can_sleep();
            if let Some(entry) = self.attempt(start, end, write, flush.as_deref_mut(), can_sleep) {
                return Ok(entry);
            }

            if !can_sleep {
                return Err("Block MMIO request already in progress (context cannot sleep)");
            }

            if self
                .waiters
                .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
                .is_none()
            {
                return Err("Block MMIO request already in progress (waiter publication refused)");
            }

            if self.wedged.load(Ordering::Acquire) {
                self.waiters.finish_wait();
                return Err(BLOCK_MMIO_WEDGED_ERROR);
            }

            // Published before this check, so a release after it wakes us.
            if let Some(entry) = self.attempt(start, end, write, flush.as_deref_mut(), can_sleep) {
                self.waiters.finish_wait();
                return Ok(entry);
            }
            crate::task::waitqueue::schedule_current_wait();
            self.waiters.finish_wait();

            if self.wedged.load(Ordering::Acquire) {
                return Err(BLOCK_MMIO_WEDGED_ERROR);
            }
        }
    }

    fn release(&self, entry: usize) {
        crate::arch_without_interrupts(|| self.ranges.lock().release(entry));
        self.waiters.wake_up();
    }

    fn is_busy(&self) -> bool {
        !crate::arch_without_interrupts(|| self.ranges.lock().is_empty())
    }
}

impl BlockMmioRequestGuard<'_> {
    /// The device keeps a request it may still complete: its range stays
    /// held and later callers are refused.
    fn wedge(mut self) {
        self.release_on_drop = false;
        self.gate.wedged.store(true, Ordering::Release);
        self.gate.waiters.wake_up();
    }
}

impl Drop for BlockMmioRequestGuard<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.gate.release(self.entry);
        }
    }
}

#[inline]
fn block_mmio_request_gate_can_sleep() -> bool {
    crate::task::scheduler::current_thread_id().is_some()
        && crate::per_cpu_aarch64::preempt_count() > 0
        && crate::arch_impl::aarch64::timer_interrupt::is_initialized()
}

#[cfg(feature = "boot_tests")]
const BLOCK_WEDGE_ORACLE_IDLE: u32 = 0;
#[cfg(feature = "boot_tests")]
const BLOCK_WEDGE_ORACLE_SLEEP_PERMITTED: u32 = 1;
#[cfg(feature = "boot_tests")]
const BLOCK_WEDGE_ORACLE_REFUSED: u32 = 2;
#[cfg(feature = "boot_tests")]
const BLOCK_WEDGE_ORACLE_BAD_CONTEXT: u32 = 3;
#[cfg(feature = "boot_tests")]
const BLOCK_WEDGE_ORACLE_WRONG_RESULT: u32 = 4;
#[cfg(feature = "boot_tests")]
const BLOCK_WEDGE_ORACLE_BOUND_NS: u64 = 100_000_000;
#[cfg(feature = "boot_tests")]
const BLOCK_WEDGE_ORACLE_SETUP_NS: u64 = 500_000_000;

#[cfg(feature = "boot_tests")]
static BLOCK_WEDGE_ORACLE_GATE: BlockMmioRequestGate = BlockMmioRequestGate::new();
#[cfg(feature = "boot_tests")]
static BLOCK_WEDGE_ORACLE_PARKED_STATE: AtomicU32 = AtomicU32::new(BLOCK_WEDGE_ORACLE_IDLE);
#[cfg(feature = "boot_tests")]
static BLOCK_WEDGE_ORACLE_REFUSE_STATE: AtomicU32 = AtomicU32::new(BLOCK_WEDGE_ORACLE_IDLE);
#[cfg(feature = "boot_tests")]
static BLOCK_WEDGE_ORACLE_REFUSE_START_NS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BLOCK_WEDGE_ORACLE_REFUSE_ELAPSED_NS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
struct BlockWedgeOracleSleepContext;

#[cfg(feature = "boot_tests")]
impl BlockWedgeOracleSleepContext {
    fn enter() -> Option<Self> {
        if crate::per_cpu_aarch64::preempt_count() != 0 {
            return None;
        }
        crate::per_cpu_aarch64::preempt_disable();
        if !block_mmio_request_gate_can_sleep() {
            crate::per_cpu_aarch64::preempt_enable();
            return None;
        }
        Some(Self)
    }
}

#[cfg(feature = "boot_tests")]
impl Drop for BlockWedgeOracleSleepContext {
    fn drop(&mut self) {
        crate::per_cpu_aarch64::preempt_enable();
    }
}

#[cfg(feature = "boot_tests")]
fn block_wedge_oracle_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

#[cfg(feature = "boot_tests")]
fn block_wedge_oracle_wait_until(deadline_ns: u64, mut condition: impl FnMut() -> bool) -> bool {
    while block_wedge_oracle_now_ns() < deadline_ns {
        if condition() {
            return true;
        }
        crate::arch_halt_with_interrupts();
    }
    condition()
}

/// Exercise the production MMIO block request gate without touching a real device.
#[cfg(feature = "boot_tests")]
pub(crate) fn block_wedge_oracle_test() -> crate::test_framework::registry::TestResult {
    use crate::task::kthread::{kthread_has_exited_for_test, kthread_join, kthread_run};
    use crate::test_framework::registry::TestResult;

    BLOCK_WEDGE_ORACLE_PARKED_STATE.store(BLOCK_WEDGE_ORACLE_IDLE, Ordering::Release);
    BLOCK_WEDGE_ORACLE_REFUSE_STATE.store(BLOCK_WEDGE_ORACLE_IDLE, Ordering::Release);
    BLOCK_WEDGE_ORACLE_REFUSE_START_NS.store(0, Ordering::Release);
    BLOCK_WEDGE_ORACLE_REFUSE_ELAPSED_NS.store(0, Ordering::Release);

    let request_guard = match BLOCK_WEDGE_ORACLE_GATE.admit(0, 1, true) {
        Ok(guard) => guard,
        Err(_) => return TestResult::Fail("block wedge oracle could not take the initial lock"),
    };

    let parked_handle = match kthread_run(
        || {
            let Some(_sleep_context) = BlockWedgeOracleSleepContext::enter() else {
                BLOCK_WEDGE_ORACLE_PARKED_STATE
                    .store(BLOCK_WEDGE_ORACLE_BAD_CONTEXT, Ordering::Release);
                return;
            };
            BLOCK_WEDGE_ORACLE_PARKED_STATE
                .store(BLOCK_WEDGE_ORACLE_SLEEP_PERMITTED, Ordering::Release);
            match BLOCK_WEDGE_ORACLE_GATE.admit(0, 1, true) {
                Err(BLOCK_MMIO_WEDGED_ERROR) => BLOCK_WEDGE_ORACLE_PARKED_STATE
                    .store(BLOCK_WEDGE_ORACLE_REFUSED, Ordering::Release),
                Err(_) | Ok(_) => BLOCK_WEDGE_ORACLE_PARKED_STATE
                    .store(BLOCK_WEDGE_ORACLE_WRONG_RESULT, Ordering::Release),
            }
        },
        "block_wedge_parked",
    ) {
        Ok(handle) => handle,
        Err(_) => return TestResult::Fail("block wedge oracle could not spawn parked requester"),
    };

    let setup_deadline = block_wedge_oracle_now_ns().saturating_add(BLOCK_WEDGE_ORACLE_SETUP_NS);
    let waiter_parked = block_wedge_oracle_wait_until(setup_deadline, || {
        BLOCK_WEDGE_ORACLE_GATE.waiters.has_waiters()
            || BLOCK_WEDGE_ORACLE_PARKED_STATE.load(Ordering::Acquire)
                >= BLOCK_WEDGE_ORACLE_BAD_CONTEXT
    });
    if BLOCK_WEDGE_ORACLE_PARKED_STATE.load(Ordering::Acquire)
        == BLOCK_WEDGE_ORACLE_BAD_CONTEXT
    {
        return TestResult::Fail("block wedge oracle parked lock was not sleep-permitted");
    }
    if !waiter_parked || !BLOCK_WEDGE_ORACLE_GATE.waiters.has_waiters() {
        return TestResult::Fail(
            "block wedge oracle requester never parked on the real wait queue",
        );
    }

    request_guard.wedge();

    let wake_deadline = block_wedge_oracle_now_ns().saturating_add(BLOCK_WEDGE_ORACLE_BOUND_NS);
    if !block_wedge_oracle_wait_until(wake_deadline, || {
        kthread_has_exited_for_test(&parked_handle)
    }) {
        return TestResult::Fail("block wedge oracle did not release an already parked requester");
    }
    if !matches!(kthread_join(&parked_handle), Ok(0))
        || BLOCK_WEDGE_ORACLE_PARKED_STATE.load(Ordering::Acquire) != BLOCK_WEDGE_ORACLE_REFUSED
        || BLOCK_WEDGE_ORACLE_GATE.waiters.has_waiters()
    {
        return TestResult::Fail("block wedge oracle left a parked requester behind");
    }

    let refuse_handle = match kthread_run(
        || {
            let Some(_sleep_context) = BlockWedgeOracleSleepContext::enter() else {
                BLOCK_WEDGE_ORACLE_REFUSE_STATE
                    .store(BLOCK_WEDGE_ORACLE_BAD_CONTEXT, Ordering::Release);
                return;
            };
            let started_at = block_wedge_oracle_now_ns();
            BLOCK_WEDGE_ORACLE_REFUSE_START_NS.store(started_at, Ordering::Release);
            BLOCK_WEDGE_ORACLE_REFUSE_STATE
                .store(BLOCK_WEDGE_ORACLE_SLEEP_PERMITTED, Ordering::Release);
            let result = BLOCK_WEDGE_ORACLE_GATE.admit(0, 1, true);
            let elapsed_ns = block_wedge_oracle_now_ns().saturating_sub(started_at);
            BLOCK_WEDGE_ORACLE_REFUSE_ELAPSED_NS.store(elapsed_ns, Ordering::Release);
            match result {
                Err(BLOCK_MMIO_WEDGED_ERROR) => BLOCK_WEDGE_ORACLE_REFUSE_STATE
                    .store(BLOCK_WEDGE_ORACLE_REFUSED, Ordering::Release),
                Err(_) | Ok(_) => BLOCK_WEDGE_ORACLE_REFUSE_STATE
                    .store(BLOCK_WEDGE_ORACLE_WRONG_RESULT, Ordering::Release),
            }
        },
        "block_wedge_refuse",
    ) {
        Ok(handle) => handle,
        Err(_) => return TestResult::Fail("block wedge oracle could not spawn second requester"),
    };

    let start_deadline = block_wedge_oracle_now_ns().saturating_add(BLOCK_WEDGE_ORACLE_SETUP_NS);
    if !block_wedge_oracle_wait_until(start_deadline, || {
        BLOCK_WEDGE_ORACLE_REFUSE_STATE.load(Ordering::Acquire) != BLOCK_WEDGE_ORACLE_IDLE
    }) {
        return TestResult::Fail("block wedge oracle second requester never ran");
    }
    let state = BLOCK_WEDGE_ORACLE_REFUSE_STATE.load(Ordering::Acquire);
    if state == BLOCK_WEDGE_ORACLE_BAD_CONTEXT {
        return TestResult::Fail("block wedge oracle second lock was not sleep-permitted");
    }

    let refuse_deadline = BLOCK_WEDGE_ORACLE_REFUSE_START_NS
        .load(Ordering::Acquire)
        .saturating_add(BLOCK_WEDGE_ORACLE_BOUND_NS);
    if !block_wedge_oracle_wait_until(refuse_deadline, || {
        BLOCK_WEDGE_ORACLE_REFUSE_STATE.load(Ordering::Acquire)
            != BLOCK_WEDGE_ORACLE_SLEEP_PERMITTED
    }) {
        return TestResult::Fail("block wedge oracle second requester parked behind the dead gate");
    }
    let exit_deadline = block_wedge_oracle_now_ns().saturating_add(BLOCK_WEDGE_ORACLE_BOUND_NS);
    if !block_wedge_oracle_wait_until(exit_deadline, || {
        kthread_has_exited_for_test(&refuse_handle)
    }) {
        return TestResult::Fail("block wedge oracle second requester did not exit in time");
    }
    if !matches!(kthread_join(&refuse_handle), Ok(0))
        || BLOCK_WEDGE_ORACLE_REFUSE_STATE.load(Ordering::Acquire) != BLOCK_WEDGE_ORACLE_REFUSED
    {
        return TestResult::Fail("block wedge oracle second lock returned the wrong result");
    }

    let refuse_ns = BLOCK_WEDGE_ORACLE_REFUSE_ELAPSED_NS.load(Ordering::Acquire);
    if refuse_ns >= BLOCK_WEDGE_ORACLE_BOUND_NS {
        return TestResult::Fail("block wedge oracle refusal exceeded 100 ms");
    }
    let refuse_ms = refuse_ns / 1_000_000;
    crate::serial_println!(
        "[BLOCK_WEDGE_ORACLE:locked=1:wedged=1:refused=1:parked=0:refuse_ms={}]",
        refuse_ms
    );
    TestResult::Pass
}


/// One chain slot's completion. Its token is the slot's engine tag: reset
/// and tagged under the queue lock when the slot is reserved, so the token a
/// drain completes is the current owner's (the host check
/// a_slot_reports_the_tag_of_the_owner_whose_chain_completed).
struct SlotCompletion {
    completion: Completion,
}

impl SlotCompletion {
    const fn new() -> Self {
        Self { completion: Completion::new() }
    }

    /// Wait for the device to complete the published request.
    ///
    /// The device owns the descriptors and DMA buffers until it completes the
    /// request, and a slow device is not a failed one, so there is no
    /// timeout: the wait ends when the device completes the request. It
    /// sleeps in slices only to notice a device that has failed, the one case
    /// where the request is abandoned. A wait that cannot sleep spins
    /// instead, and is abandoned past the bootstrap limit rather than
    /// spinning the boot CPU forever.
    fn wait_for_completion(
        &self,
        token: u32,
        state: &BlockDeviceState,
        request: u32,
    ) -> Result<(), &'static str> {
        let started_ns = monotonic_now_ns();
        let sleeps = crate::task::completion::wait_sleeps();
        loop {
            if let Ok(true) = self
                .completion
                .wait_timeout_uninterruptible(token, BLOCK_MMIO_WAIT_SLICE_NS)
            {
                return Ok(());
            }
            if device_has_failed(state) {
                return Err("Block MMIO device failed with a request outstanding");
            }
            let waited_ns = monotonic_now_ns().saturating_sub(started_ns);
            if !sleeps && waited_ns >= BLOCK_MMIO_BOOTSTRAP_WAIT_LIMIT_NS {
                return Err("Block MMIO request not completed before the kernel could sleep");
            }
            report_slow_request(request, waited_ns);
        }
    }
}

struct BlockMmioCompletion {
    slots: [SlotCompletion; MAX_CHAINS],
    next_token: AtomicU32,
}

impl BlockMmioCompletion {
    const fn new() -> Self {
        Self { slots: [const { SlotCompletion::new() }; MAX_CHAINS], next_token: AtomicU32::new(1) }
    }

    fn next_completion_token(&self) -> u32 {
        loop {
            let token = self.next_token.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            if token != 0 {
                return token;
            }
        }
    }

}

static REQUEST_GATES: [BlockMmioRequestGate; MAX_BLOCK_DEVICES] =
    [const { BlockMmioRequestGate::new() }; MAX_BLOCK_DEVICES];

static COMPLETIONS: [BlockMmioCompletion; MAX_BLOCK_DEVICES] =
    [const { BlockMmioCompletion::new() }; MAX_BLOCK_DEVICES];

/// VirtIO block feature bits
mod features {
    /// size_max bounds a single data descriptor (VIRTIO_BLK_F_SIZE_MAX, bit 1)
    pub const SIZE_MAX: u64 = 1 << 1;
    /// seg_max bounds a request's data descriptors (VIRTIO_BLK_F_SEG_MAX, bit 2)
    pub const SEG_MAX: u64 = 1 << 2;
    /// Device is read-only (VIRTIO_BLK_F_RO, bit 5)
    pub const RO: u64 = 1 << 5;
    /// Device supports cache flush requests (VIRTIO_BLK_F_FLUSH, bit 9)
    pub const FLUSH: u64 = 1 << 9;
    /// The modern interface (VIRTIO_F_VERSION_1, bit 32), which a driver must
    /// accept when a version 2 transport offers it (virtio 1.2 section 6.1).
    pub const VERSION_1: u64 = 1 << 32;
}

/// Sector size in bytes
pub const SECTOR_SIZE: usize = 512;

/// Size of each device's static DMA data buffer: one page. A device uses it,
/// as a single one-frame chain, only when no data frames could be allocated.
const DMA_DATA_SIZE: usize = 4096;

/// Data frames each chain slot owns: 64 KiB, the block cache's RUN_BYTES, so
/// one coalesced cache run is one request (E24 ledger 368, #3040).
const CHAIN_FRAMES: usize = MAX_SEGMENTS;
/// Data frames one device may hold for its chains.
const DATA_BUDGET_BYTES: usize = 512 * 1024;
/// The largest queue this driver sets up.
const QUEUE_SIZE_LIMIT: u16 = 256;
const QUEUE_LAYOUT: SplitLayout = split_layout(QUEUE_SIZE_LIMIT, 4096);
const QUEUE_BYTES: usize = QUEUE_LAYOUT.bytes.div_ceil(4096) * 4096;

/// VirtIO block request header
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioBlkReq {
    type_: u32,
    reserved: u32,
    sector: u64,
}

/// Virtqueue descriptor, as the device reads it.
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Static queue memory: a split ring of up to QUEUE_SIZE_LIMIT entries in the
/// legacy layout (used ring 4 KiB aligned), page-aligned for the device.
#[repr(C, align(4096))]
struct QueueMemory {
    bytes: [u8; QUEUE_BYTES],
}

/// Static request headers, one per chain slot.
#[repr(C, align(16))]
struct RequestHeader {
    req: [VirtioBlkReq; MAX_CHAINS],
}

/// Static data buffer. Page-aligned and one page long, so it is physically
/// contiguous and a single descriptor covers it.
#[repr(C, align(4096))]
struct DataBuffer {
    data: [u8; DMA_DATA_SIZE],
}

/// Static status bytes, one per chain slot, 16 bytes apart.
#[repr(C, align(16))]
struct StatusBuffer {
    status: [[u8; 16]; MAX_CHAINS],
}

// Per-device DMA storage. Element alignment keeps each queue and data buffer
// page-aligned while giving each transport independent request storage.
static mut QUEUE_MEM: [QueueMemory; MAX_BLOCK_DEVICES] =
    [const { QueueMemory { bytes: [0; QUEUE_BYTES] } }; MAX_BLOCK_DEVICES];
static mut REQ_HEADERS: [RequestHeader; MAX_BLOCK_DEVICES] = [const {
    RequestHeader { req: [VirtioBlkReq { type_: 0, reserved: 0, sector: 0 }; MAX_CHAINS] }
}; MAX_BLOCK_DEVICES];
static mut DATA_BUFS: [DataBuffer; MAX_BLOCK_DEVICES] =
    [const { DataBuffer { data: [0; DMA_DATA_SIZE] } }; MAX_BLOCK_DEVICES];
static mut STATUS_BUFS: [StatusBuffer; MAX_BLOCK_DEVICES] =
    [const { StatusBuffer { status: [[0xff; 16]; MAX_CHAINS] } }; MAX_BLOCK_DEVICES];

/// VirtIO block device states (one per device)
static mut BLOCK_DEVICES: [Option<BlockDeviceState>; MAX_BLOCK_DEVICES] =
    [const { None }; MAX_BLOCK_DEVICES];

struct BlockDeviceState {
    base: u64,
    capacity: u64,
    device_features: u64,
    slot: usize,
    /// Data bytes one chain carries, in whole sectors.
    chain_bytes: usize,
}

/// One device's split ring as the engine writes and reads it.
struct MmioRing {
    base: *mut u8,
    avail: usize,
    used: usize,
}

// The ring is reached only through its device's queue lock.
unsafe impl Send for MmioRing {}

impl RingMemory for MmioRing {
    fn write_desc(&mut self, index: u16, desc: Desc) {
        let desc = VirtqDesc { addr: desc.addr, len: desc.len, flags: desc.flags, next: desc.next };
        unsafe { write_volatile((self.base as *mut VirtqDesc).add(index as usize), desc) }
    }

    fn write_avail_entry(&mut self, pos: u16, head: u16) {
        unsafe { write_volatile(self.base.add(self.avail + 4 + 2 * pos as usize) as *mut u16, head) }
    }

    fn barrier_before_avail_idx(&mut self) {
        fence(Ordering::SeqCst);
        dsb_sy();
    }

    fn write_avail_idx(&mut self, idx: u16) {
        unsafe { write_volatile(self.base.add(self.avail + 2) as *mut u16, idx) }
    }

    fn barrier_before_notify(&mut self) {
        // DSB: the index store is visible to the device before the MMIO notify.
        fence(Ordering::SeqCst);
        dsb_sy();
    }

    fn read_used_idx(&mut self) -> u16 {
        let idx = unsafe { read_volatile(self.base.add(self.used + 2) as *const u16) };
        dsb_sy();
        fence(Ordering::SeqCst);
        idx
    }

    fn read_used_elem(&mut self, pos: u16) -> (u32, u32) {
        let elem = unsafe { self.base.add(self.used + 4 + 8 * pos as usize) };
        unsafe { (read_volatile(elem as *const u32), read_volatile(elem.add(4) as *const u32)) }
    }
}

/// The chains' buffers through the direct map.
struct DirectMap;

impl DmaMemory for DirectMap {
    fn write_bytes(&mut self, addr: u64, bytes: &[u8]) {
        let virt = addr + crate::memory::physical_memory_offset().as_u64();
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), virt as *mut u8, bytes.len()) }
    }

    fn read_bytes(&mut self, addr: u64, out: &mut [u8]) {
        let virt = addr + crate::memory::physical_memory_offset().as_u64();
        unsafe { core::ptr::copy_nonoverlapping(virt as *const u8, out.as_mut_ptr(), out.len()) }
    }
}

/// One device's queue engine. The interrupt handler drains it with
/// `try_lock`; when a holder on another CPU has it, the handler leaves
/// `drain_pending` set and the holder drains on release.
struct DeviceQueue {
    engine: spin::Mutex<Option<BlkQueue<MmioRing>>>,
    drain_pending: AtomicBool,
}

impl DeviceQueue {
    const fn new() -> Self {
        Self { engine: spin::Mutex::new(None), drain_pending: AtomicBool::new(false) }
    }
}

static QUEUES: [DeviceQueue; MAX_BLOCK_DEVICES] = [const { DeviceQueue::new() }; MAX_BLOCK_DEVICES];

/// Run `f` on the device's engine from thread context, then drain whatever
/// an interrupt handler could not.
fn with_engine<R>(device_index: usize, f: impl FnOnce(&mut BlkQueue<MmioRing>) -> R) -> Option<R> {
    let queue = &QUEUES[device_index];
    let result = crate::arch_without_interrupts(|| queue.engine.lock().as_mut().map(f));
    drain_after_release(device_index);
    result
}

/// Every thread-side holder of the engine lock calls this after releasing
/// it: an interrupt handler that found the lock held left its drain here.
fn drain_after_release(device_index: usize) {
    fence(Ordering::SeqCst);
    if QUEUES[device_index].drain_pending.load(Ordering::SeqCst) {
        drain_device(device_index);
    }
}

/// Take the chains the device has returned and wake their waiters. Callable
/// from the interrupt handler and from threads; it only tries the lock.
fn drain_device(device_index: usize) {
    let queue = &QUEUES[device_index];
    let completions = &COMPLETIONS[device_index];
    loop {
        queue.drain_pending.store(false, Ordering::SeqCst);
        let mut woken = [(0usize, 0u32); MAX_CHAINS];
        let mut count = 0;
        let mut broken = false;
        let locked = crate::arch_without_interrupts(|| {
            let Some(mut guard) = queue.engine.try_lock() else {
                return false;
            };
            if let Some(engine) = guard.as_mut() {
                let outcome = engine.drain(&mut DirectMap, |slot, tag| {
                    woken[count] = (slot, tag);
                    count += 1;
                });
                if outcome == Drain::Broken {
                    // The chains the device held are lost: wake their owners.
                    broken = true;
                    count = owner_tags(engine, &mut woken);
                }
            }
            true
        });
        if !locked {
            // The holder drains after it releases (with_engine), unless it
            // released between our attempt and this store: retry once then.
            queue.drain_pending.store(true, Ordering::SeqCst);
            fence(Ordering::SeqCst);
            if queue.engine.is_locked() {
                return;
            }
            continue;
        }
        for &(slot, tag) in &woken[..count] {
            if tag != 0 {
                completions.slots[slot].completion.complete(tag);
            }
        }
        if broken {
            REQUEST_GATES[device_index].wedged.store(true, Ordering::Release);
            REQUEST_GATES[device_index].waiters.wake_up();
        }
        fence(Ordering::SeqCst);
        if !queue.drain_pending.load(Ordering::SeqCst) {
            return;
        }
    }
}

/// The owner tags of the slots that have owners, for waking them after a stop.
fn owner_tags(engine: &BlkQueue<MmioRing>, out: &mut [(usize, u32); MAX_CHAINS]) -> usize {
    let mut count = 0;
    for slot in 0..engine.chains() {
        let tag = engine.tag(slot);
        if tag != 0 {
            out[count] = (slot, tag);
            count += 1;
        }
    }
    count
}

/// Stop the device's queue after an abandoned wait: no chain it holds is
/// completed or reused, each waiter is woken, and later callers are refused.
fn stop_device(device_index: usize, guard: BlockMmioRequestGuard<'_>) {
    let mut owners = [(0usize, 0u32); MAX_CHAINS];
    let count = with_engine(device_index, |engine| {
        engine.stop();
        owner_tags(engine, &mut owners)
    })
    .unwrap_or(0);
    for &(slot, tag) in &owners[..count] {
        COMPLETIONS[device_index].slots[slot].completion.complete(tag);
    }
    guard.wedge();
}

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    addr - crate::memory::physical_memory_offset().as_u64()
}

/// Chain storage for a device: its static headers and status bytes, and
/// `frames` data frames per chain allocated one by one. A device that gets no
/// frames runs one chain over its static page.
fn chain_storage(device_index: usize, chains: usize, frames: usize, out: &mut [ChainStorage; MAX_CHAINS]) -> usize {
    use crate::memory::frame_allocator::{allocate_frame, deallocate_frame};
    let (header_base, status_base, page) = unsafe {
        (
            (&raw const REQ_HEADERS[device_index]) as u64,
            (&raw const STATUS_BUFS[device_index]) as u64,
            (&raw const DATA_BUFS[device_index]) as u64,
        )
    };
    let mut made = 0;
    'chains: for (index, chain) in out.iter_mut().enumerate().take(chains) {
        let mut storage = ChainStorage::EMPTY;
        storage.header = virt_to_phys(header_base + 16 * index as u64);
        storage.status = virt_to_phys(status_base + 16 * index as u64);
        let mut taken = [None; CHAIN_FRAMES];
        for count in 0..frames {
            match allocate_frame() {
                Some(allocated) => {
                    storage.frames[count] = allocated.start_address().as_u64();
                    taken[count] = Some(allocated);
                }
                None => {
                    // Chains carry equal bytes: a short one is
                    // given back rather than shrinking the others.
                    taken.iter().flatten().for_each(|frame| deallocate_frame(*frame));
                    break 'chains;
                }
            }
        }
        storage.frame_count = frames;
        *chain = storage;
        made += 1;
    }
    if made == 0 {
        crate::serial_println!(
            "[virtio-blk] Device {} has no data frames; requests stay one page",
            device_index
        );
        let mut storage = ChainStorage::EMPTY;
        storage.header = virt_to_phys(header_base);
        storage.status = virt_to_phys(status_base);
        storage.frames[0] = virt_to_phys(page);
        storage.frame_count = 1;
        out[0] = storage;
        made = 1;
    }
    made
}

/// Number of initialized block devices
static DEVICE_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Return true when at least one VirtIO MMIO block device initialized.
pub fn is_initialized() -> bool {
    DEVICE_COUNT.load(core::sync::atomic::Ordering::Acquire) > 0
}

/// Initialize all VirtIO block devices
pub fn init() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-blk] Searching for block devices...");

    let mut found = 0usize;

    // Find block devices in MMIO space
    for i in 0..VIRTIO_MMIO_COUNT {
        if found >= MAX_BLOCK_DEVICES {
            break;
        }
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;
        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            if device.device_id() == device_id::BLOCK {
                crate::serial_println!("[virtio-blk] Found block device {} at {:#x}", found, base);
                init_device(&mut device, base, found, i)?;
                found += 1;
                DEVICE_COUNT.store(found, core::sync::atomic::Ordering::Release);
                crate::drivers::device_class::storage_initialized("virtio-blk", found);
            }
        }
    }

    DEVICE_COUNT.store(found, core::sync::atomic::Ordering::Release);

    if found == 0 {
        Err("No VirtIO block device found")
    } else {
        crate::serial_println!("[virtio-blk] Initialized {} block device(s)", found);
        Ok(())
    }
}

fn init_device(
    device: &mut VirtioMmioDevice,
    base: u64,
    device_index: usize,
    slot: usize,
) -> Result<(), &'static str> {
    let version = device.version();
    crate::serial_println!("[virtio-blk] Device {} version: {}", device_index, version);

    // For v1 (legacy), we must set guest page size BEFORE init
    if version == 1 {
        device.set_guest_page_size(4096);
    }

    // Let cached devices batch writes between filesystem durability barriers.
    // Without FLUSH negotiation, even each 512-byte bitmap write is durable.
    let mut requested = features::RO | features::FLUSH | features::SIZE_MAX | features::SEG_MAX;
    if version != 1 {
        requested |= features::VERSION_1;
    }
    device.init(requested)?;

    let device_features = device.device_features() & requested;
    if device_features & features::RO != 0 {
        crate::serial_println!("[virtio-blk] Device {} is read-only", device_index);
    }

    // Read capacity from config space
    // For VirtIO block: offset 0 = capacity (u64)
    let capacity = device.read_config_u64(0);
    let limits = Limits {
        // size_max (config offset 8) bounds a single data descriptor.
        size_max: if device_features & features::SIZE_MAX != 0 { device.read_config_u32(8) } else { 0 },
        // seg_max (config offset 12) bounds a request's data descriptors.
        seg_max: if device_features & features::SEG_MAX != 0 { device.read_config_u32(12) } else { 0 },
        exact_used_len: device_features & features::VERSION_1 != 0,
    };
    if limits.size_max != 0 && (limits.size_max as usize) < SECTOR_SIZE {
        return Err("VirtIO maximum segment smaller than a sector");
    }
    if device_features & features::SEG_MAX != 0 && limits.seg_max == 0 {
        return Err("VirtIO block device allows no data segments");
    }
    crate::serial_println!(
        "[virtio-blk] Device {} capacity: {} sectors ({} MB)",
        device_index,
        capacity,
        (capacity * SECTOR_SIZE as u64) / (1024 * 1024)
    );

    // Set up the request queue (queue 0)
    device.select_queue(0);
    let queue_num_max = device.get_queue_num_max();
    crate::serial_println!(
        "[virtio-blk] Device {} queue max size: {}",
        device_index,
        queue_num_max
    );

    if queue_num_max == 0 {
        return Err("Device reports queue size 0");
    }

    let queue_size = split_queue_size(queue_num_max, QUEUE_SIZE_LIMIT);
    device.set_queue_num(u32::from(queue_size));
    let layout = split_layout(queue_size, 4096);

    let queue_mem = unsafe { &raw mut QUEUE_MEM[device_index] } as *mut u8;
    let queue_phys = virt_to_phys(queue_mem as u64);
    unsafe { core::ptr::write_bytes(queue_mem, 0, QUEUE_BYTES) };

    if version == 1 {
        // VirtIO MMIO v1 (legacy) queue setup
        crate::serial_println!(
            "[virtio-blk] Device {} using v1 (legacy) queue setup at PFN {:#x}",
            device_index,
            queue_phys / 4096
        );

        device.set_queue_align(4096);
        device.set_queue_pfn((queue_phys / 4096) as u32);
    } else {
        // VirtIO MMIO v2 (modern) queue setup
        let desc_addr = queue_phys;
        let avail_addr = queue_phys + layout.avail as u64;
        let used_addr = queue_phys + layout.used as u64;

        crate::serial_println!(
            "[virtio-blk] Device {} using v2 queue setup: desc={:#x} avail={:#x} used={:#x}",
            device_index,
            desc_addr,
            avail_addr,
            used_addr
        );

        device.set_queue_desc(desc_addr);
        device.set_queue_avail(avail_addr);
        device.set_queue_used(used_addr);
        device.set_queue_ready(true);
    }

    // A small ring gets a shorter chain rather than no device.
    let (chains, frames) = limits
        .chain_plan(queue_size, CHAIN_FRAMES, DATA_BUDGET_BYTES / 4096)
        .ok_or("VirtIO block queue too small for one request")?;

    // Taken after the queue checks, so a refused device holds no frames.
    let mut storage = [ChainStorage::EMPTY; MAX_CHAINS];
    let made = chain_storage(device_index, chains, frames, &mut storage);
    let ring = MmioRing { base: queue_mem, avail: layout.avail, used: layout.used };
    let engine = BlkQueue::new(ring, queue_size, limits, &storage[..made])
        .map_err(|_| "VirtIO block chain storage does not fit the queue")?;
    let chain_bytes = engine.chain_bytes();
    crate::arch_without_interrupts(|| *QUEUES[device_index].engine.lock() = Some(engine));

    // Mark device as ready
    device.driver_ok();

    // Store device state
    unsafe {
        let ptr = &raw mut BLOCK_DEVICES;
        (*ptr)[device_index] = Some(BlockDeviceState {
            base,
            capacity,
            device_features,
            slot,
            chain_bytes,
        });
    }

    let irq = VIRTIO_IRQ_BASE + slot as u32;
    gic::Gicv2::enable_irq(irq as u8);
    crate::serial_println!(
        "[virtio-blk] Block MMIO IRQ {} enabled for device {}: queue {} with {} chains of {} KiB",
        irq,
        device_index,
        queue_size,
        made,
        chain_bytes / 1024
    );

    crate::serial_println!(
        "[virtio-blk] Block device {} initialized successfully",
        device_index
    );
    Ok(())
}

fn block_device_state(device_index: usize) -> Result<&'static BlockDeviceState, &'static str> {
    if device_index >= MAX_BLOCK_DEVICES {
        return Err("Invalid device index");
    }

    unsafe {
        let ptr = &raw const BLOCK_DEVICES;
        (*ptr)[device_index]
            .as_ref()
            .ok_or("Block device not initialized")
    }
}

#[inline]
fn irq_completion_available() -> bool {
    crate::task::scheduler::current_thread_id().is_some() || Aarch64Cpu::interrupts_enabled()
}

/// Snapshot on the failing path. Without admission these are concurrent
/// samples of whatever requests the device holds.
fn read_failure(
    device_index: usize,
    state: Option<&BlockDeviceState>,
    sector: u64,
    bytes: usize,
    reason: &'static str,
    stage: &'static str,
) -> VirtioReadFailure {
    read_failure_at(device_index, state, None, sector, bytes, reason, stage)
}

fn read_failure_at(
    device_index: usize,
    state: Option<&BlockDeviceState>,
    slot: Option<usize>,
    sector: u64,
    bytes: usize,
    reason: &'static str,
    stage: &'static str,
) -> VirtioReadFailure {
    let gate = REQUEST_GATES.get(device_index);
    let entry = COMPLETIONS.get(device_index).zip(slot).map(|(c, s)| &c.slots[s]);
    let (last_used, head) = match state {
        Some(_) => with_engine(device_index, |engine| {
            (u32::from(engine.last_used()), slot.and_then(|s| engine.slot_head(s)))
        })
        .unwrap_or((0, None)),
        None => (0, None),
    };
    VirtioReadFailure {
        reason, stage, device: device_index, sector, bytes,
        thread: crate::task::scheduler::current_thread_id(),
        preempt_count: crate::per_cpu_aarch64::preempt_count(),
        kill_custody: crate::per_cpu_aarch64::current_thread().map(|thread|
            thread.kill_custody.load(Ordering::Acquire)),
        in_interrupt: crate::per_cpu_aarch64::in_interrupt(),
        interrupts_enabled: Aarch64Cpu::interrupts_enabled(),
        timer_ready: crate::arch_impl::aarch64::timer_interrupt::is_initialized(),
        gate_locked: gate.is_some_and(|gate| gate.is_busy()),
        gate_wedged: gate.is_some_and(|gate| gate.wedged.load(Ordering::Acquire)),
        pending_token: match (state, slot) {
            (Some(_), Some(slot)) => with_engine(device_index, |engine| engine.tag(slot)).unwrap_or(0),
            _ => 0,
        },
        completed_token: entry.map_or(0, |e| e.completion.completed_token()),
        waiter_thread: entry.map_or(0, |e| e.completion.waiter_tid()),
        completed_desc: head.map_or(NO_COMPLETED_DESC, u32::from),
        completed_status: NO_COMPLETED_STATUS,
        last_used_idx: last_used,
        used_idx: state
            .and_then(|_| with_engine(device_index, |engine| u32::from(engine.device_used_idx())))
            .unwrap_or(0),
        device_status: state.map_or(u32::MAX, |state| unsafe {
            crate::arch_impl::aarch64::mmio::read32((block_mmio_virt_base(state) + 0x70) as *const u32)
        }),
    }
}

/// Why a transfer failed, with the chain slot it concerned.
struct TransferFailure {
    reason: &'static str,
    stage: &'static str,
    slot: Option<usize>,
    sector: u64,
}

fn request_error_reason(error: RequestError) -> (&'static str, &'static str) {
    match error {
        RequestError::Media => ("Block MMIO request failed", "finish-status"),
        RequestError::Unsupported => ("Block MMIO request unsupported by the device", "finish-status"),
        RequestError::InvalidStatus(_) => ("Block MMIO request completed without a valid status", "finish-status"),
        RequestError::UsedLength(_) => ("Block MMIO request completed with the wrong used length", "finish-length"),
        RequestError::Lost => (BLOCK_MMIO_WEDGED_ERROR, "finish-lost"),
        RequestError::Pending | RequestError::NotIssued => ("Block MMIO request woke without completion", "take-completion"),
    }
}

/// The data a transfer moves: a read's destination or a write's source.
enum Data<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
    Flush,
}

/// One synchronous request of any length: whole chains, a bounded window of
/// them on the device at once, one notification per window. The caller holds
/// admission for the whole range. Each chain issued is waited for, even
/// after another has failed, so no chain outlives the call; the first failure
/// is returned. A wait that has to be abandoned stops the device instead.
fn transfer(
    device_index: usize,
    state: &BlockDeviceState,
    guard: BlockMmioRequestGuard<'_>,
    start_sector: u64,
    mut data: Data<'_>,
) -> Result<(), TransferFailure> {
    let (kind, total) = match &data {
        Data::Read(buffer) => (request_type::IN, buffer.len()),
        Data::Write(buffer) => (request_type::OUT, buffer.len()),
        Data::Flush => (request_type::FLUSH, 0),
    };
    let completions = &COMPLETIONS[device_index];
    let chunk = state.chain_bytes;
    // The boot thread's bounded polling keeps one request on the device.
    let window = if crate::task::completion::wait_sleeps() { MAX_CHAINS } else { 1 };
    let chunks = if total == 0 { 1 } else { total.div_ceil(chunk) };
    // (slot, token, chunk index), oldest first.
    let mut in_flight = [(0usize, 0u32, 0usize); MAX_CHAINS];
    let (mut head, mut count) = (0usize, 0usize);
    let mut next = 0usize;
    let mut failure: Option<TransferFailure> = None;

    loop {
        // Fill the window, then make it visible with one index store.
        let mut published = false;
        while failure.is_none() && next < chunks && count < window {
            let offset = next * chunk;
            let len = (total - offset).min(chunk);
            let sector = start_sector + (offset / SECTOR_SIZE) as u64;
            let submitted = with_engine(device_index, |engine| {
                if engine.is_stopped() {
                    return Err(Some(SubmitError::Stopped));
                }
                let token = completions.next_completion_token();
                let slot = engine.reserve(token).ok_or(None)?;
                completions.slots[slot].completion.reset();
                let write = match &data {
                    Data::Write(buffer) => Some(&buffer[offset..offset + len]),
                    _ => None,
                };
                match engine.submit(&mut DirectMap, slot, kind, sector, write, len) {
                    Ok(_) => Ok((slot, token)),
                    Err(error) => {
                        engine.unreserve(slot);
                        Err(Some(error))
                    }
                }
            });
            match submitted {
                Some(Ok((slot, token))) => {
                    in_flight[(head + count) % MAX_CHAINS] = (slot, token, next);
                    count += 1;
                    next += 1;
                    published = true;
                }
                Some(Err(None)) if count > 0 => break,
                Some(Err(None)) => {
                    // Each slot belongs to another caller: wait for one.
                    if let Err(reason) = wait_for_slot(device_index) {
                        failure = Some(TransferFailure { reason, stage: "slot-wait", slot: None, sector });
                    }
                }
                Some(Err(Some(SubmitError::Stopped))) | None => {
                    failure = Some(TransferFailure { reason: BLOCK_MMIO_WEDGED_ERROR, stage: "gate-wedged", slot: None, sector });
                }
                Some(Err(Some(_))) => {
                    failure = Some(TransferFailure { reason: "Block MMIO request does not fit its chain", stage: "submit", slot: None, sector });
                }
            }
        }
        if published && with_engine(device_index, |engine| engine.publish()) == Some(true) {
            match VirtioMmioDevice::probe(state.base) {
                Some(device) => device.notify_queue(0),
                None => {
                    // The published chains are the device's now; the driver
                    // cannot tell whether it will complete them.
                    let reason = "Device disappeared";
                    stop_device(device_index, guard);
                    return Err(TransferFailure { reason, stage: "submit", slot: None, sector: start_sector });
                }
            }
        }
        if count == 0 {
            break;
        }

        let (slot, token, index) = in_flight[head];
        let sector = start_sector + (index * chunk / SECTOR_SIZE) as u64;
        if let Err(reason) = completions.slots[slot].wait_for_completion(token, state, kind) {
            stop_device(device_index, guard);
            return Err(TransferFailure { reason, stage: "wait-completion", slot: Some(slot), sector });
        }
        head = (head + 1) % MAX_CHAINS;
        count -= 1;
        let offset = index * chunk;
        let len = (total - offset).min(chunk);
        let result = with_engine(device_index, |engine| {
            let out = match &mut data {
                Data::Read(buffer) => Some(&mut buffer[offset..offset + len]),
                _ => None,
            };
            engine.finish(&mut DirectMap, slot, out)
        });
        match result {
            Some(Ok(())) => {}
            Some(Err(RequestError::Lost)) | None => {
                // Another caller stopped the queue; the chain stays owned.
                failure.get_or_insert(TransferFailure { reason: BLOCK_MMIO_WEDGED_ERROR, stage: "finish-lost", slot: Some(slot), sector });
                continue;
            }
            Some(Err(error)) => {
                let (reason, stage) = request_error_reason(error);
                failure.get_or_insert(TransferFailure { reason, stage, slot: Some(slot), sector });
            }
        }
        REQUEST_GATES[device_index].waiters.wake_up();
    }
    drop(guard);
    match failure {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}

/// Wait until a chain slot is free, as admission waits for a range.
fn wait_for_slot(device_index: usize) -> Result<(), &'static str> {
    let gate = &REQUEST_GATES[device_index];
    let free = || with_engine(device_index, |engine| engine.free_slots() > 0 || engine.is_stopped()).unwrap_or(true);
    if free() {
        return Ok(());
    }
    if !block_mmio_request_gate_can_sleep() {
        return Err("Block MMIO request already in progress (context cannot sleep)");
    }
    if gate.waiters.prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO).is_none() {
        return Err("Block MMIO request already in progress (waiter publication refused)");
    }
    if !free() {
        crate::task::waitqueue::schedule_current_wait();
    }
    gate.waiters.finish_wait();
    Ok(())
}

fn admission_stage(reason: &'static str) -> &'static str {
    match reason {
        "Block MMIO request already in progress (context cannot sleep)" => "gate-cannot-sleep",
        "Block MMIO request already in progress (waiter publication refused)" => "gate-publish-refused",
        "Block MMIO request gate refused a kill-claimed thread" => "gate-kill-claimed",
        _ => "gate-wedged",
    }
}

/// Read a sector from the block device at given device_index.
pub fn read_sector(
    device_index: usize,
    sector: u64,
    buffer: &mut [u8; SECTOR_SIZE],
) -> Result<(), &'static str> {
    read_sectors(device_index, sector, buffer).map_err(|failure| failure.reason)
}

/// Read consecutive sectors into `buffer`, a nonzero multiple of the sector
/// size, in a window of chains.
pub fn read_sectors(
    device_index: usize,
    start_sector: u64,
    buffer: &mut [u8],
) -> Result<(), VirtioReadFailure> {
    let state = block_device_state(device_index).map_err(|reason|
        read_failure(device_index, None, start_sector, buffer.len(), reason, "device-state"))?;
    if buffer.is_empty() || buffer.len() % SECTOR_SIZE != 0 {
        return Err(read_failure(device_index, Some(state), start_sector, buffer.len(),
            "Buffer size must be a nonzero multiple of 512", "read-length"));
    }
    let end = start_sector.checked_add((buffer.len() / SECTOR_SIZE) as u64);
    let Some(end) = end.filter(|end| *end <= state.capacity) else {
        return Err(read_failure(device_index, Some(state), start_sector, buffer.len(),
            "Sector out of range", "read-range"));
    };
    if !irq_completion_available() {
        return Err(read_failure(device_index, Some(state), start_sector, buffer.len(),
            "Block MMIO IRQ completion unavailable before interrupts are enabled", "irq-unavailable"));
    }
    let bytes = buffer.len();
    let guard = REQUEST_GATES[device_index].admit(start_sector, end, false).map_err(|reason|
        read_failure(device_index, Some(state), start_sector, bytes, reason, admission_stage(reason)))?;
    transfer(device_index, state, guard, start_sector, Data::Read(buffer)).map_err(|failure|
        read_failure_at(device_index, Some(state), failure.slot, failure.sector, bytes, failure.reason, failure.stage))
}

/// Write a sector to the block device at given device_index.
pub fn write_sector(
    device_index: usize,
    sector: u64,
    buffer: &[u8; SECTOR_SIZE],
) -> Result<(), &'static str> {
    write_sectors(device_index, sector, buffer)
}

/// Write consecutive sectors from `buffer`, a nonzero multiple of the sector
/// size, in a window of chains. The first failed chain fails the call once
/// each chain issued has returned; a flush waits for them.
pub fn write_sectors(
    device_index: usize,
    start_sector: u64,
    buffer: &[u8],
) -> Result<(), &'static str> {
    let state = block_device_state(device_index)?;
    if state.device_features & features::RO != 0 { return Err("Read-only filesystem"); }
    if buffer.is_empty() || buffer.len() % SECTOR_SIZE != 0 {
        return Err("Buffer size must be a nonzero multiple of 512");
    }
    let end = start_sector
        .checked_add((buffer.len() / SECTOR_SIZE) as u64)
        .filter(|end| *end <= state.capacity)
        .ok_or("Sector out of range")?;
    if !irq_completion_available() {
        return Err("Block MMIO IRQ completion unavailable before interrupts are enabled");
    }
    let guard = REQUEST_GATES[device_index].admit(start_sector, end, true)?;
    transfer(device_index, state, guard, start_sector, Data::Write(buffer)).map_err(|failure| failure.reason)
}

/// Persist prior writes before reporting a filesystem durability barrier.
/// Devices without FLUSH remain in writethrough mode. The flush holds the
/// whole device, so it waits for the writes in flight and later requests wait
/// for it, from the moment it starts waiting.
pub fn flush(device_index: usize) -> Result<(), &'static str> {
    let state = block_device_state(device_index)?;
    if state.device_features & features::RO != 0 || state.device_features & features::FLUSH == 0 {
        return Ok(());
    }
    if !irq_completion_available() {
        return Err("Block MMIO IRQ completion unavailable before interrupts are enabled");
    }
    let guard = REQUEST_GATES[device_index].admit_flush()?;
    transfer(device_index, state, guard, 0, Data::Flush).map_err(|failure| failure.reason)
}

/// Return the GIC SPI assigned to a VirtIO MMIO block device.
pub fn get_irq(device_index: usize) -> Option<u32> {
    if device_index >= MAX_BLOCK_DEVICES {
        return None;
    }

    unsafe {
        let ptr = &raw const BLOCK_DEVICES;
        (*ptr)[device_index]
            .as_ref()
            .map(|state| VIRTIO_IRQ_BASE + state.slot as u32)
    }
}

#[inline]
fn block_mmio_virt_base(state: &BlockDeviceState) -> u64 {
    crate::memory::physical_memory_offset().as_u64() + state.base
}

/// The device can no longer complete the requests it was given: it asks for
/// a reset, it has been reset (DRIVER_OK is gone), or it no longer answers
/// (a register that has gone away reads as all ones).
fn device_has_failed(state: &BlockDeviceState) -> bool {
    use super::mmio::status::{DEVICE_NEEDS_RESET, DRIVER_OK};
    // Offset 0x70 is the MMIO transport's Status register.
    let status = unsafe { crate::arch_impl::aarch64::mmio::read32((block_mmio_virt_base(state) + 0x70) as *const u32) };
    status == u32::MAX || status & DRIVER_OK == 0 || status & DEVICE_NEEDS_RESET != 0
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Report a request the device has not completed yet. One line at most per
/// interval across all devices; it changes nothing about the request.
fn report_slow_request(request: u32, waited_ns: u64) {
    if waited_ns < BLOCK_MMIO_SLOW_REPORT_INTERVAL_NS {
        return;
    }
    let now = monotonic_now_ns();
    let last = LAST_SLOW_REPORT_NS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < BLOCK_MMIO_SLOW_REPORT_INTERVAL_NS {
        return;
    }
    if LAST_SLOW_REPORT_NS
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        log::warn!(
            "VirtIO block: request type {} still waiting for the device after {} ms",
            request,
            waited_ns / 1_000_000
        );
        for device_index in 0..device_count() {
            report_queue_state(device_index);
        }
    }
}

/// Failure-only report of one device's queue: whether the device returned
/// chains the driver has not drained, and each slot's state and tokens.
fn report_queue_state(device_index: usize) {
    let Ok(state) = block_device_state(device_index) else {
        return;
    };
    let queue = &QUEUES[device_index];
    let sample = crate::arch_without_interrupts(|| {
        queue.engine.try_lock().map(|mut guard| {
            guard.as_mut().map(|engine| {
                let mut slots = [' '; MAX_CHAINS];
                let mut tags = [0u32; MAX_CHAINS];
                for (slot, state) in slots.iter_mut().enumerate().take(engine.chains()) {
                    *state = engine.slot_state(slot);
                    tags[slot] = engine.tag(slot);
                }
                (engine.device_used_idx(), engine.last_used(), engine.published_idx(),
                    engine.outstanding(), engine.free_descriptors(), engine.is_stopped(), slots, tags)
            })
        })
    });
    // As sampled; the drain below then takes what an interrupt left while
    // the sample held the lock.
    let pending = queue.drain_pending.load(Ordering::Acquire);
    drain_after_release(device_index);
    let base = block_mmio_virt_base(state);
    let (interrupt_status, device_status) = unsafe {
        (
            crate::arch_impl::aarch64::mmio::read32((base + 0x60) as *const u32),
            crate::arch_impl::aarch64::mmio::read32((base + 0x70) as *const u32),
        )
    };
    match sample {
        Some(Some((used, last_used, published, outstanding, free, stopped, slots, tags))) => {
            log::warn!(
            "VirtIO block {}: used={} last_used={} published={} outstanding={} free_desc={} stopped={} drain_pending={} isr={:#x} status={:#x} slots={}",
            device_index, used, last_used, published, outstanding, free, stopped, pending,
            interrupt_status, device_status, slots.iter().collect::<alloc::string::String>()
            );
            for (slot, entry) in COMPLETIONS[device_index].slots.iter().enumerate() {
                if tags[slot] != 0 {
                    log::warn!(
                        "VirtIO block {} slot {}: tag={} done={} waiter={}",
                        device_index, slot, tags[slot], entry.completion.completed_token(), entry.completion.waiter_tid()
                    );
                }
            }
        }
        _ => log::warn!(
            "VirtIO block {}: queue lock busy; drain_pending={} isr={:#x} status={:#x}",
            device_index, pending, interrupt_status, device_status
        ),
    }
}

/// Handle a VirtIO MMIO block interrupt.
///
/// Hard IRQ path: no logging, allocation or unbounded work, and the queue
/// lock is only tried, not waited for.
pub fn handle_interrupt(device_index: usize) {
    let Ok(state) = block_device_state(device_index) else {
        return;
    };

    let base = block_mmio_virt_base(state);
    let interrupt_status = unsafe { crate::arch_impl::aarch64::mmio::read32((base + 0x60) as *const u32) };
    if interrupt_status == 0 {
        return;
    }

    unsafe {
        crate::arch_impl::aarch64::mmio::write32((base + 0x64) as *mut u32, interrupt_status);
    }
    dsb_sy();

    drain_device(device_index);
}

/// Get the number of initialized block devices
pub fn device_count() -> usize {
    DEVICE_COUNT.load(core::sync::atomic::Ordering::Acquire)
}

/// Get the capacity in sectors for a given device
pub fn capacity(device_index: usize) -> Option<u64> {
    if device_index >= MAX_BLOCK_DEVICES {
        return None;
    }
    unsafe {
        let ptr = &raw const BLOCK_DEVICES;
        (*ptr)[device_index].as_ref().map(|s| s.capacity)
    }
}

/// Check if the block device is read-only
///
/// Returns true if the device has the VIRTIO_BLK_F_RO feature bit set,
/// meaning write operations will fail. Returns None if device not initialized.
pub fn is_readonly(device_index: usize) -> Option<bool> {
    if device_index >= MAX_BLOCK_DEVICES {
        return None;
    }
    unsafe {
        let ptr = &raw const BLOCK_DEVICES;
        (*ptr)[device_index]
            .as_ref()
            .map(|s| s.device_features & features::RO != 0)
    }
}

/// Test the block device by reading sector 0
pub fn test_read() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-blk] Testing read of sector 0...");

    let mut buffer = [0u8; SECTOR_SIZE];
    read_sector(0, 0, &mut buffer)?;

    // Print first 32 bytes
    crate::serial_print!("[virtio-blk] Sector 0 data: ");
    for i in 0..32 {
        crate::serial_print!("{:02x} ", buffer[i]);
    }
    crate::serial_println!("...");

    crate::serial_println!("[virtio-blk] Read test passed!");
    Ok(())
}

/// Stress test the block device by reading sector 0 multiple times in rapid succession.
/// This exercises the DSB barrier and descriptor/notification path repeatedly.
pub fn test_multi_read() -> Result<(), &'static str> {
    const READ_COUNT: usize = 10;
    crate::serial_println!(
        "[virtio-blk] Starting multi-read stress test ({} reads)...",
        READ_COUNT
    );

    let mut buffer = [0u8; SECTOR_SIZE];

    for i in 0..READ_COUNT {
        read_sector(0, 0, &mut buffer)?;
        crate::serial_println!("[virtio-blk] Read {} of {} complete", i + 1, READ_COUNT);
    }

    crate::serial_println!("[virtio-blk] Multi-read stress test passed!");
    Ok(())
}

/// Test sequential sector reads to verify queue index wrap-around behavior.
///
/// The virtqueue has 16 entries, so reading 32+ sectors causes the available
/// ring index to wrap around twice. This tests that wrap-around handling is
/// correct (originally crashed at sector 32 due to wrap-around issues).
///
/// The disk capacity is 16384 sectors (8 MB), so sectors 0-31 are valid.
pub fn test_sequential_read() -> Result<(), &'static str> {
    const NUM_SECTORS: u64 = 32; // 2x wrap-around for 16-entry queue

    crate::serial_println!(
        "[virtio-blk] Testing sequential read of sectors 0-{}...",
        NUM_SECTORS - 1
    );

    let mut buffer = [0u8; SECTOR_SIZE];

    for sector in 0..NUM_SECTORS {
        read_sector(0, sector, &mut buffer)?;

        // Log progress every 8 sectors
        if sector % 8 == 7 {
            crate::serial_println!(
                "[virtio-blk] Read sectors 0-{} OK (avail_idx wrap count: {})",
                sector,
                (sector + 1) / 16
            );
        }
    }

    crate::serial_println!(
        "[virtio-blk] Sequential read test passed! ({} sectors, {} queue wraps)",
        NUM_SECTORS,
        NUM_SECTORS / 16
    );
    Ok(())
}

/// Test that reading an invalid (out-of-range) sector returns an error
///
/// The disk capacity is typically 16384 sectors (8MB), so reading beyond that is invalid.
/// This test verifies that the driver returns an appropriate error rather than panicking.
pub fn test_invalid_sector() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-blk] Testing invalid sector read...");

    // First verify device is initialized and get capacity
    let cap = capacity(0).ok_or("Block device not initialized for invalid sector test")?;
    crate::serial_println!("[virtio-blk] Device capacity: {} sectors", cap);

    // Try to read a sector that's definitely beyond capacity
    let invalid_sector = cap + 1000; // Well beyond the end
    let mut buffer = [0u8; SECTOR_SIZE];

    match read_sector(0, invalid_sector, &mut buffer) {
        Ok(_) => {
            crate::serial_println!(
                "[virtio-blk] ERROR: Read of invalid sector {} succeeded unexpectedly!",
                invalid_sector
            );
            Err("Invalid sector read should have failed but succeeded")
        }
        Err(e) => {
            crate::serial_println!("[virtio-blk] Invalid sector correctly rejected: {}", e);
            // Only the range check may answer: any other error means the
            // request reached (or failed to reach) the device.
            if e == "Sector out of range" {
                crate::serial_println!("[virtio-blk] Invalid sector test passed!");
                Ok(())
            } else {
                crate::serial_println!("[virtio-blk] Got unexpected error: {}", e);
                Err("Invalid sector read failed with an error other than 'Sector out of range'")
            }
        }
    }
}

/// Test behavior when attempting to read from a slot with no device
///
/// The boot has already initialized the devices it found, so the test reads
/// from the first slot that holds no device and requires the exact
/// "Block device not initialized" refusal. If every slot holds a device it
/// reads from the first index past the table and requires "Invalid device
/// index" instead, the same lookup's other refusal.
pub fn test_uninitialized_read() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-blk] Testing uninitialized device handling...");

    let (device_index, expected) = match (0..MAX_BLOCK_DEVICES).find(|&i| capacity(i).is_none())
    {
        Some(free) => (free, "Block device not initialized"),
        None => (MAX_BLOCK_DEVICES, "Invalid device index"),
    };
    crate::serial_println!(
        "[virtio-blk] Reading from device slot {} (no device there)...",
        device_index
    );

    let mut buffer = [0u8; SECTOR_SIZE];
    match read_sector(device_index, 0, &mut buffer) {
        Ok(_) => {
            crate::serial_println!("[virtio-blk] ERROR: Read succeeded on uninitialized device!");
            Err("Read should fail on uninitialized device")
        }
        Err(e) if e == expected => {
            crate::serial_println!("[virtio-blk] Correctly rejected with: {}", e);
            crate::serial_println!("[virtio-blk] Uninitialized test passed!");
            Ok(())
        }
        Err(e) => {
            crate::serial_println!("[virtio-blk] Unexpected error: {}", e);
            Err("Read from a slot with no device failed with the wrong error")
        }
    }
}

/// Test write-read-verify cycle to exercise the write_sector() path
///
/// This test:
/// 1. Checks if device is read-only (skip if so)
/// 2. Saves original sector data
/// 3. Writes a known pattern to a high sector number (sector 1000)
/// 4. Reads the sector back
/// 5. Verifies data matches byte-for-byte
/// 6. Restores original data (if possible)
///
/// Uses a high sector number (1000) to avoid damaging filesystem metadata
/// which is typically in the first few sectors.
pub fn test_write_read_verify() -> Result<(), &'static str> {
    const TEST_SECTOR: u64 = 1000; // High sector to avoid filesystem damage

    crate::serial_println!("[virtio-blk] Testing write-read-verify cycle...");

    // Check if device is initialized
    let cap = capacity(0).ok_or("Block device not initialized")?;
    crate::serial_println!("[virtio-blk] Device capacity: {} sectors", cap);

    // Verify test sector is within capacity
    if TEST_SECTOR >= cap {
        crate::serial_println!(
            "[virtio-blk] Test sector {} beyond capacity {}, skipping",
            TEST_SECTOR,
            cap
        );
        return Ok(()); // Skip gracefully
    }

    // Check if device is read-only
    if let Some(true) = is_readonly(0) {
        crate::serial_println!("[virtio-blk] Device is read-only, skipping write test");
        return Ok(()); // Skip gracefully
    }

    // Save original sector data
    let mut original = [0u8; SECTOR_SIZE];
    crate::serial_println!(
        "[virtio-blk] Reading original data from sector {}...",
        TEST_SECTOR
    );
    read_sector(0, TEST_SECTOR, &mut original)?;
    crate::serial_println!(
        "[virtio-blk] Original first 16 bytes: {:02x?}",
        &original[..16]
    );

    // Create test pattern: alternating 0xAA and sequence bytes
    let mut test_pattern = [0u8; SECTOR_SIZE];
    for i in 0..SECTOR_SIZE {
        test_pattern[i] = if i % 2 == 0 { 0xAA } else { (i & 0xFF) as u8 };
    }
    crate::serial_println!(
        "[virtio-blk] Test pattern first 16 bytes: {:02x?}",
        &test_pattern[..16]
    );

    // Write test pattern
    crate::serial_println!(
        "[virtio-blk] Writing test pattern to sector {}...",
        TEST_SECTOR
    );
    match write_sector(0, TEST_SECTOR, &test_pattern) {
        Ok(()) => {
            crate::serial_println!("[virtio-blk] Write succeeded");
        }
        Err(e) => {
            // The device advertised itself writable, so a failed write is a
            // failed test, not a skip.
            crate::serial_println!("[virtio-blk] Write failed: {}", e);
            return Err("Write to a writable device failed");
        }
    }

    // Read back
    let mut readback = [0u8; SECTOR_SIZE];
    crate::serial_println!("[virtio-blk] Reading back sector {}...", TEST_SECTOR);
    read_sector(0, TEST_SECTOR, &mut readback)?;
    crate::serial_println!(
        "[virtio-blk] Readback first 16 bytes: {:02x?}",
        &readback[..16]
    );

    // Verify data matches
    let mut mismatches = 0;
    for i in 0..SECTOR_SIZE {
        if readback[i] != test_pattern[i] {
            if mismatches < 10 {
                crate::serial_println!(
                    "[virtio-blk] Mismatch at byte {}: expected {:02x}, got {:02x}",
                    i,
                    test_pattern[i],
                    readback[i]
                );
            }
            mismatches += 1;
        }
    }

    // Restore the original data; a test that leaves its pattern behind fails.
    crate::serial_println!(
        "[virtio-blk] Restoring original data to sector {}...",
        TEST_SECTOR
    );
    if let Err(e) = write_sector(0, TEST_SECTOR, &original) {
        crate::serial_println!("[virtio-blk] Failed to restore original data: {}", e);
        return Err("Write-read-verify could not restore the test sector");
    }
    let mut restored = [0u8; SECTOR_SIZE];
    read_sector(0, TEST_SECTOR, &mut restored)?;
    if restored != original {
        crate::serial_println!("[virtio-blk] Restored sector does not match the original");
        return Err("Write-read-verify left the test sector modified");
    }

    // Report result
    if mismatches == 0 {
        crate::serial_println!(
            "[virtio-blk] Write-read-verify test passed! All {} bytes match.",
            SECTOR_SIZE
        );
        Ok(())
    } else {
        crate::serial_println!(
            "[virtio-blk] Write-read-verify test FAILED! {} mismatches out of {} bytes",
            mismatches,
            SECTOR_SIZE
        );
        Err("Write-read-verify data mismatch")
    }
}
