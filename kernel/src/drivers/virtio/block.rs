//! VirtIO Block Device Driver
//!
//! Implements a block device driver using the VirtIO block device protocol.
//!
//! # VirtIO Block Request Format
//!
//! Each request consists of three parts chained together:
//! 1. Request header (VirtioBlkReq) - read by device
//! 2. Data buffer - read/write depending on request type
//! 3. Status byte - written by device
//!
//! # Device Configuration
//!
//! The device-specific configuration space contains:
//! - capacity (u64 at offset 0): Disk size in 512-byte sectors
//! - size_max (u32 at offset 8): Max segment size
//! - seg_max (u32 at offset 12): Max number of segments
//! - geometry (at offset 16): Disk geometry


use super::blk_config;
use super::blk_queue::{
    self, request_type, Admission, BlkQueue, ChainStorage, Desc, DmaMemory, Drain, RangeTable,
    RequestError, RingMemory, SubmitError, MAX_CHAINS, MAX_SEGMENTS,
};
use super::pci_transport::VirtioPciDevice;
use super::queue::{VirtqAvail, VirtqDesc, VirtqUsed, Virtqueue};
use super::VirtioDevice;
use crate::drivers::pci::Device as PciDevice;
use crate::memory::frame_allocator;
use crate::task::completion::Completion;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

/// A request waits for its completion in sleeps of this length, checking
/// between them that the device has not failed.
const BLOCK_WAIT_SLICE_NS: u64 = 1_000_000_000;
/// A request outstanding this long is reported, at most once per interval.
const BLOCK_SLOW_REPORT_INTERVAL_NS: u64 = 10_000_000_000;
/// A wait that cannot sleep (before the scheduler, or on the boot thread
/// before its timer runs) busy-polls the CPU, so it is bounded: past this the
/// request is abandoned, the queue is stopped so its DMA buffers are not
/// reused, and the boot carries on without the disk.
const BLOCK_BOOTSTRAP_WAIT_LIMIT_NS: u64 = 30_000_000_000;
/// When a slow request was last reported, so reports stay rate-limited.
static LAST_SLOW_REPORT_NS: AtomicU64 = AtomicU64::new(0);
const WEDGED_ERROR: &str = "Block device wedged after an abandoned request";
/// Callers admitted to one device at once.
const MAX_ADMITTED: usize = 32;
/// Data frames each chain slot owns: 64 KiB, the block cache's RUN_BYTES.
const CHAIN_FRAMES: usize = MAX_SEGMENTS;
/// Data frames one device may hold for its chains.
const DATA_BUDGET_BYTES: usize = 512 * 1024;

/// Admission to a device: a caller holds its sector range from before its
/// first chain is built until its last result is taken. Overlapping ranges
/// wait for each other when either writes; a flush holds the whole device.
struct BlockRequestGate {
    wedged: AtomicBool,
    ranges: Mutex<RangeTable<MAX_ADMITTED>>,
    /// Callers waiting for a range or for a chain slot.
    waiters: crate::task::waitqueue::WaitQueueHead,
}

struct BlockRequestGuard<'a> {
    gate: &'a BlockRequestGate,
    entry: usize,
    release_on_drop: bool,
    /// The admitted caller's context: it waits for its commands by
    /// sleeping, or polls them in early boot.
    io: IoContext,
}

use crate::task::park::IoContext;

/// A request from a context that may neither park nor poll (design §6).
const NOT_SLEEPABLE_ERROR: &str = "Block request from a context that may not sleep";

impl BlockRequestGate {
    const fn new() -> Self {
        Self {
            wedged: AtomicBool::new(false),
            ranges: Mutex::new(RangeTable::new()),
            waiters: crate::task::waitqueue::WaitQueueHead::new(),
        }
    }

    /// Admit the sectors `[start, end)` now if they are free, behind any
    /// flush that holds its turn.
    fn try_admit(&self, start: u64, end: u64, write: bool) -> Option<usize> {
        self.with_table(|table| table.try_admit(start, end, write))
    }

    /// One admission attempt. A flush (`flush` is its turn) that has to wait
    /// holds its turn; a caller that cannot sleep tries once, ahead of
    /// waiting flushes, because it cannot wait its turn.
    fn attempt(&self, start: u64, end: u64, write: bool, flush: Option<&mut bool>, can_sleep: bool) -> Option<usize> {
        match flush {
            _ if !can_sleep => self.with_table(|table| table.try_admit_ahead(start, end, write)),
            Some(queued) => self.with_table(|table| table.try_admit_flush(queued)),
            None => self.try_admit(start, end, write),
        }
    }

    fn with_table(&self, f: impl FnOnce(&mut RangeTable<MAX_ADMITTED>) -> Admission) -> Option<usize> {
        x86_64::instructions::interrupts::without_interrupts(|| match f(&mut self.ranges.lock()) {
            Admission::Admitted(entry) => Some(entry),
            Admission::Wait => None,
        })
    }

    /// Admit the sectors `[start, end)`, waiting for conflicting requests.
    fn admit(&self, start: u64, end: u64, write: bool) -> Result<BlockRequestGuard<'_>, &'static str> {
        self.admit_claim(start, end, write, false)
    }

    /// Admit a flush for the whole device, waiting its turn.
    fn admit_flush(&self) -> Result<BlockRequestGuard<'_>, &'static str> {
        self.admit_claim(0, u64::MAX, true, true)
    }

    fn admit_claim(&self, start: u64, end: u64, write: bool, flush: bool) -> Result<BlockRequestGuard<'_>, &'static str> {
        if self.wedged.load(Ordering::Acquire) {
            return Err(WEDGED_ERROR);
        }
        let mut queued = false;
        let entry = self.wait_for_admission(start, end, write, flush.then_some(&mut queued));
        if queued {
            // A flush that gave up: requests waiting behind it go ahead.
            x86_64::instructions::interrupts::without_interrupts(|| self.ranges.lock().cancel_flush(&mut queued));
            self.waiters.wake_up();
        }
        entry.map(|(entry, io)| BlockRequestGuard { gate: self, entry, release_on_drop: true, io })
    }

    /// The block I/O caller rule (design §6): a context that may park waits
    /// its turn; early boot, the only thread, tries once; any other context
    /// is refused with a receipt. Returns the entry and the caller's context,
    /// which picks how its commands are waited for.
    fn wait_for_admission(&self, start: u64, end: u64, write: bool, mut flush: Option<&mut bool>) -> Result<(usize, IoContext), &'static str> {
        use crate::task::park::{self, WaitEnd, WaitMode};
        let io = park::io_context().map_err(|_| NOT_SLEEPABLE_ERROR)?;
        let sleeps = io == IoContext::Sleepable;
        loop {
            if self.wedged.load(Ordering::Acquire) {
                return Err(WEDGED_ERROR);
            }
            if let Some(entry) = self.attempt(start, end, write, flush.as_deref_mut(), sleeps) {
                return Ok((entry, io));
            }
            if !sleeps {
                return Err("Block request already in progress during early boot");
            }
            // The attempt runs under the queue lock, atomically with
            // publishing the wait, so a release before the enqueue is seen.
            let mut admitted = None;
            let ended = park::wait(
                &self.waiters,
                || {
                    if self.wedged.load(Ordering::Acquire) {
                        return true;
                    }
                    admitted = self.attempt(start, end, write, flush.as_deref_mut(), true);
                    admitted.is_some()
                },
                None,
                WaitMode::Uninterruptible,
            );
            if let Some(entry) = admitted {
                return Ok((entry, io));
            }
            match ended {
                Ok(WaitEnd::NoMemory) => return Err("Block request waiter publication refused"),
                Ok(WaitEnd::Killed) => return Err("Block request waiter publication refused"),
                Ok(_) => {}
                Err(_) => return Err(NOT_SLEEPABLE_ERROR),
            }
        }
    }

    fn release(&self, entry: usize) {
        x86_64::instructions::interrupts::without_interrupts(|| self.ranges.lock().release(entry));
        self.waiters.wake_up();
    }
}

impl BlockRequestGuard<'_> {
    /// The device keeps a request it may still complete: its range stays
    /// held and later callers are refused.
    fn wedge(mut self) {
        self.release_on_drop = false;
        self.gate.wedged.store(true, Ordering::Release);
        self.gate.waiters.wake_up();
    }
}

impl Drop for BlockRequestGuard<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.gate.release(self.entry);
        }
    }
}

/// VirtIO block feature bits
mod features {
    pub const RO: u32 = 1 << 5;
    pub const FLUSH: u32 = 1 << 9;
}

/// Sector size in bytes
pub const SECTOR_SIZE: usize = blk_queue::SECTOR_BYTES;

/// Sectors the sector cache holds: 512 KiB.
const CACHE_SECTORS: usize = 1024;
const CACHE_EMPTY: u64 = u64::MAX;

struct CachedSector {
    sector: u64,
    data: [u8; SECTOR_SIZE],
}

/// A write-through copy of recently transferred sectors. The filesystem reads
/// the same metadata sectors (inodes, directory blocks, bitmaps, the
/// superblock) for every operation, and each device request waits for the
/// device, so without it a file creation is dozens of requests. Direct-mapped:
/// sector `s` can only be in slot `s % CACHE_SECTORS`. Every store and
/// invalidation is made with the request gate held, right after the transfer
/// it reflects, so the cache changes in device order; a lookup takes only the
/// slot lock and sees the disk as of the last completed request. Allocated
/// once at device initialization; a device that cannot have one runs without.
struct SectorCache {
    slots: Mutex<Vec<CachedSector>>,
}

impl SectorCache {
    fn new() -> Option<Self> {
        let mut slots = Vec::new();
        slots.try_reserve_exact(CACHE_SECTORS).ok()?;
        slots.resize_with(CACHE_SECTORS, || CachedSector { sector: CACHE_EMPTY, data: [0; SECTOR_SIZE] });
        Some(Self { slots: Mutex::new(slots) })
    }

    /// The slot lock is never taken by the interrupt handler, but a holder
    /// must not be preempted by a thread that then spins on it.
    fn with_slots<R>(&self, f: impl FnOnce(&mut [CachedSector]) -> R) -> R {
        x86_64::instructions::interrupts::without_interrupts(|| f(&mut self.slots.lock()))
    }

    /// Copy `buffer.len() / SECTOR_SIZE` sectors from `sector` on into
    /// `buffer` if every one of them is cached.
    fn lookup(&self, sector: u64, buffer: &mut [u8]) -> bool {
        self.with_slots(|slots| {
            let all = buffer.chunks(SECTOR_SIZE).zip(sector..).all(|(_, s)| slots[(s % CACHE_SECTORS as u64) as usize].sector == s);
            if all {
                for (chunk, s) in buffer.chunks_mut(SECTOR_SIZE).zip(sector..) {
                    chunk.copy_from_slice(&slots[(s % CACHE_SECTORS as u64) as usize].data);
                }
            }
            all
        })
    }

    /// Record sectors as the device now holds them. The request gate is held.
    fn store(&self, sector: u64, buffer: &[u8]) {
        self.with_slots(|slots| {
            for (chunk, s) in buffer.chunks(SECTOR_SIZE).zip(sector..) {
                let slot = &mut slots[(s % CACHE_SECTORS as u64) as usize];
                slot.sector = s;
                slot.data.copy_from_slice(chunk);
            }
        })
    }

    /// Forget sectors whose contents on the device are unknown, after a
    /// write that did not complete. The request gate is held.
    fn invalidate(&self, sector: u64, len: usize) {
        self.with_slots(|slots| {
            for s in sector..sector + (len / SECTOR_SIZE) as u64 {
                let slot = &mut slots[(s % CACHE_SECTORS as u64) as usize];
                if slot.sector == s {
                    slot.sector = CACHE_EMPTY;
                }
            }
        })
    }
}

/// Both PCI interfaces share the same split queue, DMA buffers and completions.
enum BlockTransport {
    Legacy(VirtioDevice),
    Modern(VirtioPciDevice),
}

impl BlockTransport {
    fn notify_queue(&self, queue: u16) {
        match self {
            Self::Legacy(device) => device.notify_queue(queue),
            Self::Modern(device) => device.notify_queue_fast(queue as u32),
        }
    }

    fn read_isr(&self) -> u8 {
        match self {
            Self::Legacy(device) => device.read_isr(),
            Self::Modern(device) => device.read_interrupt_status() as u8,
        }
    }

    /// The device can no longer complete the requests it was given: it asks
    /// for a reset, it has been reset (DRIVER_OK is gone), or it no longer
    /// answers (a function that has gone away reads as all ones).
    fn has_failed(&self) -> bool {
        use super::status::{DEVICE_NEEDS_RESET, DRIVER_OK};
        let status = match self {
            Self::Legacy(device) => device.read_status(),
            Self::Modern(device) => device.read_status(),
        };
        status == u8::MAX || status & DRIVER_OK == 0 || status & DEVICE_NEEDS_RESET != 0
    }
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}


/// Report a request the device has not completed yet. One line at most per
/// interval across all devices; it changes nothing about the request.
fn report_slow_request(device: &VirtioBlockDevice, request: u32, waited_ns: u64) {
    if waited_ns < BLOCK_SLOW_REPORT_INTERVAL_NS {
        return;
    }
    let now = monotonic_now_ns();
    let last = LAST_SLOW_REPORT_NS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < BLOCK_SLOW_REPORT_INTERVAL_NS {
        return;
    }
    if LAST_SLOW_REPORT_NS
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        // Read used memory and controller state only. Reading the VirtIO ISR
        // here would clear the device's interrupt and steal its completion.
        let (used, seen, outstanding, apic_state) =
            x86_64::instructions::interrupts::without_interrupts(|| {
                let (used, seen, outstanding) = device.with_engine(|engine| {
                    (engine.device_used_idx(), engine.last_used(), engine.outstanding())
                });
                #[cfg(target_arch = "x86_64")]
                let apic_state = if crate::arch_impl::x86_64::apic::active() {
                    crate::arch_impl::x86_64::ioapic::route_state(device.interrupt_line).map(
                        |(low, high)| {
                            let vector = low as u8;
                            let (irr, isr) =
                                crate::arch_impl::x86_64::apic::vector_pending_state(vector);
                            (low, high, vector, irr, isr)
                        },
                    )
                } else {
                    None
                };
                #[cfg(not(target_arch = "x86_64"))]
                let apic_state: Option<(u32, u32, u8, bool, bool)> = None;
                (used, seen, outstanding, apic_state)
            });
        log::warn!("VirtIO block: request type {} still waiting after {} ms; IRQ {} used={} last_seen={} outstanding={}",
            request, waited_ns / 1_000_000, device.interrupt_line, used, seen, outstanding);
        if let Some((low, high, vector, irr, isr)) = apic_state {
            log::warn!("VirtIO block: IRQ {} vector {} low={:#010x} high={:#010x} remote_irr={} delivery_pending={} masked={} LAPIC_IRR={} LAPIC_ISR={}",
                device.interrupt_line, vector, low, high, low & (1 << 14) != 0,
                low & (1 << 12) != 0, low & (1 << 16) != 0, irr, isr);
        }
    }
}

/// The device's split ring, in the queue memory `Virtqueue` allocated.
struct PciRing {
    desc: *mut VirtqDesc,
    avail: *mut VirtqAvail,
    used: *mut VirtqUsed,
}

// Reached only through the device's engine lock.
unsafe impl Send for PciRing {}

impl RingMemory for PciRing {
    fn write_desc(&mut self, index: u16, desc: Desc) {
        let desc = VirtqDesc { addr: desc.addr, len: desc.len, flags: desc.flags, next: desc.next };
        unsafe { core::ptr::write_volatile(self.desc.add(index as usize), desc) }
    }

    fn write_avail_entry(&mut self, pos: u16, head: u16) {
        unsafe { core::ptr::write_volatile(&raw mut (*self.avail).ring[pos as usize], head) }
    }

    fn barrier_before_avail_idx(&mut self) {
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    fn write_avail_idx(&mut self, idx: u16) {
        unsafe { core::ptr::write_volatile(&raw mut (*self.avail).idx, idx) }
    }

    fn barrier_before_notify(&mut self) {
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    fn read_used_idx(&mut self) -> u16 {
        let idx = unsafe { core::ptr::read_volatile(&raw const (*self.used).idx) };
        core::sync::atomic::fence(Ordering::SeqCst);
        idx
    }

    fn read_used_elem(&mut self, pos: u16) -> (u32, u32) {
        unsafe {
            let elem = &raw const (*self.used).ring[pos as usize];
            (core::ptr::read_volatile(&raw const (*elem).id), core::ptr::read_volatile(&raw const (*elem).len))
        }
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
}

/// The data a transfer moves: a read's destination or a write's source.
enum Data<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
    Flush,
}

/// VirtIO block device driver
pub struct VirtioBlockDevice {
    /// VirtIO device abstraction
    device: BlockTransport,
    interrupt_line: u8,
    /// The queue memory; the engine below builds and drains its ring.
    _queue: Virtqueue,
    /// Request chains in flight on queue 0. The interrupt handler drains it,
    /// so thread-side holders mask interrupts (`with_engine`).
    engine: Mutex<BlkQueue<PciRing>>,
    /// Admission by sector range, and the waiters for ranges and slots.
    request_gate: BlockRequestGate,
    /// ISR-to-submitter completion, one per chain slot.
    completions: [SlotCompletion; MAX_CHAINS],
    /// Monotonic nonzero token source for the completions.
    next_token: AtomicU32,
    /// Disk capacity in sectors
    capacity: u64,
    flush_supported: bool,
    read_only: bool,
    /// Data bytes one chain carries.
    chain_bytes: usize,
    writes_pending: AtomicBool,
    /// Number of completed operations (for stats)
    ops_completed: AtomicU64,
    /// Recently transferred sectors, when the allocation succeeded.
    cache: Option<SectorCache>,
}

impl VirtioBlockDevice {
    /// Initialize a VirtIO block device from a PCI device
    pub fn new(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        let requested = blk_config::SIZE_MAX | blk_config::SEG_MAX | blk_config::BLK_SIZE
            | blk_config::TOPOLOGY | u64::from(features::RO | features::FLUSH);
        pci_dev.enable_intx();
        let (device, queue, config, offered) =
            if pci_dev.device_id == crate::drivers::pci::VIRTIO_BLOCK_DEVICE_ID_MODERN {
                #[cfg(target_arch = "x86_64")]
                if !matches!(pci_dev.interrupt_line, 10 | 11) {
                    return Err("Modern VirtIO block: unsupported PCI interrupt line");
                }
                let mut device = VirtioPciDevice::probe(pci_dev.clone())
                    .ok_or("No modern VirtIO PCI transport")?;
                if device.read_device_features() & blk_config::VERSION_1 == 0 {
                    return Err("Modern VirtIO requires VERSION_1");
                }
                let requested = requested | blk_config::VERSION_1;
                device.init(requested)?;
                let offered = device.device_features() & requested;
                if device.config_bytes() < blk_config::required_config_bytes(offered) {
                    return Err("Incomplete VirtIO block configuration");
                }
                let config = blk_config::read_config(
                    offered,
                    || Some(device.config_generation()),
                    |offset| device.read_config_u8(offset),
                    |offset| device.read_config_u16(offset),
                    |offset| device.read_config_u32(offset),
                )?;
                device.set_config_msix_vector(u16::MAX);
                device.select_queue(0);
                let max_size = device.get_queue_num_max();
                if max_size < 4 {
                    return Err("VirtIO block queue too small");
                }
                // Modern queue sizes are writable. Use a supported power of two.
                let limit = max_size.min(256);
                let queue_size = 1u16 << (31 - limit.leading_zeros());
                device.set_queue_num(queue_size as u32);
                let queue = Virtqueue::new(queue_size)?;
                device.set_queue_desc(queue.phys_addr());
                device.set_queue_avail(queue.avail_phys_addr());
                device.set_queue_used(queue.used_phys_addr());
                device.set_queue_msix_vector(u16::MAX);
                device.set_queue_ready(true);
                if !device.queue_notify_addr_valid(0) {
                    return Err("VirtIO queue doorbell outside notify capability");
                }
                device.cache_queue_notify_addr(0);
                (BlockTransport::Modern(device), queue, config, offered)
            } else {
                let io_bar = pci_dev.get_io_bar().ok_or("No I/O BAR found")?;
                pci_dev.enable_bus_master();
                pci_dev.enable_io_space();
                let mut device = VirtioDevice::new(io_bar.address as u16);
                device.init(requested as u32)?;
                let offered = u64::from(device.read_device_features()) & requested;
                let config = blk_config::read_config(
                    offered,
                    || None,
                    |offset| device.read_config_u8(offset as u16),
                    |offset| device.read_config_u16(offset as u16),
                    |offset| device.read_config_u32(offset as u16),
                )?;
                device.select_queue(0);
                // The legacy queue-size register is read-only; use its exact size.
                let queue = Virtqueue::new(device.get_queue_size())?;
                device.set_queue_address(queue.phys_addr());
                if device.get_queue_address() != (queue.phys_addr() / 4096) as u32 {
                    log::error!(
                        "VirtIO block queue address mismatch: expected {:#x}, actual {:#x}",
                        queue.phys_addr() / 4096,
                        device.get_queue_address()
                    );
                    return Err("Queue address was not set correctly");
                }
                (BlockTransport::Legacy(device), queue, config, offered)
            };

        let capacity = config.capacity;
        let limits = config.limits;
        let flush_supported = offered & u64::from(features::FLUSH) != 0;
        log::info!("VirtIO block configuration: {:?}", config);

        // Headers and status bytes share one page; data frames are taken one
        // by one, within the budget and the ring.
        let control = Self::alloc_dma_page()?;
        let queue_size = queue.queue_size();
        // A small ring gets a shorter chain rather than no device.
        let (chains, frames) = limits
            .chain_plan(queue_size, CHAIN_FRAMES, DATA_BUDGET_BYTES / 4096)
            .ok_or("VirtIO block queue too small for one request")?;
        let mut storage = [ChainStorage::EMPTY; MAX_CHAINS];
        let mut made = 0;
        'chains: for (index, chain) in storage.iter_mut().enumerate().take(chains) {
            let mut taken = [None; CHAIN_FRAMES];
            chain.header = control + 16 * index as u64;
            chain.status = control + 2048 + 16 * index as u64;
            for count in 0..frames {
                match frame_allocator::allocate_frame() {
                    Some(frame) => {
                        chain.frames[count] = frame.start_address().as_u64();
                        taken[count] = Some(frame);
                    }
                    None => {
                        taken.iter().flatten().for_each(|frame| frame_allocator::deallocate_frame(*frame));
                        break 'chains;
                    }
                }
            }
            chain.frame_count = frames;
            made += 1;
        }
        if made == 0 {
            // One chain over one page, as before.
            let page = Self::alloc_dma_page()?;
            storage[0].header = control;
            storage[0].status = control + 2048;
            storage[0].frames[0] = page;
            storage[0].frame_count = 1;
            made = 1;
        }
        let (desc, avail, used) = queue.raw_rings();
        let engine = BlkQueue::new(PciRing { desc, avail, used }, queue_size, limits, &storage[..made])
            .map_err(|_| "VirtIO block chain storage does not fit the queue")?;
        let chain_bytes = engine.chain_bytes();

        log::info!(
            "VirtIO block: Device initialization complete (queue {}, {} chains of {} KiB)",
            queue_size,
            made,
            chain_bytes / 1024
        );

        match &device {
            BlockTransport::Legacy(device) => device.driver_ok(),
            BlockTransport::Modern(device) => device.driver_ok(),
        }

        let read_only = offered & features::RO as u64 != 0;
        Ok(VirtioBlockDevice {
            device,
            interrupt_line: pci_dev.interrupt_line,
            _queue: queue,
            engine: Mutex::new(engine),
            request_gate: BlockRequestGate::new(),
            completions: [const { SlotCompletion::new() }; MAX_CHAINS],
            next_token: AtomicU32::new(0),
            capacity,
            flush_supported,
            read_only,
            chain_bytes,
            writes_pending: AtomicBool::new(true),
            ops_completed: AtomicU64::new(0),
            cache: SectorCache::new(),
        })
    }

    /// Get disk capacity in sectors
    #[allow(dead_code)] // Part of public block device API
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only || crate::block::hardware_disk_read_only(
            crate::block::HardwareDisk::Virtio(self as *const Self as usize),
        )
    }

    /// Allocate one zeroed DMA page, by physical address.
    fn alloc_dma_page() -> Result<u64, &'static str> {
        let frame = frame_allocator::allocate_frame().ok_or("Failed to allocate DMA buffer")?;
        let phys = frame.start_address().as_u64();
        let virt = phys + crate::memory::physical_memory_offset().as_u64();
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, 4096);
        }
        Ok(phys)
    }

    fn next_completion_token(&self) -> u32 {
        loop {
            let token = self.next_token.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            if token != 0 {
                return token;
            }
        }
    }

    /// Wait for the device to complete the published request.
    ///
    /// The device owns the descriptors and DMA buffers until it completes the
    /// request, and a slow device is not a failed one, so there is no
    /// timeout: the wait ends when the device completes the request. It
    /// sleeps in slices only to notice a device that has failed, the one case
    /// where the request is abandoned. Early boot polls instead, and is
    /// abandoned past the bootstrap limit rather than spinning the boot CPU
    /// forever.
    fn wait_for_completion(&self, slot: usize, token: u32, request: u32, io: IoContext) -> Result<(), &'static str> {
        let started_ns = monotonic_now_ns();
        let sleeps = io == IoContext::Sleepable;
        loop {
            match self.completions[slot].completion.wait_timeout_uninterruptible(token, BLOCK_WAIT_SLICE_NS) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(_) => return Err(NOT_SLEEPABLE_ERROR),
            }
            if self.device.has_failed() {
                return Err("Block device failed with a request outstanding");
            }
            let waited_ns = monotonic_now_ns().saturating_sub(started_ns);
            if !sleeps && waited_ns >= BLOCK_BOOTSTRAP_WAIT_LIMIT_NS {
                return Err("Block request not completed during early boot");
            }
            report_slow_request(self, request, waited_ns);
        }
    }

    /// Whether something will complete the admitted caller's commands
    /// (design §6): early boot polls them, and a sleeping caller needs the
    /// devices' legacy interrupt line unmasked (`arm_completion_source`).
    /// A device that is not ready refuses the request rather than polling.
    fn completion_ready(&self, io: IoContext) -> Result<(), &'static str> {
        use crate::task::park::{completion_source_ready, BlockSource};
        if io == IoContext::EarlyBoot || completion_source_ready(BlockSource::VirtioBlkPci, 0) {
            Ok(())
        } else {
            Err("Block IRQ completion not armed")
        }
    }

    /// Run `f` with the engine locked and interrupts masked. The interrupt
    /// handler takes the same lock to drain completions, so it must not
    /// find the lock held by the thread it interrupted.
    fn with_engine<R>(&self, f: impl FnOnce(&mut BlkQueue<PciRing>) -> R) -> R {
        x86_64::instructions::interrupts::without_interrupts(|| f(&mut self.engine.lock()))
    }

    /// The owner tags of the slots that have owners, for waking them after a stop.
    fn owner_tags(engine: &BlkQueue<PciRing>, out: &mut [(usize, u32); MAX_CHAINS]) -> usize {
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

    /// Stop the queue after an abandoned wait: no chain it holds is completed
    /// or reused, each waiter is woken, and later callers are refused.
    fn stop(&self, guard: BlockRequestGuard<'_>) {
        let mut owners = [(0usize, 0u32); MAX_CHAINS];
        let count = self.with_engine(|engine| {
            engine.stop();
            Self::owner_tags(engine, &mut owners)
        });
        for &(slot, tag) in &owners[..count] {
            self.completions[slot].completion.complete(tag);
        }
        guard.wedge();
    }

    /// Read consecutive sectors in a window of chains.
    pub fn read_sectors(&self, start_sector: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        let end = self.validate_transfer(start_sector, buffer.len())?;
        if self.cache.as_ref().is_some_and(|cache| cache.lookup(start_sector, buffer)) {
            return Ok(());
        }
        let guard = self.request_gate.admit(start_sector, end, false)?;
        self.completion_ready(guard.io)?;
        self.transfer(guard, start_sector, Data::Read(buffer))
    }

    /// Write consecutive sectors in a window of chains.
    pub fn write_sectors(&self, start_sector: u64, buffer: &[u8]) -> Result<(), &'static str> {
        let end = self.validate_transfer(start_sector, buffer.len())?;
        let disk = crate::block::HardwareDisk::Virtio(self as *const Self as usize);
        if self.read_only || !crate::block::hardware_write_allowed(disk, start_sector, buffer.len() / SECTOR_SIZE) {
            return Err("Read-only filesystem");
        }
        let guard = self.request_gate.admit(start_sector, end, true)?;
        self.completion_ready(guard.io)?;
        self.transfer(guard, start_sector, Data::Write(buffer))
    }

    fn validate_transfer(&self, sector: u64, len: usize) -> Result<u64, &'static str> {
        if len == 0 || len % SECTOR_SIZE != 0 { return Err("Buffer size must be a nonzero multiple of 512"); }
        sector
            .checked_add((len / SECTOR_SIZE) as u64)
            .filter(|end| *end <= self.capacity)
            .ok_or("Sector out of range")
    }

    /// Read one sector into the start of `buffer`.
    pub fn read_sector(&self, sector: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        if buffer.len() < SECTOR_SIZE { return Err("Buffer too small"); }
        self.read_sectors(sector, &mut buffer[..SECTOR_SIZE])
    }

    /// Write one sector from the start of `buffer`.
    #[allow(dead_code)] // Part of public block device API
    pub fn write_sector(&self, sector: u64, buffer: &[u8]) -> Result<(), &'static str> {
        if buffer.len() < SECTOR_SIZE { return Err("Buffer too small"); }
        self.write_sectors(sector, &buffer[..SECTOR_SIZE])
    }

    /// One synchronous request of any length: whole chains, a bounded window
    /// of them on the device at once, one notification per window. The
    /// caller holds admission for the whole range, so the sector cache
    /// changes in device order. Each chain issued is waited for, even after
    /// another has failed; the first failure is returned. A wait that has to
    /// be abandoned stops the device instead.
    fn transfer(&self, guard: BlockRequestGuard<'_>, start_sector: u64, mut data: Data<'_>) -> Result<(), &'static str> {
        let (kind, total) = match &data {
            Data::Read(buffer) => (request_type::IN, buffer.len()),
            Data::Write(buffer) => (request_type::OUT, buffer.len()),
            Data::Flush => (request_type::FLUSH, 0),
        };
        let chunk = self.chain_bytes;
        // The boot thread's bounded polling keeps one request on the device.
        let window = if guard.io == IoContext::Sleepable { MAX_CHAINS } else { 1 };
        let chunks = if total == 0 { 1 } else { total.div_ceil(chunk) };
        // (slot, token, chunk index), oldest first.
        let mut in_flight = [(0usize, 0u32, 0usize); MAX_CHAINS];
        let (mut head, mut count) = (0usize, 0usize);
        let mut next = 0usize;
        let mut failure: Option<&'static str> = None;
        // Set before any chain is built: another caller's publish can make
        // this write visible, so our own publish result does not tell whether
        // the device has it. Admission keeps a flush out until we return.
        if kind == request_type::OUT {
            self.writes_pending.store(true, Ordering::Release);
        }

        loop {
            let mut published = false;
            while failure.is_none() && next < chunks && count < window {
                let offset = next * chunk;
                let len = (total - offset).min(chunk);
                let sector = start_sector + (offset / SECTOR_SIZE) as u64;
                let token = self.next_completion_token();
                let submitted = self.with_engine(|engine| {
                    if engine.is_stopped() {
                        return Err(Some(SubmitError::Stopped));
                    }
                    let slot = engine.reserve(token).ok_or(None)?;
                    self.completions[slot].completion.reset();
                    let write = match &data {
                        Data::Write(buffer) => Some(&buffer[offset..offset + len]),
                        _ => None,
                    };
                    match engine.submit(&mut DirectMap, slot, kind, sector, write, len) {
                        Ok(_) => Ok(slot),
                        Err(error) => {
                            engine.unreserve(slot);
                            Err(Some(error))
                        }
                    }
                });
                match submitted {
                    Ok(slot) => {
                        in_flight[(head + count) % MAX_CHAINS] = (slot, token, next);
                        count += 1;
                        next += 1;
                        published = true;
                    }
                    Err(None) if count > 0 => break,
                    Err(None) => {
                        if let Err(reason) = self.wait_for_slot(guard.io) {
                            failure = Some(reason);
                        }
                    }
                    Err(Some(SubmitError::Stopped)) => failure = Some(WEDGED_ERROR),
                    Err(Some(_)) => failure = Some("Block request does not fit its chain"),
                }
            }
            if published && self.with_engine(|engine| engine.publish()) {
                self.device.notify_queue(0);
            }
            if count == 0 {
                break;
            }

            let (slot, token, index) = in_flight[head];
            let offset = index * chunk;
            let len = (total - offset).min(chunk);
            let sector = start_sector + (offset / SECTOR_SIZE) as u64;
            if let Err(reason) = self.wait_for_completion(slot, token, kind, guard.io) {
                if kind == request_type::OUT {
                    self.invalidate_cached(start_sector, total);
                }
                self.stop(guard);
                return Err(reason);
            }
            head = (head + 1) % MAX_CHAINS;
            count -= 1;
            let result = self.with_engine(|engine| {
                let out = match &mut data {
                    Data::Read(buffer) => Some(&mut buffer[offset..offset + len]),
                    _ => None,
                };
                engine.finish(&mut DirectMap, slot, out)
            });
            match result {
                Ok(()) => {
                    self.ops_completed.fetch_add(1, Ordering::Relaxed);
                    if let Some(cache) = &self.cache {
                        match &data {
                            Data::Read(buffer) => cache.store(sector, &buffer[offset..offset + len]),
                            Data::Write(buffer) => cache.store(sector, &buffer[offset..offset + len]),
                            Data::Flush => {}
                        }
                    }
                }
                Err(RequestError::Lost) => {
                    // Another caller stopped the queue; the chain stays owned.
                    if kind == request_type::OUT {
                        self.invalidate_cached(sector, len);
                    }
                    failure.get_or_insert(WEDGED_ERROR);
                    continue;
                }
                Err(error) => {
                    if kind == request_type::OUT {
                        self.invalidate_cached(sector, len);
                    }
                    failure.get_or_insert(match error {
                        RequestError::UsedLength(_) => "Device returned the wrong used length",
                        _ if kind == request_type::FLUSH => "Device flush failed",
                        _ => "Device returned error status",
                    });
                }
            }
            self.request_gate.waiters.wake_up();
        }
        if failure.is_none() && kind == request_type::FLUSH {
            self.writes_pending.store(false, Ordering::Release);
        }
        drop(guard);
        failure.map_or(Ok(()), Err)
    }

    /// Wait until a chain slot is free, as admission waits for a range.
    /// Early boot, with one request on the device at a time, never waits.
    fn wait_for_slot(&self, io: IoContext) -> Result<(), &'static str> {
        use crate::task::park::{self, WaitEnd, WaitMode};
        let free = || self.with_engine(|engine| engine.free_slots() > 0 || engine.is_stopped());
        if free() {
            return Ok(());
        }
        if io != IoContext::Sleepable {
            return Err("Block request already in progress during early boot");
        }
        match park::wait(&self.request_gate.waiters, free, None, WaitMode::Uninterruptible) {
            Ok(WaitEnd::NoMemory) => Err("Block request waiter publication refused"),
            Ok(WaitEnd::Killed) => Err("Block request waiter publication refused"),
            Ok(_) => Ok(()),
            Err(_) => Err(NOT_SLEEPABLE_ERROR),
        }
    }

    fn invalidate_cached(&self, sector: u64, len: usize) {
        if let Some(cache) = &self.cache {
            cache.invalidate(sector, len);
        }
    }

    /// Flush the device cache using a header/status-only request. A device
    /// without FLUSH advertises writethrough operation by the VirtIO block
    /// ABI. The flush holds the whole device, so it waits for the writes in
    /// flight and later requests wait for it.
    pub fn flush(&self) -> Result<(), &'static str> {
        if crate::block::hardware_disk_read_only(crate::block::HardwareDisk::Virtio(self as *const Self as usize)) { return Ok(()); }
        if !self.flush_supported {
            return Ok(());
        }
        let guard = self.request_gate.admit_flush()?;
        self.completion_ready(guard.io)?;
        // A write sets this flag before it builds a chain; admission
        // excludes writes while it is read.
        if !self.writes_pending.load(Ordering::Acquire) { return Ok(()); }
        self.transfer(guard, 0, Data::Flush)
    }

    /// A shared PCI handler polls only the devices routed to its own line.
    #[cfg(target_arch = "x86_64")]
    pub fn handle_interrupt_on_line(&self, irq: u8) -> bool {
        self.interrupt_line == irq && self.handle_interrupt()
    }

    /// Handle interrupt from device
    ///
    /// This should be called from the IRQ handler.
    /// Returns true if there was work to do.
    ///
    /// CRITICAL: This function must be extremely fast. No logging, no allocations.
    pub fn handle_interrupt(&self) -> bool {
        // Read and acknowledge ISR
        let isr = self.device.read_isr();
        if isr == 0 {
            return false;
        }

        // Every thread-side holder masks interrupts, so on this CPU the lock
        // is free, and a holder on another CPU releases it within a few
        // descriptor updates. Skipping the drain would lose the completion:
        // the ISR read above has already acknowledged it.
        let mut woken = [(0usize, 0u32); MAX_CHAINS];
        let mut count = 0;
        let broken = {
            let mut engine = self.engine.lock();
            let outcome = engine.drain(&mut DirectMap, |slot, tag| {
                woken[count] = (slot, tag);
                count += 1;
            });
            if outcome == Drain::Broken {
                // The chains the device held are lost: wake their owners.
                count = Self::owner_tags(&engine, &mut woken);
            }
            outcome == Drain::Broken
        };
        for &(slot, tag) in &woken[..count] {
            if tag != 0 {
                self.completions[slot].completion.complete(tag);
            }
        }
        if broken {
            self.request_gate.wedged.store(true, Ordering::Release);
            self.request_gate.waiters.wake_up();
        }

        true
    }

    /// Get the number of completed operations
    #[allow(dead_code)] // Part of public block device API
    pub fn ops_completed(&self) -> u64 {
        self.ops_completed.load(Ordering::Relaxed)
    }
}

// Global block device instances
static BLOCK_DEVICE: Mutex<Option<Arc<VirtioBlockDevice>>> = Mutex::new(None);
static BLOCK_DEVICES: Mutex<alloc::vec::Vec<Arc<VirtioBlockDevice>>> =
    Mutex::new(alloc::vec::Vec::new());

/// Run `f` with the device list locked and interrupts masked. The shared-line
/// interrupt handler looks devices up here, so it must never find the lock
/// held by the thread it interrupted.
fn with_block_devices<R>(f: impl FnOnce(&mut alloc::vec::Vec<Arc<VirtioBlockDevice>>) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| f(&mut BLOCK_DEVICES.lock()))
}

/// Initialize the VirtIO block driver
///
/// Finds and initializes all VirtIO block devices.
pub fn init() -> Result<(), &'static str> {
    let devices = crate::drivers::pci::find_virtio_block_devices();

    if devices.is_empty() {
        log::warn!("VirtIO block: No devices found");
        return Err("No VirtIO block devices found");
    }

    log::info!("VirtIO block: Found {} device(s)", devices.len());

    let mut initialized_devices = alloc::vec::Vec::new();

    for (idx, pci_dev) in devices.iter().enumerate() {
        log::info!(
            "VirtIO block: Initializing device {} at {:02x}:{:02x}.{}",
            idx,
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function
        );

        match VirtioBlockDevice::new(pci_dev) {
            Ok(block_dev) => {
                let block_dev = Arc::new(block_dev);
                initialized_devices.push(block_dev.clone());

                // Keep first device as primary for backward compatibility
                if idx == 0 {
                    *BLOCK_DEVICE.lock() = Some(block_dev);
                }

                log::info!("VirtIO block: Device {} initialized successfully", idx);
            }
            Err(e) => {
                log::error!("VirtIO block: Failed to initialize device {}: {}", idx, e);
            }
        }
    }

    if initialized_devices.is_empty() {
        return Err("Failed to initialize any VirtIO block devices");
    }

    let device_count = initialized_devices.len();
    with_block_devices(|devices| *devices = initialized_devices);
    // The devices' completion source, their legacy interrupt line, is armed
    // once it is unmasked (`drivers::run_post_init_self_tests`, design §6).
    crate::task::park::attach_completion_source(crate::task::park::BlockSource::VirtioBlkPci, 0, false);

    // NOTE: The VirtIO interrupt handler is registered directly in the IDT.
    // See kernel/src/interrupts.rs -> virtio_block_interrupt_handler()
    // No dynamic registration needed - the handler is static.

    log::info!(
        "VirtIO block: Driver initialized with {} device(s)",
        device_count
    );
    crate::drivers::device_class::storage_initialized("virtio-blk", device_count);

    Ok(())
}

/// Get a reference to the block device (primary/first device)
pub fn get_device() -> Option<Arc<VirtioBlockDevice>> {
    BLOCK_DEVICE.lock().clone()
}

/// Get a reference to a specific block device by index
pub fn get_device_by_index(index: usize) -> Option<Arc<VirtioBlockDevice>> {
    with_block_devices(|devices| devices.get(index).cloned())
}

/// Test the block device by reading sector 0
pub fn test_read() -> Result<(), &'static str> {
    let device = get_device().ok_or("Block device not initialized")?;

    log::info!("VirtIO block test: Reading sector 0...");

    let mut buffer = [0u8; SECTOR_SIZE];
    device.read_sector(0, &mut buffer)?;

    log::info!("VirtIO block test: Read successful!");
    log::info!("  First 16 bytes: {:02x?}", &buffer[..16]);

    // Check for MBR signature
    if buffer[510] == 0x55 && buffer[511] == 0xAA {
        log::info!("  MBR signature found (0x55AA)");
    }

    Ok(())
}
