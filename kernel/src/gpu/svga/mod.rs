//! Experimental SVGA3 command buffers and guest-backed objects for ARM64 Fusion.
//!
//! Wire definitions and their unresolved source disposition are recorded in
//! docs/gui/provenance/gpu-core.md (#1913). Allocation and lifetime policy is Breenix's.

pub mod wire;
pub mod commands;
pub mod memory;
pub mod objects;

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::drivers::pci;
use crate::gpu::dma::{DmaBox, Pages, PAGE};
use commands::Commands;
use memory::PageTree;

/// A command buffer and its payload, retained until its terminal status is observed.
pub struct Submission {
    header: DmaBox,
    payload: DmaBox,
    id: u64,
    pub context: Option<u32>,
    // Objects a submitted command may access, retained independently of caller lifetime.
    pub holds: Vec<Arc<PageTree>>,
}

#[derive(Clone, Copy, Debug)]
pub struct Completion {
    pub id: u64,
    pub status: u32,
    pub error_offset: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum Error {
    NoDevice,
    Missing(u32),
    Refused(Completion),
    Timeout,
    InvalidCommand,
    NoMemory,
    DeviceFault,
}

impl Submission {
    /// A physically contiguous command stream; command buffers are control data, while
    /// resource and MOB data use frame-allocated Pages. Large shader programs use MOBs.
    pub fn new(id: u64, context: Option<u32>, words: &[u32]) -> Result<Self, Error> {
        // Keep control packets within one page, including device-context commands.
        if words.is_empty() || words.len() > PAGE / 4 { return Err(Error::InvalidCommand); }
        let mut payload = DmaBox::new(words.len() * 4);
        for (i, &word) in words.iter().enumerate() { payload.put_u32(i * 4, word); }
        let mut header = DmaBox::new(wire::CB_HEADER);
        header.put_u64(8, id);
        header.put_u32(16, wire::CB_NO_IRQ | if context.is_some() { wire::CB_DX_CONTEXT } else { 0 });
        header.put_u32(20, payload.len() as u32);
        header.put_u64(24, payload.addr());
        header.put_u32(36, context.unwrap_or(0));
        Ok(Self { header, payload, id, context, holds: Vec::new() })
    }
    pub fn poll(&self) -> Result<Option<Completion>, Error> {
        self.header.observe();
        let status = self.header.u32_at(0);
        if self.header.u64_at(8) != self.id || status > wire::status::PARTIAL_COMPLETE {
            return Err(Error::DeviceFault);
        }
        if status == wire::status::PENDING { return Ok(None); }
        Ok(Some(Completion { id: self.id, status, error_offset: self.header.u32_at(4) }))
    }
    fn publish(&self) {
        self.payload.publish();
        self.header.publish();
    }
}

/// Capability values read through SVGA_REG_DEV_CAP (gpu.md section 1.6.3, admission).
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub dx: u32,
    pub bgra: u32,
    pub rgba: u32,
    pub r8: u32,
    pub width: u32,
    pub height: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub contexts: u32,
    pub surfaces: u32,
}

impl Profile {
    pub fn missing(&self) -> Option<u32> {
        use gpu_abi::capability as abi;
        use wire::cap::*;
        if self.dx == 0 { Some(abi::FEATURE_3D) }
        else if self.bgra & (SUPPORTED | RENDER) != SUPPORTED | RENDER { Some(abi::BGRA_RENDER) }
        else if self.bgra & (SUPPORTED | SAMPLE) != SUPPORTED | SAMPLE { Some(abi::BGRA_SAMPLE) }
        else if self.rgba & (SUPPORTED | RENDER) != SUPPORTED | RENDER { Some(abi::RGBA_RENDER) }
        else if self.rgba & (SUPPORTED | SAMPLE) != SUPPORTED | SAMPLE { Some(abi::RGBA_SAMPLE) }
        else if self.r8 & (SUPPORTED | SAMPLE) != SUPPORTED | SAMPLE { Some(abi::R8_SAMPLE) }
        else if self.width < 2560 || self.height < 1600 || self.screen_width < 2560 || self.screen_height < 1600 { Some(abi::TARGET_SIZE) }
        else { None }
    }
}

/// One command queue, its outstanding requests, and device-owned global object tables.
/// The caller serializes register accesses; DEV_CAP and COMMAND_HIGH/LOW are paired writes.
pub struct Device {
    registers: u64,
    pub profile: Profile,
    pending: VecDeque<Submission>,
    tables: Vec<PageTree>,
    next_id: u64,
    stopped: bool,
}

impl Device {
    pub fn probe() -> Result<Self, Error> {
        if !crate::platform_config::is_vmware() { return Err(Error::NoDevice); }
        let pci = pci::find_device(wire::VENDOR, wire::DEVICE).ok_or(Error::NoDevice)?;
        let bar = pci.bars[0];
        if bar.is_io || bar.address == 0 || bar.size < 4 * (wire::reg::IRQ_STATUS as u64 + 1) { return Err(Error::DeviceFault); }
        pci.enable_memory_space();
        pci.enable_bus_master();
        let registers = crate::memory::physical_memory_offset().as_u64() + bar.address;
        let mut dev = Self {
            registers,
            profile: Profile { dx: 0, bgra: 0, rgba: 0, r8: 0, width: 0, height: 0, screen_width: 0, screen_height: 0, contexts: 0, surfaces: 0 },
            pending: VecDeque::new(), tables: Vec::new(), next_id: 1, stopped: true,
        };
        dev.write(wire::reg::ID, wire::ID_3);
        if dev.read(wire::reg::ID) != wire::ID_3 { return Err(Error::DeviceFault); }
        let caps = dev.read(wire::reg::CAPABILITIES);
        if caps & wire::COMMAND_BUFFERS == 0 { return Err(Error::Missing(gpu_abi::capability::CAPABILITY_SET)); }
        if caps & wire::GB_OBJECTS == 0 { return Err(Error::Missing(gpu_abi::capability::CAPABILITY_SET)); }
        use wire::cap;
        dev.profile = Profile {
            dx: dev.cap(cap::DX_CONTEXT), bgra: dev.cap(cap::BGRA), rgba: dev.cap(cap::RGBA), r8: dev.cap(cap::R8),
            width: dev.cap(cap::MAX_TEXTURE_WIDTH).min(16384), height: dev.cap(cap::MAX_TEXTURE_HEIGHT).min(16384),
            screen_width: dev.read(wire::reg::SCREEN_MAX_WIDTH), screen_height: dev.read(wire::reg::SCREEN_MAX_HEIGHT),
            contexts: dev.cap(cap::MAX_CONTEXTS), surfaces: dev.cap(cap::MAX_SURFACES),
        };
        if let Some(reason) = dev.profile.missing() { return Err(Error::Missing(reason)); }
        dev.write(wire::reg::IRQMASK, 0);
        dev.write(wire::reg::ENABLE, 1);
        dev.write(wire::reg::CONFIG_DONE, 1);
        dev.stopped = false;
        // Device context commands are synchronous register-like operations (svga_reg.h).
        if let Err(e) = dev.device_command(&[1, 1, 0]).and_then(|_| dev.init_tables()) {
            // Initialization may have exposed a table to DMA. Failure is not evidence
            // that access ended, so retain it rather than freeing an ambiguous allocation.
            core::mem::forget(dev);
            return Err(e);
        }
        Ok(dev)
    }

    fn read(&self, reg: u32) -> u32 {
        // SAFETY: BAR0 is mapped device memory, and reg is a published register index.
        unsafe { core::ptr::read_volatile((self.registers + 4 * reg as u64) as *const u32) }
    }
    fn write(&self, reg: u32, value: u32) {
        // SAFETY: as in read; accesses to paired registers are serialized by the owner.
        unsafe { core::ptr::write_volatile((self.registers + 4 * reg as u64) as *mut u32, value); }
    }
    fn cap(&self, index: u32) -> u32 {
        self.write(wire::reg::DEV_CAP, index);
        self.read(wire::reg::DEV_CAP)
    }
    fn issue(&self, submission: &Submission, queue: u32) {
        submission.publish();
        let address = submission.header.addr();
        assert_eq!(address & 63, 0, "SVGA command header alignment");
        assert!(queue <= 63, "SVGA queue selector");
        self.write(wire::reg::COMMAND_HIGH, (address >> 32) as u32);
        self.write(wire::reg::COMMAND_LOW, address as u32 | queue);
    }
    fn take_id(&mut self) -> u64 { let id = self.next_id; self.next_id += 1; id }

    fn device_command(&mut self, words: &[u32]) -> Result<(), Error> {
        let submission = Submission::new(self.take_id(), None, words)?;
        self.issue(&submission, wire::CB_DEVICE);
        match submission.poll() {
            Ok(Some(c)) if c.status == wire::status::COMPLETED => Ok(()),
            Ok(Some(c)) => Err(Error::Refused(c)),
            Ok(None) | Err(_) => { core::mem::forget(submission); Err(Error::DeviceFault) }
        }
    }

    fn init_tables(&mut self) -> Result<(), Error> {
        // Initial capacities cover one process's quota plus kernel scratch; resource
        // identifiers must be allocated within these capacities by the DX backend.
        // A surface needs its own MOB, as do context state, shaders and cotables.
        let contexts = self.profile.contexts.min(256) as usize;
        let surfaces = self.profile.surfaces.min(32 * 1024) as usize;
        if contexts == 0 || surfaces < 256 { return Err(Error::DeviceFault); }
        // Per DX context: state (1), cotables (7), catalogue shaders (17*2), and
        // vertex, constant and fence MOBs (3); one MOB per surface.
        let capacities = [surfaces + contexts * (1 + 7 + 17 * 2 + 3), surfaces, 1, 1, 1, contexts];
        let widths = [wire::entry::MOB, wire::entry::SURFACE, wire::entry::CONTEXT,
            wire::entry::SHADER, wire::entry::SCREEN, wire::entry::DX_CONTEXT];
        for (kind, (&capacity, &width)) in capacities.iter().zip(widths.iter()).enumerate() {
            let bytes = capacity * width;
            let data = Arc::new(Pages::new(bytes.div_ceil(PAGE)).ok_or(Error::NoMemory)?);
            let tree = PageTree::new(data).ok_or(Error::NoMemory)?;
            let mut commands = Commands::new();
            commands.table(kind as u32, &tree, bytes as u32);
            // Publish ownership before issuing the table definition.
            self.tables.push(tree);
            self.exec_init(None, commands.words())?;
        }
        Ok(())
    }

    /// Initialization only. Runtime waits belong to the GPU worker and its watchdog.
    fn exec_init(&mut self, context: Option<u32>, words: &[u32]) -> Result<Completion, Error> {
        if !self.pending.is_empty() { return Err(Error::InvalidCommand); }
        let submission = Submission::new(self.take_id(), context, words)?;
        self.send(submission).map_err(|_| Error::DeviceFault)?;
        let start = crate::signal::monotonic_nanos();
        loop {
            if let Some(c) = self.reap()? {
                if c.status == wire::status::COMPLETED { return Ok(c); }
                return Err(Error::Refused(c));
            }
            if crate::signal::monotonic_nanos().saturating_sub(start) >= 2_000_000_000 { return Err(Error::Timeout); }
            core::hint::spin_loop();
        }
    }

    pub fn send(&mut self, submission: Submission) -> Result<(), Submission> {
        if self.stopped || self.pending.len() >= wire::QUEUE_CAPACITY { return Err(submission); }
        self.issue(&submission, 0);
        self.pending.push_back(submission);
        Ok(())
    }
    pub fn reap(&mut self) -> Result<Option<Completion>, Error> {
        let Some(front) = self.pending.front() else { return Ok(None); };
        let Some(completion) = front.poll()? else { return Ok(None); };
        self.pending.pop_front();
        Ok(Some(completion))
    }
    pub fn outstanding(&self) -> usize { self.pending.len() }
}

impl Drop for Device {
    fn drop(&mut self) {
        if !self.stopped {
            // A dropped owner is not a hardware reset. Retain DMA-visible allocations.
            for table in self.tables.drain(..) { core::mem::forget(table); }
            for submission in self.pending.drain(..) { core::mem::forget(submission); }
        }
    }
}
