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

use super::pci_transport::VirtioPciDevice;
use super::queue::Virtqueue;
use super::VirtioDevice;
use crate::drivers::pci::Device as PciDevice;
use crate::memory::frame_allocator;
use crate::task::completion::Completion;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

/// A request waits for its completion in sleeps of this length, checking
/// between them that the device has not failed.
const BLOCK_WAIT_SLICE_NS: u64 = 1_000_000_000;
/// A request outstanding this long is reported, at most once per interval.
const BLOCK_SLOW_REPORT_INTERVAL_NS: u64 = 10_000_000_000;
/// A wait that cannot sleep (before the scheduler, or on the boot thread
/// before its timer runs) busy-polls the CPU, so it is bounded: past this the
/// request is abandoned, the gate is wedged so its DMA buffers are never
/// reused, and the boot carries on without the disk.
const BLOCK_BOOTSTRAP_WAIT_LIMIT_NS: u64 = 30_000_000_000;
/// When a slow request was last reported, so reports stay rate-limited.
static LAST_SLOW_REPORT_NS: AtomicU64 = AtomicU64::new(0);
const NO_COMPLETED_DESC: u32 = u32::MAX;
const NO_COMPLETED_STATUS: u32 = u32::MAX;

struct BlockRequestGate {
    locked: AtomicBool,
    wedged: AtomicBool,
    waiters: crate::task::waitqueue::WaitQueueHead,
}

struct BlockRequestGuard<'a> {
    gate: &'a BlockRequestGate,
    release_on_drop: bool,
}

impl BlockRequestGate {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            wedged: AtomicBool::new(false),
            waiters: crate::task::waitqueue::WaitQueueHead::new(),
        }
    }

    fn lock(&self) -> Result<BlockRequestGuard<'_>, &'static str> {
        if self.wedged.load(Ordering::Acquire) {
            return Err("Block device wedged after an abandoned request");
        }

        loop {
            if self
                .locked
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(BlockRequestGuard {
                    gate: self,
                    release_on_drop: true,
                });
            }

            if !block_request_gate_can_sleep() {
                return Err("Block request already in progress");
            }

            if self
                .waiters
                .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
                .is_none()
            {
                return Err("Block request already in progress");
            }

            if self.wedged.load(Ordering::Acquire) {
                self.waiters.finish_wait();
                return Err("Block device wedged after an abandoned request");
            }

            if self.locked.load(Ordering::Acquire) {
                crate::task::waitqueue::schedule_current_wait();
            }
            self.waiters.finish_wait();

            if self.wedged.load(Ordering::Acquire) {
                return Err("Block device wedged after an abandoned request");
            }
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        self.waiters.wake_up_one();
    }
}

impl BlockRequestGuard<'_> {
    fn wedge(mut self) {
        self.release_on_drop = false;
        self.gate.wedged.store(true, Ordering::Release);
        self.gate.waiters.wake_up();
    }
}

impl Drop for BlockRequestGuard<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.gate.unlock();
        }
    }
}

#[inline]
fn block_request_gate_can_sleep() -> bool {
    if crate::task::scheduler::current_thread_id().is_none() {
        return false;
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::per_cpu::preempt_count() > 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// VirtIO block request types
mod request_type {
    pub const IN: u32 = 0; // Read from device
    #[allow(dead_code)] // Part of block device API, used by write_sector
    pub const OUT: u32 = 1; // Write to device
    pub const FLUSH: u32 = 4; // Persist cached writes
}

/// VirtIO block status codes
mod status_code {
    pub const OK: u8 = 0;
}

/// VirtIO block feature bits
mod features {
    /// Maximum size of any single segment is in size_max
    pub const SIZE_MAX: u32 = 1 << 1;
    /// Maximum number of segments in a request is in seg_max
    pub const SEG_MAX: u32 = 1 << 2;
    /// Cache flush command support
    pub const FLUSH: u32 = 1 << 9;
}

/// VirtIO block request header
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioBlkReq {
    /// Request type (IN, OUT, FLUSH, etc.)
    type_: u32,
    /// Reserved
    reserved: u32,
    /// Starting sector for the request
    sector: u64,
}

/// Sector size in bytes
pub const SECTOR_SIZE: usize = 512;

/// Cached DMA buffers for I/O operations
/// These are allocated once and reused to prevent frame exhaustion
struct DmaBuffers {
    /// Header buffer (physical, virtual)
    header: (u64, u64),
    /// Data buffer (physical, virtual)
    data: (u64, u64),
    /// Status buffer (physical, virtual)
    status: (u64, u64),
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
        let (used, seen, pending, completed, apic_state) =
            x86_64::instructions::interrupts::without_interrupts(|| {
                let (used, seen) = device
                    .with_queue(|queue| (queue.debug_used_idx(), queue.debug_last_used_idx()));
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
                (
                    used,
                    seen,
                    device.pending_token.load(Ordering::Acquire),
                    device.completed_desc.load(Ordering::Acquire),
                    apic_state,
                )
            });
        log::warn!("VirtIO block: request type {} still waiting after {} ms; IRQ {} used={} last_seen={} pending_token={} completed_desc={}",
            request, waited_ns / 1_000_000, device.interrupt_line, used, seen, pending, completed);
        if let Some((low, high, vector, irr, isr)) = apic_state {
            log::warn!("VirtIO block: IRQ {} vector {} low={:#010x} high={:#010x} remote_irr={} delivery_pending={} masked={} LAPIC_IRR={} LAPIC_ISR={}",
                device.interrupt_line, vector, low, high, low & (1 << 14) != 0,
                low & (1 << 12) != 0, low & (1 << 16) != 0, irr, isr);
        }
    }
}

/// VirtIO block device driver
pub struct VirtioBlockDevice {
    /// VirtIO device abstraction
    device: BlockTransport,
    interrupt_line: u8,
    /// Request virtqueue
    queue: Mutex<Virtqueue>,
    /// Serializes the shared DMA buffers without involving the IRQ handler.
    request_gate: BlockRequestGate,
    /// ISR-to-submitter completion for the single outstanding request.
    completion: Completion,
    /// Monotonic non-zero token expected by the current waiter.
    next_token: AtomicU32,
    /// Token currently armed for IRQ completion; 0 means no armed request.
    pending_token: AtomicU32,
    /// Descriptor head drained by the interrupt handler.
    completed_desc: AtomicU32,
    /// Status byte read by the interrupt handler.
    completed_status: AtomicU32,
    /// Disk capacity in sectors
    capacity: u64,
    flush_supported: bool,
    /// Number of completed operations (for stats)
    ops_completed: AtomicU64,
    /// Cached DMA buffers (protected by queue mutex)
    dma_buffers: DmaBuffers,
}

impl VirtioBlockDevice {
    /// Initialize a VirtIO block device from a PCI device
    pub fn new(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        let requested = features::SIZE_MAX | features::SEG_MAX | features::FLUSH;
        pci_dev.enable_intx();
        let (device, queue, capacity, flush_supported) =
            if pci_dev.device_id == crate::drivers::pci::VIRTIO_BLOCK_DEVICE_ID_MODERN {
                #[cfg(target_arch = "x86_64")]
                if !matches!(pci_dev.interrupt_line, 10 | 11) {
                    return Err("Modern VirtIO block: unsupported PCI interrupt line");
                }
                const VERSION_1: u64 = 1 << 32;
                let mut device = VirtioPciDevice::probe(pci_dev.clone())
                    .ok_or("No modern VirtIO PCI transport")?;
                if device.read_device_features() & VERSION_1 == 0 {
                    return Err("Modern VirtIO requires VERSION_1");
                }
                device.init(VERSION_1 | requested as u64)?;
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
                // Capacity is a multiword config field; retry if its generation changes.
                let mut capacity = None;
                for _ in 0..100 {
                    let generation = device.config_generation();
                    let value = device.read_config_u64(0);
                    if device.config_generation() == generation {
                        capacity = Some(value);
                        break;
                    }
                }
                let capacity = capacity.ok_or("Unstable VirtIO block capacity")?;
                let flush_supported = device.device_features() & features::FLUSH as u64 != 0;
                device.driver_ok();
                (
                    BlockTransport::Modern(device),
                    queue,
                    capacity,
                    flush_supported,
                )
            } else {
                let io_bar = pci_dev.get_io_bar().ok_or("No I/O BAR found")?;
                pci_dev.enable_bus_master();
                pci_dev.enable_io_space();
                let mut device = VirtioDevice::new(io_bar.address as u16);
                device.init(requested)?;
                let capacity = device.read_config_u64(0);
                let flush_supported = device.read_device_features() & features::FLUSH != 0;
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
                device.driver_ok();
                (
                    BlockTransport::Legacy(device),
                    queue,
                    capacity,
                    flush_supported,
                )
            };

        log::info!(
            "VirtIO block: capacity {} sectors ({} MiB)",
            capacity,
            capacity / 2048
        );

        // Pre-allocate DMA buffers for I/O operations
        // These are reused for all read/write operations to prevent frame exhaustion
        let header_buf = Self::alloc_dma_buffer(16)?;
        let data_buf = Self::alloc_dma_buffer(SECTOR_SIZE)?;
        let status_buf = Self::alloc_dma_buffer(1)?;

        let dma_buffers = DmaBuffers {
            header: header_buf,
            data: data_buf,
            status: status_buf,
        };

        log::info!("VirtIO block: Device initialization complete (with cached DMA buffers)");

        Ok(VirtioBlockDevice {
            device,
            interrupt_line: pci_dev.interrupt_line,
            queue: Mutex::new(queue),
            request_gate: BlockRequestGate::new(),
            completion: Completion::new(),
            next_token: AtomicU32::new(0),
            pending_token: AtomicU32::new(0),
            completed_desc: AtomicU32::new(NO_COMPLETED_DESC),
            completed_status: AtomicU32::new(NO_COMPLETED_STATUS),
            capacity,
            flush_supported,
            ops_completed: AtomicU64::new(0),
            dma_buffers,
        })
    }

    /// Get disk capacity in sectors
    #[allow(dead_code)] // Part of public block device API
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Allocate a DMA buffer of the given size
    ///
    /// Returns (physical_address, virtual_address)
    fn alloc_dma_buffer(size: usize) -> Result<(u64, u64), &'static str> {
        if size > 4096 {
            return Err("Buffer too large for single page");
        }

        let frame = frame_allocator::allocate_frame().ok_or("Failed to allocate DMA buffer")?;

        let phys = frame.start_address().as_u64();
        let phys_offset = crate::memory::physical_memory_offset();
        let virt = phys + phys_offset.as_u64();

        // Zero the buffer
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, 4096);
        }

        Ok((phys, virt))
    }

    fn next_completion_token(&self) -> u32 {
        let mut token = self
            .next_token
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        if token == 0 {
            token = self
                .next_token
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
        }
        token
    }

    fn prepare_completion_wait(&self) -> u32 {
        let token = self.next_completion_token();
        self.completion.reset();
        self.completed_desc
            .store(NO_COMPLETED_DESC, Ordering::Release);
        self.completed_status
            .store(NO_COMPLETED_STATUS, Ordering::Release);
        self.pending_token.store(token, Ordering::Release);
        token
    }

    fn clear_completion_state(&self) {
        self.pending_token.store(0, Ordering::Release);
        self.completed_desc
            .store(NO_COMPLETED_DESC, Ordering::Release);
        self.completed_status
            .store(NO_COMPLETED_STATUS, Ordering::Release);
    }

    /// Wait for the device to complete the published request.
    ///
    /// The device owns the descriptors and DMA buffers until it completes the
    /// request, and a slow device is not a failed one, so, as in Linux's
    /// virtio-blk, there is no timeout: the wait ends when the device
    /// completes the request. It sleeps in slices only to notice a device
    /// that has failed, the one case where the request is abandoned. A wait
    /// that cannot sleep spins instead, and is abandoned past the bootstrap
    /// limit rather than spinning the boot CPU forever.
    fn wait_for_completion(&self, token: u32, request: u32) -> Result<(), &'static str> {
        let started_ns = monotonic_now_ns();
        let sleeps = crate::task::completion::wait_sleeps();
        loop {
            if let Ok(true) = self
                .completion
                .wait_timeout_uninterruptible(token, BLOCK_WAIT_SLICE_NS)
            {
                return Ok(());
            }
            if self.device.has_failed() {
                return Err("Block device failed with a request outstanding");
            }
            let waited_ns = monotonic_now_ns().saturating_sub(started_ns);
            if !sleeps && waited_ns >= BLOCK_BOOTSTRAP_WAIT_LIMIT_NS {
                return Err("Block request not completed before the kernel could sleep");
            }
            report_slow_request(self, request, waited_ns);
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn irq_completion_available(&self) -> bool {
        crate::task::scheduler::current_thread_id().is_some()
            || x86_64::instructions::interrupts::are_enabled()
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn irq_completion_available(&self) -> bool {
        true
    }

    /// Run `f` with the queue locked and interrupts masked. The interrupt
    /// handler takes the same lock to drain a completion, so it must never
    /// find the lock held by the thread it interrupted.
    fn with_queue<R>(&self, f: impl FnOnce(&mut Virtqueue) -> R) -> R {
        x86_64::instructions::interrupts::without_interrupts(|| f(&mut self.queue.lock()))
    }

    fn take_completed_request(&self) -> Result<(u16, u8), &'static str> {
        let desc = self
            .completed_desc
            .swap(NO_COMPLETED_DESC, Ordering::AcqRel);
        if desc == NO_COMPLETED_DESC {
            return Err("Block request woke without completion");
        }
        let status = self.completed_status.load(Ordering::Acquire);
        if status == NO_COMPLETED_STATUS {
            return Err("Block request woke without status");
        }
        Ok((desc as u16, status as u8))
    }

    /// Read multiple contiguous sectors into a buffer.
    ///
    /// Buffer size must be a multiple of SECTOR_SIZE (512 bytes).
    /// This is a simple loop-based implementation that calls read_sector()
    /// for each sector. While less efficient than scatter-gather, it's
    /// simple, uses tested code, and provides acceptable performance.
    #[allow(dead_code)] // Part of public block device API
    pub fn read_sectors(&self, start_sector: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        // Validate buffer size
        if buffer.is_empty() {
            return Err("Buffer is empty");
        }
        if buffer.len() % SECTOR_SIZE != 0 {
            return Err("Buffer size must be multiple of 512");
        }

        // Calculate number of sectors
        let num_sectors = buffer.len() / SECTOR_SIZE;

        // Check sector range
        if start_sector >= self.capacity {
            return Err("Start sector out of range");
        }
        if start_sector
            .checked_add(num_sectors as u64)
            .ok_or("Sector overflow")?
            > self.capacity
        {
            return Err("Sector range exceeds disk capacity");
        }

        // Read each sector
        for i in 0..num_sectors {
            let sector = start_sector + i as u64;
            let offset = i * SECTOR_SIZE;
            let sector_buffer = &mut buffer[offset..offset + SECTOR_SIZE];

            self.read_sector(sector, sector_buffer)?;
        }

        Ok(())
    }

    /// Submit a read request
    ///
    /// This is an asynchronous operation. The data will be available after
    /// the device signals completion via interrupt.
    pub fn read_sector(&self, sector: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        if buffer.len() < SECTOR_SIZE {
            return Err("Buffer too small");
        }
        if sector >= self.capacity {
            return Err("Sector out of range");
        }
        if !self.irq_completion_available() {
            return Err("Block IRQ completion unavailable before interrupts are enabled");
        }

        // The DMA header/data/status buffers are shared across callers. Keep
        // one request in flight, but do not make the IRQ handler take this gate.
        let request_guard = self.request_gate.lock()?;
        let completion_token = self.prepare_completion_wait();

        // Use cached DMA buffers (protected by queue mutex)
        let (header_phys, header_virt) = self.dma_buffers.header;
        let (data_phys, data_virt) = self.dma_buffers.data;
        let (status_phys, status_virt) = self.dma_buffers.status;

        let added = self.with_queue(|queue| {
            // Set up request header using volatile writes.
            unsafe {
                let header = header_virt as *mut VirtioBlkReq;
                core::ptr::write_volatile(&mut (*header).type_, request_type::IN);
                core::ptr::write_volatile(&mut (*header).reserved, 0);
                core::ptr::write_volatile(&mut (*header).sector, sector);
                core::ptr::write_volatile(status_virt as *mut u8, 0xff);
            }
            core::sync::atomic::fence(Ordering::SeqCst);

            let buffers = [
                (header_phys, 16, false),              // Header: device reads
                (data_phys, SECTOR_SIZE as u32, true), // Data: device writes
                (status_phys, 1, true),                // Status: device writes
            ];

            queue.add_chain(&buffers).is_some()
        });
        if !added {
            self.clear_completion_state();
            return Err("Queue full");
        }

        core::sync::atomic::fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        if let Err(e) = self.wait_for_completion(completion_token, request_type::IN) {
            request_guard.wedge();
            return Err(e);
        }

        let (completed_desc, status) = match self.take_completed_request() {
            Ok(completed) => completed,
            Err(e) => {
                self.clear_completion_state();
                return Err(e);
            }
        };
        let result = self.with_queue(|queue| {
            // Check status
            if status != status_code::OK {
                queue.free_chain(completed_desc);
                return Err("Device returned error status");
            }

            // Copy data to user buffer
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data_virt as *const u8,
                    buffer.as_mut_ptr(),
                    SECTOR_SIZE,
                );
            }

            // Free descriptor chain
            queue.free_chain(completed_desc);
            Ok(())
        });

        self.clear_completion_state();
        result?;
        self.ops_completed.fetch_add(1, Ordering::Relaxed);
        drop(request_guard);

        Ok(())
    }

    /// Submit a write request
    #[allow(dead_code)] // Part of public block device API
    pub fn write_sector(&self, sector: u64, buffer: &[u8]) -> Result<(), &'static str> {
        if buffer.len() < SECTOR_SIZE {
            return Err("Buffer too small");
        }
        if sector >= self.capacity {
            return Err("Sector out of range");
        }
        if !self.irq_completion_available() {
            return Err("Block IRQ completion unavailable before interrupts are enabled");
        }

        let request_guard = self.request_gate.lock()?;
        let completion_token = self.prepare_completion_wait();

        // Use cached DMA buffers (protected by queue mutex)
        let (header_phys, header_virt) = self.dma_buffers.header;
        let (data_phys, data_virt) = self.dma_buffers.data;
        let (status_phys, status_virt) = self.dma_buffers.status;

        let added = self.with_queue(|queue| {
            unsafe {
                let header = header_virt as *mut VirtioBlkReq;
                core::ptr::write_volatile(&mut (*header).type_, request_type::OUT);
                core::ptr::write_volatile(&mut (*header).reserved, 0);
                core::ptr::write_volatile(&mut (*header).sector, sector);
                core::ptr::write_volatile(status_virt as *mut u8, 0xff);
                core::ptr::copy_nonoverlapping(buffer.as_ptr(), data_virt as *mut u8, SECTOR_SIZE);
            }

            let buffers = [
                (header_phys, 16, false),               // Header: device reads
                (data_phys, SECTOR_SIZE as u32, false), // Data: device reads
                (status_phys, 1, true),                 // Status: device writes
            ];

            queue.add_chain(&buffers).is_some()
        });
        if !added {
            self.clear_completion_state();
            return Err("Queue full");
        }

        core::sync::atomic::fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        if let Err(e) = self.wait_for_completion(completion_token, request_type::OUT) {
            request_guard.wedge();
            return Err(e);
        }

        let (completed_desc, status) = match self.take_completed_request() {
            Ok(completed) => completed,
            Err(e) => {
                self.clear_completion_state();
                return Err(e);
            }
        };
        // Free descriptor chain
        self.with_queue(|queue| queue.free_chain(completed_desc));
        self.clear_completion_state();

        // Check status
        if status != status_code::OK {
            return Err("Device returned error status");
        }

        self.ops_completed.fetch_add(1, Ordering::Relaxed);
        drop(request_guard);

        Ok(())
    }

    /// Flush the device cache using a header/status-only request. A device
    /// without FLUSH advertises writethrough operation by the VirtIO block ABI.
    pub fn flush(&self) -> Result<(), &'static str> {
        if !self.flush_supported {
            return Ok(());
        }
        if !self.irq_completion_available() {
            return Err("Block IRQ completion unavailable before interrupts are enabled");
        }
        let request_guard = self.request_gate.lock()?;
        let token = self.prepare_completion_wait();
        let (header_phys, header_virt) = self.dma_buffers.header;
        let (status_phys, status_virt) = self.dma_buffers.status;
        let added = self.with_queue(|queue| {
            unsafe {
                core::ptr::write_volatile(
                    header_virt as *mut VirtioBlkReq,
                    VirtioBlkReq {
                        type_: request_type::FLUSH,
                        reserved: 0,
                        sector: 0,
                    },
                );
                core::ptr::write_volatile(status_virt as *mut u8, 0xff);
            }
            let buffers = [(header_phys, 16, false), (status_phys, 1, true)];
            queue.add_chain(&buffers).is_some()
        });
        if !added {
            self.clear_completion_state();
            return Err("Queue full");
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        self.device.notify_queue(0);
        if let Err(error) = self.wait_for_completion(token, request_type::FLUSH) {
            request_guard.wedge();
            return Err(error);
        }
        let (desc, status) = match self.take_completed_request() {
            Ok(completed) => completed,
            Err(error) => {
                self.clear_completion_state();
                return Err(error);
            }
        };
        self.with_queue(|queue| queue.free_chain(desc));
        self.clear_completion_state();
        if status != status_code::OK {
            return Err("Device flush failed");
        }
        self.ops_completed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// A shared PCI handler must not acknowledge a different line's device.
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

        let token = self.pending_token.load(Ordering::Acquire);
        if token == 0 {
            return true;
        }

        // Every thread-side holder masks interrupts, so on this CPU the lock
        // is free, and a holder on another CPU releases it within a few
        // descriptor updates. Skipping the drain would lose the completion:
        // the ISR read above has already acknowledged it.
        let mut queue = self.queue.lock();

        if let Some((completed_desc, _bytes)) = queue.get_used() {
            let (_, status_virt) = self.dma_buffers.status;
            let status = unsafe { core::ptr::read_volatile(status_virt as *const u8) };
            self.completed_status
                .store(status as u32, Ordering::Release);
            self.completed_desc
                .store(completed_desc as u32, Ordering::Release);
            self.completion.complete(token);
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

    // NOTE: The VirtIO interrupt handler is registered directly in the IDT.
    // See kernel/src/interrupts.rs -> virtio_block_interrupt_handler()
    // No dynamic registration needed - the handler is static.

    log::info!(
        "VirtIO block: Driver initialized with {} device(s)",
        device_count
    );

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
