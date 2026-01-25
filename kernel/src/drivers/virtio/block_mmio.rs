//! VirtIO Block Device Driver for ARM64 (MMIO Transport)
//!
//! Implements a block device driver using VirtIO MMIO transport.
//! Uses static buffers with identity mapping for simplicity.

use super::mmio::{VirtioMmioDevice, device_id, VIRTIO_MMIO_BASE, VIRTIO_MMIO_SIZE, VIRTIO_MMIO_COUNT};
use core::ptr::read_volatile;
use core::sync::atomic::{fence, Ordering};

/// VirtIO block request types
mod request_type {
    pub const IN: u32 = 0;    // Read from device
    pub const OUT: u32 = 1;   // Write to device
}

/// VirtIO block status codes
mod status_code {
    pub const OK: u8 = 0;
    #[allow(dead_code)]
    pub const IOERR: u8 = 1;
    #[allow(dead_code)]
    pub const UNSUPP: u8 = 2;
}

/// Sector size in bytes
pub const SECTOR_SIZE: usize = 512;

/// VirtIO block request header
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioBlkReq {
    type_: u32,
    reserved: u32,
    sector: u64,
}

/// Virtqueue descriptor
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Descriptor flags
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

/// Available ring
#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; 16],  // Small queue for simplicity
}

/// Used ring element
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

/// Used ring
#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; 16],
}

/// Static queue memory - must be aligned to 4KB for VirtIO
#[repr(C, align(4096))]
struct QueueMemory {
    /// Descriptor table (16 entries * 16 bytes = 256 bytes)
    desc: [VirtqDesc; 16],
    /// Available ring (4 + 16*2 = 36 bytes, padded)
    avail: VirtqAvail,
    /// Padding to align used ring
    _padding: [u8; 4096 - 256 - 36],
    /// Used ring (4 + 16*8 = 132 bytes)
    used: VirtqUsed,
}

/// Static request header
#[repr(C, align(16))]
struct RequestHeader {
    req: VirtioBlkReq,
}

/// Static data buffer
#[repr(C, align(512))]
struct DataBuffer {
    data: [u8; SECTOR_SIZE],
}

/// Static status byte
#[repr(C, align(16))]
struct StatusBuffer {
    status: u8,
    _padding: [u8; 15],
}

// Static buffers for the block driver
static mut QUEUE_MEM: QueueMemory = QueueMemory {
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; 16],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; 16] },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed { flags: 0, idx: 0, ring: [VirtqUsedElem { id: 0, len: 0 }; 16] },
};

static mut REQ_HEADER: RequestHeader = RequestHeader {
    req: VirtioBlkReq { type_: 0, reserved: 0, sector: 0 },
};

static mut DATA_BUF: DataBuffer = DataBuffer {
    data: [0; SECTOR_SIZE],
};

static mut STATUS_BUF: StatusBuffer = StatusBuffer {
    status: 0xff,
    _padding: [0; 15],
};

/// VirtIO block device state
static mut BLOCK_DEVICE: Option<BlockDeviceState> = None;

struct BlockDeviceState {
    base: u64,
    capacity: u64,
    last_used_idx: u16,
}

/// Initialize the VirtIO block device
pub fn init() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-blk] Searching for block device...");

    // Find a block device in MMIO space
    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;
        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            if device.device_id() == device_id::BLOCK {
                crate::serial_println!("[virtio-blk] Found block device at {:#x}", base);
                return init_device(&mut device, base);
            }
        }
    }

    Err("No VirtIO block device found")
}

fn init_device(device: &mut VirtioMmioDevice, base: u64) -> Result<(), &'static str> {
    let version = device.version();
    crate::serial_println!("[virtio-blk] Device version: {}", version);

    // For v1 (legacy), we must set guest page size BEFORE init
    if version == 1 {
        device.set_guest_page_size(4096);
    }

    // Initialize the device (reset, ack, driver, features)
    device.init(0)?;  // No special features requested

    // Read capacity from config space
    // For VirtIO block: offset 0 = capacity (u64)
    let capacity = device.read_config_u64(0);
    crate::serial_println!(
        "[virtio-blk] Capacity: {} sectors ({} MB)",
        capacity,
        (capacity * SECTOR_SIZE as u64) / (1024 * 1024)
    );

    // Set up the request queue (queue 0)
    device.select_queue(0);
    let queue_num_max = device.get_queue_num_max();
    crate::serial_println!("[virtio-blk] Queue max size: {}", queue_num_max);

    if queue_num_max == 0 {
        return Err("Device reports queue size 0");
    }

    // Use a small queue size (16 entries)
    let queue_size = core::cmp::min(queue_num_max, 16);
    device.set_queue_num(queue_size);

    // Get physical address of queue memory using raw pointer (safe for &raw const)
    // With identity mapping, VA == PA for our static buffers
    let queue_phys = &raw const QUEUE_MEM as u64;

    // Initialize descriptor free list and rings
    unsafe {
        let queue_ptr = &raw mut QUEUE_MEM;
        for i in 0..15 {
            (*queue_ptr).desc[i].next = (i + 1) as u16;
        }
        (*queue_ptr).desc[15].next = 0;  // End of list
        (*queue_ptr).avail.flags = 0;
        (*queue_ptr).avail.idx = 0;
        (*queue_ptr).used.flags = 0;
        (*queue_ptr).used.idx = 0;
    }

    if version == 1 {
        // VirtIO MMIO v1 (legacy) queue setup
        // Memory layout: desc table, then avail ring, then used ring (page-aligned)
        // The PFN is the physical address divided by page size
        crate::serial_println!("[virtio-blk] Using v1 (legacy) queue setup at PFN {:#x}",
            queue_phys / 4096);

        device.set_queue_align(4096);
        device.set_queue_pfn((queue_phys / 4096) as u32);
        // In v1, writing to QUEUE_PFN enables the queue
    } else {
        // VirtIO MMIO v2 (modern) queue setup
        let desc_addr = queue_phys;
        let avail_addr = queue_phys + 256;  // After desc table
        let used_addr = queue_phys + 4096;  // After padding, 4KB aligned

        crate::serial_println!("[virtio-blk] Using v2 queue setup: desc={:#x} avail={:#x} used={:#x}",
            desc_addr, avail_addr, used_addr);

        device.set_queue_desc(desc_addr);
        device.set_queue_avail(avail_addr);
        device.set_queue_used(used_addr);
        device.set_queue_ready(true);
    }

    // Mark device as ready
    device.driver_ok();

    // Store device state
    unsafe {
        let ptr = &raw mut BLOCK_DEVICE;
        *ptr = Some(BlockDeviceState {
            base,
            capacity,
            last_used_idx: 0,
        });
    }

    crate::serial_println!("[virtio-blk] Block device initialized successfully");
    Ok(())
}

/// Read a sector from the block device
pub fn read_sector(sector: u64, buffer: &mut [u8; SECTOR_SIZE]) -> Result<(), &'static str> {
    // Use raw pointers to avoid references to mutable statics
    let state = unsafe {
        let ptr = &raw mut BLOCK_DEVICE;
        (*ptr).as_mut().ok_or("Block device not initialized")?
    };

    if sector >= state.capacity {
        return Err("Sector out of range");
    }

    // Set up request header
    unsafe {
        let req_ptr = &raw mut REQ_HEADER;
        (*req_ptr).req = VirtioBlkReq {
            type_: request_type::IN,
            reserved: 0,
            sector,
        };
        let status_ptr = &raw mut STATUS_BUF;
        (*status_ptr).status = 0xff;  // Not yet completed
    }

    // Get physical addresses using raw pointers (safe for &raw const)
    let header_phys = &raw const REQ_HEADER as u64;
    let data_phys = &raw const DATA_BUF as u64;
    let status_phys = &raw const STATUS_BUF as u64;

    // Build descriptor chain:
    // [0] header (device reads) -> [1] data (device writes) -> [2] status (device writes)
    unsafe {
        let queue_ptr = &raw mut QUEUE_MEM;

        // Descriptor 0: header
        (*queue_ptr).desc[0] = VirtqDesc {
            addr: header_phys,
            len: core::mem::size_of::<VirtioBlkReq>() as u32,
            flags: DESC_F_NEXT,
            next: 1,
        };

        // Descriptor 1: data buffer
        (*queue_ptr).desc[1] = VirtqDesc {
            addr: data_phys,
            len: SECTOR_SIZE as u32,
            flags: DESC_F_NEXT | DESC_F_WRITE,  // Device writes to this
            next: 2,
        };

        // Descriptor 2: status
        (*queue_ptr).desc[2] = VirtqDesc {
            addr: status_phys,
            len: 1,
            flags: DESC_F_WRITE,  // Device writes status
            next: 0,
        };

        // Add to available ring
        let avail_idx = (*queue_ptr).avail.idx;
        (*queue_ptr).avail.ring[(avail_idx % 16) as usize] = 0;  // Head of chain
        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify device
    let device = VirtioMmioDevice::probe(state.base).ok_or("Device disappeared")?;
    device.notify_queue(0);

    // Poll for completion
    let mut timeout = 1_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let ptr = &raw const QUEUE_MEM;
            read_volatile(&(*ptr).used.idx)
        };
        if used_idx != state.last_used_idx {
            state.last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("Block read timeout");
        }
        core::hint::spin_loop();
    }

    // Check status
    let status = unsafe {
        let ptr = &raw const STATUS_BUF;
        read_volatile(&(*ptr).status)
    };
    if status != status_code::OK {
        return Err("Block read failed");
    }

    // Copy data to caller's buffer
    unsafe {
        let ptr = &raw const DATA_BUF;
        buffer.copy_from_slice(&(*ptr).data);
    }

    Ok(())
}

/// Write a sector to the block device
#[allow(dead_code)]
pub fn write_sector(sector: u64, buffer: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {
    // Use raw pointers to avoid references to mutable statics
    let state = unsafe {
        let ptr = &raw mut BLOCK_DEVICE;
        (*ptr).as_mut().ok_or("Block device not initialized")?
    };

    if sector >= state.capacity {
        return Err("Sector out of range");
    }

    // Copy data to our buffer
    unsafe {
        let ptr = &raw mut DATA_BUF;
        (*ptr).data.copy_from_slice(buffer);
    }

    // Set up request header
    unsafe {
        let req_ptr = &raw mut REQ_HEADER;
        (*req_ptr).req = VirtioBlkReq {
            type_: request_type::OUT,
            reserved: 0,
            sector,
        };
        let status_ptr = &raw mut STATUS_BUF;
        (*status_ptr).status = 0xff;
    }

    // Get physical addresses using raw pointers (safe for &raw const)
    let header_phys = &raw const REQ_HEADER as u64;
    let data_phys = &raw const DATA_BUF as u64;
    let status_phys = &raw const STATUS_BUF as u64;

    // Build descriptor chain for write:
    // [0] header (device reads) -> [1] data (device reads) -> [2] status (device writes)
    unsafe {
        let queue_ptr = &raw mut QUEUE_MEM;

        (*queue_ptr).desc[0] = VirtqDesc {
            addr: header_phys,
            len: core::mem::size_of::<VirtioBlkReq>() as u32,
            flags: DESC_F_NEXT,
            next: 1,
        };

        (*queue_ptr).desc[1] = VirtqDesc {
            addr: data_phys,
            len: SECTOR_SIZE as u32,
            flags: DESC_F_NEXT,  // Device reads this (no WRITE flag)
            next: 2,
        };

        (*queue_ptr).desc[2] = VirtqDesc {
            addr: status_phys,
            len: 1,
            flags: DESC_F_WRITE,
            next: 0,
        };

        let avail_idx = (*queue_ptr).avail.idx;
        (*queue_ptr).avail.ring[(avail_idx % 16) as usize] = 0;
        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify device
    let device = VirtioMmioDevice::probe(state.base).ok_or("Device disappeared")?;
    device.notify_queue(0);

    // Poll for completion
    let mut timeout = 1_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let ptr = &raw const QUEUE_MEM;
            read_volatile(&(*ptr).used.idx)
        };
        if used_idx != state.last_used_idx {
            state.last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("Block write timeout");
        }
        core::hint::spin_loop();
    }

    // Check status
    let status = unsafe {
        let ptr = &raw const STATUS_BUF;
        read_volatile(&(*ptr).status)
    };
    if status != status_code::OK {
        return Err("Block write failed");
    }

    Ok(())
}

/// Get the capacity in sectors
pub fn capacity() -> Option<u64> {
    unsafe {
        let ptr = &raw const BLOCK_DEVICE;
        (*ptr).as_ref().map(|s| s.capacity)
    }
}

/// Test the block device by reading sector 0
pub fn test_read() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-blk] Testing read of sector 0...");

    let mut buffer = [0u8; SECTOR_SIZE];
    read_sector(0, &mut buffer)?;

    // Print first 32 bytes
    crate::serial_print!("[virtio-blk] Sector 0 data: ");
    for i in 0..32 {
        crate::serial_print!("{:02x} ", buffer[i]);
    }
    crate::serial_println!("...");

    crate::serial_println!("[virtio-blk] Read test passed!");
    Ok(())
}
