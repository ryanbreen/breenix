//! x86-metal xHCI host controller, written from Intel xHCI revision 1.2.
//!
//! Source authority: Intel's published xHCI 1.2 (4.2-4.6, 4.9, 4.17,
//! 4.19, 4.22.1 and chapters 5-7); USB 2.0 chapter 9 for descriptors.
//! The existing ARM USB stack is neither used nor a reference.
//! DMA is coherent on x86. One worker owns the rings and device contexts;
//! the MSI handler accesses only published MMIO addresses and atomic work.
//! `keyboard` adds HID boot keyboards, hot-plug and slot reclamation.
mod keyboard;
mod spec;

use crate::{
    drivers::pci,
    task::{kthread, scheduler, thread::ThreadState},
};
use alloc::{collections::VecDeque, vec, vec::Vec};
use core::{
    ptr::{read_volatile, write_volatile},
    sync::atomic::{fence, AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use spec::{Consumer, Contexts, Producer, Trb, Work, EVENT_TRBS, PORT_CHANGES, TRBS};

const SECOND: u64 = 1_000_000_000;
const USBCMD: usize = 0;
const USBSTS: usize = 4;
const CNR: u32 = 1 << 11;
const HALTED: u32 = 1;
const RUN: u32 = 1;
const INTE: u32 = 1 << 2;
const EINT: u32 = 1 << 3;
const IMAN: usize = 0;
const ERDP: usize = 0x18;
const EHB: u64 = 1 << 3; // RW1C, not a bit to preserve (5.5.2.3.3).
static OP: AtomicUsize = AtomicUsize::new(0);
static IR: AtomicUsize = AtomicUsize::new(0);
static INTERRUPTS: AtomicU64 = AtomicU64::new(0);
static WORK: Work = Work::new();
/// A thread-context console request, serviced after initial keyboard attach.
static CONSOLE_START: AtomicBool = AtomicBool::new(false);
static REPLUG: AtomicBool = AtomicBool::new(false);
static AVAILABLE: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);
static CONSOLE_READY: crate::task::completion::Completion = crate::task::completion::Completion::new();

/// Prepare input before init starts getty. Only measured boot targets request
/// the destructive replug exercise; ordinary sessions only arm input markers.
pub fn console_session_starting(replug: bool) {
    if !AVAILABLE.load(Ordering::Acquire) {
        CONSOLE_READY.complete(1);
        return;
    }
    REPLUG.store(replug, Ordering::Relaxed);
    // Store, then read the waiter: the worker publishes, then checks the flag.
    CONSOLE_START.store(true, Ordering::SeqCst);
    let tid = WORK.waiter();
    if tid != 0 {
        scheduler::wake_waitqueue_thread(tid);
    }
}

/// Wait from the startup kthread with preemption disabled so Completion uses
/// its sleeping path. Report failure or timeout so the caller can retain
/// serial login when USB input is unavailable.
pub fn wait_console_ready() -> bool {
    crate::per_cpu::preempt_disable();
    let completed = CONSOLE_READY.wait_timeout_uninterruptible(1, 60 * SECOND);
    crate::per_cpu::preempt_enable();
    matches!(completed, Ok(true)) && !FAILED.load(Ordering::Acquire)
}

fn rd(base: usize, offset: usize) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}
fn wr(base: usize, offset: usize, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) }
}
fn wr64(base: usize, offset: usize, value: u64) {
    // xHCI 5.1: low Dword first, then high Dword, for split 64-bit writes.
    wr(base, offset, value as u32);
    wr(base, offset + 4, (value >> 32) as u32);
}
fn now() -> u64 {
    let (s, ns) = crate::time::get_monotonic_time_ns();
    s * SECOND + ns
}
fn delay(ns: u64) {
    let end = now().saturating_add(ns);
    while now() < end {
        scheduler::yield_current();
    }
}
fn until(base: usize, offset: usize, mask: u32, value: u32, ns: u64) -> Result<(), &'static str> {
    let end = now().saturating_add(ns);
    loop {
        if rd(base, offset) & mask == value {
            return Ok(());
        }
        if now() >= end {
            return Err("register transition timed out");
        }
        scheduler::yield_current();
    }
}

/// Hard IRQ: bounded MMIO acknowledgements and atomic publication only.
/// EHB stays set until the worker has advanced the dequeue pointer.
fn interrupt() {
    let op = OP.load(Ordering::Acquire);
    let ir = IR.load(Ordering::Acquire);
    if op == 0 || ir == 0 {
        return;
    }
    wr(op, USBSTS, EINT);
    wr(ir, IMAN, 3); // IP RW1C; retain IE. MSI may already have cleared IP.
    let _ = rd(ir, IMAN); // flush posted acknowledgements before APIC EOI
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    let tid = WORK.signal();
    if tid != 0 {
        // A persistent per-CPU bit cannot overflow the shared ISR TID queue.
        crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::IrqPoll);
    }
}

/// The IRQ-exit softirq callback runs outside hard IRQ context. Wake through
/// the scheduler's owner-queue path, which retains a busy-owner wake, rather
/// than the bounded ISR TID buffer. No hardware polling occurs here.
fn deferred_wake(_: crate::task::softirqd::SoftirqType) {
    let tid = WORK.waiter();
    if tid != 0 && WORK.pending() {
        scheduler::wake_waitqueue_thread(tid);
    }
}

/// Permanent controller-owned DMA allocation. Frames are deliberately pinned
/// for this controller's lifetime, including failed commands: an unresponsive
/// DMA engine must retain ownership of its pages. C2 keeps the controller
/// attached. Device hot-unplug/reclamation belongs to the class-driver lane.
struct Dma {
    phys: u64,
    virt: usize,
    bytes: usize,
}
impl Dma {
    fn new(bytes: usize, address64: bool) -> Result<Self, &'static str> {
        let pages = (bytes + 4095) / 4096;
        let mut frames = vec![None; pages];
        let frame = crate::memory::frame_allocator::allocate_contiguous_frames(pages, &mut frames)
            .ok_or("no contiguous DMA frames")?;
        let phys = frame.start_address().as_u64();
        if !address64 && phys + (pages * 4096) as u64 > 1u64 << 32 {
            for frame in frames.into_iter().flatten() {
                crate::memory::frame_allocator::deallocate_frame(frame);
            }
            return Err("controller requires DMA memory below 4 GiB");
        }
        let virt = crate::memory::physical_memory_offset().as_u64() as usize + phys as usize;
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, pages * 4096);
        }
        Ok(Self {
            phys,
            virt,
            bytes: pages * 4096,
        })
    }
    fn put32(&self, offset: usize, value: u32) {
        assert!(offset + 4 <= self.bytes);
        wr(self.virt, offset, value);
    }
    fn put64(&self, offset: usize, value: u64) {
        assert!(offset + 8 <= self.bytes);
        wr64(self.virt, offset, value);
    }
    fn get32(&self, offset: usize) -> u32 {
        assert!(offset + 4 <= self.bytes);
        rd(self.virt, offset)
    }
    fn copy(&self, offset: usize, len: usize) -> Vec<u8> {
        assert!(offset + len <= self.bytes);
        (offset..offset + len)
            .map(|i| unsafe { read_volatile((self.virt + i) as *const u8) })
            .collect()
    }
    fn put8(&self, offset: usize, value: u8) {
        assert!(offset < self.bytes);
        unsafe { write_volatile((self.virt + offset) as *mut u8, value) }
    }
    /// Return the frames. Only for memory the controller no longer owns: a
    /// device's, after its Disable Slot command completed (4.6.4).
    fn release(self) -> usize {
        let pages = self.bytes / 4096;
        for page in 0..pages {
            let address = x86_64::PhysAddr::new(self.phys + (page * 4096) as u64);
            crate::memory::frame_allocator::deallocate_frame(
                x86_64::structures::paging::PhysFrame::containing_address(address),
            );
        }
        pages
    }
}
struct Ring {
    memory: Dma,
    producer: Producer,
}
impl Ring {
    fn new(address64: bool) -> Result<Self, &'static str> {
        Ok(Self {
            memory: Dma::new(4096, address64)?,
            producer: Producer::new(),
        })
    }
    fn push(&mut self, mut trb: Trb) -> u64 {
        let index = self.producer.index;
        trb.control = (trb.control & !1) | self.producer.cycle as u32;
        let addr = self.memory.phys + (index * 16) as u64;
        self.store(index, trb);
        let old_cycle = self.producer.cycle;
        if self.producer.advance() {
            // TC toggles the controller's consumer cycle on following the link.
            self.store(
                TRBS - 1,
                Trb::new(
                    6,
                    self.memory.phys,
                    0,
                    2 | (trb.control & (1 << 4)) | old_cycle as u32,
                ),
            );
        }
        addr
    }
    /// Where the next TRB goes and its cycle bit, for Set TR Dequeue Pointer.
    fn enqueue(&self) -> (u64, bool) {
        self.producer.next(self.memory.phys)
    }
    fn store(&self, index: usize, trb: Trb) {
        let at = index * 16;
        self.memory.put64(at, trb.parameter);
        self.memory.put32(at + 8, trb.status);
        fence(Ordering::Release);
        self.memory.put32(at + 12, trb.control);
    }
}
#[derive(Clone, Copy, Default)]
struct Protocol {
    major: u8,
    minor: u8,
    slot_type: u8,
    speeds: [u64; 16],
}
struct Device {
    slot: u8,
    port: u8,
    output: Dma,
    input: Dma,
    ep0: Ring,
    data: Dma,
    packet: u16,
    speed: u64,
    /// The first configuration's full descriptor, for class drivers.
    config: Vec<u8>,
    keyboard: Option<keyboard::Pipe>,
    /// EP0 requests sent again since Address Device.
    ep0_retries: u32,
}
struct Controller {
    pci: pci::Device,
    op: usize,
    ir: usize,
    doorbells: usize,
    ports: u8,
    slots: u8,
    hcc: u32,
    address64: bool,
    contexts: Contexts,
    dcbaa: Dma,
    command: Ring,
    events: Dma,
    erst: Dma,
    consumer: Consumer,
    completions: VecDeque<Trb>,
    /// Port IDs from Port Status Change Events, handled by the worker loop.
    port_events: VecDeque<u8>,
    protocols: Vec<Protocol>,
    devices: Vec<Device>,
    console: crate::drivers::hid_boot::Console,
    /// Absolute console lock state, shared across keyboard pipes and reattach.
    keyboard_leds: u8,
}

/// One x86-metal attach hook, after clocks and the interrupt controller exist.
pub fn init() {
    if crate::drivers::fw_cfg::present() {
        return;
    }
    if let Some(device) = pci::find_by_class(pci::DeviceClass::SerialBus, 3, 0x30) {
        crate::task::softirqd::register_softirq_handler(
            crate::task::softirqd::SoftirqType::IrqPoll,
            deferred_wake,
        );
        AVAILABLE.store(true, Ordering::Release);
        if let Err(error) = kthread::kthread_run(
            move || {
                if let Err(error) = start(device) {
                    FAILED.store(true, Ordering::Release);
                    CONSOLE_READY.complete(1);
                    log::error!("[xhci] FAILED: {error}");
                }
            },
            "xhci",
        ) {
            FAILED.store(true, Ordering::Release);
            CONSOLE_READY.complete(1);
            log::error!("[xhci] FAILED: cannot create worker: {error:?}");
        }
    } else {
        log::error!("[xhci] FAILED: no xHCI PCI function");
    }
}

// PCI Local Bus 3.0 6.2.5.1: size both halves with decoding disabled,
// restore the exact BARs and command register before mapping anything.
fn size_bar(device: &pci::Device) -> Result<(u64, usize), &'static str> {
    let (bus, dev, fun) = (device.bus, device.device, device.function);
    let command = pci::pci_read_config_word(bus, dev, fun, 4);
    let low = pci::pci_read_config_dword(bus, dev, fun, 0x10);
    if low & 1 != 0 || !matches!(low & 6, 0 | 4) {
        return Err("BAR0 is not a memory BAR");
    }
    let wide = low & 6 == 4;
    let high = if wide {
        pci::pci_read_config_dword(bus, dev, fun, 0x14)
    } else {
        0
    };
    pci::pci_write_config_word(bus, dev, fun, 4, command & !6);
    pci::pci_write_config_dword(bus, dev, fun, 0x10, u32::MAX);
    if wide {
        pci::pci_write_config_dword(bus, dev, fun, 0x14, u32::MAX);
    }
    let mask_low = pci::pci_read_config_dword(bus, dev, fun, 0x10) & !15;
    let mask_high = if wide {
        pci::pci_read_config_dword(bus, dev, fun, 0x14)
    } else {
        u32::MAX
    };
    pci::pci_write_config_dword(bus, dev, fun, 0x10, low);
    if wide {
        pci::pci_write_config_dword(bus, dev, fun, 0x14, high);
    }
    pci::pci_write_config_word(bus, dev, fun, 4, command);
    let mask = ((mask_high as u64) << 32) | mask_low as u64;
    let size = (!mask).wrapping_add(1);
    let base = ((high as u64) << 32) | (low & !15) as u64;
    if base == 0
        || size < 4096
        || size > 16 * 1024 * 1024
        || !size.is_power_of_two()
        || base & (size - 1) != 0
    {
        return Err("invalid BAR0 base or size");
    }
    Ok((base, size as usize))
}

fn start(device: pci::Device) -> Result<(), &'static str> {
    let (phys, size) = size_bar(&device)?;
    pci::publish_xhci_bar0(&device, phys, size as u64);
    device.enable_memory_space();
    let base = crate::memory::map_mmio(phys, size)?;
    let cap = rd(base, 0);
    let caplen = (cap & 255) as usize;
    let hcs1 = rd(base, 4);
    let hcc = rd(base, 0x10);
    let ports = (hcs1 >> 24) as u8;
    let slots = hcs1 as u8;
    let ir_offset = (rd(base, 0x18) & !31) as usize + 0x20;
    let db_offset = (rd(base, 0x14) & !3) as usize;
    if caplen < 0x20
        || ports == 0
        || slots == 0
        || (hcs1 >> 8) & 0x7ff == 0
        || caplen + 0x400 + ports as usize * 16 > size
        || ir_offset + 0x20 > size
        || db_offset + (slots as usize + 1) * 4 > size
    {
        return Err("capability registers exceed BAR0 or have no resources");
    }
    log::info!(
        "[xhci] found {:02x}:{:02x}.{} version={:x}.{:02x} ports={} slots={} BAR0={} bytes",
        device.bus,
        device.device,
        device.function,
        cap >> 24,
        (cap >> 16) & 255,
        ports,
        slots,
        size
    );
    let op = base + caplen;
    let mut protocols = vec![Protocol::default(); ports as usize + 1];
    handoff_and_protocols(base, size, hcc, op, &mut protocols)?;
    // 4.2, 5.4.1: no Operational/Runtime writes while CNR; stop and wait
    // for HCHalted before HCRST; then wait for HCRST=0 AND CNR=0.
    until(op, USBSTS, CNR, 0, SECOND)?;
    wr(op, USBCMD, rd(op, USBCMD) & !(RUN | INTE));
    until(op, USBSTS, HALTED, HALTED, SECOND)?;
    wr(op, USBCMD, 2);
    until(op, USBCMD, 2, 0, SECOND)?;
    until(op, USBSTS, CNR, 0, SECOND)?;
    if rd(op, 8) & 1 == 0 {
        return Err("controller does not support 4 KiB pages");
    }
    let address64 = hcc & 1 != 0;
    let dcbaa = Dma::new((slots as usize + 1) * 8, address64)?;
    // HCSPARAMS2: Max Scratchpad Buffers Hi[25:21], Lo[31:27] (5.3.4).
    let hcs2 = rd(base, 8);
    let scratchpads = (((hcs2 >> 21) & 31) << 5) | ((hcs2 >> 27) & 31);
    if scratchpads != 0 {
        let array = Dma::new(scratchpads as usize * 8, address64)?;
        for i in 0..scratchpads as usize {
            let buffer = Dma::new(4096, address64)?;
            array.put64(i * 8, buffer.phys);
        }
        dcbaa.put64(0, array.phys);
    }
    let command = Ring::new(address64)?;
    let events = Dma::new(4096, address64)?;
    let erst = Dma::new(4096, address64)?;
    erst.put64(0, events.phys);
    erst.put32(8, EVENT_TRBS as u32);
    let mut controller = Controller {
        pci: device,
        op,
        ir: base + ir_offset,
        doorbells: base + db_offset,
        ports,
        slots,
        hcc,
        address64,
        contexts: Contexts::new(hcc & 4 != 0),
        dcbaa,
        command,
        events,
        erst,
        consumer: Consumer::new(),
        completions: VecDeque::new(),
        port_events: VecDeque::new(),
        protocols,
        devices: Vec::new(),
        console: crate::drivers::hid_boot::Console::new(),
        keyboard_leds: 0,
    };
    let result = controller.run(scratchpads);
    if result.is_err() {
        // Stop the failed controller; Dma retains its pages if the halt fails.
        wr(
            controller.op,
            USBCMD,
            rd(controller.op, USBCMD) & !(RUN | INTE),
        );
        let stopped = until(controller.op, USBSTS, HALTED, HALTED, SECOND).is_ok();
        controller.pci.disable_msi();
        log::error!(
            "[xhci] stopped after failure: halted={stopped} status={:#x}",
            rd(controller.op, USBSTS)
        );
    }
    result
}

fn handoff_and_protocols(
    base: usize,
    size: usize,
    hcc: u32,
    op: usize,
    protocols: &mut [Protocol],
) -> Result<(), &'static str> {
    let mut offset = ((hcc >> 16) * 4) as usize;
    let mut legacy = false;
    let mut visited = 0;
    while offset != 0 {
        visited += 1;
        if offset < 0x20 || offset + 4 > size || visited > size / 4 {
            return Err("invalid extended capability chain");
        }
        let header = rd(base, offset);
        match header & 255 {
            1 => {
                if legacy || offset + 8 > size {
                    return Err("invalid legacy capability");
                }
                legacy = true;
                let began = now();
                // 7.1.1: update the OS-owned byte, preserving its RsvdP bits.
                // The adjacent BIOS-owned byte is not written.
                unsafe {
                    let owned = (base + offset + 3) as *mut u8;
                    write_volatile(owned, read_volatile(owned) | 1);
                }
                let released = until(base, offset, 1 << 16, 0, SECOND).is_ok();
                // 7.1.2: 0xe011 selects the five defined SMI enables;
                // 0xe0000000 acknowledges the three RW1C status flags.
                // Preserve the reserved fields.
                let ctl = rd(base, offset + 4);
                wr(base, offset + 4, (ctl & !0x0000_e011) | 0xe000_0000);
                let own = rd(base, offset);
                let smi = rd(base, offset + 4);
                if !released || own & 0x0101_0000 != 0x0100_0000 || smi & 0xe011 != 0 {
                    log::error!(
                        "[xhci] firmware handoff FAILED: ownership={own:#x} control={smi:#x}"
                    );
                    return Err("firmware ownership/quiescence not demonstrated");
                }
                // Ownership alone is insufficient: firmware must stop DMA too.
                until(op, USBSTS, CNR, 0, SECOND)?;
                wr(op, USBCMD, rd(op, USBCMD) & !(RUN | INTE));
                until(op, USBSTS, HALTED, HALTED, SECOND)?;
                log::info!(
                    "[xhci] firmware handoff: released in {} ms; SMI disabled, halted",
                    (now() - began) / 1_000_000
                );
            }
            2 => {
                if offset + 16 > size || rd(base, offset + 4) != 0x2042_5355 {
                    return Err("invalid USB protocol capability");
                }
                let info = rd(base, offset + 8);
                let first = (info & 255) as usize;
                let count = ((info >> 8) & 255) as usize;
                let psi_count = (info >> 28) as usize;
                if first == 0
                    || count == 0
                    || first + count > protocols.len()
                    || offset + 16 + psi_count * 4 > size
                {
                    return Err("invalid protocol port range");
                }
                let mut protocol = Protocol {
                    major: (header >> 24) as u8,
                    minor: (header >> 16) as u8,
                    slot_type: (rd(base, offset + 12) & 31) as u8,
                    speeds: [0; 16],
                };
                if psi_count == 0 {
                    match protocol.major {
                        2 => {
                            protocol.speeds[1] = 12_000_000;
                            protocol.speeds[2] = 1_500_000;
                            protocol.speeds[3] = 480_000_000;
                        }
                        3 => {
                            protocol.speeds[4] = 5_000_000_000;
                        }
                        _ => return Err("unsupported USB protocol major revision"),
                    }
                }
                for i in 0..psi_count {
                    let psi = rd(base, offset + 16 + i * 4);
                    let id = (psi & 15) as usize;
                    let exponent = (psi >> 4) & 3;
                    let rate = (psi >> 16) as u64 * 1000u64.pow(exponent);
                    if id == 0 || rate == 0 {
                        return Err("invalid protocol speed ID");
                    }
                    protocol.speeds[id] = rate;
                }
                for entry in &mut protocols[first..first + count] {
                    if entry.major != 0 {
                        return Err("overlapping supported protocol ranges");
                    }
                    *entry = protocol;
                }
                log::info!(
                    "[xhci] protocol USB {}.{:02x}: ports {}-{} slot-type={}",
                    protocol.major,
                    protocol.minor,
                    first,
                    first + count - 1,
                    protocol.slot_type
                );
            }
            _ => {}
        }
        let next = ((header >> 8) & 255) as usize * 4;
        offset = if next == 0 {
            0
        } else {
            offset
                .checked_add(next)
                .ok_or("extended capability overflow")?
        };
    }
    if protocols[1..].iter().any(|p| p.major == 0) {
        return Err("port without a supported protocol");
    }
    if !legacy {
        until(op, USBSTS, CNR, 0, SECOND)?;
        wr(op, USBCMD, rd(op, USBCMD) & !(RUN | INTE));
        until(op, USBSTS, HALTED, HALTED, SECOND)?;
        log::info!("[xhci] firmware handoff: no legacy capability; halted");
    }
    Ok(())
}

impl Controller {
    fn run(&mut self, scratchpads: u32) -> Result<(), &'static str> {
        // 4.2: program DMA resources and interrupt enables before Run/Stop.
        wr(self.op, 0x38, self.slots as u32);
        wr64(self.op, 0x30, self.dcbaa.phys);
        wr64(self.op, 0x18, self.command.memory.phys | 1);
        wr(self.ir, 8, 1); // ERSTSZ
        wr64(self.ir, ERDP, self.events.phys);
        fence(Ordering::Release);
        wr64(self.ir, 0x10, self.erst.phys); // ERSTBA enables the ring
        wr(self.ir, 4, 4000); // 1 ms moderation, 250 ns units (5.5.2.2)
        OP.store(self.op, Ordering::Release);
        IR.store(self.ir, Ordering::Release);
        let vector = crate::arch_impl::x86_64::msi::enable(&self.pci, interrupt)?;
        self.pci.enable_bus_master();
        wr(self.op, USBSTS, EINT | (1 << 4));
        wr(self.ir, IMAN, 3);
        wr(self.op, USBCMD, RUN | INTE);
        until(self.op, USBSTS, HALTED, 0, SECOND)?;
        log::info!("[xhci] running: scratchpads={scratchpads} context={} page=4096 MSI={vector:#x} INTx-disabled", self.contexts.stride);
        // 4.3 / 5.4.8.1: explicitly power each root port before the census.
        // PP is read-only one when per-port power switching is absent.
        for port in 1..=self.ports {
            if self.portsc(port) & (1 << 9) == 0 {
                self.port_action(port, 1 << 9);
            }
        }
        delay(100_000_000);
        // Sequential completed commands require repeated MSI rearming. More
        // than two segment lengths also exercises consumer-cycle inversion.
        let began = INTERRUPTS.load(Ordering::Acquire);
        for _ in 0..EVENT_TRBS * 2 + 1 {
            self.command(Trb::new(23, 0, 0, 0))?;
        }
        let interrupts = INTERRUPTS.load(Ordering::Acquire) - began;
        if interrupts < 3 || self.consumer.wraps < 2 {
            return Err("MSI repetition/event-ring wrap proof missing");
        }
        log::info!(
            "[xhci] no-op completed by MSI: interrupts={interrupts} event-wraps={}",
            self.consumer.wraps
        );
        let mut connected = 0;
        for port in 1..=self.ports {
            let status = self.portsc(port);
            if status & 1 == 0 {
                log::info!(
                    "[xhci] port {port}: USB {}.{:02x} empty",
                    self.protocols[port as usize].major,
                    self.protocols[port as usize].minor
                );
                continue;
            }
            connected += 1;
            if let Err(error) = self.enumerate(port) {
                log::error!(
                    "[xhci] port {port}: enumeration FAILED: {error}; PORTSC={:#x}",
                    self.portsc(port)
                );
                return Err("attached device enumeration incomplete");
            }
        }
        log::info!("[xhci] port census: ports={} connected={connected} enumerated={}; descriptors complete", self.ports, self.devices.len());
        // Keep owning the event ring: class drivers, port changes, reclamation.
        self.serve()
    }
    fn ring_doorbell(&self, slot: u8, target: u32) {
        fence(Ordering::Release);
        wr(self.doorbells, slot as usize * 4, target);
    }
    fn service(&mut self) -> Result<(), &'static str> {
        // Only called after the MSI handler published pending work. Do not
        // poll a completion into existence when interrupt delivery is broken.
        let mut failed = false;
        loop {
            let at = self.consumer.index * 16;
            let control = self.events.get32(at + 12);
            if control & 1 != self.consumer.cycle as u32 {
                break;
            }
            fence(Ordering::Acquire);
            let parameter =
                self.events.get32(at) as u64 | ((self.events.get32(at + 4) as u64) << 32);
            let event = Trb {
                parameter,
                status: self.events.get32(at + 8),
                control,
            };
            match event.kind() {
                32 | 33 => self.completions.push_back(event),
                // PORTSC changes stay latched until `port_changed` acknowledges them.
                34 => self.port_events.push_back((event.parameter >> 24) as u8),
                37 => {
                    log::error!("[xhci] host controller event code={}", event.code());
                    failed = true;
                }
                _ => {
                    log::error!("[xhci] unexpected event type={}", event.kind());
                    failed = true;
                }
            }
            self.consumer.advance();
        }
        fence(Ordering::Release);
        // EHB is RW1C: this service exit writes one, even for an empty ring.
        wr64(
            self.ir,
            ERDP,
            (self.events.phys + (self.consumer.index * 16) as u64) | EHB,
        );
        let _ = rd(self.ir, ERDP);
        if failed || rd(self.op, USBSTS) & ((1 << 2) | (1 << 12)) != 0 {
            Err("controller reported a host error")
        } else {
            Ok(())
        }
    }
    /// Wait for MSI-published work (true), or until `deadline` passes or, when
    /// `requests` is set, a thread-context request arrives (false).
    fn wait_work(&self, deadline: Option<u64>, requests: bool) -> Result<bool, &'static str> {
        let tid = scheduler::current_thread_id().ok_or("worker has no thread identity")?;
        let requested = || requests && CONSOLE_START.load(Ordering::SeqCst);
        loop {
            WORK.publish(tid);
            if WORK.take() {
                WORK.withdraw();
                return Ok(true);
            }
            if requested() || deadline.is_some_and(|end| now() >= end) {
                WORK.withdraw();
                return Ok(false);
            }
            let published = scheduler::with_current(|sched| {
                if WORK.pending() || requested() {
                    return false;
                }
                sched.block_current_for_io_with_timeout(deadline)
            })
            .unwrap_or(false);
            // A wake can race the blocked-state publication. Normalizing the
            // state is necessary even when pending lets us skip the sleep.
            if published && !WORK.pending() && !requested() {
                while scheduler::with_current(|s| {
                    s.current_thread_mut()
                        .map(|t| t.state == ThreadState::BlockedOnIO)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
                {
                    crate::task::waitqueue::halt_blocked_current();
                }
            }
            scheduler::with_current(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    if thread.state == ThreadState::BlockedOnIO {
                        thread.set_running();
                    }
                    thread.wake_time_ns = None;
                    thread.blocked_in_syscall = false;
                }
            });
            WORK.withdraw();
            if WORK.take() {
                return Ok(true);
            }
            if requested() || deadline.is_some_and(|end| now() >= end) {
                return Ok(false);
            }
        }
    }
    /// The completion for one of `pointers`, whatever its completion code.
    fn event_any(&mut self, pointers: &[u64], kind: u32) -> Result<Trb, &'static str> {
        let deadline = now().saturating_add(3 * SECOND);
        loop {
            if let Some(i) = self
                .completions
                .iter()
                .position(|e| e.kind() == kind && pointers.contains(&(e.parameter & !15)))
            {
                return self.completions.remove(i).ok_or("completion queue changed");
            }
            if !self.wait_work(Some(deadline), false)? {
                return Err("MSI completion timed out");
            }
            self.service()?;
        }
    }
    fn event(&mut self, pointers: &[u64], kind: u32) -> Result<Trb, &'static str> {
        let event = self.event_any(pointers, kind)?;
        if event.code() != 1 {
            log::error!(
                "[xhci] completion type={kind} code={} slot={}",
                event.code(),
                event.slot()
            );
            return Err("command/transfer completion failed");
        }
        Ok(event)
    }
    fn command(&mut self, trb: Trb) -> Result<Trb, &'static str> {
        let pointer = self.command.push(trb);
        self.ring_doorbell(0, 0);
        self.event(&[pointer], 33)
    }
    /// A command's completion code, for commands whose failure codes the
    /// caller handles (Stop Endpoint's Context State Error, for one).
    fn command_code(&mut self, trb: Trb) -> Result<u8, &'static str> {
        let pointer = self.command.push(trb);
        self.ring_doorbell(0, 0);
        Ok(self.event_any(&[pointer], 33)?.code())
    }
    fn portsc(&self, port: u8) -> u32 {
        rd(self.op, 0x400 + (port as usize - 1) * 16)
    }
    fn port_action(&self, port: u8, action: u32) {
        wr(
            self.op,
            0x400 + (port as usize - 1) * 16,
            spec::port_write(self.portsc(port), action),
        );
    }
    fn reset_port(&mut self, port: u8) -> Result<(u8, u64), &'static str> {
        // USB 2.0 9.1.2 and xHCI 4.3.1: 100 ms stable connection, bounded
        // debounce, reset-complete+PED, then 10 ms recovery before traffic.
        let end = now() + 2 * SECOND;
        let mut stable = now();
        loop {
            let sc = self.portsc(port);
            if sc & 1 == 0 {
                return Err("device disconnected during debounce");
            }
            if sc & (1 << 17) != 0 {
                self.port_action(port, 1 << 17);
                stable = now();
            }
            if now() - stable >= 100_000_000 {
                break;
            }
            if now() >= end {
                return Err("port debounce did not stabilize");
            }
            scheduler::yield_current();
        }
        let protocol = self.protocols[port as usize];
        if protocol.major == 2 {
            self.port_action(port, PORT_CHANGES);
            self.port_action(port, 1 << 4);
            let end = now() + SECOND;
            loop {
                let sc = self.portsc(port);
                if sc & 1 == 0 {
                    return Err("device disconnected during reset");
                }
                if sc & (1 << 4) == 0 && sc & (1 << 21) != 0 {
                    if sc & 2 == 0 || sc & (15 << 5) != 0 {
                        return Err("reset completed without enabled U0 port");
                    }
                    self.port_action(port, 1 << 21);
                    break;
                }
                if now() >= end {
                    return Err("port reset timed out");
                }
                scheduler::yield_current();
            }
        } else {
            // SuperSpeed link training performs the attach reset (4.3).
            until(
                self.op,
                0x400 + (port as usize - 1) * 16,
                3 | (15 << 5),
                3,
                SECOND,
            )?;
        }
        delay(10_000_000);
        let sc = self.portsc(port);
        if sc & 3 != 3 {
            return Err("port lost enable during reset recovery");
        }
        let speed_id = ((sc >> 10) & 15) as u8;
        let speed = protocol.speeds[speed_id as usize];
        if speed == 0 {
            return Err("port reported undefined speed ID");
        }
        self.port_action(port, PORT_CHANGES);
        Ok((speed_id, speed))
    }
    /// Reset the port, give the device a slot and an address, and read its
    /// descriptors. A device that fails after Enable Slot is reclaimed.
    fn enumerate(&mut self, port: u8) -> Result<(), &'static str> {
        let (speed_id, speed) = self.reset_port(port)?;
        let protocol = self.protocols[port as usize];
        let slot = self
            .command(Trb::new(9, 0, 0, (protocol.slot_type as u32) << 16))?
            .slot();
        if slot == 0 || slot > self.slots {
            return Err("Enable Slot returned invalid slot");
        }
        let allocated = (|| {
            Ok::<_, &'static str>(Device {
                slot,
                port,
                output: Dma::new(32 * self.contexts.stride, self.address64)?,
                input: Dma::new(33 * self.contexts.stride, self.address64)?,
                ep0: Ring::new(self.address64)?,
                data: Dma::new(65536, self.address64)?,
                packet: if speed >= 5_000_000_000 {
                    512
                } else if speed == 480_000_000 {
                    64
                } else {
                    8
                },
                speed,
                config: Vec::new(),
                keyboard: None,
                ep0_retries: 0,
            })
        })();
        let mut device = match allocated {
            Ok(device) => device,
            Err(error) => {
                // The controller has no contexts for this slot yet; free the slot.
                let _ = self.command_code(spec::disable_slot(slot));
                return Err(error);
            }
        };
        match self.address_and_describe(&mut device, port, speed_id, protocol) {
            Ok(()) => {
                self.devices.push(device);
                Ok(())
            }
            Err(error) => {
                self.reclaim(device);
                Err(error)
            }
        }
    }
    fn address_and_describe(
        &mut self,
        device: &mut Device,
        port: u8,
        speed_id: u8,
        protocol: Protocol,
    ) -> Result<(), &'static str> {
        let (slot, speed) = (device.slot, device.speed);
        let input = &device.input;
        input.put32(4, 3); // Add Slot + EP0 contexts (6.2.5).
        let slot_at = self.contexts.input(0);
        input.put32(slot_at, (1 << 27) | ((speed_id as u32) << 20));
        input.put32(slot_at + 4, (port as u32) << 16);
        let ep_at = self.contexts.input(1);
        input.put32(ep_at + 4, (3 << 1) | (4 << 3) | ((device.packet as u32) << 16));
        input.put64(ep_at + 8, device.ep0.memory.phys | 1);
        input.put32(ep_at + 16, 8); // Average TRB length for control, 6.2.3.
        self.dcbaa.put64(slot as usize * 8, device.output.phys);
        fence(Ordering::Release);
        self.command(Trb::new(11, device.input.phys, 0, (slot as u32) << 24))?;
        // The SetAddress() recovery interval is software's to time (4.6.5).
        delay(spec::SET_ADDRESS_RECOVERY_NS);
        // Read the first eight bytes; FS needs Evaluate Context before
        // the full descriptor if firmware/device chose a different packet size.
        let first = self.descriptor(device, 1, 0, 8)?;
        if first.len() != 8 || first[0] != 18 || first[1] != 1 {
            return Err("invalid initial device descriptor");
        }
        let actual = spec::packet_size(device.speed, first[7])?;
        if actual != device.packet {
            // Preserve the output EP0 context, updating only MaxPacketSize.
            for offset in (0..self.contexts.stride).step_by(4) {
                device.input.put32(
                    ep_at + offset,
                    device.output.get32(self.contexts.output(1) + offset),
                );
            }
            device.input.put32(4, 2);
            let dw1 = device.input.get32(ep_at + 4);
            device
                .input
                .put32(ep_at + 4, (dw1 & 0xffff) | ((actual as u32) << 16));
            self.command(Trb::new(13, device.input.phys, 0, (slot as u32) << 24))?;
            device.packet = actual;
        }
        let desc = self.descriptor(device, 1, 0, 18)?;
        if desc.len() != 18 || desc[0] != 18 || desc[1] != 1 || desc[17] == 0 {
            return Err("invalid device descriptor");
        }
        log::info!("[xhci] port {port}: USB {}.{:02x} speed={} bit/s id={} slot={} address={} class={:02x}/{:02x}/{:02x} EP0={} configurations={}",
            protocol.major, protocol.minor, speed, speed_id, slot, device.output.get32(self.contexts.output(0) + 12) & 255,
            desc[4], desc[5], desc[6], device.packet, desc[17]);
        for index in 0..desc[17] {
            let header = self.descriptor(device, 2, index, 9)?;
            if header.len() != 9 || header[0] != 9 || header[1] != 2 {
                return Err("invalid configuration header");
            }
            let total = u16::from_le_bytes([header[2], header[3]]);
            if total < 9 {
                return Err("configuration shorter than its header");
            }
            let config = self.descriptor(device, 2, index, total)?;
            if config.len() != total as usize || config[..9] != header {
                return Err("configuration length/header changed");
            }
            self.report_configuration(port, &config)?;
            if crate::drivers::hid_boot::interface::prefer_configuration(&device.config, &config) {
                device.config = config;
            }
        }
        Ok(())
    }
    fn descriptor(
        &mut self,
        device: &mut Device,
        kind: u8,
        index: u8,
        len: u16,
    ) -> Result<Vec<u8>, &'static str> {
        let request = spec::get_descriptor(kind, index, len).parameter;
        let event = self.ep0_transfer(device, request, &|device: &mut Device| {
            // Each Data Stage TRB is contained in a 64 KiB boundary (4.11.2.2).
            // The DMA allocation need not itself be 64 KiB aligned, so split there.
            let mut pointers = vec![device.ep0.push(spec::get_descriptor(kind, index, len))];
            let mut offset = 0usize;
            while offset < len as usize {
                let phys = device.data.phys + offset as u64;
                let count = (len as usize - offset).min(65536 - (phys as usize & 65535));
                let remaining = len as usize - offset - count;
                let packets = remaining.div_ceil(device.packet as usize).min(31) as u32;
                // First is Data Stage; following fragments are Normal TRBs (4.11.2.2).
                let kind = if offset == 0 { 3 } else { 1 };
                // ISP reports a short packet at its Data/Normal TRB. Reject it
                // rather than parsing unwritten bytes from an earlier descriptor.
                let flags = (1 << 2)
                    | (if remaining != 0 { 1 << 4 } else { 0 })
                    | if offset == 0 { 1 << 16 } else { 0 };
                pointers.push(device.ep0.push(Trb::new(
                    kind,
                    phys,
                    count as u32 | (packets << 17),
                    flags,
                )));
                offset += count;
            }
            pointers.push(device.ep0.push(Trb::new(4, 0, 0, 1 << 5))); // OUT status + IOC
            pointers
        })?;
        if event.code() != 1 {
            log::error!(
                "[xhci] slot {}: GET_DESCRIPTOR type={kind} completion code={}",
                device.slot,
                event.code()
            );
            return Err("command/transfer completion failed");
        }
        if event.status & 0x00ff_ffff != 0 {
            return Err("EP0 transfer identity or length mismatch");
        }
        Ok(device.data.copy(0, len as usize))
    }
    /// One control transfer on EP0, its TDs queued by `queue` (which returns
    /// their TRB addresses), for the setup packet `request`. A transfer that
    /// halts EP0 or reads short is sent again (`spec::ep0_next`); the
    /// completion returned is the last attempt's.
    fn ep0_transfer(
        &mut self,
        device: &mut Device,
        request: u64,
        queue: &dyn Fn(&mut Device) -> Vec<u64>,
    ) -> Result<Trb, &'static str> {
        let mut attempt = 1;
        loop {
            let pointers = queue(device);
            self.ring_doorbell(device.slot, 1);
            let event = self.event_any(&pointers, 32)?;
            if event.slot() != device.slot || (event.control >> 16) & 31 != 1 {
                return Err("EP0 transfer identity mismatch");
            }
            let after_ns = match spec::ep0_next(event.code(), attempt) {
                spec::Ep0Next::Done | spec::Ep0Next::Fail => return Ok(event),
                spec::Ep0Next::RecoverAndFail => {
                    self.recover_ep0(device)?;
                    return Ok(event);
                }
                spec::Ep0Next::Retry { after_ns } => {
                    self.recover_ep0(device)?;
                    after_ns
                }
                spec::Ep0Next::Resend { after_ns } => {
                    // The Status TD still runs (4.10.1.1); take its completion.
                    let status = pointers[pointers.len() - 1];
                    if event.parameter & !15 != status {
                        let done = self.event_any(&[status], 32)?;
                        if spec::halts_endpoint(done.code()) {
                            self.recover_ep0(device)?;
                        }
                    }
                    after_ns
                }
            };
            device.ep0_retries += 1;
            log::warn!(
                "[usb] slot {}: EP0 request {request:#018x} completed with code={} on attempt {attempt}; sending it again",
                device.slot,
                event.code()
            );
            delay(after_ns);
            attempt += 1;
        }
    }
    /// 4.6.8: Reset Endpoint, then a Set TR Dequeue Pointer onto the next
    /// Setup TD, past the halted TD's remaining stages.
    fn recover_ep0(&mut self, device: &Device) -> Result<(), &'static str> {
        self.reset_endpoint(device.slot, 1)?;
        self.set_dequeue(device.slot, 1, device.ep0.enqueue())
    }
    /// Reset Endpoint with TSP 0, for a halted endpoint (4.6.8).
    fn reset_endpoint(&mut self, slot: u8, dci: u8) -> Result<(), &'static str> {
        let code = self.command_code(spec::reset_endpoint(slot, dci))?;
        if code != 1 {
            log::error!("[usb] slot {slot}: Reset Endpoint {dci} code={code}");
            return Err("Reset Endpoint failed");
        }
        Ok(())
    }
    /// Point a stopped endpoint's dequeue at `next`, a ring's next free TRB (4.6.10).
    fn set_dequeue(&mut self, slot: u8, dci: u8, next: (u64, bool)) -> Result<(), &'static str> {
        self.command(spec::set_dequeue(slot, dci, next.0, next.1))?;
        Ok(())
    }
    fn report_configuration(&self, port: u8, config: &[u8]) -> Result<(), &'static str> {
        let mut offset = 0;
        while offset < config.len() {
            if offset + 2 > config.len() {
                return Err("truncated descriptor header");
            }
            let len = config[offset] as usize;
            if len < 2 || offset + len > config.len() {
                return Err("invalid descriptor boundary");
            }
            if config[offset + 1] == 4 {
                if len < 9 {
                    return Err("short interface descriptor");
                }
                log::info!("[xhci] port {port}: configuration={} interface={} alternate={} class={:02x}/{:02x}/{:02x} endpoints={}",
                    config[5], config[offset+2], config[offset+3], config[offset+5], config[offset+6], config[offset+7], config[offset+4]);
            }
            offset += len;
        }
        Ok(())
    }
}
