//! VirtIO Network Device Driver for ARM64 (MMIO Transport)
//!
//! Implements a network device driver using VirtIO MMIO transport.
//! Uses static buffers with identity mapping for simplicity.

use super::mmio::{VirtioMmioDevice, device_id, VIRTIO_MMIO_BASE, VIRTIO_MMIO_SIZE, VIRTIO_MMIO_COUNT};
use core::ptr::read_volatile;
use core::sync::atomic::{fence, Ordering};

/// VirtIO network header flags
#[allow(dead_code)]
mod hdr_flags {
    pub const NEEDS_CSUM: u8 = 1;
    pub const DATA_VALID: u8 = 2;
}

/// VirtIO network GSO types
#[allow(dead_code)]
mod gso_type {
    pub const NONE: u8 = 0;
    pub const TCPV4: u8 = 1;
    pub const UDP: u8 = 3;
    pub const TCPV6: u8 = 4;
}

/// VirtIO network features
mod features {
    #[allow(dead_code)]
    pub const MAC: u64 = 1 << 5;           // Device has given MAC address
    #[allow(dead_code)]
    pub const STATUS: u64 = 1 << 16;       // Link status available
    #[allow(dead_code)]
    pub const MRG_RXBUF: u64 = 1 << 15;    // Merge receive buffers
}

/// Maximum packet size (MTU + headers)
pub const MAX_PACKET_SIZE: usize = 1514;

/// VirtIO network header - prepended to each packet
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    num_buffers: u16,  // Only valid when VIRTIO_NET_F_MRG_RXBUF is negotiated
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
#[allow(dead_code)]
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

/// Available ring
#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; 16],
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

/// Static RX queue memory - must be aligned to 4KB for VirtIO
#[repr(C, align(4096))]
struct RxQueueMemory {
    desc: [VirtqDesc; 16],
    avail: VirtqAvail,
    _padding: [u8; 4096 - 256 - 36],
    used: VirtqUsed,
}

/// Static TX queue memory
#[repr(C, align(4096))]
struct TxQueueMemory {
    desc: [VirtqDesc; 16],
    avail: VirtqAvail,
    _padding: [u8; 4096 - 256 - 36],
    used: VirtqUsed,
}

/// RX buffer with header
#[repr(C, align(16))]
struct RxBuffer {
    hdr: VirtioNetHdr,
    data: [u8; MAX_PACKET_SIZE],
}

/// TX buffer with header
#[repr(C, align(16))]
struct TxBuffer {
    hdr: VirtioNetHdr,
    data: [u8; MAX_PACKET_SIZE],
}

// Static buffers for the network driver
static mut RX_QUEUE: RxQueueMemory = RxQueueMemory {
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; 16],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; 16] },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed { flags: 0, idx: 0, ring: [VirtqUsedElem { id: 0, len: 0 }; 16] },
};

static mut TX_QUEUE: TxQueueMemory = TxQueueMemory {
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; 16],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; 16] },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed { flags: 0, idx: 0, ring: [VirtqUsedElem { id: 0, len: 0 }; 16] },
};

// RX/TX buffers - need to initialize each element separately due to size
static mut RX_BUFFER_0: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0, num_buffers: 0 },
    data: [0; MAX_PACKET_SIZE],
};
static mut RX_BUFFER_1: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0, num_buffers: 0 },
    data: [0; MAX_PACKET_SIZE],
};
static mut RX_BUFFER_2: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0, num_buffers: 0 },
    data: [0; MAX_PACKET_SIZE],
};
static mut RX_BUFFER_3: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0, num_buffers: 0 },
    data: [0; MAX_PACKET_SIZE],
};

static mut TX_BUFFER: TxBuffer = TxBuffer {
    hdr: VirtioNetHdr { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0, num_buffers: 0 },
    data: [0; MAX_PACKET_SIZE],
};

/// Network device state
static mut NET_DEVICE: Option<NetDeviceState> = None;

struct NetDeviceState {
    base: u64,
    mac: [u8; 6],
    rx_last_used_idx: u16,
    tx_last_used_idx: u16,
}

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    addr - crate::memory::physical_memory_offset().as_u64()
}

/// Initialize the VirtIO network device
pub fn init() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-net] Searching for network device...");

    // Find a network device in MMIO space
    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;
        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            if device.device_id() == device_id::NETWORK {
                crate::serial_println!("[virtio-net] Found network device at {:#x}", base);
                return init_device(&mut device, base);
            }
        }
    }

    Err("No VirtIO network device found")
}

fn init_device(device: &mut VirtioMmioDevice, base: u64) -> Result<(), &'static str> {
    let version = device.version();
    crate::serial_println!("[virtio-net] Device version: {}", version);

    // For v1 (legacy), we must set guest page size BEFORE init
    if version == 1 {
        device.set_guest_page_size(4096);
    }

    // Initialize the device (reset, ack, driver, features)
    // Request MAC feature
    device.init(features::MAC)?;

    // Read MAC address from config space
    // VirtIO network config: MAC at offset 0 (6 bytes)
    let mac = [
        device.read_config_u8(0),
        device.read_config_u8(1),
        device.read_config_u8(2),
        device.read_config_u8(3),
        device.read_config_u8(4),
        device.read_config_u8(5),
    ];
    crate::serial_println!(
        "[virtio-net] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    // Set up RX queue (queue 0)
    setup_rx_queue(device, version)?;

    // Set up TX queue (queue 1)
    setup_tx_queue(device, version)?;

    // Mark device as ready
    device.driver_ok();

    // Store device state
    unsafe {
        let ptr = &raw mut NET_DEVICE;
        *ptr = Some(NetDeviceState {
            base,
            mac,
            rx_last_used_idx: 0,
            tx_last_used_idx: 0,
        });
    }

    // Post initial RX buffers
    post_rx_buffers()?;

    crate::serial_println!("[virtio-net] Network device initialized successfully");
    Ok(())
}

fn setup_rx_queue(device: &mut VirtioMmioDevice, version: u32) -> Result<(), &'static str> {
    device.select_queue(0);  // RX queue
    let queue_num_max = device.get_queue_num_max();
    crate::serial_println!("[virtio-net] RX queue max size: {}", queue_num_max);

    if queue_num_max == 0 {
        return Err("RX queue size is 0");
    }

    let queue_size = core::cmp::min(queue_num_max, 16);
    device.set_queue_num(queue_size);

    let queue_phys = virt_to_phys(&raw const RX_QUEUE as u64);

    // Initialize descriptor table and rings
    unsafe {
        let queue_ptr = &raw mut RX_QUEUE;
        for i in 0..15 {
            (*queue_ptr).desc[i].next = (i + 1) as u16;
        }
        (*queue_ptr).desc[15].next = 0;
        (*queue_ptr).avail.flags = 0;
        (*queue_ptr).avail.idx = 0;
        (*queue_ptr).used.flags = 0;
        (*queue_ptr).used.idx = 0;
    }

    if version == 1 {
        device.set_queue_align(4096);
        device.set_queue_pfn((queue_phys / 4096) as u32);
    } else {
        device.set_queue_desc(queue_phys);
        device.set_queue_avail(queue_phys + 256);
        device.set_queue_used(queue_phys + 4096);
        device.set_queue_ready(true);
    }

    Ok(())
}

fn setup_tx_queue(device: &mut VirtioMmioDevice, version: u32) -> Result<(), &'static str> {
    device.select_queue(1);  // TX queue
    let queue_num_max = device.get_queue_num_max();
    crate::serial_println!("[virtio-net] TX queue max size: {}", queue_num_max);

    if queue_num_max == 0 {
        return Err("TX queue size is 0");
    }

    let queue_size = core::cmp::min(queue_num_max, 16);
    device.set_queue_num(queue_size);

    let queue_phys = virt_to_phys(&raw const TX_QUEUE as u64);

    // Initialize descriptor table and rings
    unsafe {
        let queue_ptr = &raw mut TX_QUEUE;
        for i in 0..15 {
            (*queue_ptr).desc[i].next = (i + 1) as u16;
        }
        (*queue_ptr).desc[15].next = 0;
        (*queue_ptr).avail.flags = 0;
        (*queue_ptr).avail.idx = 0;
        (*queue_ptr).used.flags = 0;
        (*queue_ptr).used.idx = 0;
    }

    if version == 1 {
        device.set_queue_align(4096);
        device.set_queue_pfn((queue_phys / 4096) as u32);
    } else {
        device.set_queue_desc(queue_phys);
        device.set_queue_avail(queue_phys + 256);
        device.set_queue_used(queue_phys + 4096);
        device.set_queue_ready(true);
    }

    Ok(())
}

/// Get the physical address of an RX buffer by index
fn rx_buffer_phys(idx: usize) -> u64 {
    match idx {
        0 => virt_to_phys(&raw const RX_BUFFER_0 as u64),
        1 => virt_to_phys(&raw const RX_BUFFER_1 as u64),
        2 => virt_to_phys(&raw const RX_BUFFER_2 as u64),
        3 => virt_to_phys(&raw const RX_BUFFER_3 as u64),
        _ => 0,
    }
}

/// Get the data portion of an RX buffer by index
fn rx_buffer_data(idx: usize) -> Option<&'static [u8]> {
    unsafe {
        match idx {
            0 => Some(&(&raw const RX_BUFFER_0).as_ref().unwrap().data[..]),
            1 => Some(&(&raw const RX_BUFFER_1).as_ref().unwrap().data[..]),
            2 => Some(&(&raw const RX_BUFFER_2).as_ref().unwrap().data[..]),
            3 => Some(&(&raw const RX_BUFFER_3).as_ref().unwrap().data[..]),
            _ => None,
        }
    }
}

/// Post RX buffers to the device for receiving packets
fn post_rx_buffers() -> Result<(), &'static str> {
    let state = unsafe {
        let ptr = &raw mut NET_DEVICE;
        (*ptr).as_mut().ok_or("Network device not initialized")?
    };

    let device = VirtioMmioDevice::probe(state.base).ok_or("Device disappeared")?;

    unsafe {
        let queue_ptr = &raw mut RX_QUEUE;

        // Post 4 RX buffers
        for i in 0..4 {
            let buf_phys = rx_buffer_phys(i);

            // Single descriptor for entire buffer (header + data)
            (*queue_ptr).desc[i] = VirtqDesc {
                addr: buf_phys,
                len: (core::mem::size_of::<VirtioNetHdr>() + MAX_PACKET_SIZE) as u32,
                flags: DESC_F_WRITE,  // Device writes to this
                next: 0,
            };

            // Add to available ring
            (*queue_ptr).avail.ring[i] = i as u16;
        }

        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = 4;
        fence(Ordering::SeqCst);
    }

    // Notify device about RX queue (queue 0)
    device.notify_queue(0);

    Ok(())
}

/// Transmit a packet
#[allow(dead_code)]
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    if data.len() > MAX_PACKET_SIZE {
        return Err("Packet too large");
    }

    let state = unsafe {
        let ptr = &raw mut NET_DEVICE;
        (*ptr).as_mut().ok_or("Network device not initialized")?
    };

    let device = VirtioMmioDevice::probe(state.base).ok_or("Device disappeared")?;

    // Set up TX buffer
    unsafe {
        let tx_ptr = &raw mut TX_BUFFER;
        (*tx_ptr).hdr = VirtioNetHdr {
            flags: 0,
            gso_type: gso_type::NONE,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
            num_buffers: 0,
        };
        (&mut (*tx_ptr).data)[..data.len()].copy_from_slice(data);
    }

    // Build descriptor
    let tx_phys = virt_to_phys(&raw const TX_BUFFER as u64);
    let total_len = core::mem::size_of::<VirtioNetHdr>() + data.len();

    unsafe {
        let queue_ptr = &raw mut TX_QUEUE;

        (*queue_ptr).desc[0] = VirtqDesc {
            addr: tx_phys,
            len: total_len as u32,
            flags: 0,  // Device reads this
            next: 0,
        };

        let avail_idx = (*queue_ptr).avail.idx;
        (*queue_ptr).avail.ring[(avail_idx % 16) as usize] = 0;
        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify device about TX queue (queue 1)
    device.notify_queue(1);

    // Wait for completion
    let mut timeout = 1_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let ptr = &raw const TX_QUEUE;
            read_volatile(&(*ptr).used.idx)
        };
        if used_idx != state.tx_last_used_idx {
            state.tx_last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("TX timeout");
        }
        core::hint::spin_loop();
    }

    Ok(())
}

/// Check for received packets (non-blocking)
/// Returns the packet data if one is available
#[allow(dead_code)]
pub fn receive() -> Option<&'static [u8]> {
    let state = unsafe {
        let ptr = &raw mut NET_DEVICE;
        (*ptr).as_mut()?
    };

    fence(Ordering::SeqCst);
    let used_idx = unsafe {
        let ptr = &raw const RX_QUEUE;
        read_volatile(&(*ptr).used.idx)
    };

    if used_idx == state.rx_last_used_idx {
        return None;  // No new packets
    }

    // Get the used element
    let (desc_idx, packet_len) = unsafe {
        let ptr = &raw const RX_QUEUE;
        let elem = &(*ptr).used.ring[(state.rx_last_used_idx % 16) as usize];
        (elem.id as usize, elem.len as usize)
    };

    state.rx_last_used_idx = state.rx_last_used_idx.wrapping_add(1);

    // Get packet data (skip header)
    let hdr_size = core::mem::size_of::<VirtioNetHdr>();
    if packet_len <= hdr_size {
        return None;  // Invalid packet
    }

    let data_len = packet_len - hdr_size;
    rx_buffer_data(desc_idx).map(|buf| &buf[..data_len])
}

/// Get the MAC address
pub fn mac_address() -> Option<[u8; 6]> {
    unsafe {
        let ptr = &raw const NET_DEVICE;
        (*ptr).as_ref().map(|s| s.mac)
    }
}

/// Test the network device
pub fn test_device() -> Result<(), &'static str> {
    let mac = mac_address().ok_or("Network device not initialized")?;
    crate::serial_println!(
        "[virtio-net] Device test - MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    crate::serial_println!("[virtio-net] Test passed!");
    Ok(())
}
