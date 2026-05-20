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

use super::queue::Virtqueue;
use super::VirtioDevice;
use crate::drivers::pci::Device as PciDevice;
use crate::memory::frame_allocator;
use crate::task::completion::Completion;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

const BLOCK_COMPLETION_TIMEOUT_NS: u64 = 5_000_000_000;
const BLOCK_EARLY_COMPLETION_TIMEOUT_NS: u64 = 100_000_000_000;
const NO_COMPLETED_DESC: u32 = u32::MAX;
const NO_COMPLETED_STATUS: u32 = u32::MAX;

struct BlockRequestGate {
    locked: AtomicBool,
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
            waiters: crate::task::waitqueue::WaitQueueHead::new(),
        }
    }

    fn lock(&self) -> Result<BlockRequestGuard<'_>, &'static str> {
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

            if self.locked.load(Ordering::Acquire) {
                crate::task::waitqueue::schedule_current_wait();
            }
            self.waiters.finish_wait();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        self.waiters.wake_up_one();
    }
}

impl BlockRequestGuard<'_> {
    fn keep_locked(mut self) {
        self.release_on_drop = false;
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

/// VirtIO block device driver
pub struct VirtioBlockDevice {
    /// VirtIO device abstraction
    device: VirtioDevice,
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
    /// Number of completed operations (for stats)
    ops_completed: AtomicU64,
    /// Cached DMA buffers (protected by queue mutex)
    dma_buffers: DmaBuffers,
}

impl VirtioBlockDevice {
    /// Initialize a VirtIO block device from a PCI device
    pub fn new(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        // Get I/O port base from BAR0
        let io_bar = pci_dev.get_io_bar().ok_or("No I/O BAR found")?;
        let io_base = io_bar.address as u16;

        log::info!(
            "VirtIO block: Initializing device at I/O base {:#x}",
            io_base
        );

        // Enable bus mastering for DMA
        pci_dev.enable_bus_master();
        pci_dev.enable_io_space();
        pci_dev.enable_intx();

        // Create VirtIO device
        let mut device = VirtioDevice::new(io_base);

        // Initialize with requested features
        let requested_features = features::SIZE_MAX | features::SEG_MAX | features::FLUSH;
        device.init(requested_features)?;

        // Read device capacity
        let capacity = device.read_config_u64(0);
        log::info!(
            "VirtIO block: Capacity = {} sectors ({} MB)",
            capacity,
            (capacity * SECTOR_SIZE as u64) / (1024 * 1024)
        );

        // Set up the request queue (queue 0)
        device.select_queue(0);
        let queue_size = device.get_queue_size();

        if queue_size == 0 {
            return Err("Device reports queue size 0");
        }

        // In VirtIO legacy mode, we MUST use the device's queue size exactly.
        // The QUEUE_SIZE register (0x0C) is read-only - the driver cannot negotiate
        // a smaller size. QEMU uses vring.num to calculate avail/used ring offsets,
        // so if we allocate a differently-sized queue, the offsets won't match.
        log::info!(
            "VirtIO block: Device queue size = {} (must use exactly)",
            queue_size
        );

        // Allocate virtqueue
        let queue = Virtqueue::new(queue_size)?;
        let queue_phys = queue.phys_addr();

        // Tell device about the queue
        // NOTE: In legacy VirtIO, QUEUE_SIZE at offset 0x0C is read-only.
        // We read it to get the device's queue size, but don't write back.
        // We MUST allocate a queue of exactly that size because the device
        // uses it to calculate avail/used ring offsets.
        device.select_queue(0);
        log::info!(
            "VirtIO block: Setting queue address phys={:#x}, PFN={:#x}",
            queue_phys,
            queue_phys / 4096
        );
        device.set_queue_address(queue_phys);

        // Read back and verify the queue address was set correctly
        let readback_pfn = device.get_queue_address();
        let expected_pfn = (queue_phys / 4096) as u32;
        if readback_pfn != expected_pfn {
            log::error!(
                "VirtIO block: Queue address mismatch! Expected PFN={:#x}, got PFN={:#x}",
                expected_pfn,
                readback_pfn
            );
            return Err("Queue address was not set correctly");
        }
        log::info!(
            "VirtIO block: Queue address verified: PFN={:#x}",
            readback_pfn
        );

        // Device is ready
        device.driver_ok();

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
            queue: Mutex::new(queue),
            request_gate: BlockRequestGate::new(),
            completion: Completion::new(),
            next_token: AtomicU32::new(0),
            pending_token: AtomicU32::new(0),
            completed_desc: AtomicU32::new(NO_COMPLETED_DESC),
            completed_status: AtomicU32::new(NO_COMPLETED_STATUS),
            capacity,
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

    fn wait_for_completion(&self, token: u32) -> Result<(), &'static str> {
        let scheduler_thread_present = crate::task::scheduler::current_thread_id().is_some();
        let timeout_ns = if scheduler_thread_present {
            BLOCK_COMPLETION_TIMEOUT_NS
        } else {
            // The x86 early-boot Completion path has no scheduler thread to
            // park, so it uses its internal no-scheduler wait. Keep that path
            // long enough for the existing boot-time sector-0 probe.
            BLOCK_EARLY_COMPLETION_TIMEOUT_NS
        };

        let result = self.completion.wait_timeout(token, timeout_ns);

        match result {
            Ok(true) => Ok(()),
            Ok(false) => Err("Block request timed out"),
            Err(_eintr) => Err("Block request interrupted"),
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

        {
            let mut queue = self.queue.lock();

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

            if queue.add_chain(&buffers).is_none() {
                self.clear_completion_state();
                return Err("Queue full");
            }
        }

        core::sync::atomic::fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        if let Err(e) = self.wait_for_completion(completion_token) {
            request_guard.keep_locked();
            return Err(e);
        }

        let (completed_desc, status) = match self.take_completed_request() {
            Ok(completed) => completed,
            Err(e) => {
                self.clear_completion_state();
                return Err(e);
            }
        };
        let mut queue = self.queue.lock();

        // Check status
        if status != status_code::OK {
            queue.free_chain(completed_desc);
            self.clear_completion_state();
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

        self.clear_completion_state();
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

        {
            let mut queue = self.queue.lock();

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

            if queue.add_chain(&buffers).is_none() {
                self.clear_completion_state();
                return Err("Queue full");
            }
        }

        core::sync::atomic::fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        if let Err(e) = self.wait_for_completion(completion_token) {
            request_guard.keep_locked();
            return Err(e);
        }

        let (completed_desc, status) = match self.take_completed_request() {
            Ok(completed) => completed,
            Err(e) => {
                self.clear_completion_state();
                return Err(e);
            }
        };
        let mut queue = self.queue.lock();

        // Check status
        if status != status_code::OK {
            queue.free_chain(completed_desc);
            self.clear_completion_state();
            return Err("Device returned error status");
        }

        // Free descriptor chain
        queue.free_chain(completed_desc);

        self.clear_completion_state();
        self.ops_completed.fetch_add(1, Ordering::Relaxed);
        drop(request_guard);

        Ok(())
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

        let Some(mut queue) = self.queue.try_lock() else {
            return true;
        };

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

    *BLOCK_DEVICES.lock() = initialized_devices;

    // NOTE: The VirtIO interrupt handler is registered directly in the IDT.
    // See kernel/src/interrupts.rs -> virtio_block_interrupt_handler()
    // No dynamic registration needed - the handler is static.

    log::info!(
        "VirtIO block: Driver initialized with {} device(s)",
        BLOCK_DEVICES.lock().len()
    );

    Ok(())
}

/// Get a reference to the block device (primary/first device)
pub fn get_device() -> Option<Arc<VirtioBlockDevice>> {
    BLOCK_DEVICE.lock().clone()
}

/// Get a reference to a specific block device by index
pub fn get_device_by_index(index: usize) -> Option<Arc<VirtioBlockDevice>> {
    let devices = BLOCK_DEVICES.lock();
    devices.get(index).cloned()
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
