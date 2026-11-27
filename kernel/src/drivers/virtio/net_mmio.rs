//! VirtIO Network Device Driver for ARM64 (MMIO Transport)
//!
//! Implements a network device driver using VirtIO MMIO transport, with its
//! queues and buffers in static memory.
//!
//! Each queue has its own lock: `TX` holds the transmit queue's bookkeeping
//! (`net_tx_queue::TxQueue`) and `RX` the receive queue's, and the driver
//! reaches a queue's memory under that queue's lock. The device's addresses are written
//! once, at the end of `init`, into `DEVICE`.

use super::mmio::{
    device_id, VirtioMmioDevice, VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_SIZE,
};
use super::net_tx_queue::{TxError, TxQueue, TxRing};
use crate::irq_safe_mutex::IrqSafeMutex;
use core::cell::UnsafeCell;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
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
    pub const MAC: u64 = 1 << 5; // Device has given MAC address
    #[allow(dead_code)]
    pub const STATUS: u64 = 1 << 16; // Link status available
    #[allow(dead_code)]
    pub const MRG_RXBUF: u64 = 1 << 15; // Merge receive buffers
}

/// Maximum packet size (MTU + headers)
pub const MAX_PACKET_SIZE: usize = 1514;

/// VirtIO network header - prepended to each packet
/// For legacy mode without MRG_RXBUF, this is 10 bytes (no num_buffers field)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    // Note: num_buffers is NOT included for legacy mode without MRG_RXBUF
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
#[derive(Clone, Copy)]
struct TxBuffer {
    hdr: VirtioNetHdr,
    data: [u8; MAX_PACKET_SIZE],
}

const TX_POOL_SIZE: usize = 16;
const EMPTY_TX_BUFFER: TxBuffer = TxBuffer {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: gso_type::NONE,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    data: [0; MAX_PACKET_SIZE],
};

const EMPTY_RX_BUFFER: RxBuffer = RxBuffer {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: gso_type::NONE,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    data: [0; MAX_PACKET_SIZE],
};

/// Entries in each static ring.
const RING_ENTRIES: u16 = 16;
/// Receive buffers posted to the device.
const RX_BUFFERS_POSTED: usize = 4;
const _: () = assert!(TX_POOL_SIZE == RING_ENTRIES as usize);

/// Memory the device reaches by DMA. The driver touches it only through raw
/// pointers: a queue's ring and buffers under that queue's lock (`TX` or
/// `RX`), and both rings during `init`, before `DEVICE` is published.
#[repr(transparent)]
struct Dma<T>(UnsafeCell<T>);

// SAFETY: accesses go through `get` under the owning queue's lock, as
// described on the type; `Dma` hands out raw pointers only.
unsafe impl<T> Sync for Dma<T> {}

impl<T> Dma<T> {
    const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    fn get(&self) -> *mut T {
        self.0.get()
    }
}

const EMPTY_QUEUE_DESC: [VirtqDesc; 16] = [VirtqDesc {
    addr: 0,
    len: 0,
    flags: 0,
    next: 0,
}; 16];

static RX_QUEUE: Dma<RxQueueMemory> = Dma::new(RxQueueMemory {
    desc: EMPTY_QUEUE_DESC,
    avail: VirtqAvail {
        flags: 0,
        idx: 0,
        ring: [0; 16],
    },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed {
        flags: 0,
        idx: 0,
        ring: [VirtqUsedElem { id: 0, len: 0 }; 16],
    },
});

static TX_QUEUE: Dma<TxQueueMemory> = Dma::new(TxQueueMemory {
    desc: EMPTY_QUEUE_DESC,
    avail: VirtqAvail {
        flags: 0,
        idx: 0,
        ring: [0; 16],
    },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed {
        flags: 0,
        idx: 0,
        ring: [VirtqUsedElem { id: 0, len: 0 }; 16],
    },
});

static RX_BUFFERS: Dma<[RxBuffer; RX_BUFFERS_POSTED]> = Dma::new([EMPTY_RX_BUFFER; RX_BUFFERS_POSTED]);
static TX_BUFFERS: Dma<[TxBuffer; TX_POOL_SIZE]> = Dma::new([EMPTY_TX_BUFFER; TX_POOL_SIZE]);

/// The device, written once when `init` has finished.
struct DeviceInfo {
    /// Physical base of the MMIO window.
    base: u64,
    /// Its HHDM address, for register access without a probe.
    base_virt: u64,
    /// The MMIO slot, which fixes the interrupt.
    slot: usize,
    mac: [u8; 6],
}

static DEVICE: spin::Once<DeviceInfo> = spin::Once::new();

/// The transmit queue: descriptor ownership, the available index and the used
/// cursor, changed under this lock (virtio 1.2 section 2.7.13).
static TX: IrqSafeMutex<TxQueue<MmioTxRing>> = IrqSafeMutex::new(TxQueue::new(MmioTxRing));

/// The receive queue's driver state.
struct RxState {
    /// Descriptors in the queue, a power of two; 0 until configured.
    size: u16,
    last_used: u16,
    /// Completed descriptors held until the stack finishes borrowing their frames.
    recycle: u8,
}

static RX: IrqSafeMutex<RxState> = IrqSafeMutex::new(RxState {
    size: 0,
    last_used: 0,
    recycle: 0,
});

/// Base IRQ for VirtIO MMIO devices on QEMU virt machine
/// IRQ = VIRTIO_IRQ_BASE + slot_number
const VIRTIO_IRQ_BASE: u32 = 48;

/// VirtIO MMIO register offsets used without a probe.
const REG_QUEUE_NOTIFY: u64 = 0x050;
const REG_INTERRUPT_STATUS: u64 = 0x060;
const REG_INTERRUPT_ACK: u64 = 0x064;

/// The transmit queue's memory, `TX_QUEUE`, as the device sees it. Each method
/// is called only by `TxQueue` under the `TX` lock.
struct MmioTxRing;

impl TxRing for MmioTxRing {
    fn write_desc(&mut self, slot: u16, addr: u64, len: u32) {
        let desc = VirtqDesc {
            addr,
            len,
            flags: 0, // Device reads this
            next: 0,
        };
        // SAFETY: `slot` is below the configured size, at most RING_ENTRIES.
        unsafe { write_volatile(addr_of_mut!((*TX_QUEUE.get()).desc[slot as usize]), desc) }
    }

    fn write_avail_entry(&mut self, pos: u16, slot: u16) {
        unsafe { write_volatile(addr_of_mut!((*TX_QUEUE.get()).avail.ring[pos as usize]), slot) }
    }

    fn barrier_before_avail_idx(&mut self) {
        // The device is an observer outside the inner-shareable domain.
        unsafe { core::arch::asm!("dmb oshst", options(nostack, preserves_flags)) }
    }

    fn write_avail_idx(&mut self, idx: u16) {
        unsafe { write_volatile(addr_of_mut!((*TX_QUEUE.get()).avail.idx), idx) }
    }

    fn barrier_before_notify(&mut self) {
        // The QueueNotify write is a device-memory store: complete the index
        // store before it can be issued.
        unsafe { core::arch::asm!("dsb st", options(nostack, preserves_flags)) }
    }

    fn read_used_idx(&mut self) -> u16 {
        let idx = unsafe { read_volatile(addr_of!((*TX_QUEUE.get()).used.idx)) };
        unsafe { core::arch::asm!("dmb oshld", options(nostack, preserves_flags)) }
        idx
    }

    fn read_used_id(&mut self, pos: u16) -> u32 {
        unsafe { read_volatile(addr_of!((*TX_QUEUE.get()).used.ring[pos as usize].id)) }
    }
}

/// Write QueueNotify for `queue` through the HHDM mapping.
fn notify(base_virt: u64, queue: u32) {
    unsafe {
        crate::arch_impl::aarch64::mmio::write32((base_virt + REG_QUEUE_NOTIFY) as *mut u32, queue);
    }
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
                crate::serial_println!(
                    "[virtio-net] Found network device at {:#x} (slot {})",
                    base,
                    i
                );
                let mac = init_device(&mut device)?;

                // Note: IRQ will be enabled later by enable_net_irq() after
                // the network stack is fully initialized.
                let irq = VIRTIO_IRQ_BASE + i as u32;
                crate::serial_println!(
                    "[virtio-net] Network device IRQ {} (will enable after net init)",
                    irq
                );

                // Publish the device for the transmit, receive and interrupt paths.
                DEVICE.call_once(|| DeviceInfo {
                    base,
                    base_virt: crate::memory::physical_memory_offset().as_u64() + base,
                    slot: i,
                    mac,
                });

                return Ok(());
            }
        }
    }

    Err("No VirtIO network device found")
}

/// Bring the device up and return its MAC address.
fn init_device(device: &mut VirtioMmioDevice) -> Result<[u8; 6], &'static str> {
    let version = device.version();
    crate::serial_println!("[virtio-net] Device version: {}", version);

    // For v1 (legacy), we must set guest page size BEFORE init
    if version == 1 {
        device.set_guest_page_size(4096);
    }

    // Initialize the device (reset, ack, driver, features)
    // Request MAC feature and STATUS for link awareness
    device.init(features::MAC | features::STATUS)?;

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
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );

    // Set up RX queue (queue 0)
    setup_rx_queue(device, version)?;

    // Set up TX queue (queue 1)
    setup_tx_queue(device, version)?;

    // Mark device as ready
    device.driver_ok();

    // Post initial RX buffers
    post_rx_buffers(device);

    crate::serial_println!("[virtio-net] Network device initialized successfully");
    Ok(mac)
}

/// Hand the selected queue's ring addresses, from the static queue at
/// `queue_phys`, to the device.
fn place_queue(device: &mut VirtioMmioDevice, version: u32, queue_phys: u64) {
    if version == 1 {
        device.set_queue_align(4096);
        device.set_queue_pfn((queue_phys / 4096) as u32);
    } else {
        device.set_queue_desc(queue_phys);
        device.set_queue_avail(queue_phys + 256);
        device.set_queue_used(queue_phys + 4096);
        device.set_queue_ready(true);
    }
}

fn device_queue_max(device: &VirtioMmioDevice) -> u16 {
    device.get_queue_num_max().min(u32::from(u16::MAX)) as u16
}

fn setup_rx_queue(device: &mut VirtioMmioDevice, version: u32) -> Result<(), &'static str> {
    device.select_queue(0); // RX queue
    let queue_num_max = device_queue_max(device);
    crate::serial_println!("[virtio-net] RX queue max size: {}", queue_num_max);

    let queue_size = super::net_tx_queue::split_queue_size(queue_num_max, RING_ENTRIES);
    if usize::from(queue_size) < RX_BUFFERS_POSTED {
        return Err("RX queue too small");
    }
    device.set_queue_num(u32::from(queue_size));

    let mut rx = RX.lock();
    // SAFETY: the device has not been given this queue yet.
    unsafe {
        let queue_ptr = RX_QUEUE.get();
        for i in 0..15 {
            (*queue_ptr).desc[i].next = (i + 1) as u16;
        }
        (*queue_ptr).desc[15].next = 0;
        (*queue_ptr).avail.flags = 0;
        (*queue_ptr).avail.idx = 0;
        (*queue_ptr).used.flags = 0;
        (*queue_ptr).used.idx = 0;
    }
    *rx = RxState {
        size: queue_size,
        last_used: 0,
        recycle: 0,
    };
    drop(rx);

    place_queue(device, version, virt_to_phys(RX_QUEUE.get() as u64));
    Ok(())
}

fn setup_tx_queue(device: &mut VirtioMmioDevice, version: u32) -> Result<(), &'static str> {
    device.select_queue(1); // TX queue
    let queue_num_max = device_queue_max(device);
    crate::serial_println!("[virtio-net] TX queue max size: {}", queue_num_max);

    let mut tx = TX.lock();
    // SAFETY: the device has not been given this queue yet.
    unsafe {
        let queue_ptr = TX_QUEUE.get();
        for i in 0..15 {
            (*queue_ptr).desc[i].next = (i + 1) as u16;
        }
        (*queue_ptr).desc[15].next = 0;
        (*queue_ptr).avail.flags = 0;
        (*queue_ptr).avail.idx = 0;
        (*queue_ptr).used.flags = 0;
        (*queue_ptr).used.idx = 0;
    }
    tx.configure(queue_num_max, RING_ENTRIES);
    let queue_size = tx.size();
    drop(tx);
    if queue_size == 0 {
        return Err("TX queue size is 0");
    }
    device.set_queue_num(u32::from(queue_size));

    place_queue(device, version, virt_to_phys(TX_QUEUE.get() as u64));
    Ok(())
}

/// The RX buffer behind descriptor `idx`, if it is one of the posted ones.
fn rx_buffer(idx: usize) -> Option<*mut RxBuffer> {
    if idx < RX_BUFFERS_POSTED {
        // SAFETY: in bounds of the static array.
        Some(unsafe { RX_BUFFERS.get().cast::<RxBuffer>().add(idx) })
    } else {
        None
    }
}

/// Offer descriptor `desc_idx` again at the next available position. Called
/// under the `RX` lock; the caller notifies the device.
fn repost_rx_buffer(rx: &mut RxState, desc_idx: usize) {
    if desc_idx >= RX_BUFFERS_POSTED {
        return;
    }

    unsafe {
        let queue_ptr = RX_QUEUE.get();
        let avail_idx = (*queue_ptr).avail.idx;
        (*queue_ptr).avail.ring[(avail_idx % rx.size) as usize] = desc_idx as u16;
        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }
}

/// Post RX buffers to the device for receiving packets
fn post_rx_buffers(device: &VirtioMmioDevice) {
    let rx = RX.lock();
    unsafe {
        let queue_ptr = RX_QUEUE.get();

        for i in 0..RX_BUFFERS_POSTED {
            let buf_phys = rx_buffer(i).map_or(0, |buf| virt_to_phys(buf as u64));
            let buf_len = (core::mem::size_of::<VirtioNetHdr>() + MAX_PACKET_SIZE) as u32;

            // Single descriptor for entire buffer (header + data)
            (*queue_ptr).desc[i] = VirtqDesc {
                addr: buf_phys,
                len: buf_len,
                flags: DESC_F_WRITE, // Device writes to this
                next: 0,
            };

            // Add to available ring
            (*queue_ptr).avail.ring[i] = i as u16;
        }

        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = RX_BUFFERS_POSTED as u16;
        fence(Ordering::SeqCst);
    }
    drop(rx);

    // Notify device about RX queue (queue 0)
    device.notify_queue(0);
}

/// Transmit a packet. The descriptor claim, its available-ring entry and the
/// `avail.idx` store happen under the `TX` lock, after taking back whatever the
/// device has finished sending.
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    if data.len() > MAX_PACKET_SIZE {
        return Err("Packet too large");
    }
    let device = DEVICE.get().ok_or("Network device not initialized")?;

    let submitted = TX.lock().submit(|slot| {
        // SAFETY: `slot` is below the queue size, at most TX_POOL_SIZE, and its
        // buffer is ours until the device returns the descriptor.
        let tx_ptr = unsafe { TX_BUFFERS.get().cast::<TxBuffer>().add(usize::from(slot)) };
        unsafe {
            (*tx_ptr).hdr = VirtioNetHdr::default();
            core::ptr::copy_nonoverlapping(data.as_ptr(), (*tx_ptr).data.as_mut_ptr(), data.len());
        }
        let total_len = core::mem::size_of::<VirtioNetHdr>() + data.len();
        (virt_to_phys(tx_ptr as u64), total_len as u32)
    });
    match submitted {
        Ok(_) => {}
        Err(TxError::Full) => return Err("TX queue full"),
        Err(TxError::NotReady) => return Err("Network device not initialized"),
    }

    // Notify device about TX queue (queue 1)
    notify(device.base_virt, 1);
    Ok(())
}

/// Reclaim completed TX descriptors and free their buffer slots.
pub fn reclaim_tx_completed() -> usize {
    TX.lock().reclaim()
}

/// Check for received packets (non-blocking)
/// Returns the packet data if one is available
pub fn receive() -> Option<&'static [u8]> {
    let info = DEVICE.get()?;

    // Check and clear interrupt status (VirtIO requires this)
    let device = VirtioMmioDevice::probe(info.base)?;
    let int_status = device.read_interrupt_status();
    if int_status != 0 {
        device.ack_interrupt(int_status);
    }

    // ARM64 requires DSB to ensure device writes are visible
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    fence(Ordering::SeqCst);

    let mut rx = RX.lock();
    let used_idx = unsafe { read_volatile(addr_of!((*RX_QUEUE.get()).used.idx)) };

    if used_idx == rx.last_used {
        return None; // No new packets
    }

    // Get the used element
    let elem = unsafe {
        read_volatile(addr_of!((*RX_QUEUE.get()).used.ring[(rx.last_used % rx.size) as usize]))
    };
    let (desc_idx, packet_len) = (elem.id as usize, elem.len as usize);

    rx.last_used = rx.last_used.wrapping_add(1);

    // Get packet data (skip header)
    let hdr_size = core::mem::size_of::<VirtioNetHdr>();
    if packet_len <= hdr_size {
        repost_rx_buffer(&mut rx, desc_idx);
        drop(rx);
        device.notify_queue(0);
        return None;
    }

    let buf = rx_buffer(desc_idx)?;
    let data_len = (packet_len - hdr_size).min(MAX_PACKET_SIZE);
    rx.recycle |= 1 << desc_idx;
    // SAFETY: the descriptor is held in `recycle`, so the device cannot write
    // this buffer again until `recycle_rx_buffers` offers it back.
    let data: &'static [u8; MAX_PACKET_SIZE] = unsafe { &(*buf).data };
    Some(&data[..data_len])
}

/// Re-post all consumed RX buffers back to the device.
///
/// Call this after processing a batch of received packets to make the
/// buffers available for new incoming packets.
pub fn recycle_rx_buffers() {
    let Some(info) = DEVICE.get() else {
        return;
    };

    // VirtIO 1.2 section 2.7: only a used descriptor has returned to the
    // driver. Reposting device-owned descriptors exposes the same DMA buffer
    // twice, allowing another frame to overwrite one still being consumed.
    let mut rx = RX.lock();
    let recycled = core::mem::take(&mut rx.recycle);
    if recycled == 0 {
        return;
    }
    unsafe {
        let queue_ptr = RX_QUEUE.get();
        let mut avail_idx = (*queue_ptr).avail.idx;

        for i in 0..RX_BUFFERS_POSTED as u16 {
            if recycled & (1 << i) != 0 {
                (*queue_ptr).avail.ring[(avail_idx % rx.size) as usize] = i;
                avail_idx = avail_idx.wrapping_add(1);
            }
        }

        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = avail_idx;
        fence(Ordering::SeqCst);
    }
    drop(rx);

    notify(info.base_virt, 0);
}

/// Get the MAC address
pub fn mac_address() -> Option<[u8; 6]> {
    DEVICE.get().map(|d| d.mac)
}

/// Enable the network device's interrupt in the GIC.
///
/// Called after the network stack is fully initialized (ARP resolved, etc.)
/// to avoid interrupt storms during the synchronous init polling loop.
pub fn enable_net_irq() {
    let Some(irq) = get_irq() else {
        return;
    };
    crate::serial_println!("[virtio-net] Enabling IRQ {} for network device", irq);
    use crate::arch_impl::aarch64::gic;
    use crate::arch_impl::traits::InterruptController;
    gic::Gicv2::enable_irq(irq as u8);
}

/// Get the IRQ number for the network device (if initialized)
pub fn get_irq() -> Option<u32> {
    DEVICE.get().map(|d| VIRTIO_IRQ_BASE + d.slot as u32)
}

/// Interrupt handler for VirtIO network device
///
/// Called from the GIC interrupt dispatcher when the network device generates
/// an interrupt. Acknowledges the device interrupt and raises the NetRx softirq
/// for deferred packet processing (same pattern as x86_64 E1000 driver).
///
/// Uses direct volatile MMIO access (no probe/lock) — safe in interrupt context.
pub fn handle_interrupt() {
    let Some(info) = DEVICE.get() else {
        return;
    };

    // Acknowledge the interrupt from the device using direct MMIO access
    unsafe {
        let status_addr = (info.base_virt + REG_INTERRUPT_STATUS) as *const u32;
        let status = crate::arch_impl::aarch64::mmio::read32(status_addr);
        if status != 0 {
            let ack_addr = (info.base_virt + REG_INTERRUPT_ACK) as *mut u32;
            crate::arch_impl::aarch64::mmio::write32(ack_addr, status);
        }
    }

    // Raise NetRx softirq to process received packets in softirq context
    // This defers packet processing to avoid doing heavy work in interrupt context
    crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
}

/// Test the network device
pub fn test_device() -> Result<(), &'static str> {
    let mac = mac_address().ok_or("Network device not initialized")?;
    crate::serial_println!(
        "[virtio-net] Device test - MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
    crate::serial_println!("[virtio-net] Test passed!");
    Ok(())
}
