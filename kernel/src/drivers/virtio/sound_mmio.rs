//! VirtIO Sound Device Driver for ARM64 (MMIO Transport)
//!
//! Implements a basic audio playback driver using VirtIO MMIO transport.
//! Provides PCM audio output at 44100 Hz, S16_LE, stereo.

use super::mmio::{
    device_id, VirtioMmioDevice, VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_SIZE,
};
use crate::arch_impl::aarch64::cpu::{dsb_sy, Aarch64Cpu};
use crate::arch_impl::aarch64::gic;
use crate::arch_impl::traits::{CpuOps, InterruptController};
use crate::task::completion::Completion;
use crate::task::waitqueue::WaitQueueHead;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, AtomicU32, Ordering};

const VIRTIO_IRQ_BASE: u32 = 48;
const SOUND_MMIO_COMPLETION_TIMEOUT_NS: u64 = 5_000_000_000;
const NO_COMPLETED_DESC: u32 = u32::MAX;

struct SoundMmioRequestGate {
    locked: AtomicBool,
    waiters: WaitQueueHead,
}

struct SoundMmioRequestGuard<'a> {
    gate: &'a SoundMmioRequestGate,
    release_on_drop: bool,
}

impl SoundMmioRequestGate {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiters: WaitQueueHead::new(),
        }
    }

    fn lock(&self) -> Result<SoundMmioRequestGuard<'_>, &'static str> {
        loop {
            if self
                .locked
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(SoundMmioRequestGuard {
                    gate: self,
                    release_on_drop: true,
                });
            }

            if !sound_mmio_request_gate_can_sleep() {
                return Err("Sound MMIO request already in progress");
            }

            if self
                .waiters
                .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
                .is_none()
            {
                return Err("Sound MMIO request already in progress");
            }

            if self.locked.load(Ordering::Acquire) {
                crate::task::waitqueue::schedule_current_wait();
            }
            self.waiters.finish_wait();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        self.waiters.wake_up_one();
    }
}

impl SoundMmioRequestGuard<'_> {
    fn keep_locked(&mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for SoundMmioRequestGuard<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.gate.unlock();
        }
    }
}

#[inline]
fn sound_mmio_request_gate_can_sleep() -> bool {
    crate::task::scheduler::current_thread_id().is_some()
        && crate::per_cpu_aarch64::preempt_count() > 0
        && crate::arch_impl::aarch64::timer_interrupt::is_initialized()
}

struct SoundMmioQueueCompletion {
    completion: Completion,
    next_token: AtomicU32,
    pending_token: AtomicU32,
    completed_desc: AtomicU32,
    last_used_idx: AtomicU32,
}

impl SoundMmioQueueCompletion {
    const fn new() -> Self {
        Self {
            completion: Completion::new(),
            next_token: AtomicU32::new(0),
            pending_token: AtomicU32::new(0),
            completed_desc: AtomicU32::new(NO_COMPLETED_DESC),
            last_used_idx: AtomicU32::new(0),
        }
    }

    fn next_completion_token(&self) -> u32 {
        let mut token = self
            .next_token
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        if token == 0 {
            token = self
                .next_token
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
        }
        token
    }

    fn prepare_wait(&self) -> u32 {
        let token = self.next_completion_token();
        self.completion.reset();
        self.completed_desc
            .store(NO_COMPLETED_DESC, Ordering::Release);
        self.pending_token.store(token, Ordering::Release);
        token
    }

    fn clear(&self) {
        self.pending_token.store(0, Ordering::Release);
        self.completed_desc
            .store(NO_COMPLETED_DESC, Ordering::Release);
    }

    fn wait_for_completion(
        &self,
        token: u32,
        timeout_error: &'static str,
        interrupted_error: &'static str,
    ) -> Result<(), &'static str> {
        match self
            .completion
            .wait_timeout(token, SOUND_MMIO_COMPLETION_TIMEOUT_NS)
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(timeout_error),
            Err(_eintr) => Err(interrupted_error),
        }
    }

    fn take_completed_desc(&self, missing_error: &'static str) -> Result<u16, &'static str> {
        let desc = self
            .completed_desc
            .swap(NO_COMPLETED_DESC, Ordering::AcqRel);
        if desc == NO_COMPLETED_DESC {
            return Err(missing_error);
        }
        Ok(desc as u16)
    }

    fn has_pending(&self) -> bool {
        self.pending_token.load(Ordering::Acquire) != 0
    }

    fn complete_desc(&self, completed_desc: u16) -> bool {
        let token = self.pending_token.load(Ordering::Acquire);
        if token == 0 {
            return false;
        }

        self.completed_desc
            .store(completed_desc as u32, Ordering::Release);
        self.completion.complete(token);
        true
    }
}

static CTRL_GATE: SoundMmioRequestGate = SoundMmioRequestGate::new();
static TX_GATE: SoundMmioRequestGate = SoundMmioRequestGate::new();
static CTRL_COMPLETION: SoundMmioQueueCompletion = SoundMmioQueueCompletion::new();
static TX_COMPLETION: SoundMmioQueueCompletion = SoundMmioQueueCompletion::new();

/// VirtIO Sound command codes
mod cmd {
    pub const SET_PARAMS: u32 = 0x0101;
    pub const PREPARE: u32 = 0x0102;
    pub const START: u32 = 0x0104;
}

/// VirtIO Sound response status
mod resp {
    pub const OK: u32 = 0x8000;
}

/// VirtIO Sound PCM formats (from virtio_snd.h)
mod pcm_format {
    pub const S16: u8 = 5; // VIRTIO_SND_PCM_FMT_S16
}

/// VirtIO Sound PCM rates (from virtio_snd.h)
mod pcm_rate {
    pub const RATE_44100: u8 = 6; // VIRTIO_SND_PCM_RATE_44100
}

/// Control header for sound commands
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndHdr {
    code: u32,
}

/// PCM set params request
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmSetParams {
    hdr: VirtioSndHdr,
    stream_id: u32,
    buffer_bytes: u32,
    period_bytes: u32,
    features: u32,
    channels: u8,
    format: u8,
    rate: u8,
    _padding: u8,
}

/// PCM stream control (prepare/start/stop)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmCtrl {
    hdr: VirtioSndHdr,
    stream_id: u32,
}

/// PCM xfer header (for TX queue)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmXfer {
    stream_id: u32,
}

/// PCM xfer status (device writes back)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmStatus {
    status: u32,
    latency_bytes: u32,
}

// ---- Virtqueue structures (same as gpu_mmio) ----

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; 16],
}

// ---- Static queue memory ----

#[repr(C, align(4096))]
struct QueueMemory {
    desc: [VirtqDesc; 16],
    avail: VirtqAvail,
    _padding: [u8; 4096 - 256 - 36],
    used: VirtqUsed,
}

// Control queue (queue 0)
static mut CTRL_QUEUE: QueueMemory = QueueMemory {
    desc: [VirtqDesc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; 16],
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
};

// TX queue (queue 2)
static mut TX_QUEUE: QueueMemory = QueueMemory {
    desc: [VirtqDesc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; 16],
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
};

// Command/response buffers
#[repr(C, align(64))]
struct CmdBuffer {
    data: [u8; 256],
}

static mut CMD_BUF: CmdBuffer = CmdBuffer { data: [0; 256] };
static mut RESP_BUF: CmdBuffer = CmdBuffer { data: [0; 256] };

// TX path buffers
static mut TX_XFER: VirtioSndPcmXfer = VirtioSndPcmXfer { stream_id: 0 };
static mut TX_STATUS: VirtioSndPcmStatus = VirtioSndPcmStatus {
    status: 0,
    latency_bytes: 0,
};

// PCM data buffer (16KB)
const PCM_BUF_SIZE: usize = 16384;
#[repr(C, align(64))]
struct PcmBuffer {
    data: [u8; PCM_BUF_SIZE],
}
static mut PCM_BUF: PcmBuffer = PcmBuffer {
    data: [0; PCM_BUF_SIZE],
};

/// Sound device state
static mut SOUND_DEVICE: Option<SoundDeviceState> = None;

struct SoundDeviceState {
    base: u64,
    slot: usize,
    stream_started: bool,
}

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    addr - crate::memory::physical_memory_offset().as_u64()
}

/// Initialize the VirtIO Sound device
pub fn init() -> Result<(), &'static str> {
    unsafe {
        let ptr = &raw const SOUND_DEVICE;
        if (*ptr).is_some() {
            return Ok(());
        }
    }

    crate::serial_println!("[virtio-sound] Searching for sound device...");

    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;
        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            if device.device_id() == device_id::SOUND {
                crate::serial_println!("[virtio-sound] Found sound device at {:#x}", base);
                return init_device(&mut device, base, i);
            }
        }
    }

    Err("No VirtIO Sound device found")
}

fn init_device(device: &mut VirtioMmioDevice, base: u64, slot: usize) -> Result<(), &'static str> {
    let version = device.version();

    if version == 1 {
        device.set_guest_page_size(4096);
    }

    // Initialize the device (no special features needed)
    device.init(0)?;

    // Set up control queue (queue 0)
    setup_queue(device, 0, version, &raw mut CTRL_QUEUE)?;

    // Set up TX queue (queue 2)
    setup_queue(device, 2, version, &raw mut TX_QUEUE)?;

    // Mark device ready
    device.driver_ok();

    CTRL_COMPLETION.last_used_idx.store(0, Ordering::Release);
    TX_COMPLETION.last_used_idx.store(0, Ordering::Release);

    unsafe {
        let ptr = &raw mut SOUND_DEVICE;
        *ptr = Some(SoundDeviceState {
            base,
            slot,
            stream_started: false,
        });
    }

    let irq = VIRTIO_IRQ_BASE + slot as u32;
    gic::Gicv2::enable_irq(irq as u8);
    crate::serial_println!("[virtio-sound] Sound MMIO IRQ {} enabled", irq);

    crate::serial_println!("[virtio-sound] Sound device initialized");
    Ok(())
}

fn setup_queue(
    device: &mut VirtioMmioDevice,
    queue_idx: u32,
    version: u32,
    queue: *mut QueueMemory,
) -> Result<(), &'static str> {
    device.select_queue(queue_idx);
    let queue_num_max = device.get_queue_num_max();

    if queue_num_max == 0 {
        return Err("Queue size is 0");
    }

    let queue_size = core::cmp::min(queue_num_max, 16);
    device.set_queue_num(queue_size);

    let queue_phys = virt_to_phys(queue as u64);

    unsafe {
        for i in 0..15 {
            (*queue).desc[i].next = (i + 1) as u16;
        }
        (*queue).desc[15].next = 0;
        (*queue).avail.flags = 0;
        (*queue).avail.idx = 0;
        (*queue).used.flags = 0;
        (*queue).used.idx = 0;
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

fn sound_device_state() -> Result<&'static SoundDeviceState, &'static str> {
    unsafe {
        let ptr = &raw const SOUND_DEVICE;
        (*ptr).as_ref().ok_or("Sound device not initialized")
    }
}

fn sound_device_state_mut() -> Result<&'static mut SoundDeviceState, &'static str> {
    unsafe {
        let ptr = &raw mut SOUND_DEVICE;
        (*ptr).as_mut().ok_or("Sound device not initialized")
    }
}

#[inline]
fn irq_completion_available() -> bool {
    crate::task::scheduler::current_thread_id().is_some() || Aarch64Cpu::interrupts_enabled()
}

fn send_ctrl_command(
    device: &VirtioMmioDevice,
    request_guard: &mut SoundMmioRequestGuard<'_>,
    cmd_phys: u64,
    cmd_len: u32,
    resp_phys: u64,
    resp_len: u32,
) -> Result<(), &'static str> {
    let completion_token = CTRL_COMPLETION.prepare_wait();

    unsafe {
        let queue_ptr = &raw mut CTRL_QUEUE;

        // Descriptor 0: command (device reads)
        (*queue_ptr).desc[0] = VirtqDesc {
            addr: cmd_phys,
            len: cmd_len,
            flags: DESC_F_NEXT,
            next: 1,
        };

        // Descriptor 1: response (device writes)
        (*queue_ptr).desc[1] = VirtqDesc {
            addr: resp_phys,
            len: resp_len,
            flags: DESC_F_WRITE,
            next: 0,
        };

        // Add to available ring
        let avail_idx = (*queue_ptr).avail.idx;
        (*queue_ptr).avail.ring[(avail_idx % 16) as usize] = 0;
        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify device (queue 0 = controlq)
    dsb_sy();
    device.notify_queue(0);

    if let Err(e) = CTRL_COMPLETION.wait_for_completion(
        completion_token,
        "Sound MMIO control command timeout",
        "Sound MMIO control command interrupted",
    ) {
        request_guard.keep_locked();
        return Err(e);
    }

    let completed_desc = match CTRL_COMPLETION
        .take_completed_desc("Sound MMIO control command woke without completion")
    {
        Ok(desc) => desc,
        Err(e) => {
            CTRL_COMPLETION.clear();
            return Err(e);
        }
    };

    if completed_desc != 0 {
        CTRL_COMPLETION.clear();
        return Err("Sound MMIO control completed unexpected descriptor");
    }

    dsb_sy();
    fence(Ordering::SeqCst);

    CTRL_COMPLETION.clear();
    Ok(())
}

/// Set up the PCM stream for playback (S16_LE, 44100 Hz, stereo)
pub fn setup_stream() -> Result<(), &'static str> {
    if !irq_completion_available() {
        return Err("Sound MMIO IRQ completion unavailable before interrupts are enabled");
    }

    let mut request_guard = CTRL_GATE.lock()?;

    let base = {
        let state = sound_device_state()?;
        if state.stream_started {
            return Ok(());
        }
        state.base
    };
    let device = VirtioMmioDevice::probe(base).ok_or("Device disappeared")?;

    let cmd_phys = virt_to_phys(&raw const CMD_BUF as u64);
    let resp_phys = virt_to_phys(&raw const RESP_BUF as u64);

    // 1. SET_PARAMS
    unsafe {
        let cmd_ptr = &raw mut CMD_BUF;
        let params = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioSndPcmSetParams);
        *params = VirtioSndPcmSetParams {
            hdr: VirtioSndHdr {
                code: cmd::SET_PARAMS,
            },
            stream_id: 0,
            buffer_bytes: 32768,
            period_bytes: 16384,
            features: 0,
            channels: 2,
            format: pcm_format::S16,
            rate: pcm_rate::RATE_44100,
            _padding: 0,
        };
    }
    send_ctrl_command(
        &device,
        &mut request_guard,
        cmd_phys,
        core::mem::size_of::<VirtioSndPcmSetParams>() as u32,
        resp_phys,
        core::mem::size_of::<VirtioSndHdr>() as u32,
    )?;
    check_response("SET_PARAMS")?;

    // 2. PREPARE
    unsafe {
        let cmd_ptr = &raw mut CMD_BUF;
        let ctrl = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioSndPcmCtrl);
        *ctrl = VirtioSndPcmCtrl {
            hdr: VirtioSndHdr { code: cmd::PREPARE },
            stream_id: 0,
        };
    }
    send_ctrl_command(
        &device,
        &mut request_guard,
        cmd_phys,
        core::mem::size_of::<VirtioSndPcmCtrl>() as u32,
        resp_phys,
        core::mem::size_of::<VirtioSndHdr>() as u32,
    )?;
    check_response("PREPARE")?;

    // 3. START
    unsafe {
        let cmd_ptr = &raw mut CMD_BUF;
        let ctrl = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioSndPcmCtrl);
        *ctrl = VirtioSndPcmCtrl {
            hdr: VirtioSndHdr { code: cmd::START },
            stream_id: 0,
        };
    }
    send_ctrl_command(
        &device,
        &mut request_guard,
        cmd_phys,
        core::mem::size_of::<VirtioSndPcmCtrl>() as u32,
        resp_phys,
        core::mem::size_of::<VirtioSndHdr>() as u32,
    )?;
    check_response("START")?;

    {
        let state = sound_device_state_mut()?;
        state.stream_started = true;
    }

    crate::serial_println!("[virtio-sound] Stream started (S16_LE, 44100 Hz, stereo)");
    Ok(())
}

fn check_response(cmd_name: &str) -> Result<(), &'static str> {
    unsafe {
        let resp_ptr = &raw const RESP_BUF;
        let hdr = &*((*resp_ptr).data.as_ptr() as *const VirtioSndHdr);
        if hdr.code != resp::OK {
            crate::serial_println!(
                "[virtio-sound] {} failed with code {:#x}",
                cmd_name,
                hdr.code
            );
            return Err("Sound command failed");
        }
    }
    Ok(())
}

/// Write PCM data to the sound device
///
/// Data must be S16_LE stereo at 44100 Hz. Maximum 16KB per call.
pub fn write_pcm(data: &[u8]) -> Result<usize, &'static str> {
    if data.is_empty() {
        return Ok(0);
    }
    if !irq_completion_available() {
        return Err("Sound MMIO IRQ completion unavailable before interrupts are enabled");
    }

    let len = core::cmp::min(data.len(), PCM_BUF_SIZE);

    let base = {
        let state = sound_device_state()?;
        if !state.stream_started {
            return Err("Stream not started");
        }
        state.base
    };
    let device = VirtioMmioDevice::probe(base).ok_or("Device disappeared")?;
    let mut request_guard = TX_GATE.lock()?;
    let completion_token = TX_COMPLETION.prepare_wait();

    // Copy data to static PCM buffer
    unsafe {
        let buf_ptr = &raw mut PCM_BUF;
        (&mut (*buf_ptr).data)[..len].copy_from_slice(&data[..len]);
    }

    let xfer_phys = virt_to_phys(&raw const TX_XFER as u64);
    let pcm_phys = virt_to_phys(&raw const PCM_BUF as u64);
    let status_phys = virt_to_phys(&raw const TX_STATUS as u64);

    unsafe {
        let xfer_ptr = &raw mut TX_XFER;
        (*xfer_ptr).stream_id = 0;

        let status_ptr = &raw mut TX_STATUS;
        (*status_ptr).status = 0;
        (*status_ptr).latency_bytes = 0;

        let queue_ptr = &raw mut TX_QUEUE;

        // Descriptor 0: xfer header (device reads)
        (*queue_ptr).desc[0] = VirtqDesc {
            addr: xfer_phys,
            len: core::mem::size_of::<VirtioSndPcmXfer>() as u32,
            flags: DESC_F_NEXT,
            next: 1,
        };

        // Descriptor 1: PCM data (device reads)
        (*queue_ptr).desc[1] = VirtqDesc {
            addr: pcm_phys,
            len: len as u32,
            flags: DESC_F_NEXT,
            next: 2,
        };

        // Descriptor 2: status (device writes)
        (*queue_ptr).desc[2] = VirtqDesc {
            addr: status_phys,
            len: core::mem::size_of::<VirtioSndPcmStatus>() as u32,
            flags: DESC_F_WRITE,
            next: 0,
        };

        // Add to available ring
        let avail_idx = (*queue_ptr).avail.idx;
        (*queue_ptr).avail.ring[(avail_idx % 16) as usize] = 0;
        fence(Ordering::SeqCst);
        (*queue_ptr).avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify device (queue 2 = txq)
    dsb_sy();
    device.notify_queue(2);

    if let Err(e) = TX_COMPLETION.wait_for_completion(
        completion_token,
        "Sound MMIO TX timeout",
        "Sound MMIO TX interrupted",
    ) {
        request_guard.keep_locked();
        return Err(e);
    }

    let completed_desc =
        match TX_COMPLETION.take_completed_desc("Sound MMIO TX woke without completion") {
            Ok(desc) => desc,
            Err(e) => {
                TX_COMPLETION.clear();
                return Err(e);
            }
        };

    if completed_desc != 0 {
        TX_COMPLETION.clear();
        return Err("Sound MMIO TX completed unexpected descriptor");
    }

    dsb_sy();
    fence(Ordering::SeqCst);
    let tx_status = unsafe {
        let status_ptr = &raw const TX_STATUS;
        read_volatile(&(*status_ptr).status)
    };

    TX_COMPLETION.clear();
    if tx_status != resp::OK {
        crate::serial_println!("[virtio-sound] TX failed with code {:#x}", tx_status);
        return Err("Sound MMIO TX failed");
    }

    Ok(len)
}

/// Return the GIC SPI assigned to the VirtIO MMIO sound device.
pub fn get_irq() -> Option<u32> {
    unsafe {
        let ptr = &raw const SOUND_DEVICE;
        (*ptr)
            .as_ref()
            .map(|state| VIRTIO_IRQ_BASE + state.slot as u32)
    }
}

#[inline]
fn sound_mmio_virt_base(state: &SoundDeviceState) -> u64 {
    crate::memory::physical_memory_offset().as_u64() + state.base
}

/// Handle a VirtIO MMIO sound interrupt.
///
/// Hard IRQ path: no logging, allocation, locks, or unbounded work.
pub fn handle_interrupt() {
    let Ok(state) = sound_device_state() else {
        return;
    };

    let base = sound_mmio_virt_base(state);
    let interrupt_status = unsafe { read_volatile((base + 0x60) as *const u32) };
    if interrupt_status == 0 {
        return;
    }

    unsafe {
        write_volatile((base + 0x64) as *mut u32, interrupt_status);
    }
    dsb_sy();

    drain_queue_completion(&CTRL_COMPLETION, &raw const CTRL_QUEUE);
    drain_queue_completion(&TX_COMPLETION, &raw const TX_QUEUE);
}

fn drain_queue_completion(completion: &SoundMmioQueueCompletion, queue: *const QueueMemory) {
    if !completion.has_pending() {
        return;
    }

    fence(Ordering::SeqCst);

    let previous_used_idx = completion.last_used_idx.load(Ordering::Acquire) as u16;
    let used_idx = unsafe { read_volatile(&(*queue).used.idx) };
    if used_idx == previous_used_idx {
        return;
    }

    let ring_index = (previous_used_idx % 16) as usize;
    let used_elem = unsafe { read_volatile(&(*queue).used.ring[ring_index]) };
    completion
        .last_used_idx
        .store(used_idx as u32, Ordering::Release);
    completion.complete_desc(used_elem.id as u16);
}
