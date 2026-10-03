//! NVMe (NVM Express) storage driver for x86-64.
//!
//! One controller per PCI function (class 01, subclass 08, prog-if 02). Each
//! controller is reset, given an admin queue pair, identified, and given one
//! I/O queue pair. The first active namespace becomes a 512-byte-sector block
//! device.
//!
//! # Commands
//!
//! Admin commands (Identify, Set Features, Create I/O CQ/SQ) run during
//! `init()`, before interrupts are enabled, and complete by polling the admin
//! completion queue with the controller's interrupt masked (INTMS).
//!
//! I/O commands (Read, Write, Flush) are serialized: one command is in flight
//! per controller, and each moves at most one 4 KiB page through a single
//! PRP entry, so no PRP lists are needed.
//!
//! # Completion
//!
//! When firmware routed the function's INTx pin to IRQ 10 or 11 (the lines
//! the x86 IRQ handlers dispatch), the I/O completion queue is created with
//! interrupts enabled and the submitter sleeps on a `Completion` that the IRQ
//! handler signals. With no scheduler thread and interrupts masked (early
//! boot), or when the pin is routed anywhere else, the submitter polls the
//! completion queue instead. Every
//! consumer of the I/O completion queue goes through `drain_io_cq()`, which
//! always consumes new entries and rings the head doorbell so a level-triggered
//! INTx line is deasserted.

use crate::drivers::pci::{self, Device as PciDevice};
use crate::memory::frame_allocator;
use crate::task::completion::Completion;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{fence, AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};

/// Sector size exposed to the block layer. Namespaces formatted with any
/// other LBA size are rejected at init.
pub const SECTOR_SIZE: usize = 512;

/// Largest transfer per command: one memory page, one PRP entry.
const PAGE_SIZE: usize = 4096;
const SECTORS_PER_PAGE: usize = PAGE_SIZE / SECTOR_SIZE;

/// PCI class code for an NVM Express controller.
const PCI_SUBCLASS_NVM: u8 = 0x08;
const PCI_PROG_IF_NVME: u8 = 0x02;

/// Queue depths. Both queues fit in one page:
/// SQ entries are 64 bytes, CQ entries 16 bytes.
const ADMIN_QUEUE_DEPTH: u16 = 32;
const IO_QUEUE_DEPTH: u16 = 32;
const IO_QUEUE_ID: u16 = 1;

const IO_COMPLETION_TIMEOUT_NS: u64 = 5_000_000_000;
const EARLY_IO_COMPLETION_TIMEOUT_NS: u64 = 100_000_000_000;
/// Poll iterations per admin or polled I/O command. Each iteration performs
/// a port-0x80 write (about a microsecond), so this bounds a command at
/// roughly five seconds.
const POLL_ITERATIONS: u64 = 5_000_000;

const NO_STATUS: u32 = u32::MAX;

/// Maximum controllers the driver attaches.
const MAX_CONTROLLERS: usize = 8;

/// Controller register offsets (NVMe 1.4, section 3.1).
mod reg {
    pub const CAP: usize = 0x00;
    pub const VS: usize = 0x08;
    pub const INTMS: usize = 0x0C;
    pub const INTMC: usize = 0x10;
    pub const CC: usize = 0x14;
    pub const CSTS: usize = 0x1C;
    pub const AQA: usize = 0x24;
    pub const ASQ: usize = 0x28;
    pub const ACQ: usize = 0x30;
    pub const DOORBELL_BASE: usize = 0x1000;
}

/// Controller Configuration fields.
mod cc {
    pub const EN: u32 = 1 << 0;
    /// I/O submission queue entry size: 2^6 = 64 bytes.
    pub const IOSQES_64: u32 = 6 << 16;
    /// I/O completion queue entry size: 2^4 = 16 bytes.
    pub const IOCQES_16: u32 = 4 << 20;
}

/// Controller Status fields.
mod csts {
    pub const RDY: u32 = 1 << 0;
    pub const CFS: u32 = 1 << 1;
}

/// Admin command opcodes.
mod admin_op {
    pub const CREATE_IO_SQ: u8 = 0x01;
    pub const CREATE_IO_CQ: u8 = 0x05;
    pub const IDENTIFY: u8 = 0x06;
    pub const SET_FEATURES: u8 = 0x09;
}

/// NVM command set opcodes.
mod io_op {
    pub const FLUSH: u8 = 0x00;
    pub const WRITE: u8 = 0x01;
    pub const READ: u8 = 0x02;
}

/// Identify CNS values.
mod cns {
    pub const NAMESPACE: u32 = 0x00;
    pub const CONTROLLER: u32 = 0x01;
    pub const ACTIVE_NAMESPACES: u32 = 0x02;
}

const FEATURE_NUMBER_OF_QUEUES: u32 = 0x07;

/// One physically contiguous, zeroed page reachable through the physical
/// memory map. x86 DMA is cache-coherent, so the cacheable mapping is used.
#[derive(Clone, Copy)]
struct DmaPage {
    phys: u64,
    virt: u64,
}

impl DmaPage {
    fn allocate() -> Result<Self, &'static str> {
        let frame = frame_allocator::allocate_frame().ok_or("NVMe: out of DMA frames")?;
        let phys = frame.start_address().as_u64();
        let virt = phys + crate::memory::physical_memory_offset().as_u64();
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, PAGE_SIZE);
        }
        Ok(Self { phys, virt })
    }

    fn read_u32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.virt as usize + offset) as *const u32) }
    }

    fn read_u64(&self, offset: usize) -> u64 {
        unsafe { core::ptr::read_volatile((self.virt as usize + offset) as *const u64) }
    }

    fn read_u8(&self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.virt as usize + offset) as *const u8) }
    }
}

/// A 64-byte submission queue entry.
#[derive(Clone, Copy, Default)]
struct Command {
    opcode: u8,
    cid: u16,
    nsid: u32,
    prp1: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
}

impl Command {
    fn write_to(&self, slot: u64) {
        let dwords: [u32; 16] = [
            self.opcode as u32 | ((self.cid as u32) << 16),
            self.nsid,
            0,
            0,
            0,
            0,
            self.prp1 as u32,
            (self.prp1 >> 32) as u32,
            0,
            0,
            self.cdw10,
            self.cdw11,
            self.cdw12,
            0,
            0,
            0,
        ];
        let base = slot as *mut u32;
        for (i, dword) in dwords.iter().enumerate() {
            unsafe { core::ptr::write_volatile(base.add(i), *dword) };
        }
    }
}

/// Status field of a completion entry (DW3 bits 31:17), zero on success.
fn completion_status(dw3: u32) -> u32 {
    (dw3 >> 17) & 0x7FFF
}

/// Short delay for polling loops: a write to the POST diagnostic port.
#[inline]
fn poll_delay() {
    unsafe {
        x86_64::instructions::port::Port::<u8>::new(0x80).write(0);
    }
    core::hint::spin_loop();
}

/// Serializes the controller's single in-flight I/O command and its shared
/// data page. A command abandoned on timeout leaves the gate held and the
/// controller wedged, since the device may still DMA into the page.
struct RequestGate {
    locked: AtomicBool,
    wedged: AtomicBool,
    waiters: crate::task::waitqueue::WaitQueueHead,
}

struct RequestGuard<'a> {
    gate: &'a RequestGate,
    release_on_drop: bool,
}

impl RequestGate {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            wedged: AtomicBool::new(false),
            waiters: crate::task::waitqueue::WaitQueueHead::new(),
        }
    }

    fn lock(&self) -> Result<RequestGuard<'_>, &'static str> {
        loop {
            if self.wedged.load(Ordering::Acquire) {
                return Err("NVMe controller wedged after an abandoned command");
            }
            if self
                .locked
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(RequestGuard {
                    gate: self,
                    release_on_drop: true,
                });
            }
            if !request_gate_can_sleep() {
                return Err("NVMe command already in progress");
            }
            if self
                .waiters
                .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
                .is_none()
            {
                return Err("NVMe command already in progress");
            }
            if self.locked.load(Ordering::Acquire) && !self.wedged.load(Ordering::Acquire) {
                crate::task::waitqueue::schedule_current_wait();
            }
            self.waiters.finish_wait();
        }
    }
}

impl RequestGuard<'_> {
    fn wedge(mut self) {
        self.release_on_drop = false;
        self.gate.wedged.store(true, Ordering::Release);
        self.gate.waiters.wake_up();
    }
}

impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.gate.locked.store(false, Ordering::Release);
            self.gate.waiters.wake_up_one();
        }
    }
}

#[inline]
fn request_gate_can_sleep() -> bool {
    crate::task::scheduler::current_thread_id().is_some() && crate::per_cpu::preempt_count() > 0
}

/// An attached NVMe controller and its namespace.
pub struct NvmeController {
    /// Virtual address of BAR0.
    regs: usize,
    /// Distance between doorbell registers, in bytes.
    doorbell_stride: usize,
    bus: u8,
    device: u8,
    function: u8,

    io_sq: DmaPage,
    io_cq: DmaPage,
    data: DmaPage,
    /// Next free submission slot; written only by the gate holder.
    sq_tail: AtomicU16,
    /// Next completion slot to inspect and its expected phase tag; written
    /// only by the holder of `cq_claim`.
    cq_head: AtomicU16,
    cq_phase: AtomicBool,
    cq_claim: AtomicBool,

    /// Whether the I/O completion queue raises INTx on IRQ 10 or 11.
    irq_driven: bool,

    nsid: u32,
    /// Namespace size in 512-byte sectors.
    capacity: u64,
    /// Whether the controller has a volatile write cache to flush.
    volatile_write_cache: bool,

    request_gate: RequestGate,
    completion: Completion,
    next_token: AtomicU32,
    /// Token of the command in flight; 0 when none is armed.
    pending_token: AtomicU32,
    /// Command identifier the in-flight command was submitted with.
    pending_cid: AtomicU32,
    /// Status of the in-flight command once its completion was consumed.
    completed_status: AtomicU32,
    ops_completed: AtomicU64,
}

/// Polled admin queue pair, used only while the controller is initialized.
struct AdminQueue {
    sq: DmaPage,
    cq: DmaPage,
    sq_tail: u16,
    cq_head: u16,
    cq_phase: bool,
    next_cid: u16,
}

impl NvmeController {
    fn read32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.regs + offset) as *const u32) }
    }

    fn write32(&self, offset: usize, value: u32) {
        unsafe { core::ptr::write_volatile((self.regs + offset) as *mut u32, value) }
    }

    fn sq_doorbell(&self, qid: u16) -> usize {
        reg::DOORBELL_BASE + (2 * qid as usize) * self.doorbell_stride
    }

    fn cq_doorbell(&self, qid: u16) -> usize {
        reg::DOORBELL_BASE + (2 * qid as usize + 1) * self.doorbell_stride
    }

    /// Namespace size in 512-byte sectors.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Reset, configure and identify one controller.
    fn new(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        let bar = pci_dev.bars[0];
        if !bar.is_valid() || bar.is_io {
            return Err("BAR0 is not a memory BAR");
        }

        pci_dev.enable_memory_space();
        pci_dev.enable_bus_master();
        // The controller signals only through INTx here: firmware's MSI or
        // MSI-X setup, if any, is turned off so the pin is the one vector.
        if let Some(cap) = pci_dev.find_msix_capability() {
            pci_dev.disable_msix(cap);
        }
        pci_dev.disable_msi();

        let regs = crate::memory::map_mmio(bar.address, bar.size as usize)?;

        let irq_driven = matches!(pci_dev.interrupt_line, 10 | 11) && pci_dev.interrupt_pin != 0;
        if irq_driven {
            pci_dev.enable_intx();
        } else {
            pci_dev.disable_intx();
        }

        let mut ctrl = NvmeController {
            regs,
            doorbell_stride: 4,
            bus: pci_dev.bus,
            device: pci_dev.device,
            function: pci_dev.function,
            io_sq: DmaPage::allocate()?,
            io_cq: DmaPage::allocate()?,
            data: DmaPage::allocate()?,
            sq_tail: AtomicU16::new(0),
            cq_head: AtomicU16::new(0),
            cq_phase: AtomicBool::new(true),
            cq_claim: AtomicBool::new(false),
            irq_driven,
            nsid: 0,
            capacity: 0,
            volatile_write_cache: false,
            request_gate: RequestGate::new(),
            completion: Completion::new(),
            next_token: AtomicU32::new(0),
            pending_token: AtomicU32::new(0),
            pending_cid: AtomicU32::new(0),
            completed_status: AtomicU32::new(NO_STATUS),
            ops_completed: AtomicU64::new(0),
        };

        let cap = ctrl.read32(reg::CAP) as u64 | ((ctrl.read32(reg::CAP + 4) as u64) << 32);
        let version = ctrl.read32(reg::VS);
        let max_queue_entries = (cap & 0xFFFF) as u16 + 1;
        let timeout_units = ((cap >> 24) & 0xFF) + 1;
        let doorbell_stride_shift = ((cap >> 32) & 0xF) as usize;
        let mps_min = (cap >> 48) & 0xF;
        let nvm_command_set = (cap >> 37) & 0x1 != 0;
        if mps_min != 0 {
            return Err("controller does not support 4 KiB memory pages");
        }
        if !nvm_command_set {
            return Err("controller does not support the NVM command set");
        }
        if max_queue_entries < ADMIN_QUEUE_DEPTH.max(IO_QUEUE_DEPTH) {
            return Err("controller queues are too small");
        }
        ctrl.doorbell_stride = 4 << doorbell_stride_shift;
        let ready_polls = timeout_units * 500_000;

        log::info!(
            "NVMe {:02x}:{:02x}.{}: version {}.{}, max queue entries {}, IRQ {} ({})",
            ctrl.bus,
            ctrl.device,
            ctrl.function,
            version >> 16,
            (version >> 8) & 0xFF,
            max_queue_entries,
            pci_dev.interrupt_line,
            if irq_driven { "INTx" } else { "polled" }
        );

        // Firmware may have left the controller enabled with its own queues.
        if ctrl.read32(reg::CC) & cc::EN != 0 {
            ctrl.write32(reg::CC, ctrl.read32(reg::CC) & !cc::EN);
        }
        ctrl.wait_ready(false, ready_polls)?;

        // Mask the pin while the admin queue is in use; it is polled.
        ctrl.write32(reg::INTMS, 1);

        let mut admin = AdminQueue {
            sq: DmaPage::allocate()?,
            cq: DmaPage::allocate()?,
            sq_tail: 0,
            cq_head: 0,
            cq_phase: true,
            next_cid: 1,
        };
        let depth = (ADMIN_QUEUE_DEPTH - 1) as u32;
        ctrl.write32(reg::AQA, (depth << 16) | depth);
        ctrl.write32(reg::ASQ, admin.sq.phys as u32);
        ctrl.write32(reg::ASQ + 4, (admin.sq.phys >> 32) as u32);
        ctrl.write32(reg::ACQ, admin.cq.phys as u32);
        ctrl.write32(reg::ACQ + 4, (admin.cq.phys >> 32) as u32);
        ctrl.write32(reg::CC, cc::EN | cc::IOSQES_64 | cc::IOCQES_16);
        ctrl.wait_ready(true, ready_polls)?;

        // The data page is idle until the I/O queues exist; Identify uses it.
        let identify = ctrl.data;

        // Identify Controller: volatile write cache (byte 525, bit 0).
        ctrl.admin_command(
            &mut admin,
            Command {
                opcode: admin_op::IDENTIFY,
                prp1: identify.phys,
                cdw10: cns::CONTROLLER,
                ..Command::default()
            },
        )?;
        ctrl.volatile_write_cache = identify.read_u8(525) & 0x1 != 0;

        // The first active namespace becomes the block device.
        ctrl.admin_command(
            &mut admin,
            Command {
                opcode: admin_op::IDENTIFY,
                prp1: identify.phys,
                cdw10: cns::ACTIVE_NAMESPACES,
                ..Command::default()
            },
        )?;
        let nsid = identify.read_u32(0);
        if nsid == 0 {
            return Err("controller has no active namespace");
        }

        ctrl.admin_command(
            &mut admin,
            Command {
                opcode: admin_op::IDENTIFY,
                nsid,
                prp1: identify.phys,
                cdw10: cns::NAMESPACE,
                ..Command::default()
            },
        )?;
        let namespace_size = identify.read_u64(0);
        let format_index = (identify.read_u8(26) & 0xF) as usize;
        let lba_format = identify.read_u32(128 + 4 * format_index);
        let lba_shift = (lba_format >> 16) & 0xFF;
        let metadata_size = lba_format & 0xFFFF;
        if lba_shift != 9 || metadata_size != 0 {
            return Err("namespace is not formatted with 512-byte sectors");
        }
        ctrl.nsid = nsid;
        ctrl.capacity = namespace_size;

        // One I/O submission queue and one I/O completion queue.
        ctrl.admin_command(
            &mut admin,
            Command {
                opcode: admin_op::SET_FEATURES,
                cdw10: FEATURE_NUMBER_OF_QUEUES,
                cdw11: 0,
                ..Command::default()
            },
        )?;

        let io_depth = (IO_QUEUE_DEPTH - 1) as u32;
        // Physically contiguous; interrupts enabled on vector 0 when the pin
        // is dispatched.
        let cq_flags = 0x1 | if irq_driven { 0x2 } else { 0 };
        ctrl.admin_command(
            &mut admin,
            Command {
                opcode: admin_op::CREATE_IO_CQ,
                prp1: ctrl.io_cq.phys,
                cdw10: (io_depth << 16) | IO_QUEUE_ID as u32,
                cdw11: cq_flags,
                ..Command::default()
            },
        )?;
        ctrl.admin_command(
            &mut admin,
            Command {
                opcode: admin_op::CREATE_IO_SQ,
                prp1: ctrl.io_sq.phys,
                cdw10: (io_depth << 16) | IO_QUEUE_ID as u32,
                cdw11: ((IO_QUEUE_ID as u32) << 16) | 0x1,
                ..Command::default()
            },
        )?;

        if irq_driven {
            ctrl.write32(reg::INTMC, 1);
        }

        log::info!(
            "NVMe {:02x}:{:02x}.{}: namespace {} has {} sectors ({} MB), write cache {}",
            ctrl.bus,
            ctrl.device,
            ctrl.function,
            ctrl.nsid,
            ctrl.capacity,
            ctrl.capacity * SECTOR_SIZE as u64 / (1024 * 1024),
            if ctrl.volatile_write_cache {
                "present"
            } else {
                "absent"
            }
        );

        Ok(ctrl)
    }

    fn wait_ready(&self, ready: bool, polls: u64) -> Result<(), &'static str> {
        for _ in 0..polls {
            let status = self.read32(reg::CSTS);
            if status & csts::CFS != 0 {
                return Err("controller fatal status");
            }
            if (status & csts::RDY != 0) == ready {
                return Ok(());
            }
            poll_delay();
        }
        Err(if ready {
            "controller did not become ready"
        } else {
            "controller did not stop"
        })
    }

    /// Submit one admin command and poll for its completion.
    fn admin_command(&self, admin: &mut AdminQueue, mut cmd: Command) -> Result<(), &'static str> {
        cmd.cid = admin.next_cid;
        admin.next_cid = admin.next_cid.wrapping_add(1).max(1);
        cmd.write_to(admin.sq.virt + admin.sq_tail as u64 * 64);
        admin.sq_tail = (admin.sq_tail + 1) % ADMIN_QUEUE_DEPTH;
        fence(Ordering::SeqCst);
        self.write32(self.sq_doorbell(0), admin.sq_tail as u32);

        let entry = admin.cq.virt as usize + admin.cq_head as usize * 16;
        for _ in 0..POLL_ITERATIONS {
            let dw3 = unsafe { core::ptr::read_volatile((entry + 12) as *const u32) };
            if ((dw3 >> 16) & 1 != 0) == admin.cq_phase {
                fence(Ordering::SeqCst);
                admin.cq_head += 1;
                if admin.cq_head == ADMIN_QUEUE_DEPTH {
                    admin.cq_head = 0;
                    admin.cq_phase = !admin.cq_phase;
                }
                self.write32(self.cq_doorbell(0), admin.cq_head as u32);
                if dw3 & 0xFFFF != cmd.cid as u32 {
                    return Err("admin completion for an unexpected command");
                }
                if completion_status(dw3) != 0 {
                    log::warn!(
                        "NVMe {:02x}:{:02x}.{}: admin opcode {:#x} failed with status {:#x}",
                        self.bus,
                        self.device,
                        self.function,
                        cmd.opcode,
                        completion_status(dw3)
                    );
                    return Err("admin command failed");
                }
                return Ok(());
            }
            poll_delay();
        }
        Err("admin command timed out")
    }

    /// Consume every new I/O completion entry and ring the head doorbell.
    ///
    /// Called from the IRQ handler and from polling submitters; `cq_claim`
    /// keeps one consumer at a time. No logging, no allocation, no locks.
    fn drain_io_cq(&self) {
        if self
            .cq_claim
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let mut head = self.cq_head.load(Ordering::Relaxed);
        let mut phase = self.cq_phase.load(Ordering::Relaxed);
        let mut consumed = false;
        loop {
            let entry = self.io_cq.virt as usize + head as usize * 16;
            let dw3 = unsafe { core::ptr::read_volatile((entry + 12) as *const u32) };
            if ((dw3 >> 16) & 1 != 0) != phase {
                break;
            }
            fence(Ordering::Acquire);
            consumed = true;
            head += 1;
            if head == IO_QUEUE_DEPTH {
                head = 0;
                phase = !phase;
            }

            let token = self.pending_token.load(Ordering::Acquire);
            if token != 0 && dw3 & 0xFFFF == self.pending_cid.load(Ordering::Acquire) {
                self.completed_status
                    .store(completion_status(dw3), Ordering::Release);
                self.completion.complete(token);
            }
        }

        if consumed {
            self.cq_head.store(head, Ordering::Relaxed);
            self.cq_phase.store(phase, Ordering::Relaxed);
            self.write32(self.cq_doorbell(IO_QUEUE_ID), head as u32);
        }
        self.cq_claim.store(false, Ordering::Release);
    }

    fn next_completion_token(&self) -> u32 {
        loop {
            let token = self
                .next_token
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            if token != 0 {
                return token;
            }
        }
    }

    /// Whether the submitter can sleep until the IRQ handler completes the
    /// command. A scheduler thread can, even with interrupts masked (a
    /// syscall): the completion wait parks it and interrupts are serviced
    /// while it sleeps, as for VirtIO block. Busy-polling there instead
    /// would hold the only CPU with interrupts off for the whole command.
    fn irq_completion_available(&self) -> bool {
        self.irq_driven
            && (crate::task::scheduler::current_thread_id().is_some()
                || x86_64::instructions::interrupts::are_enabled())
    }

    /// Submit one I/O command and wait for it. The caller holds the gate and
    /// has staged any write data in the data page.
    fn execute_io<'a>(
        &'a self,
        guard: RequestGuard<'a>,
        mut cmd: Command,
    ) -> Result<RequestGuard<'a>, &'static str> {
        let token = self.next_completion_token();
        cmd.cid = token as u16;
        self.completion.reset();
        self.completed_status.store(NO_STATUS, Ordering::Release);
        self.pending_cid.store(cmd.cid as u32, Ordering::Release);
        self.pending_token.store(token, Ordering::Release);

        let tail = self.sq_tail.load(Ordering::Relaxed);
        cmd.write_to(self.io_sq.virt + tail as u64 * 64);
        let tail = (tail + 1) % IO_QUEUE_DEPTH;
        self.sq_tail.store(tail, Ordering::Relaxed);
        fence(Ordering::SeqCst);
        self.write32(self.sq_doorbell(IO_QUEUE_ID), tail as u32);

        let completed = if self.irq_completion_available() {
            let timeout_ns = if crate::task::scheduler::current_thread_id().is_some() {
                IO_COMPLETION_TIMEOUT_NS
            } else {
                EARLY_IO_COMPLETION_TIMEOUT_NS
            };
            // The command is already published to the device, so abandoning
            // this wait would leave the data page live for DMA.
            matches!(
                self.completion
                    .wait_timeout_uninterruptible(token, timeout_ns),
                Ok(true)
            )
        } else {
            let mut done = false;
            for _ in 0..POLL_ITERATIONS {
                self.drain_io_cq();
                if self.completed_status.load(Ordering::Acquire) != NO_STATUS {
                    done = true;
                    break;
                }
                poll_delay();
            }
            done
        };

        self.pending_token.store(0, Ordering::Release);
        if !completed {
            guard.wedge();
            return Err("NVMe command timed out");
        }
        let status = self.completed_status.swap(NO_STATUS, Ordering::AcqRel);
        if status != 0 {
            return Err("NVMe command failed");
        }
        self.ops_completed.fetch_add(1, Ordering::Relaxed);
        Ok(guard)
    }

    fn check_range(&self, sector: u64, count: usize) -> Result<(), &'static str> {
        if count == 0 || count > SECTORS_PER_PAGE {
            return Err("Invalid sector count");
        }
        let end = sector
            .checked_add(count as u64)
            .ok_or("Sector out of range")?;
        if end > self.capacity {
            return Err("Sector out of range");
        }
        Ok(())
    }

    /// Read up to eight consecutive sectors into `buffer`.
    pub fn read_sectors(&self, sector: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        if buffer.is_empty() || buffer.len() % SECTOR_SIZE != 0 {
            return Err("Buffer size must be a non-zero multiple of 512");
        }
        let count = buffer.len() / SECTOR_SIZE;
        self.check_range(sector, count)?;

        let guard = self.request_gate.lock()?;
        let guard = self.execute_io(
            guard,
            Command {
                opcode: io_op::READ,
                nsid: self.nsid,
                prp1: self.data.phys,
                cdw10: sector as u32,
                cdw11: (sector >> 32) as u32,
                cdw12: (count - 1) as u32,
                ..Command::default()
            },
        )?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.data.virt as *const u8,
                buffer.as_mut_ptr(),
                buffer.len(),
            );
        }
        drop(guard);
        Ok(())
    }

    /// Write up to eight consecutive sectors from `buffer`.
    pub fn write_sectors(&self, sector: u64, buffer: &[u8]) -> Result<(), &'static str> {
        if buffer.is_empty() || buffer.len() % SECTOR_SIZE != 0 {
            return Err("Buffer size must be a non-zero multiple of 512");
        }
        let count = buffer.len() / SECTOR_SIZE;
        self.check_range(sector, count)?;

        let guard = self.request_gate.lock()?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                buffer.as_ptr(),
                self.data.virt as *mut u8,
                buffer.len(),
            );
        }
        let guard = self.execute_io(
            guard,
            Command {
                opcode: io_op::WRITE,
                nsid: self.nsid,
                prp1: self.data.phys,
                cdw10: sector as u32,
                cdw11: (sector >> 32) as u32,
                cdw12: (count - 1) as u32,
                ..Command::default()
            },
        )?;
        drop(guard);
        Ok(())
    }

    /// Commit the volatile write cache, when the controller has one.
    pub fn flush(&self) -> Result<(), &'static str> {
        if !self.volatile_write_cache {
            return Ok(());
        }
        let guard = self.request_gate.lock()?;
        let guard = self.execute_io(
            guard,
            Command {
                opcode: io_op::FLUSH,
                nsid: self.nsid,
                ..Command::default()
            },
        )?;
        drop(guard);
        Ok(())
    }
}

/// Attached controllers in PCI order, published once by `init()` and read
/// without locking by the IRQ handler.
static CONTROLLERS: spin::Once<Vec<Arc<NvmeController>>> = spin::Once::new();

fn is_nvme(dev: &PciDevice) -> bool {
    dev.class == pci::DeviceClass::MassStorage
        && dev.subclass == PCI_SUBCLASS_NVM
        && dev.prog_if == PCI_PROG_IF_NVME
}

/// Attach every NVMe controller on the PCI bus. Returns the number attached.
pub fn init() -> Result<usize, &'static str> {
    let candidates: Vec<PciDevice> = pci::get_devices()
        .unwrap_or_default()
        .into_iter()
        .filter(is_nvme)
        .collect();
    if candidates.is_empty() {
        CONTROLLERS.call_once(Vec::new);
        return Err("No NVMe controllers found");
    }

    let mut attached = Vec::new();
    for dev in candidates.iter().take(MAX_CONTROLLERS) {
        match NvmeController::new(dev) {
            Ok(ctrl) => attached.push(Arc::new(ctrl)),
            Err(e) => log::warn!(
                "NVMe {:02x}:{:02x}.{}: not attached: {}",
                dev.bus,
                dev.device,
                dev.function,
                e
            ),
        }
    }
    let count = attached.len();
    CONTROLLERS.call_once(|| attached);
    log::info!("NVMe: Driver initialized with {} controller(s)", count);
    if count == 0 {
        return Err("No NVMe controller could be attached");
    }
    Ok(count)
}

/// Number of attached controllers.
pub fn controller_count() -> usize {
    CONTROLLERS.get().map_or(0, |list| list.len())
}

/// The controller at `index`, in PCI order.
pub fn controller(index: usize) -> Option<Arc<NvmeController>> {
    CONTROLLERS.get()?.get(index).cloned()
}

/// IRQ 10/11 dispatch: drain every INTx-driven controller's I/O completion
/// queue. No logging, no allocation, no locks.
pub fn handle_interrupt() {
    let Some(list) = CONTROLLERS.get() else {
        return;
    };
    for ctrl in list.iter() {
        if ctrl.irq_driven {
            ctrl.drain_io_cq();
        }
    }
}

/// Read sector 0 of the first controller's namespace.
pub fn test_read() -> Result<(), &'static str> {
    let ctrl = controller(0).ok_or("No NVMe controller attached")?;
    let mut buffer = [0u8; SECTOR_SIZE];
    ctrl.read_sectors(0, &mut buffer)?;
    log::info!("NVMe test: Read successful!");
    log::info!("  First 16 bytes: {:02x?}", &buffer[..16]);
    Ok(())
}
