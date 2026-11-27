//! Intel 82540EM (e1000) and 82574 (e1000e) Gigabit Ethernet.
//!
//! Clean room: written only from Intel's "PCI/PCI-X Family of Gigabit Ethernet
//! Controllers Software Developer's Manual" (317453006, revision 4.0), the
//! "Intel 82574 GbE Controller Family Datasheet" (317694, revision 3.3) and
//! the PCI Local Bus 3.0 specification. No other driver's source was read.
//! `nic.rs` holds the MAC programming with its section references; this file
//! attaches it to the kernel: PCI discovery, the register window, DMA memory,
//! locking and the interrupt entry.
//!
//! The functions this binds: the 82540EM (8086:100E), QEMU's default x86-64
//! NIC, and the 82574L (8086:10D3), which VMware Fusion presents to ARM64
//! guests as `e1000e`.
//!
//! Interrupts: on x86-64 the function interrupts by MSI where it can, or on
//! IRQ 10/11 (`drivers::init`). `handle_interrupt` acknowledges ICR through the
//! register window, without a lock, and raises NetRx. On ARM64 every cause
//! stays masked and `net::rx_poll` polls `can_receive` once per tick.
//!
//! Memory: one page holds both descriptor rings and every further page holds
//! two 2048-byte buffers. All come from the frame allocator, are reached
//! through the direct map and stay with the device for the kernel's lifetime;
//! none is on the kernel heap. On ARM64 the data cache is cleaned after the
//! CPU writes a descriptor or buffer and invalidated before it reads one; x86
//! DMA is cache-coherent.

mod nic;
mod regs;

use core::sync::atomic::{compiler_fence, AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::drivers::pci::{self, Device, INTEL_VENDOR_ID};
use nic::{Model, Nic, Registers, Rings};

/// Intel 82540EM device ID (SDM 5.2, Table 5-1).
pub const E1000_DEVICE_ID: u16 = nic::I82540EM_DEVICE_ID;
/// Intel 82574L device ID (DS 9.1.2.2).
pub const E1000E_DEVICE_ID: u16 = nic::I82574_DEVICE_ID;

const PAGE: usize = 4096;
const BUFFERS_PER_PAGE: usize = PAGE / nic::BUFFER_SIZE;
const RX_PAGES: usize = nic::RX_RING / BUFFERS_PER_PAGE;
const TX_PAGES: usize = nic::TX_RING / BUFFERS_PER_PAGE;
/// The transmit ring follows the receive ring in the ring page.
const TX_RING_OFFSET: usize = nic::RX_RING * nic::DESCRIPTOR_BYTES;
const _: () = assert!(TX_RING_OFFSET + nic::TX_RING * nic::DESCRIPTOR_BYTES <= PAGE);

/// Upper bound on the spins of one `delay_us` microsecond, so a delay ends
/// even before the monotonic clock advances.
const SPINS_PER_US: u64 = 100;

static DRIVER: Mutex<Option<Driver>> = Mutex::new(None);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
/// The mapped register window, published once the function is running; the
/// interrupt entry and `link_up` use it without taking `DRIVER`.
static REGISTER_BASE: AtomicUsize = AtomicUsize::new(0);
/// The model, for the interrupt entry.
#[cfg(target_arch = "x86_64")]
static IS_82574: AtomicBool = AtomicBool::new(false);
/// The station address, bytes 0-5 in bits 0-47.
static STATION_ADDRESS: AtomicU64 = AtomicU64::new(0);

struct Driver {
    nic: Nic,
    hw: Hardware,
    #[cfg(target_arch = "x86_64")]
    device: Device,
}

/// BAR0's register window (SDM 13.1; DS 10.1).
struct Window {
    base: usize,
}

impl Registers for Window {
    fn read(&mut self, offset: u32) -> u32 {
        // SAFETY: `base` maps the function's whole BAR0 and every offset in
        // regs.rs lies inside it.
        unsafe { core::ptr::read_volatile((self.base + offset as usize) as *const u32) }
    }

    fn write(&mut self, offset: u32, value: u32) {
        // Descriptor and buffer stores before a tail write must not move
        // after it; on ARM64 the cache maintenance already ended in `dsb sy`.
        compiler_fence(Ordering::SeqCst);
        // SAFETY: as for `read`.
        unsafe { core::ptr::write_volatile((self.base + offset as usize) as *mut u32, value) }
    }

    fn delay_us(&mut self, us: u32) {
        let start = now_ns();
        let end = start.saturating_add(u64::from(us) * 1000);
        for _ in 0..u64::from(us).saturating_mul(SPINS_PER_US) {
            if now_ns() >= end {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

fn now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// One frame: its device address and its direct-map address.
#[derive(Clone, Copy)]
struct Page {
    phys: u64,
    virt: usize,
}

impl Page {
    const NONE: Self = Self { phys: 0, virt: 0 };

    fn allocate() -> Result<Self, &'static str> {
        let frame = crate::memory::frame_allocator::allocate_frame()
            .ok_or("e1000: out of DMA frames")?;
        let phys = frame.start_address().as_u64();
        let virt = (crate::memory::physical_memory_offset().as_u64() + phys) as usize;
        // SAFETY: a frame the allocator just handed out, mapped by the direct map.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, PAGE) };
        clean(virt, PAGE);
        Ok(Self { phys, virt })
    }

    fn release(self) {
        #[cfg(target_arch = "aarch64")]
        use crate::memory::arch_stub::{PhysAddr, PhysFrame};
        #[cfg(target_arch = "x86_64")]
        use x86_64::{structures::paging::PhysFrame, PhysAddr};
        if self.virt != 0 {
            crate::memory::frame_allocator::deallocate_frame(PhysFrame::containing_address(
                PhysAddr::new(self.phys),
            ));
        }
    }
}

/// The register window and the DMA pages the rings use.
struct Hardware {
    window: Window,
    rings: Page,
    rx: [Page; RX_PAGES],
    tx: [Page; TX_PAGES],
}

impl Hardware {
    fn new(base: usize) -> Result<Self, &'static str> {
        let mut hw = Self {
            window: Window { base },
            rings: Page::NONE,
            rx: [Page::NONE; RX_PAGES],
            tx: [Page::NONE; TX_PAGES],
        };
        let allocated = (|| -> Result<(), &'static str> {
            hw.rings = Page::allocate()?;
            for page in hw.rx.iter_mut().chain(hw.tx.iter_mut()) {
                *page = Page::allocate()?;
            }
            Ok(())
        })();
        match allocated {
            Ok(()) => Ok(hw),
            Err(error) => {
                hw.release();
                Err(error)
            }
        }
    }

    /// Return the pages. Only before the device has been given their addresses.
    fn release(&self) {
        for page in core::iter::once(&self.rings).chain(&self.rx).chain(&self.tx) {
            page.release();
        }
    }

    fn rx_descriptor_at(&self, index: usize) -> usize {
        self.rings.virt + index * nic::DESCRIPTOR_BYTES
    }

    fn tx_descriptor_at(&self, index: usize) -> usize {
        self.rings.virt + TX_RING_OFFSET + index * nic::DESCRIPTOR_BYTES
    }

    fn buffer(pages: &[Page], index: usize) -> (u64, usize) {
        let page = pages[index / BUFFERS_PER_PAGE];
        let offset = (index % BUFFERS_PER_PAGE) * nic::BUFFER_SIZE;
        (page.phys + offset as u64, page.virt + offset)
    }
}

fn read_descriptor(at: usize) -> [u64; 2] {
    invalidate(at, nic::DESCRIPTOR_BYTES);
    let words = at as *const u64;
    // SAFETY: `at` is a 16-byte-aligned descriptor inside the ring page.
    unsafe { [core::ptr::read_volatile(words), core::ptr::read_volatile(words.add(1))] }
}

fn write_descriptor(at: usize, value: [u64; 2]) {
    let words = at as *mut u64;
    // SAFETY: as for `read_descriptor`; software owns this descriptor.
    unsafe {
        core::ptr::write_volatile(words, value[0]);
        core::ptr::write_volatile(words.add(1), value[1]);
    }
    clean(at, nic::DESCRIPTOR_BYTES);
}

impl Registers for Hardware {
    fn read(&mut self, offset: u32) -> u32 {
        self.window.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) {
        self.window.write(offset, value)
    }

    fn delay_us(&mut self, us: u32) {
        self.window.delay_us(us)
    }
}

impl Rings for Hardware {
    fn rx_ring_address(&self) -> u64 {
        self.rings.phys
    }

    fn tx_ring_address(&self) -> u64 {
        self.rings.phys + TX_RING_OFFSET as u64
    }

    fn rx_buffer_address(&self, index: usize) -> u64 {
        Self::buffer(&self.rx, index).0
    }

    fn tx_buffer_address(&self, index: usize) -> u64 {
        Self::buffer(&self.tx, index).0
    }

    fn rx_descriptor(&mut self, index: usize) -> [u64; 2] {
        read_descriptor(self.rx_descriptor_at(index))
    }

    fn set_rx_descriptor(&mut self, index: usize, words: [u64; 2]) {
        write_descriptor(self.rx_descriptor_at(index), words)
    }

    fn tx_descriptor(&mut self, index: usize) -> [u64; 2] {
        read_descriptor(self.tx_descriptor_at(index))
    }

    fn set_tx_descriptor(&mut self, index: usize, words: [u64; 2]) {
        write_descriptor(self.tx_descriptor_at(index), words)
    }

    fn read_rx_buffer(&mut self, index: usize, out: &mut [u8]) {
        let (_, virt) = Self::buffer(&self.rx, index);
        let length = out.len().min(nic::BUFFER_SIZE);
        invalidate(virt, length);
        // SAFETY: the buffer is `BUFFER_SIZE` bytes and the device has
        // finished with it (its descriptor's DD bit was set).
        unsafe { core::ptr::copy_nonoverlapping(virt as *const u8, out.as_mut_ptr(), length) };
    }

    fn write_tx_buffer(&mut self, index: usize, data: &[u8]) {
        let (_, virt) = Self::buffer(&self.tx, index);
        let length = data.len().min(nic::BUFFER_SIZE);
        // SAFETY: software owns this slot until its descriptor is queued.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), virt as *mut u8, length) };
        clean(virt, length);
    }
}

/// Clean and invalidate by VA to the point of coherency, so the device reads
/// what the CPU wrote (Arm ARM, DC CIVAC), then order it before what follows.
#[cfg(target_arch = "aarch64")]
fn clean(start: usize, length: usize) {
    const LINE: usize = 64;
    let mut line = start & !(LINE - 1);
    while line < start + length {
        // SAFETY: cache maintenance on mapped kernel memory.
        unsafe { core::arch::asm!("dc civac, {}", in(reg) line, options(nostack)) };
        line += LINE;
    }
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// The same operation discards stale lines before the CPU reads what the
/// device wrote.
#[cfg(target_arch = "aarch64")]
fn invalidate(start: usize, length: usize) {
    clean(start, length)
}

#[cfg(target_arch = "x86_64")]
fn clean(_start: usize, _length: usize) {}

#[cfg(target_arch = "x86_64")]
fn invalidate(_start: usize, _length: usize) {}

#[cfg(target_arch = "x86_64")]
fn map_registers(bar: &pci::Bar) -> Result<usize, &'static str> {
    crate::memory::map_mmio(bar.address, bar.size as usize)
}

#[cfg(target_arch = "aarch64")]
fn map_registers(bar: &pci::Bar) -> Result<usize, &'static str> {
    Ok((crate::arch_impl::aarch64::constants::HHDM_BASE + bar.address) as usize)
}

fn find_function() -> Option<(Device, Model)> {
    pci::get_devices()?.into_iter().find_map(|device| {
        if device.vendor_id != INTEL_VENDOR_ID {
            return None;
        }
        Model::from_device_id(device.device_id).map(|model| (device, model))
    })
}

#[cfg(target_arch = "x86_64")]
fn model() -> Model {
    if IS_82574.load(Ordering::Acquire) {
        Model::Pcie82574
    } else {
        Model::Pci8254x
    }
}

fn window() -> Option<Window> {
    let base = REGISTER_BASE.load(Ordering::Acquire);
    (base != 0).then_some(Window { base })
}

/// Find, reset and start the first supported function. On x86-64 its
/// interrupt causes are unmasked; the caller connects MSI or INTx.
pub fn init() -> Result<(), &'static str> {
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    let (device, model) = find_function().ok_or("no Intel 82540EM or 82574 found")?;
    let bar = *device.get_mmio_bar().ok_or("e1000: no memory BAR")?;
    device.enable_memory_space();
    device.enable_bus_master();
    let base = map_registers(&bar)?;
    let mut hw = Hardware::new(base)?;
    let nic = match Nic::bring_up(&mut hw, model) {
        Ok(nic) => nic,
        Err(error) => {
            hw.release();
            return Err(error.as_str());
        }
    };
    let mac = nic.mac();
    let mut address = [0u8; 8];
    address[..6].copy_from_slice(&mac);
    STATION_ADDRESS.store(u64::from_le_bytes(address), Ordering::Release);
    #[cfg(target_arch = "x86_64")]
    IS_82574.store(model == Model::Pcie82574, Ordering::Release);
    REGISTER_BASE.store(base, Ordering::Release);
    crate::serial_println!(
        "[e1000] {:?} {:02x}:{:02x}.{} mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link {}",
        model,
        device.bus,
        device.device,
        device.function,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        if nic::link_up(&mut hw) { "up" } else { "down" }
    );
    #[cfg(target_arch = "x86_64")]
    nic::enable_interrupts(&mut hw, model);
    {
        let _guard = crate::net::net_lock_guard();
        let mut slot = DRIVER.lock();
        *slot = Some(Driver {
            nic,
            hw,
            #[cfg(target_arch = "x86_64")]
            device,
        });
    }
    INITIALIZED.store(true, Ordering::Release);
    Ok(())
}

/// Deliver the function's interrupts by MSI (PCI Local Bus 3.0, 6.8.1) and
/// return the vector. Causes raised before the message was enabled sent none,
/// so they are acknowledged here; the next cause sends a message.
#[cfg(target_arch = "x86_64")]
pub fn interrupt_by_msi() -> Result<u8, &'static str> {
    let device = {
        let _guard = crate::net::net_lock_guard();
        let slot = DRIVER.lock();
        slot.as_ref().map(|driver| driver.device.clone())
    };
    let device = device.ok_or("e1000 not initialized")?;
    let vector = crate::arch_impl::x86_64::msi::enable(&device, handle_interrupt)?;
    if let Some(mut window) = window() {
        nic::acknowledge_interrupt(&mut window, model());
    }
    Ok(vector)
}

pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

pub fn mac_address() -> Option<[u8; 6]> {
    if !is_initialized() {
        return None;
    }
    let bytes = STATION_ADDRESS.load(Ordering::Acquire).to_le_bytes();
    let mut mac = [0; 6];
    mac.copy_from_slice(&bytes[..6]);
    Some(mac)
}

pub fn link_up() -> bool {
    match window() {
        Some(mut window) => nic::link_up(&mut window),
        None => false,
    }
}

pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    let _guard = crate::net::net_lock_guard();
    let mut slot = DRIVER.lock();
    let driver = slot.as_mut().ok_or("e1000 not initialized")?;
    let sent = driver.nic.transmit(&mut driver.hw, data).map_err(nic::TxError::as_str);
    sent
}

pub fn receive(buffer: &mut [u8]) -> Result<usize, &'static str> {
    let _guard = crate::net::net_lock_guard();
    let mut slot = DRIVER.lock();
    let driver = slot.as_mut().ok_or("e1000 not initialized")?;
    let received = driver.nic.receive(&mut driver.hw, buffer).map_err(nic::RxError::as_str);
    received
}

pub fn can_receive() -> bool {
    if !is_initialized() {
        return false;
    }
    let _guard = crate::net::net_lock_guard();
    let mut slot = DRIVER.lock();
    let waiting = match slot.as_mut() {
        Some(driver) => driver.nic.can_receive(&mut driver.hw),
        None => false,
    };
    waiting
}

/// The interrupt entry for MSI and for IRQ 10/11, which other devices share.
/// Takes no lock: it acknowledges ICR through the register window and leaves
/// the rings to the NetRx bottom half.
#[cfg(target_arch = "x86_64")]
pub fn handle_interrupt() {
    let Some(mut window) = window() else {
        return;
    };
    let model = model();
    let causes = nic::acknowledge_interrupt(&mut window, model);
    if nic::needs_receive_work(causes, model) {
        crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
    }
}
