//! VirtIO network device over the legacy PCI transport (x86_64).
//!
//! One receive queue (0) and one transmit queue (1), without mergeable receive
//! buffers, so every buffer begins with the 10-byte legacy `virtio_net_hdr`
//! (VirtIO 1.0 §5.1.6, legacy interface). Each buffer is one 4 KiB frame.
//!
//! The interrupt handler only reads the ISR, which acknowledges the device, and
//! raises NetRx. Receive, refill and transmit reclaim run in thread or softirq
//! context under the device lock with interrupts masked, so the handler never
//! finds the lock held by the code it interrupted.

use super::queue::Virtqueue;
use super::VirtioDevice;
use crate::memory::frame_allocator;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::PhysFrame;
use x86_64::PhysAddr;

/// The device has a MAC address in its configuration space.
const VIRTIO_NET_F_MAC: u32 = 1 << 5;
/// Legacy `virtio_net_hdr` without `num_buffers`.
const NET_HDR_LEN: usize = 10;
const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;
/// Receive buffers kept posted to the device.
const RX_BUFFERS: usize = 64;
/// Transmit buffers, each reused once the device has consumed it.
const TX_BUFFERS: usize = 32;
/// Bytes of each buffer offered to the device: header plus a full frame.
const BUFFER_LEN: u32 = 2048;
/// Largest Ethernet frame sent, without FCS.
const FRAME_MAX: usize = 1514;
/// Legacy ISR status register; reading it acknowledges the interrupt.
const ISR_STATUS: u16 = 0x13;
/// No buffer is posted on this descriptor.
const NO_BUFFER: u16 = u16::MAX;

struct Buffer {
    phys: u64,
    virt: *mut u8,
}

struct NetDevice {
    device: VirtioDevice,
    rx: Virtqueue,
    tx: Virtqueue,
    rx_buffers: Vec<Buffer>,
    /// The receive buffer posted on each descriptor.
    rx_by_desc: [u16; 256],
    tx_buffers: Vec<Buffer>,
    /// The transmit buffer in flight on each descriptor.
    tx_by_desc: [u16; 256],
    tx_free: Vec<u16>,
}

// SAFETY: the buffers are kernel frames reached through the direct map, used
// only under `DEVICE`'s lock.
unsafe impl Send for NetDevice {}

static DEVICE: Mutex<Option<NetDevice>> = Mutex::new(None);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static IO_BASE: AtomicU16 = AtomicU16::new(0);
static IRQ_LINE: AtomicU8 = AtomicU8::new(0);
static MAC: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];

fn with_device<R>(f: impl FnOnce(&mut NetDevice) -> R) -> Option<R> {
    x86_64::instructions::interrupts::without_interrupts(|| DEVICE.lock().as_mut().map(f))
}

impl Buffer {
    fn release(self) {
        frame_allocator::deallocate_frame(PhysFrame::containing_address(PhysAddr::new(self.phys)));
    }
}

fn alloc_buffer() -> Result<Buffer, &'static str> {
    let frame = frame_allocator::allocate_frame().ok_or("VirtIO net: no frame for a buffer")?;
    let phys = frame.start_address().as_u64();
    let virt = (crate::memory::physical_memory_offset().as_u64() + phys) as *mut u8;
    // SAFETY: the frame is exclusively owned and mapped by the direct map.
    unsafe { core::ptr::write_bytes(virt, 0, 4096) };
    Ok(Buffer { phys, virt })
}

/// Configure the legacy queue `index` and return it.
fn setup_queue(device: &VirtioDevice, index: u16) -> Result<Virtqueue, &'static str> {
    device.select_queue(index);
    let queue = Virtqueue::new(device.get_queue_size())?;
    device.set_queue_address(queue.phys_addr());
    if device.get_queue_address() != (queue.phys_addr() / 4096) as u32 {
        // Address 0 tells a legacy device the queue is not in use.
        device.set_queue_address(0);
        queue.release();
        return Err("VirtIO net: queue address was not set");
    }
    Ok(queue)
}

impl NetDevice {
    /// Post receive buffer `index` to the device.
    fn post_rx(&mut self, index: u16) -> Result<(), &'static str> {
        let phys = self.rx_buffers[index as usize].phys;
        let desc = self
            .rx
            .add_buf(phys, BUFFER_LEN, true)
            .ok_or("VirtIO net: receive queue full")?;
        self.rx_by_desc[desc as usize] = index;
        Ok(())
    }

    /// Post the receive buffers and set up the transmit buffers.
    fn fill_buffers(&mut self) -> Result<usize, &'static str> {
        let rx_count = RX_BUFFERS.min(self.rx.queue_size() as usize);
        for index in 0..rx_count {
            self.rx_buffers.push(alloc_buffer()?);
            self.post_rx(index as u16)?;
        }
        for index in 0..TX_BUFFERS.min(self.tx.queue_size() as usize) {
            self.tx_buffers.push(alloc_buffer()?);
            self.tx_free.push(index as u16);
        }
        Ok(rx_count)
    }

    /// Undo a failed `init`: reset the device, which stops it using the
    /// queues and buffers, then free them.
    fn release(self) {
        self.device.reset();
        self.rx.release();
        self.tx.release();
        for buffer in self.rx_buffers.into_iter().chain(self.tx_buffers) {
            buffer.release();
        }
    }

    /// Return every transmit buffer the device has finished with.
    fn reclaim_tx(&mut self) {
        while let Some((desc, _)) = self.tx.get_used() {
            self.tx.free_chain(desc);
            let buffer = core::mem::replace(&mut self.tx_by_desc[desc as usize], NO_BUFFER);
            if buffer != NO_BUFFER {
                self.tx_free.push(buffer);
            }
        }
    }
}

/// Find the legacy VirtIO network device and bring it up.
pub fn init() -> Result<(), &'static str> {
    let pci_dev = crate::drivers::pci::get_devices()
        .unwrap_or_default()
        .into_iter()
        .find(|dev| dev.is_virtio_net() && dev.device_id == crate::drivers::pci::VIRTIO_NET_DEVICE_ID_LEGACY)
        .ok_or("VirtIO net: no legacy device on the PCI bus")?;
    if !matches!(pci_dev.interrupt_line, 10 | 11) {
        return Err("VirtIO net: interrupt line is not 10 or 11");
    }
    let io_bar = pci_dev.get_io_bar().ok_or("VirtIO net: no I/O BAR")?;
    pci_dev.enable_bus_master();
    pci_dev.enable_io_space();

    let mut device = VirtioDevice::new(io_bar.address as u16);
    if device.read_device_features() & VIRTIO_NET_F_MAC == 0 {
        return Err("VirtIO net: device has no MAC address");
    }
    device.init(VIRTIO_NET_F_MAC).inspect_err(|_| device.reset())?;
    let mut mac = [0u8; 6];
    for (offset, byte) in mac.iter_mut().enumerate() {
        *byte = device.read_config_u8(offset as u16);
    }

    let rx = setup_queue(&device, RX_QUEUE).inspect_err(|_| device.reset())?;
    let tx = match setup_queue(&device, TX_QUEUE) {
        Ok(tx) => tx,
        Err(error) => {
            device.reset();
            rx.release();
            return Err(error);
        }
    };
    let mut net = NetDevice {
        device,
        rx,
        tx,
        rx_buffers: Vec::with_capacity(RX_BUFFERS),
        rx_by_desc: [NO_BUFFER; 256],
        tx_buffers: Vec::with_capacity(TX_BUFFERS),
        tx_by_desc: [NO_BUFFER; 256],
        tx_free: Vec::with_capacity(TX_BUFFERS),
    };
    let rx_count = match net.fill_buffers() {
        Ok(count) => count,
        Err(error) => {
            net.release();
            return Err(error);
        }
    };
    net.device.driver_ok();
    net.device.notify_queue(RX_QUEUE);

    for (slot, byte) in MAC.iter().zip(mac) {
        slot.store(byte, Ordering::Relaxed);
    }
    IO_BASE.store(io_bar.address as u16, Ordering::Relaxed);
    IRQ_LINE.store(pci_dev.interrupt_line, Ordering::Relaxed);
    x86_64::instructions::interrupts::without_interrupts(|| *DEVICE.lock() = Some(net));
    INITIALIZED.store(true, Ordering::Release);

    log::info!(
        "VirtIO net: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, IRQ {}, {} receive buffers",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        pci_dev.interrupt_line,
        rx_count
    );
    Ok(())
}

pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

/// The interrupt line the device was initialized on.
pub fn irq_line() -> Option<u8> {
    is_initialized().then(|| IRQ_LINE.load(Ordering::Relaxed))
}

pub fn mac_address() -> Option<[u8; 6]> {
    if !is_initialized() {
        return None;
    }
    let mut mac = [0u8; 6];
    for (byte, slot) in mac.iter_mut().zip(MAC.iter()) {
        *byte = slot.load(Ordering::Relaxed);
    }
    Some(mac)
}

/// Queue one Ethernet frame for transmission.
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    if data.len() > FRAME_MAX {
        return Err("Packet too large");
    }
    with_device(|net| {
        net.reclaim_tx();
        let buffer = net.tx_free.pop().ok_or("TX ring full")?;
        let target = &net.tx_buffers[buffer as usize];
        // SAFETY: the buffer is a 4 KiB frame the device is not using: it is
        // on the free list until it is posted below.
        unsafe {
            core::ptr::write_bytes(target.virt, 0, NET_HDR_LEN);
            core::ptr::copy_nonoverlapping(data.as_ptr(), target.virt.add(NET_HDR_LEN), data.len());
        }
        let Some(desc) = net
            .tx
            .add_buf(target.phys, (NET_HDR_LEN + data.len()) as u32, false)
        else {
            net.tx_free.push(buffer);
            return Err("TX ring full");
        };
        net.tx_by_desc[desc as usize] = buffer;
        net.device.notify_queue(TX_QUEUE);
        Ok(())
    })
    .unwrap_or(Err("VirtIO net not initialized"))
}

/// Copy the next received frame into `out` and give its buffer back to the
/// device. Returns the frame length, or `None` when no frame is waiting.
pub fn receive(out: &mut [u8]) -> Option<usize> {
    with_device(|net| {
        let (desc, written) = net.rx.get_used()?;
        net.rx.free_chain(desc);
        let buffer = core::mem::replace(&mut net.rx_by_desc[desc as usize], NO_BUFFER);
        if buffer == NO_BUFFER {
            return None;
        }
        let len = (written as usize)
            .saturating_sub(NET_HDR_LEN)
            .min(out.len());
        // SAFETY: the device has returned the buffer; it is not reposted until
        // the copy is done.
        unsafe {
            core::ptr::copy_nonoverlapping(
                net.rx_buffers[buffer as usize].virt.add(NET_HDR_LEN),
                out.as_mut_ptr(),
                len,
            );
        }
        if net.post_rx(buffer).is_ok() {
            net.device.notify_queue(RX_QUEUE);
        }
        Some(len)
    })
    .flatten()
}

/// Interrupt on line `irq`: acknowledge the device and raise NetRx if it has
/// returned buffers.
///
/// CRITICAL: hard-IRQ context. One port read, no lock, no logging.
pub fn handle_interrupt(irq: u8) {
    if !is_initialized() || IRQ_LINE.load(Ordering::Relaxed) != irq {
        return;
    }
    let mut isr = Port::<u8>::new(IO_BASE.load(Ordering::Relaxed) + ISR_STATUS);
    // SAFETY: a read of the device's own ISR register.
    if unsafe { isr.read() } & 1 != 0 {
        crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
    }
}
