//! VirtIO Network Device Driver for ARM64 (PCI Legacy Transport)
//!
//! Implements a network device driver using VirtIO legacy PCI transport.
//! On Parallels, the VirtIO network device (1af4:1000) is a legacy transitional
//! device that exposes registers via BAR0 memory-mapped I/O.
//!
//! Legacy VirtIO PCI register layout (at BAR0):
//!   0x00: Device Features (read, 32-bit)
//!   0x04: Guest Features (write, 32-bit)
//!   0x08: Queue PFN (r/w, 32-bit)
//!   0x0C: Queue Size (read, 16-bit)
//!   0x0E: Queue Select (write, 16-bit)
//!   0x10: Queue Notify (write, 16-bit)
//!   0x12: Device Status (r/w, 8-bit)
//!   0x13: ISR Status (read, 8-bit)
//!   0x14+: Device config (MAC at 0x14-0x19)

use crate::drivers::pci;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, AtomicU32, Ordering};

// Legacy VirtIO PCI register offsets (from BAR0)
const REG_DEVICE_FEATURES: usize = 0x00;
const REG_GUEST_FEATURES: usize = 0x04;
const REG_QUEUE_PFN: usize = 0x08;
const REG_QUEUE_SIZE: usize = 0x0C;
const REG_QUEUE_SELECT: usize = 0x0E;
const REG_QUEUE_NOTIFY: usize = 0x10;
const REG_DEVICE_STATUS: usize = 0x12;
const REG_ISR_STATUS: usize = 0x13;
const REG_DEVICE_CONFIG: usize = 0x14;

// VirtIO status bits
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

/// VirtIO network features
const FEATURE_MAC: u32 = 1 << 5;
const FEATURE_STATUS: u32 = 1 << 16;

/// Maximum packet size (MTU + headers)
pub const MAX_PACKET_SIZE: usize = 1514;

/// VirtIO network header — 10 bytes for legacy without MRG_RXBUF
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
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

const DESC_F_WRITE: u16 = 2;

/// When set in avail.flags, tells the device NOT to send interrupts (MSIs)
/// when it adds entries to the used ring. Used for NAPI-style interrupt
/// coalescing: handler sets this to suppress MSI storm, softirq clears it
/// after draining the used ring.
const VRING_AVAIL_F_NO_INTERRUPT: u16 = 1;

/// Legacy VirtIO queue size — must match what the device reports.
/// Parallels reports 256; the driver can't change it on legacy transport.
const VIRTQ_SIZE: usize = 256;

/// Available ring
#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; VIRTQ_SIZE],
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
    ring: [VirtqUsedElem; VIRTQ_SIZE],
}

/// Static queue memory — must be aligned to 4KB for VirtIO legacy.
/// Layout per VirtIO 0.9.5 legacy spec:
///   Descriptor table: VIRTQ_SIZE × 16 = 4096 bytes
///   Available ring:   4 + 2×VIRTQ_SIZE = 516 bytes
///   Padding to next 4096 boundary: 3580 bytes
///   Used ring:        4 + 8×VIRTQ_SIZE = 2052 bytes
#[repr(C, align(4096))]
struct QueueMemory {
    desc: [VirtqDesc; VIRTQ_SIZE],
    avail: VirtqAvail,
    _padding: [u8; 8192 - 4096 - (4 + 2 * VIRTQ_SIZE)],
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

// Static buffers
static mut PCI_RX_QUEUE: QueueMemory = QueueMemory {
    desc: [VirtqDesc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; VIRTQ_SIZE],
    avail: VirtqAvail {
        flags: 0,
        idx: 0,
        ring: [0; VIRTQ_SIZE],
    },
    _padding: [0; 8192 - 4096 - (4 + 2 * VIRTQ_SIZE)],
    used: VirtqUsed {
        flags: 0,
        idx: 0,
        ring: [VirtqUsedElem { id: 0, len: 0 }; VIRTQ_SIZE],
    },
};

static mut PCI_TX_QUEUE: QueueMemory = QueueMemory {
    desc: [VirtqDesc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; VIRTQ_SIZE],
    avail: VirtqAvail {
        flags: 0,
        idx: 0,
        ring: [0; VIRTQ_SIZE],
    },
    _padding: [0; 8192 - 4096 - (4 + 2 * VIRTQ_SIZE)],
    used: VirtqUsed {
        flags: 0,
        idx: 0,
        ring: [VirtqUsedElem { id: 0, len: 0 }; VIRTQ_SIZE],
    },
};

static mut PCI_RX_BUFFER_0: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: 0,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    data: [0; MAX_PACKET_SIZE],
};
static mut PCI_RX_BUFFER_1: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: 0,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    data: [0; MAX_PACKET_SIZE],
};
static mut PCI_RX_BUFFER_2: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: 0,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    data: [0; MAX_PACKET_SIZE],
};
static mut PCI_RX_BUFFER_3: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: 0,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    data: [0; MAX_PACKET_SIZE],
};

static mut PCI_TX_BUFFER: TxBuffer = TxBuffer {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: 0,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    data: [0; MAX_PACKET_SIZE],
};

/// HHDM base for physical-to-virtual translation
const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Network device state
struct NetPciState {
    /// BAR0 virtual address (HHDM-mapped)
    bar0_virt: u64,
    mac: [u8; 6],
    rx_last_used_idx: u16,
    tx_last_used_idx: u16,
}

static mut NET_PCI_STATE: Option<NetPciState> = None;
static DEVICE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static NET_PCI_IRQ: AtomicU32 = AtomicU32::new(0);
static NET_PCI_MSI_COUNT: AtomicU32 = AtomicU32::new(0);

// Legacy register access helpers
#[inline(always)]
fn reg_read_u8(bar0: u64, offset: usize) -> u8 {
    unsafe { read_volatile((bar0 + offset as u64) as *const u8) }
}

#[inline(always)]
fn reg_write_u8(bar0: u64, offset: usize, val: u8) {
    unsafe { write_volatile((bar0 + offset as u64) as *mut u8, val) }
}

#[inline(always)]
fn reg_read_u16(bar0: u64, offset: usize) -> u16 {
    unsafe { read_volatile((bar0 + offset as u64) as *const u16) }
}

#[inline(always)]
fn reg_write_u16(bar0: u64, offset: usize, val: u16) {
    unsafe { write_volatile((bar0 + offset as u64) as *mut u16, val) }
}

#[inline(always)]
fn reg_read_u32(bar0: u64, offset: usize) -> u32 {
    unsafe { read_volatile((bar0 + offset as u64) as *const u32) }
}

#[inline(always)]
fn reg_write_u32(bar0: u64, offset: usize, val: u32) {
    unsafe { write_volatile((bar0 + offset as u64) as *mut u32, val) }
}

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    addr - crate::memory::physical_memory_offset().as_u64()
}

/// Get the GIC INTID for the VirtIO PCI net interrupt, if MSI is enabled.
pub fn get_irq() -> Option<u32> {
    let irq = NET_PCI_IRQ.load(Ordering::Relaxed);
    if irq != 0 {
        Some(irq)
    } else {
        None
    }
}

/// VirtIO legacy MSI-X register offsets (present when MSI-X is enabled at PCI level).
/// These replace the device config at BAR0+0x14; device config shifts to 0x18.
const MSIX_CONFIG_VECTOR: usize = 0x14;
const MSIX_QUEUE_VECTOR: usize = 0x16;

/// Resolve a GICv2m doorbell address. Returns the MSI_SETSPI_NS physical address.
fn resolve_gicv2m_doorbell() -> Option<u64> {
    const PARALLELS_GICV2M_BASE: u64 = 0x0225_0000;
    let gicv2m_base = crate::platform_config::gicv2m_base_phys();
    let base = if gicv2m_base != 0 {
        gicv2m_base
    } else if crate::platform_config::probe_gicv2m(PARALLELS_GICV2M_BASE) {
        PARALLELS_GICV2M_BASE
    } else {
        return None;
    };
    Some(base + 0x40)
}

/// Set up PCI MSI or MSI-X delivery for the VirtIO network device through GICv2m.
fn setup_net_pci_msi(pci_dev: &crate::drivers::pci::Device) {
    use crate::arch_impl::aarch64::gic;

    pci_dev.dump_capabilities();

    // Try plain MSI first (some VirtIO devices have this)
    if let Some(cap_offset) = pci_dev.find_msi_capability() {
        crate::serial_println!(
            "[virtio-net-pci] Found MSI capability at offset {:#x}",
            cap_offset
        );
        if let Some(doorbell) = resolve_gicv2m_doorbell() {
            let spi = crate::platform_config::allocate_msi_spi();
            if spi != 0 {
                pci_dev.configure_msi(cap_offset, doorbell as u32, spi as u16);
                pci_dev.disable_intx();
                gic::configure_spi_edge_triggered(spi);
                NET_PCI_IRQ.store(spi, Ordering::Relaxed);
                gic::enable_spi(spi);
                crate::serial_println!(
                    "[virtio-net-pci] MSI enabled: SPI {} doorbell={:#x}",
                    spi,
                    doorbell
                );
                return;
            }
        }
        crate::serial_println!("[virtio-net-pci] MSI setup failed — trying MSI-X");
    }

    // Try MSI-X (Parallels VirtIO net PCI 1af4:1000 has MSI-X with 3 vectors)
    let msix_cap = match pci_dev.find_msix_capability() {
        Some(cap) => cap,
        None => {
            crate::serial_println!(
                "[virtio-net-pci] No MSI or MSI-X capability — polling fallback"
            );
            return;
        }
    };

    let table_size = pci_dev.msix_table_size(msix_cap);
    crate::serial_println!(
        "[virtio-net-pci] MSI-X cap at {:#x}: {} vectors",
        msix_cap,
        table_size
    );

    let doorbell = match resolve_gicv2m_doorbell() {
        Some(d) => d,
        None => {
            crate::serial_println!("[virtio-net-pci] GICv2m not available — polling fallback");
            return;
        }
    };

    let spi = crate::platform_config::allocate_msi_spi();
    if spi == 0 {
        crate::serial_println!("[virtio-net-pci] Failed to allocate MSI SPI — polling fallback");
        return;
    }

    // Program all MSI-X table entries with the same SPI (single-vector mode).
    for v in 0..table_size {
        pci_dev.configure_msix_entry(msix_cap, v, doorbell, spi);
    }

    gic::configure_spi_edge_triggered(spi);
    // Store IRQ but do NOT enable the SPI yet. The SPI is enabled by
    // enable_msi_spi() after init_common() completes its synchronous
    // ARP/ICMP polling. This avoids the GICv2m level-triggered SPI storm
    // during init (the device fires MSIs for ARP replies, and the level
    // stays asserted through EOI).
    NET_PCI_IRQ.store(spi, Ordering::Release);

    // Enable MSI-X at PCI level and disable legacy INTx
    pci_dev.enable_msix(msix_cap);
    pci_dev.disable_intx();

    // Assign VirtIO-level MSI-X vectors.
    let bar0_virt = unsafe {
        let ptr = &raw const NET_PCI_STATE;
        match (*ptr).as_ref() {
            Some(s) => s.bar0_virt,
            None => {
                crate::serial_println!("[virtio-net-pci] MSI-X: device state not available");
                return;
            }
        }
    };

    // Config change → no interrupt (0xFFFF). Avoids spurious config-change
    // MSIs that could cause an interrupt storm unrelated to packet RX.
    reg_write_u16(bar0_virt, MSIX_CONFIG_VECTOR, 0xFFFF);
    let cfg_rb = reg_read_u16(bar0_virt, MSIX_CONFIG_VECTOR);

    // RX queue (0) → vector 0
    reg_write_u16(bar0_virt, REG_QUEUE_SELECT, 0);
    reg_write_u16(bar0_virt, MSIX_QUEUE_VECTOR, 0);
    let rx_rb = reg_read_u16(bar0_virt, MSIX_QUEUE_VECTOR);

    // TX queue (1) → no interrupt
    reg_write_u16(bar0_virt, REG_QUEUE_SELECT, 1);
    reg_write_u16(bar0_virt, MSIX_QUEUE_VECTOR, 0xFFFF);

    crate::serial_println!(
        "[virtio-net-pci] MSI-X vector assignments: cfg={:#x} rx={:#x}",
        cfg_rb,
        rx_rb
    );

    // Only RX vector must succeed; config vector is intentionally 0xFFFF
    if rx_rb == 0xFFFF {
        crate::serial_println!(
            "[virtio-net-pci] MSI-X: device rejected RX vector — polling fallback"
        );
        pci_dev.disable_msix(msix_cap);
        pci_dev.enable_intx();
        NET_PCI_IRQ.store(0, Ordering::Relaxed);
        return;
    }

    crate::serial_println!(
        "[virtio-net-pci] MSI-X enabled: SPI {} doorbell={:#x} vectors={}",
        spi,
        doorbell,
        table_size
    );
}

/// Initialize the VirtIO network device via PCI legacy transport.
pub fn init() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-net-pci] Searching for VirtIO network device on PCI bus...");

    let net_devices = pci::find_virtio_net_devices();
    if net_devices.is_empty() {
        return Err("No VirtIO network PCI device found");
    }

    let pci_dev = &net_devices[0];
    crate::serial_println!(
        "[virtio-net-pci] Found at {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        pci_dev.vendor_id,
        pci_dev.device_id
    );

    // Get BAR0 physical address
    let bar0 = &pci_dev.bars[0];
    if !bar0.is_valid() || bar0.is_io {
        return Err("BAR0 not valid or is I/O port (need MMIO)");
    }

    let bar0_phys = bar0.address;
    let bar0_virt = HHDM_BASE + bar0_phys;
    crate::serial_println!(
        "[virtio-net-pci] BAR0: phys={:#x} virt={:#x} size={}",
        bar0_phys,
        bar0_virt,
        bar0.size
    );

    // Enable PCI memory space and bus mastering
    pci_dev.enable_memory_space();
    pci_dev.enable_bus_master();

    // VirtIO legacy initialization sequence
    // Step 1: Reset
    reg_write_u8(bar0_virt, REG_DEVICE_STATUS, 0);
    for _ in 0..10_000 {
        if reg_read_u8(bar0_virt, REG_DEVICE_STATUS) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Step 2: ACKNOWLEDGE
    reg_write_u8(bar0_virt, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE);

    // Step 3: DRIVER
    reg_write_u8(
        bar0_virt,
        REG_DEVICE_STATUS,
        STATUS_ACKNOWLEDGE | STATUS_DRIVER,
    );

    // Step 4: Negotiate features
    let device_features = reg_read_u32(bar0_virt, REG_DEVICE_FEATURES);
    crate::serial_println!(
        "[virtio-net-pci] Device features: {:#010x}",
        device_features
    );
    let guest_features = device_features & (FEATURE_MAC | FEATURE_STATUS);
    reg_write_u32(bar0_virt, REG_GUEST_FEATURES, guest_features);

    // Step 5: FEATURES_OK (for transitional devices that support it)
    reg_write_u8(
        bar0_virt,
        REG_DEVICE_STATUS,
        STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
    );
    let status = reg_read_u8(bar0_virt, REG_DEVICE_STATUS);
    if (status & STATUS_FEATURES_OK) == 0 {
        // Legacy-only device doesn't support FEATURES_OK — proceed without it
        crate::serial_println!(
            "[virtio-net-pci] FEATURES_OK not supported (pure legacy), continuing"
        );
        reg_write_u8(
            bar0_virt,
            REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER,
        );
    }

    // Read MAC address from device config (offset 0x14)
    let mac = [
        reg_read_u8(bar0_virt, REG_DEVICE_CONFIG + 0),
        reg_read_u8(bar0_virt, REG_DEVICE_CONFIG + 1),
        reg_read_u8(bar0_virt, REG_DEVICE_CONFIG + 2),
        reg_read_u8(bar0_virt, REG_DEVICE_CONFIG + 3),
        reg_read_u8(bar0_virt, REG_DEVICE_CONFIG + 4),
        reg_read_u8(bar0_virt, REG_DEVICE_CONFIG + 5),
    ];
    crate::serial_println!(
        "[virtio-net-pci] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );

    // Set up RX queue (queue 0)
    setup_legacy_queue(bar0_virt, 0, &raw const PCI_RX_QUEUE as u64)?;

    // Set up TX queue (queue 1)
    setup_legacy_queue(bar0_virt, 1, &raw const PCI_TX_QUEUE as u64)?;

    // DRIVER_OK
    let cur_status = reg_read_u8(bar0_virt, REG_DEVICE_STATUS);
    reg_write_u8(bar0_virt, REG_DEVICE_STATUS, cur_status | STATUS_DRIVER_OK);

    // Store state
    unsafe {
        let ptr = &raw mut NET_PCI_STATE;
        *ptr = Some(NetPciState {
            bar0_virt,
            mac,
            rx_last_used_idx: 0,
            tx_last_used_idx: 0,
        });
    }

    // Post initial RX buffers
    post_rx_buffers()?;

    DEVICE_INITIALIZED.store(true, Ordering::Release);
    setup_net_pci_msi(pci_dev);
    crate::serial_println!("[virtio-net-pci] Network device initialized successfully");
    Ok(())
}

fn setup_legacy_queue(
    bar0_virt: u64,
    queue_idx: u16,
    queue_virt_addr: u64,
) -> Result<(), &'static str> {
    // Select queue
    reg_write_u16(bar0_virt, REG_QUEUE_SELECT, queue_idx);

    // Read max queue size
    let queue_max = reg_read_u16(bar0_virt, REG_QUEUE_SIZE);
    crate::serial_println!(
        "[virtio-net-pci] Queue {} max size: {}",
        queue_idx,
        queue_max
    );

    if queue_max == 0 {
        return Err("Queue size is 0");
    }

    // Initialize descriptor chain
    unsafe {
        let q = queue_virt_addr as *mut QueueMemory;
        for i in 0..(VIRTQ_SIZE - 1) {
            (*q).desc[i].next = (i + 1) as u16;
        }
        (*q).desc[VIRTQ_SIZE - 1].next = 0;
        (*q).avail.flags = 0;
        (*q).avail.idx = 0;
        (*q).used.flags = 0;
        (*q).used.idx = 0;
    }

    // Legacy VirtIO: set queue PFN (physical page frame number)
    let queue_phys = virt_to_phys(queue_virt_addr);
    let queue_pfn = (queue_phys / 4096) as u32;
    reg_write_u32(bar0_virt, REG_QUEUE_PFN, queue_pfn);

    crate::serial_println!(
        "[virtio-net-pci] Queue {} PFN={:#x} (phys={:#x})",
        queue_idx,
        queue_pfn,
        queue_phys
    );

    Ok(())
}

/// Get the physical address of an RX buffer by index
fn rx_buffer_phys(idx: usize) -> u64 {
    match idx {
        0 => virt_to_phys(&raw const PCI_RX_BUFFER_0 as u64),
        1 => virt_to_phys(&raw const PCI_RX_BUFFER_1 as u64),
        2 => virt_to_phys(&raw const PCI_RX_BUFFER_2 as u64),
        3 => virt_to_phys(&raw const PCI_RX_BUFFER_3 as u64),
        _ => 0,
    }
}

/// Get the data portion of an RX buffer by index
fn rx_buffer_data(idx: usize) -> Option<&'static [u8]> {
    unsafe {
        match idx {
            0 => Some(&(&raw const PCI_RX_BUFFER_0).as_ref().unwrap().data[..]),
            1 => Some(&(&raw const PCI_RX_BUFFER_1).as_ref().unwrap().data[..]),
            2 => Some(&(&raw const PCI_RX_BUFFER_2).as_ref().unwrap().data[..]),
            3 => Some(&(&raw const PCI_RX_BUFFER_3).as_ref().unwrap().data[..]),
            _ => None,
        }
    }
}

/// Post RX buffers to the device for receiving packets
fn post_rx_buffers() -> Result<(), &'static str> {
    let state = unsafe {
        let ptr = &raw mut NET_PCI_STATE;
        (*ptr).as_ref().ok_or("Network device not initialized")?
    };

    unsafe {
        let q = &raw mut PCI_RX_QUEUE;

        for i in 0..4 {
            let buf_phys = rx_buffer_phys(i);
            let buf_len = (core::mem::size_of::<VirtioNetHdr>() + MAX_PACKET_SIZE) as u32;

            (*q).desc[i] = VirtqDesc {
                addr: buf_phys,
                len: buf_len,
                flags: DESC_F_WRITE,
                next: 0,
            };

            (*q).avail.ring[i] = i as u16;
        }

        fence(Ordering::SeqCst);
        (*q).avail.idx = 4;
        fence(Ordering::SeqCst);
    }

    // Notify device about RX queue (queue 0)
    reg_write_u16(state.bar0_virt, REG_QUEUE_NOTIFY, 0);

    Ok(())
}

/// Transmit a packet
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    if data.len() > MAX_PACKET_SIZE {
        return Err("Packet too large");
    }

    let state = unsafe {
        let ptr = &raw mut NET_PCI_STATE;
        (*ptr).as_mut().ok_or("Network device not initialized")?
    };

    // Set up TX buffer
    unsafe {
        let tx_ptr = &raw mut PCI_TX_BUFFER;
        (*tx_ptr).hdr = VirtioNetHdr::default();
        (&mut (*tx_ptr).data)[..data.len()].copy_from_slice(data);
    }

    // Build descriptor
    let tx_phys = virt_to_phys(&raw const PCI_TX_BUFFER as u64);
    let total_len = core::mem::size_of::<VirtioNetHdr>() + data.len();

    unsafe {
        let q = &raw mut PCI_TX_QUEUE;

        (*q).desc[0] = VirtqDesc {
            addr: tx_phys,
            len: total_len as u32,
            flags: 0,
            next: 0,
        };

        let avail_idx = (*q).avail.idx;
        (*q).avail.ring[(avail_idx % VIRTQ_SIZE as u16) as usize] = 0;
        fence(Ordering::SeqCst);
        (*q).avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify device about TX queue (queue 1)
    reg_write_u16(state.bar0_virt, REG_QUEUE_NOTIFY, 1);

    // Wait for completion
    let mut timeout = 1_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let q = &raw const PCI_TX_QUEUE;
            read_volatile(&(*q).used.idx)
        };
        if used_idx != state.tx_last_used_idx {
            state.tx_last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            crate::serial_println!("[virtio-net-pci] TX timeout!");
            return Err("TX timeout");
        }
        core::hint::spin_loop();
    }

    Ok(())
}

/// Check for received packets (non-blocking).
pub fn receive() -> Option<&'static [u8]> {
    let state = unsafe {
        let ptr = &raw mut NET_PCI_STATE;
        (*ptr).as_mut()?
    };

    // Check and clear ISR status (reading clears it on legacy)
    let isr = reg_read_u8(state.bar0_virt, REG_ISR_STATUS);
    let _ = isr;

    fence(Ordering::SeqCst);

    let used_idx = unsafe {
        let q = &raw const PCI_RX_QUEUE;
        read_volatile(&(*q).used.idx)
    };

    if used_idx == state.rx_last_used_idx {
        return None;
    }

    let (desc_idx, packet_len) = unsafe {
        let q = &raw const PCI_RX_QUEUE;
        let elem = &(*q).used.ring[(state.rx_last_used_idx % VIRTQ_SIZE as u16) as usize];
        (elem.id as usize, elem.len as usize)
    };

    state.rx_last_used_idx = state.rx_last_used_idx.wrapping_add(1);

    let hdr_size = core::mem::size_of::<VirtioNetHdr>();
    if packet_len <= hdr_size {
        return None;
    }

    let data_len = packet_len - hdr_size;
    rx_buffer_data(desc_idx).map(|buf| &buf[..data_len])
}

/// Re-post all consumed RX buffers back to the device.
pub fn recycle_rx_buffers() {
    let state = unsafe {
        let ptr = &raw mut NET_PCI_STATE;
        match (*ptr).as_ref() {
            Some(s) => s,
            None => return,
        }
    };

    unsafe {
        let q = &raw mut PCI_RX_QUEUE;
        let mut avail_idx = (*q).avail.idx;

        for i in 0..4u16 {
            (*q).avail.ring[(avail_idx % VIRTQ_SIZE as u16) as usize] = i;
            avail_idx = avail_idx.wrapping_add(1);
        }

        fence(Ordering::SeqCst);
        (*q).avail.idx = avail_idx;
        fence(Ordering::SeqCst);
    }

    reg_write_u16(state.bar0_virt, REG_QUEUE_NOTIFY, 0);
}

/// Get the MAC address
pub fn mac_address() -> Option<[u8; 6]> {
    unsafe {
        let ptr = &raw const NET_PCI_STATE;
        (*ptr).as_ref().map(|s| s.mac)
    }
}

/// Get the MSI interrupt count (for diagnostics).
pub fn msi_interrupt_count() -> u32 {
    NET_PCI_MSI_COUNT.load(Ordering::Relaxed)
}

/// Interrupt handler for VirtIO network PCI device (MSI-X).
///
/// Uses NAPI-style two-level suppression to prevent GICv2m SPI storms:
/// 1. Device-level: sets VRING_AVAIL_F_NO_INTERRUPT so the device stops
///    writing MSIs to GICv2m entirely.
/// 2. GIC-level: disables the SPI as a safety net.
///
/// Does NOT process packets or raise softirq (locks in the packet
/// processing path could deadlock with the interrupted thread).
/// Timer-based NetRx softirq handles packet processing and calls
/// re_enable_irq() to re-arm both levels.
pub fn handle_interrupt() {
    use crate::arch_impl::aarch64::gic;

    NET_PCI_MSI_COUNT.fetch_add(1, Ordering::Relaxed);

    let irq = NET_PCI_IRQ.load(Ordering::Relaxed);
    if irq == 0 {
        return;
    }

    // Suppress at the device level FIRST — prevents new MSI writes to GICv2m.
    unsafe {
        let q = &raw mut PCI_RX_QUEUE;
        write_volatile(&mut (*q).avail.flags, VRING_AVAIL_F_NO_INTERRUPT);
        fence(Ordering::SeqCst);
    }

    // Mask SPI at the GIC — belt-and-suspenders with device-level suppression.
    gic::disable_spi(irq);
    gic::clear_spi_pending(irq);

    // Read ISR to clear the VirtIO device's internal interrupt condition.
    let state = &raw const NET_PCI_STATE;
    unsafe {
        if let Some(ref s) = *state {
            let _isr = reg_read_u8(s.bar0_virt, REG_ISR_STATUS);
        }
    }

    // Both levels stay suppressed — re_enable_irq() called from timer softirq.
}

/// Re-enable the network device's MSI-X interrupt after softirq processing.
///
/// Called by the NetRx softirq handler after draining the used ring.
/// Follows the Linux virtqueue_enable_cb() pattern:
/// 1. Read ISR to clear any pending device interrupt condition
/// 2. Re-enable device-level interrupts (clear NO_INTERRUPT flag)
/// 3. Memory barrier + check for new used ring entries
/// 4. If more work: re-suppress and let next softirq handle it
/// 5. If clean: clear GIC pending + enable SPI
pub fn re_enable_irq() {
    use crate::arch_impl::aarch64::gic;

    let irq = NET_PCI_IRQ.load(Ordering::Relaxed);
    if irq == 0 {
        return;
    }

    // Read ISR to clear any pending device interrupt condition before re-enabling.
    let state_ptr = &raw const NET_PCI_STATE;
    unsafe {
        if let Some(ref s) = *state_ptr {
            let _isr = reg_read_u8(s.bar0_virt, REG_ISR_STATUS);
        }
    }

    // Re-enable device-level interrupts (Linux: virtqueue_enable_cb)
    unsafe {
        let q = &raw mut PCI_RX_QUEUE;
        write_volatile(&mut (*q).avail.flags, 0);
        fence(Ordering::SeqCst);
    }

    // Check if more work arrived while we were processing (race window).
    // If so, re-suppress and let the next timer softirq cycle handle it.
    let has_more = unsafe {
        let q = &raw const PCI_RX_QUEUE;
        let used_idx = read_volatile(&(*q).used.idx);
        if let Some(ref s) = *state_ptr {
            used_idx != s.rx_last_used_idx
        } else {
            false
        }
    };

    if has_more {
        // More work arrived — re-suppress device interrupts, don't enable SPI.
        unsafe {
            let q = &raw mut PCI_RX_QUEUE;
            write_volatile(&mut (*q).avail.flags, VRING_AVAIL_F_NO_INTERRUPT);
            fence(Ordering::SeqCst);
        }
        return;
    }

    // Used ring is drained — safe to re-enable the GIC SPI.
    gic::clear_spi_pending(irq);
    gic::enable_spi(irq);
}

/// Diagnostic: dump RX queue state for debugging MSI-X issues.
pub fn dump_rx_state() {
    let state = unsafe {
        let ptr = &raw const NET_PCI_STATE;
        match (*ptr).as_ref() {
            Some(s) => s,
            None => return,
        }
    };

    let isr = reg_read_u8(state.bar0_virt, REG_ISR_STATUS);
    let (used_idx, avail_idx) = unsafe {
        let q = &raw const PCI_RX_QUEUE;
        (
            read_volatile(&(*q).used.idx),
            read_volatile(&(*q).avail.idx),
        )
    };
    let msi_count = NET_PCI_MSI_COUNT.load(Ordering::Relaxed);
    crate::serial_println!(
        "[virtio-net-pci] RX diag: used_idx={} last_used={} avail_idx={} isr={:#x} msi_count={}",
        used_idx,
        state.rx_last_used_idx,
        avail_idx,
        isr,
        msi_count
    );
}

/// Enable the MSI-X SPI at the GIC after init polling is complete.
///
/// During init, the ARP/ICMP polling loop processes RX via timer-based softirq.
/// The SPI must NOT be enabled during init because the GICv2m level-triggered
/// storm would prevent the main thread from making progress. After init drains
/// all used ring entries, it's safe to enable the SPI for interrupt-driven RX.
pub fn enable_msi_spi() {
    use crate::arch_impl::aarch64::gic;

    let irq = NET_PCI_IRQ.load(Ordering::Relaxed);
    if irq == 0 {
        return;
    }

    // Read ISR to clear any pending device interrupt from init polling
    let state_ptr = &raw const NET_PCI_STATE;
    unsafe {
        if let Some(ref s) = *state_ptr {
            let _isr = reg_read_u8(s.bar0_virt, REG_ISR_STATUS);
        }
    }

    gic::clear_spi_pending(irq);
    gic::enable_spi(irq);
    crate::serial_println!("[virtio-net-pci] MSI-X SPI {} enabled (post-init)", irq);
}

/// Whether the PCI net device is initialized
pub fn is_initialized() -> bool {
    DEVICE_INITIALIZED.load(Ordering::Acquire)
}
