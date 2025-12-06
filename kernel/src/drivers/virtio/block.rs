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
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// VirtIO block request types
mod request_type {
    pub const IN: u32 = 0;    // Read from device
    #[allow(dead_code)] // Part of block device API, used by write_sector
    pub const OUT: u32 = 1;   // Write to device
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

/// Maximum queue size we'll use
const QUEUE_SIZE: u16 = 128;

/// VirtIO block device driver
pub struct VirtioBlockDevice {
    /// VirtIO device abstraction
    device: VirtioDevice,
    /// Request virtqueue
    queue: Mutex<Virtqueue>,
    /// Disk capacity in sectors
    capacity: u64,
    /// Number of completed operations (for stats)
    ops_completed: AtomicU64,
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
        let device_queue_size = device.get_queue_size();
        let queue_size = device_queue_size.min(QUEUE_SIZE);

        if queue_size == 0 {
            return Err("Device reports queue size 0");
        }

        log::info!("VirtIO block: Queue size = {}", queue_size);

        // Allocate virtqueue
        let queue = Virtqueue::new(queue_size)?;
        let queue_phys = queue.phys_addr();

        // Tell device about the queue
        device.set_queue_address(queue_phys);

        // Device is ready
        device.driver_ok();

        log::info!("VirtIO block: Device initialization complete");

        Ok(VirtioBlockDevice {
            device,
            queue: Mutex::new(queue),
            capacity,
            ops_completed: AtomicU64::new(0),
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

        // Allocate DMA buffers
        let (header_phys, header_virt) = Self::alloc_dma_buffer(16)?;
        let (data_phys, data_virt) = Self::alloc_dma_buffer(SECTOR_SIZE)?;
        let (status_phys, status_virt) = Self::alloc_dma_buffer(1)?;

        // Set up request header
        unsafe {
            let header = header_virt as *mut VirtioBlkReq;
            (*header).type_ = request_type::IN;
            (*header).reserved = 0;
            (*header).sector = sector;
        }

        // Build descriptor chain
        let buffers = [
            (header_phys, 16, false),                        // Header: device reads
            (data_phys, SECTOR_SIZE as u32, true),           // Data: device writes
            (status_phys, 1, true),                          // Status: device writes
        ];

        log::debug!(
            "VirtIO block: read_sector {} - header_phys={:#x}, data_phys={:#x}, status_phys={:#x}",
            sector, header_phys, data_phys, status_phys
        );

        let mut queue = self.queue.lock();
        let desc_head = queue
            .add_chain(&buffers)
            .ok_or("Queue full")?;

        log::debug!("VirtIO block: Descriptor chain added at head={}", desc_head);

        // Ensure queue 0 is selected before notify
        self.device.select_queue(0);

        // Debug: Check device status
        let status = self.device.read_status();
        log::debug!("VirtIO block: Device status before notify: {:#x}", status);

        // Notify device
        core::sync::atomic::fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        log::debug!("VirtIO block: Device notified, polling for completion...");

        // Poll for completion (synchronous for now)
        let mut timeout = 10_000_000u32;  // Increased timeout
        while !queue.has_used() && timeout > 0 {
            core::hint::spin_loop();
            timeout -= 1;
        }

        if timeout == 0 {
            // Debug: dump queue state on timeout
            log::error!("VirtIO block: Read timeout! Dumping queue state...");
            log::error!("  used_idx={}, last_used_idx={}",
                queue.debug_used_idx(), queue.debug_last_used_idx());
            log::error!("  ISR status: {:#x}", self.device.read_isr());
            return Err("Read request timed out");
        }

        // Get completion
        let (completed_desc, _bytes) = queue.get_used().ok_or("No completion available")?;

        // Check status
        let status = unsafe { *(status_virt as *const u8) };
        if status != status_code::OK {
            return Err("Device returned error status");
        }

        // Copy data to user buffer
        unsafe {
            core::ptr::copy_nonoverlapping(data_virt as *const u8, buffer.as_mut_ptr(), SECTOR_SIZE);
        }

        // Free descriptor chain
        queue.free_chain(completed_desc);

        self.ops_completed.fetch_add(1, Ordering::Relaxed);

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

        // Allocate DMA buffers
        let (header_phys, header_virt) = Self::alloc_dma_buffer(16)?;
        let (data_phys, data_virt) = Self::alloc_dma_buffer(SECTOR_SIZE)?;
        let (status_phys, status_virt) = Self::alloc_dma_buffer(1)?;

        // Set up request header
        unsafe {
            let header = header_virt as *mut VirtioBlkReq;
            (*header).type_ = request_type::OUT;
            (*header).reserved = 0;
            (*header).sector = sector;
        }

        // Copy data to DMA buffer
        unsafe {
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), data_virt as *mut u8, SECTOR_SIZE);
        }

        // Build descriptor chain
        let buffers = [
            (header_phys, 16, false),                        // Header: device reads
            (data_phys, SECTOR_SIZE as u32, false),          // Data: device reads
            (status_phys, 1, true),                          // Status: device writes
        ];

        let mut queue = self.queue.lock();
        let _desc_head = queue
            .add_chain(&buffers)
            .ok_or("Queue full")?;

        // Notify device
        core::sync::atomic::fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        // Poll for completion (synchronous for now)
        let mut timeout = 1_000_000u32;
        while !queue.has_used() && timeout > 0 {
            core::hint::spin_loop();
            timeout -= 1;
        }

        if timeout == 0 {
            return Err("Write request timed out");
        }

        // Get completion
        let (completed_desc, _bytes) = queue.get_used().ok_or("No completion available")?;

        // Check status
        let status = unsafe { *(status_virt as *const u8) };
        if status != status_code::OK {
            return Err("Device returned error status");
        }

        // Free descriptor chain
        queue.free_chain(completed_desc);

        self.ops_completed.fetch_add(1, Ordering::Relaxed);

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

        // Check for used buffers
        // Note: The actual completion processing is done in poll_completions
        // The interrupt just signals that there's work to do.

        true
    }

    /// Get the number of completed operations
    #[allow(dead_code)] // Part of public block device API
    pub fn ops_completed(&self) -> u64 {
        self.ops_completed.load(Ordering::Relaxed)
    }
}

// Global block device instance
static BLOCK_DEVICE: Mutex<Option<Arc<VirtioBlockDevice>>> = Mutex::new(None);

/// Initialize the VirtIO block driver
///
/// Finds and initializes the first VirtIO block device.
pub fn init() -> Result<(), &'static str> {
    let devices = crate::drivers::pci::find_virtio_block_devices();

    if devices.is_empty() {
        log::warn!("VirtIO block: No devices found");
        return Err("No VirtIO block devices found");
    }

    let pci_dev = &devices[0];
    log::info!(
        "VirtIO block: Initializing device at {:02x}:{:02x}.{}",
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function
    );

    let block_dev = VirtioBlockDevice::new(pci_dev)?;
    let block_dev = Arc::new(block_dev);

    *BLOCK_DEVICE.lock() = Some(block_dev);

    // NOTE: The VirtIO interrupt handler is registered directly in the IDT.
    // See kernel/src/interrupts.rs -> virtio_block_interrupt_handler()
    // No dynamic registration needed - the handler is static.

    log::info!("VirtIO block: Driver initialized successfully");

    Ok(())
}

/// Get a reference to the block device
pub fn get_device() -> Option<Arc<VirtioBlockDevice>> {
    BLOCK_DEVICE.lock().clone()
}

/// Test the block device by reading sector 0
pub fn test_read() -> Result<(), &'static str> {
    let device = get_device().ok_or("Block device not initialized")?;

    log::info!("VirtIO block test: Reading sector 0...");

    let mut buffer = [0u8; SECTOR_SIZE];
    device.read_sector(0, &mut buffer)?;

    log::info!("VirtIO block test: Read successful!");
    log::info!(
        "  First 16 bytes: {:02x?}",
        &buffer[..16]
    );

    // Check for MBR signature
    if buffer[510] == 0x55 && buffer[511] == 0xAA {
        log::info!("  MBR signature found (0x55AA)");
    }

    Ok(())
}
