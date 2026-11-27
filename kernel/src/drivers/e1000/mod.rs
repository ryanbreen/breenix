//! Intel 82540EM (e1000) and 82574 (e1000e) Gigabit Ethernet.
//!
//! Clean room: written only from Intel's "PCI/PCI-X Family of Gigabit Ethernet
//! Controllers Software Developer's Manual" (317453006, revision 4.0), the
//! "Intel 82574 GbE Controller Family Datasheet" (317694, revision 3.3), the
//! PCI Local Bus 3.0 specification and the Arm Architecture Reference Manual
//! (cache maintenance and barriers). No other driver's source was read.
//! `nic.rs` holds the MAC programming with its section references; this file
//! attaches it to the kernel: PCI discovery, the register window, DMA memory,
//! locking and the interrupt entry.
//!
//! The functions this binds: the 82540EM (8086:100E), QEMU's default x86-64
//! NIC, and the 82574L (8086:10D3), which VMware Fusion presents to ARM64
//! guests as `e1000e`.
//!
//! Interrupts: on x86-64 the function interrupts by MSI where it can, or on
//! the IRQ 10 or 11 line its Interrupt Line register names (`drivers::init`). Its causes stay masked until the driver state
//! is published. `handle_interrupt` acknowledges ICR through the register
//! window, without a lock, and raises NetRx. On ARM64 the causes stay masked
//! and `net::rx_poll` polls `can_receive` once per tick.
//!
//! Memory, taken from outside the kernel heap:
//! - The descriptor rings share one page. The device writes descriptors back
//!   next to ones the CPU writes, four to a 64-byte line, so on ARM64 they
//!   live in the kernel's non-cacheable `.dma` section (the linker script and
//!   the loader map it Normal Non-Cacheable), where no CPU cache line can
//!   overwrite a write-back; barriers order them against the tail writes. On
//!   x86-64, whose DMA is cache-coherent, the page is a frame from the frame
//!   allocator.
//! - Buffers: each further frame holds two 2048-byte buffers. A buffer is a
//!   whole number of cache lines and belongs to one descriptor, so on ARM64
//!   the CPU cleans one after writing it and invalidates one before reading
//!   it (DC CIVAC by the CTR_EL0 line size).
//! The memory stays with the device for the kernel's lifetime.

mod nic;
mod regs;

use core::sync::atomic::{compiler_fence, AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

#[cfg(target_arch = "aarch64")]
use crate::arch_impl::aarch64::timer as counter;
#[cfg(target_arch = "x86_64")]
use crate::arch_impl::x86_64::timer as counter;
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

/// Counter rate assumed while x86's TSC rate is not yet known: above any TSC,
/// so a delay is not shorter than asked.
#[cfg(target_arch = "x86_64")]
const UNCALIBRATED_COUNTER_HZ: u64 = 10_000_000_000;

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
        // SAFETY: `base` maps the function's BAR0 and each offset in
        // regs.rs lies inside it.
        unsafe { core::ptr::read_volatile((self.base + offset as usize) as *const u32) }
    }

    fn write(&mut self, offset: u32, value: u32) {
        // Descriptor and buffer stores before a tail write must not move
        // after it. On ARM64 those stores already ended in `dsb sy`.
        compiler_fence(Ordering::SeqCst);
        // SAFETY: as for `read`.
        unsafe { core::ptr::write_volatile((self.base + offset as usize) as *mut u32, value) }
    }

    fn delay_us(&mut self, us: u32) {
        delay_us(us)
    }
}

/// Spin for at least `us` microseconds of the architecture's counter: the
/// TSC on x86-64, CNTVCT_EL0 at CNTFRQ_EL0 on ARM64.
fn delay_us(us: u32) {
    let hz = counter::frequency_hz();
    #[cfg(target_arch = "x86_64")]
    let hz = if hz == 0 { UNCALIBRATED_COUNTER_HZ } else { hz };
    let ticks = (u128::from(hz) * u128::from(us)).div_ceil(1_000_000) as u64;
    let begin = counter::rdtsc();
    while counter::rdtsc().wrapping_sub(begin) < ticks {
        core::hint::spin_loop();
    }
}

/// One frame from the frame allocator: its device address and its direct-map
/// address.
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

/// The ring page in ARM64's non-cacheable `.dma` section.
#[cfg(target_arch = "aarch64")]
#[repr(C, align(4096))]
struct RingPage(core::cell::UnsafeCell<[u8; PAGE]>);

// SAFETY: only the one attached driver uses the page: `init`, before the
// driver is published, then holders of `DRIVER`.
#[cfg(target_arch = "aarch64")]
unsafe impl Sync for RingPage {}

#[cfg(target_arch = "aarch64")]
#[link_section = ".dma"]
static RING_PAGE: RingPage = RingPage(core::cell::UnsafeCell::new([0; PAGE]));

/// The page holding both descriptor rings.
#[cfg(target_arch = "aarch64")]
fn ring_page() -> Result<Page, &'static str> {
    let virt = RING_PAGE.0.get() as usize;
    // SAFETY: the page is the driver's alone (see `RingPage`); kernel start-up
    // zeroed the section, and the device has not been given its address yet.
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, PAGE) };
    dma_barrier();
    let phys = crate::platform_config::kernel_va_to_ipa(virt as u64);
    Ok(Page { phys, virt })
}

#[cfg(target_arch = "x86_64")]
fn ring_page() -> Result<Page, &'static str> {
    Page::allocate()
}

/// The register window and the DMA memory the rings use.
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
            rings: ring_page()?,
            rx: [Page::NONE; RX_PAGES],
            tx: [Page::NONE; TX_PAGES],
        };
        let allocated = (|| -> Result<(), &'static str> {
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

    /// Return the frames. Only before the device has been given their
    /// addresses. ARM64's ring page is a static and stays.
    fn release(&self) {
        #[cfg(target_arch = "x86_64")]
        self.rings.release();
        for page in self.rx.iter().chain(&self.tx) {
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

/// Read a descriptor the device may have written back, before reading the
/// buffer it describes.
fn read_descriptor(at: usize) -> [u64; 2] {
    let words = at as *const u64;
    // SAFETY: `at` is a 16-byte-aligned descriptor inside the ring page.
    let value = unsafe { [core::ptr::read_volatile(words), core::ptr::read_volatile(words.add(1))] };
    dma_barrier();
    value
}

/// Write a descriptor software owns, visible to the device before the next
/// register write.
fn write_descriptor(at: usize, value: [u64; 2]) {
    let words = at as *mut u64;
    // SAFETY: as for `read_descriptor`; software owns this descriptor.
    unsafe {
        core::ptr::write_volatile(words, value[0]);
        core::ptr::write_volatile(words.add(1), value[1]);
    }
    dma_barrier();
}

impl Registers for Hardware {
    fn read(&mut self, offset: u32) -> u32 {
        self.window.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) {
        self.window.write(offset, value)
    }

    fn delay_us(&mut self, us: u32) {
        delay_us(us)
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

/// Complete prior memory accesses before later ones and before the next
/// device register access (Arm ARM, DSB).
#[cfg(target_arch = "aarch64")]
fn dma_barrier() {
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// The smallest data cache line, from CTR_EL0.DminLine (log2 of its words).
#[cfg(target_arch = "aarch64")]
fn cache_line() -> usize {
    let ctr: u64;
    // SAFETY: CTR_EL0 is readable at EL1.
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    4 << ((ctr >> 16) & 0xF)
}

/// Clean and invalidate by VA to the point of coherency (Arm ARM, DC CIVAC):
/// after the CPU writes a buffer, so the device reads it; before the CPU reads
/// one the device wrote, so no stale line is used.
#[cfg(target_arch = "aarch64")]
fn clean(start: usize, length: usize) {
    let line = cache_line();
    let mut at = start & !(line - 1);
    while at < start + length {
        // SAFETY: cache maintenance on mapped kernel memory.
        unsafe { core::arch::asm!("dc civac, {}", in(reg) at, options(nostack)) };
        at += line;
    }
    dma_barrier();
}

#[cfg(target_arch = "aarch64")]
fn invalidate(start: usize, length: usize) {
    clean(start, length)
}

#[cfg(target_arch = "x86_64")]
fn dma_barrier() {}

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
/// interrupt causes are unmasked once the driver is published; the caller
/// then connects MSI or INTx.
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
    let mut address = [0u8; 8];
    address[..6].copy_from_slice(&mac);
    STATION_ADDRESS.store(u64::from_le_bytes(address), Ordering::Release);
    #[cfg(target_arch = "x86_64")]
    IS_82574.store(model == Model::Pcie82574, Ordering::Release);
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
    REGISTER_BASE.store(base, Ordering::Release);
    INITIALIZED.store(true, Ordering::Release);
    // Only now can a cause reach a consumer: an interrupt before this point
    // would be acknowledged by `handle_interrupt` and no consumer to drain it.
    #[cfg(target_arch = "x86_64")]
    nic::enable_interrupts(&mut Window { base });
    Ok(())
}

/// Deliver the function's interrupts by MSI (PCI Local Bus 3.0, 6.8.1) and
/// return the vector. A cause pending when the message was enabled sent no
/// message, and holds off the next one, so the hand-off acknowledges it and
/// raises NetRx for any frame already waiting.
#[cfg(target_arch = "x86_64")]
pub fn interrupt_by_msi() -> Result<u8, &'static str> {
    let device = {
        let _guard = crate::net::net_lock_guard();
        let slot = DRIVER.lock();
        slot.as_ref().map(|driver| driver.device.clone())
    };
    let device = device.ok_or("e1000 not initialized")?;
    let vector = crate::arch_impl::x86_64::msi::enable(&device, handle_interrupt)?;
    let owed = {
        let _guard = crate::net::net_lock_guard();
        let mut slot = DRIVER.lock();
        let owed = match slot.as_mut() {
            Some(driver) => driver.nic.interrupts_routed(&mut driver.hw, model()),
            None => false,
        };
        owed
    };
    if owed {
        crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
    }
    Ok(vector)
}

/// The line the function's Interrupt Line register names (SDM 4.1; DS
/// 9.1.2.16), for the INTx fallback.
#[cfg(target_arch = "x86_64")]
pub fn irq_line() -> Option<u8> {
    let _guard = crate::net::net_lock_guard();
    let slot = DRIVER.lock();
    let line = slot.as_ref().map(|driver| driver.device.interrupt_line);
    line
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

/// Queue one frame. On x86-64 a send also raises NetRx when a received frame
/// is waiting: an interrupt before `net::init` registered the NetRx handler
/// raised it to no one, and the frame would otherwise wait for the next
/// receive interrupt. The stack's first send drains it. ARM64 polls instead.
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    let (sent, waiting) = {
        let _guard = crate::net::net_lock_guard();
        let mut slot = DRIVER.lock();
        let driver = slot.as_mut().ok_or("e1000 not initialized")?;
        let sent = driver.nic.transmit(&mut driver.hw, data).map_err(nic::TxError::as_str);
        let waiting = cfg!(target_arch = "x86_64") && driver.nic.can_receive(&mut driver.hw);
        (sent, waiting)
    };
    if waiting {
        crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
    }
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
    let causes = nic::acknowledge_interrupt(&mut window, model());
    if nic::needs_receive_work(causes) {
        crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
    }
}
