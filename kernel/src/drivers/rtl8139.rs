//! Realtek RTL8139 Fast Ethernet controller (x86_64, port I/O).
//!
//! Receive uses the controller's single ring: an 8 KiB buffer plus 16 bytes,
//! with WRAP set so a frame that runs past the end is written on past it
//! instead of wrapping, which the 1500 bytes of slack after the ring hold.
//! Each frame in the ring is preceded by a 4-byte header (status, then length
//! including the 4-byte FCS) and the next frame starts on a 4-byte boundary.
//! Transmit uses the four TSAD/TSD descriptor pairs in turn; a slot is reused
//! once the controller has set its OWN bit, which it does when it has copied
//! the frame out of memory.
//!
//! The interrupt handler only reads and acknowledges ISR and raises NetRx.
//! Receive and transmit run under the device lock with interrupts masked, so
//! the handler never finds the lock held by the code it interrupted.

use crate::memory::frame_allocator;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

pub const VENDOR_ID: u16 = 0x10ec;
pub const DEVICE_ID: u16 = 0x8139;

// Register offsets from the I/O base.
const IDR0: u16 = 0x00;
const TSD0: u16 = 0x10;
const TSAD0: u16 = 0x20;
const RBSTART: u16 = 0x30;
const CR: u16 = 0x37;
const CAPR: u16 = 0x38;
const CBR: u16 = 0x3a;
const IMR: u16 = 0x3c;
const ISR: u16 = 0x3e;
const RCR: u16 = 0x44;
const CONFIG1: u16 = 0x52;

// Command register.
const CR_RST: u8 = 0x10;
const CR_RE: u8 = 0x08;
const CR_TE: u8 = 0x04;
const CR_BUFE: u8 = 0x01;

// Interrupt status/mask bits.
const INT_ROK: u16 = 0x0001;
const INT_RER: u16 = 0x0002;
const INT_TOK: u16 = 0x0004;
const INT_TER: u16 = 0x0008;
const INT_RXOVW: u16 = 0x0010;
const INT_FOVW: u16 = 0x0040;
const INT_RX: u16 = INT_ROK | INT_RER | INT_RXOVW | INT_FOVW;

// Receive configuration: accept broadcast, multicast and frames to our
// address; WRAP; unlimited DMA burst; 8 KiB ring; no early-receive threshold.
const RCR_APM: u32 = 1 << 1;
const RCR_AM: u32 = 1 << 2;
const RCR_AB: u32 = 1 << 3;
const RCR_WRAP: u32 = 1 << 7;
const RCR_MXDMA_UNLIMITED: u32 = 7 << 8;
const RCR_RXFTH_NONE: u32 = 7 << 13;

// Transmit status: the controller has copied the frame out (OWN).
const TSD_OWN: u32 = 1 << 13;

/// Receive frame header status: received OK.
const RX_ROK: u16 = 0x0001;
const RX_RING_LEN: usize = 8192;
/// Ring, its 16-byte tail, and room for a frame written on past the end.
const RX_BUFFER_FRAMES: usize = 3;
const TX_SLOTS: usize = 4;
const FRAME_MIN: usize = 60;
const FRAME_MAX: usize = 1514;
/// Largest length a receive header may carry: a frame plus its FCS.
const RX_LEN_MAX: usize = FRAME_MAX + 4;

struct Rtl8139 {
    io_base: u16,
    rx_virt: *const u8,
    rx_offset: usize,
    tx_phys: [u64; TX_SLOTS],
    tx_virt: [*mut u8; TX_SLOTS],
    tx_next: usize,
}

// SAFETY: the buffers are kernel frames reached through the direct map, used
// only under `DEVICE`'s lock.
unsafe impl Send for Rtl8139 {}

static DEVICE: Mutex<Option<Rtl8139>> = Mutex::new(None);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static IO_BASE: AtomicU16 = AtomicU16::new(0);
static IRQ_LINE: AtomicU8 = AtomicU8::new(0);
static MAC: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];

// Port accessors without the `nomem` option the `x86_64` crate's `Port` uses,
// so the compiler orders them with the loads and stores of the receive ring
// and transmit buffers around them.
fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: a register of the controller this driver owns.
    unsafe { core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nostack, preserves_flags)) };
    value
}
fn outb(port: u16, value: u8) {
    // SAFETY: as `inb`.
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nostack, preserves_flags)) };
}
fn inw(port: u16) -> u16 {
    let value: u16;
    // SAFETY: as `inb`.
    unsafe { core::arch::asm!("in ax, dx", out("ax") value, in("dx") port, options(nostack, preserves_flags)) };
    value
}
fn outw(port: u16, value: u16) {
    // SAFETY: as `inb`.
    unsafe { core::arch::asm!("out dx, ax", in("dx") port, in("ax") value, options(nostack, preserves_flags)) };
}
fn inl(port: u16) -> u32 {
    let value: u32;
    // SAFETY: as `inb`.
    unsafe { core::arch::asm!("in eax, dx", out("eax") value, in("dx") port, options(nostack, preserves_flags)) };
    value
}
fn outl(port: u16, value: u32) {
    // SAFETY: as `inb`.
    unsafe { core::arch::asm!("out dx, eax", in("dx") port, in("eax") value, options(nostack, preserves_flags)) };
}

fn with_device<R>(f: impl FnOnce(&mut Rtl8139) -> R) -> Option<R> {
    x86_64::instructions::interrupts::without_interrupts(|| DEVICE.lock().as_mut().map(f))
}

fn direct_map(phys: u64) -> *mut u8 {
    (crate::memory::physical_memory_offset().as_u64() + phys) as *mut u8
}

/// Find the RTL8139 and bring it up.
pub fn init() -> Result<(), &'static str> {
    let pci_dev = crate::drivers::pci::find_device(VENDOR_ID, DEVICE_ID)
        .ok_or("RTL8139: no device on the PCI bus")?;
    if !matches!(pci_dev.interrupt_line, 10 | 11) {
        return Err("RTL8139: interrupt line is not 10 or 11");
    }
    let io_base = pci_dev.get_io_bar().ok_or("RTL8139: no I/O BAR")?.address as u16;
    pci_dev.enable_io_space();

    // Power on, then software reset; the controller clears RST when done.
    outb(io_base + CONFIG1, 0);
    outb(io_base + CR, CR_RST);
    let mut reset_done = false;
    for _ in 0..100_000 {
        if inb(io_base + CR) & CR_RST == 0 {
            reset_done = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !reset_done {
        return Err("RTL8139: reset did not complete");
    }

    let mut mac = [0u8; 6];
    for (offset, byte) in mac.iter_mut().enumerate() {
        *byte = inb(io_base + IDR0 + offset as u16);
    }

    // Every buffer is allocated before the controller is told about any of
    // them, so a failure here frees memory the controller has never seen.
    let mut rx_frames = [None; RX_BUFFER_FRAMES];
    let rx_base = frame_allocator::allocate_contiguous_frames(RX_BUFFER_FRAMES, &mut rx_frames)
        .ok_or("RTL8139: no contiguous receive buffer")?;
    let mut tx_frames: [Option<PhysFrame>; TX_SLOTS] = [None; TX_SLOTS];
    let mut failure = None;
    if rx_base.start_address().as_u64() + (RX_BUFFER_FRAMES * 4096) as u64 > u32::MAX as u64 {
        failure = Some("RTL8139: receive buffer above 4 GiB");
    }
    for slot in tx_frames.iter_mut() {
        if failure.is_some() {
            break;
        }
        *slot = frame_allocator::allocate_frame();
        failure = match *slot {
            None => Some("RTL8139: no transmit buffer"),
            Some(frame) if frame.start_address().as_u64() + 4096 > u32::MAX as u64 => {
                Some("RTL8139: transmit buffer above 4 GiB")
            }
            Some(_) => None,
        };
    }
    if let Some(error) = failure {
        for frame in rx_frames.into_iter().chain(tx_frames).flatten() {
            frame_allocator::deallocate_frame(frame);
        }
        return Err(error);
    }

    let rx_phys = rx_base.start_address().as_u64();
    let rx_virt = direct_map(rx_phys);
    // SAFETY: the run is exclusively owned and mapped by the direct map.
    unsafe { core::ptr::write_bytes(rx_virt, 0, RX_BUFFER_FRAMES * 4096) };
    let mut tx_phys = [0u64; TX_SLOTS];
    let mut tx_virt = [core::ptr::null_mut(); TX_SLOTS];
    for (slot, frame) in tx_frames.into_iter().flatten().enumerate() {
        tx_phys[slot] = frame.start_address().as_u64();
        tx_virt[slot] = direct_map(tx_phys[slot]);
        outl(io_base + TSAD0 + 4 * slot as u16, tx_phys[slot] as u32);
    }

    pci_dev.enable_bus_master();
    outl(io_base + RBSTART, rx_phys as u32);
    outw(io_base + IMR, INT_RX | INT_TOK | INT_TER);
    outw(io_base + ISR, 0xffff);
    outl(
        io_base + RCR,
        RCR_APM | RCR_AM | RCR_AB | RCR_WRAP | RCR_MXDMA_UNLIMITED | RCR_RXFTH_NONE,
    );
    outb(io_base + CR, CR_RE | CR_TE);

    for (slot, byte) in MAC.iter().zip(mac) {
        slot.store(byte, Ordering::Relaxed);
    }
    IO_BASE.store(io_base, Ordering::Relaxed);
    IRQ_LINE.store(pci_dev.interrupt_line, Ordering::Relaxed);
    x86_64::instructions::interrupts::without_interrupts(|| {
        *DEVICE.lock() = Some(Rtl8139 {
            io_base,
            rx_virt,
            rx_offset: 0,
            tx_phys,
            tx_virt,
            tx_next: 0,
        })
    });
    INITIALIZED.store(true, Ordering::Release);

    log::info!(
        "RTL8139: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, IRQ {}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        pci_dev.interrupt_line
    );
    Ok(())
}

pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

/// The interrupt line the controller was initialized on.
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

/// Send one Ethernet frame, padded to the 60-byte minimum.
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    if data.len() > FRAME_MAX {
        return Err("Packet too large");
    }
    with_device(|nic| {
        let slot = nic.tx_next;
        let tsd = nic.io_base + TSD0 + 4 * slot as u16;
        // A slot whose previous frame the controller has not copied out yet
        // is still in use; it normally finishes within a few microseconds.
        let mut free = false;
        for _ in 0..100_000 {
            if inl(tsd) & TSD_OWN != 0 {
                free = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !free {
            return Err("TX ring full");
        }
        let len = data.len().max(FRAME_MIN);
        // SAFETY: the slot's frame is not being read by the controller (OWN
        // is set) and holds 4 KiB.
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), nic.tx_virt[slot], data.len());
            core::ptr::write_bytes(nic.tx_virt[slot].add(data.len()), 0, len - data.len());
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        outl(nic.io_base + TSAD0 + 4 * slot as u16, nic.tx_phys[slot] as u32);
        // Writing the size with OWN clear starts the transmission.
        outl(tsd, len as u32);
        nic.tx_next = (slot + 1) % TX_SLOTS;
        Ok(())
    })
    .unwrap_or(Err("RTL8139 not initialized"))
}

/// Copy the next received frame, without its FCS, into `out`. Returns the
/// frame length, or `None` when the ring is empty.
pub fn receive(out: &mut [u8]) -> Option<usize> {
    with_device(|nic| {
        if inb(nic.io_base + CR) & CR_BUFE != 0 {
            return None;
        }
        // The controller wrote the frame before it cleared BUFE: read the ring
        // only after the check.
        core::sync::atomic::fence(Ordering::SeqCst);
        let offset = nic.rx_offset;
        // SAFETY: `offset` is below the 8 KiB ring and the 4-byte header lies
        // within its 16-byte tail at worst.
        let header: [u8; 4] = core::array::from_fn(|byte| unsafe {
            core::ptr::read_volatile(nic.rx_virt.add(offset + byte))
        });
        let status = u16::from_le_bytes([header[0], header[1]]);
        let len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if status & RX_ROK == 0 || !(4..=RX_LEN_MAX).contains(&len) {
            // A bad header leaves no way to find the next frame: skip to the
            // controller's write pointer, dropping whatever is unread.
            let write = inw(nic.io_base + CBR) as usize % RX_RING_LEN;
            nic.rx_offset = write;
            outw(nic.io_base + CAPR, (write as u16).wrapping_sub(16));
            return None;
        }
        let next = ((offset + 4 + len + 3) & !3) % RX_RING_LEN;
        let frame_len = (len - 4).min(out.len());
        // SAFETY: with WRAP set the frame is contiguous from `offset + 4`, and
        // `offset + 4 + RX_LEN_MAX` stays inside the three-frame run.
        unsafe {
            core::ptr::copy_nonoverlapping(nic.rx_virt.add(offset + 4), out.as_mut_ptr(), frame_len);
        }
        nic.rx_offset = next;
        // The copy completes before CAPR hands the space back to the controller.
        core::sync::atomic::fence(Ordering::SeqCst);
        // CAPR trails the read pointer by 16 bytes (RTL8139 datasheet).
        outw(nic.io_base + CAPR, (next as u16).wrapping_sub(16));
        Some(frame_len)
    })
    .flatten()
}

/// Interrupt on line `irq`: acknowledge the controller and raise NetRx when it
/// has received.
///
/// CRITICAL: hard-IRQ context. Two port accesses, no lock, no logging.
pub fn handle_interrupt(irq: u8) {
    if !is_initialized() || IRQ_LINE.load(Ordering::Relaxed) != irq {
        return;
    }
    let isr = IO_BASE.load(Ordering::Relaxed) + ISR;
    let status = inw(isr);
    if status == 0 {
        return;
    }
    // Write-one-to-clear: acknowledge exactly what was read.
    outw(isr, status);
    if status & INT_RX != 0 {
        crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
    }
}
