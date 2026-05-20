//! VirtIO Sound Device Driver for x86_64 (PCI Transport)
//!
//! Implements audio playback using VirtIO PCI legacy transport.
//! Provides PCM audio output at 44100 Hz, S16_LE, stereo.

use super::queue::Virtqueue;
use super::VirtioDevice;
use crate::drivers::pci::Device as PciDevice;
use crate::memory::frame_allocator;
use crate::task::completion::Completion;
use alloc::sync::Arc;
use core::sync::atomic::{fence, AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

const SOUND_COMPLETION_TIMEOUT_NS: u64 = 5_000_000_000;
const SOUND_EARLY_COMPLETION_TIMEOUT_NS: u64 = 100_000_000_000;
const NO_COMPLETED_DESC: u32 = u32::MAX;
const SOUND_TEST_SILENCE: [u8; 16_384] = [0; 16_384];

struct SoundRequestGate {
    locked: AtomicBool,
    waiters: crate::task::waitqueue::WaitQueueHead,
}

struct SoundRequestGuard<'a> {
    gate: &'a SoundRequestGate,
    release_on_drop: bool,
}

impl SoundRequestGate {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiters: crate::task::waitqueue::WaitQueueHead::new(),
        }
    }

    fn lock(&self) -> Result<SoundRequestGuard<'_>, &'static str> {
        loop {
            if self
                .locked
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(SoundRequestGuard {
                    gate: self,
                    release_on_drop: true,
                });
            }

            if !sound_request_gate_can_sleep() {
                return Err("Sound request already in progress");
            }

            if self
                .waiters
                .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
                .is_none()
            {
                return Err("Sound request already in progress");
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

impl SoundRequestGuard<'_> {
    fn keep_locked(&mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for SoundRequestGuard<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.gate.unlock();
        }
    }
}

#[inline]
fn sound_request_gate_can_sleep() -> bool {
    if crate::task::scheduler::current_thread_id().is_none() {
        return false;
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::per_cpu::preempt_count() > 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

struct SoundQueueCompletion {
    completion: Completion,
    next_token: AtomicU32,
    pending_token: AtomicU32,
    completed_desc: AtomicU32,
}

impl SoundQueueCompletion {
    fn new() -> Self {
        Self {
            completion: Completion::new(),
            next_token: AtomicU32::new(0),
            pending_token: AtomicU32::new(0),
            completed_desc: AtomicU32::new(NO_COMPLETED_DESC),
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
        let scheduler_thread_present = crate::task::scheduler::current_thread_id().is_some();
        let timeout_ns = if scheduler_thread_present {
            SOUND_COMPLETION_TIMEOUT_NS
        } else {
            SOUND_EARLY_COMPLETION_TIMEOUT_NS
        };

        match self.completion.wait_timeout(token, timeout_ns) {
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
#[derive(Clone, Copy)]
struct VirtioSndHdr {
    code: u32,
}

/// PCM set params request
#[repr(C)]
#[derive(Clone, Copy)]
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
#[derive(Clone, Copy)]
struct VirtioSndPcmCtrl {
    hdr: VirtioSndHdr,
    stream_id: u32,
}

/// PCM xfer header (for TX queue)
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioSndPcmXfer {
    stream_id: u32,
}

/// PCM xfer status (device writes back)
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioSndPcmStatus {
    status: u32,
    latency_bytes: u32,
}

/// DMA buffers for sound operations
struct DmaBuffers {
    /// Control command buffer (phys, virt)
    cmd: (u64, u64),
    /// Control response buffer (phys, virt)
    resp: (u64, u64),
    /// TX xfer header (phys, virt)
    tx_xfer: (u64, u64),
    /// TX PCM data buffer (phys, virt) - 16KB
    tx_pcm: (u64, u64),
    /// TX status (phys, virt)
    tx_status: (u64, u64),
}

/// VirtIO sound device driver
struct VirtioSoundDevice {
    device: VirtioDevice,
    ctrl_queue: Mutex<Virtqueue>,
    tx_queue: Mutex<Virtqueue>,
    ctrl_gate: SoundRequestGate,
    tx_gate: SoundRequestGate,
    ctrl_completion: SoundQueueCompletion,
    tx_completion: SoundQueueCompletion,
    dma: DmaBuffers,
    stream_started: AtomicBool,
}

impl VirtioSoundDevice {
    fn new(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        let io_bar = pci_dev.get_io_bar().ok_or("No I/O BAR found")?;
        let io_base = io_bar.address as u16;

        log::info!(
            "VirtIO sound: Initializing device at I/O base {:#x}",
            io_base
        );

        pci_dev.enable_bus_master();
        pci_dev.enable_io_space();
        pci_dev.enable_intx();
        log::info!(
            "VirtIO sound: PCI INTx IRQ line {} pin {}",
            pci_dev.interrupt_line,
            pci_dev.interrupt_pin
        );

        let mut device = VirtioDevice::new(io_base);
        device.init(0)?; // No special features

        // Set up control queue (queue 0)
        device.select_queue(0);
        let ctrl_size = device.get_queue_size();
        if ctrl_size == 0 {
            return Err("Control queue size is 0");
        }
        let ctrl_queue = Virtqueue::new(ctrl_size)?;
        device.select_queue(0);
        device.set_queue_address(ctrl_queue.phys_addr());

        // Set up TX queue (queue 2)
        device.select_queue(2);
        let tx_size = device.get_queue_size();
        if tx_size == 0 {
            return Err("TX queue size is 0");
        }
        let tx_queue = Virtqueue::new(tx_size)?;
        device.select_queue(2);
        device.set_queue_address(tx_queue.phys_addr());

        device.driver_ok();

        // Allocate DMA buffers
        let cmd_buf = Self::alloc_dma(4096)?;
        let resp_buf = Self::alloc_dma(4096)?;
        let tx_xfer_buf = Self::alloc_dma(4096)?;
        let tx_pcm_buf = Self::alloc_dma(16384)?;
        let tx_status_buf = Self::alloc_dma(4096)?;

        let dma = DmaBuffers {
            cmd: cmd_buf,
            resp: resp_buf,
            tx_xfer: tx_xfer_buf,
            tx_pcm: tx_pcm_buf,
            tx_status: tx_status_buf,
        };

        log::info!("VirtIO sound: Device initialization complete");

        Ok(VirtioSoundDevice {
            device,
            ctrl_queue: Mutex::new(ctrl_queue),
            tx_queue: Mutex::new(tx_queue),
            ctrl_gate: SoundRequestGate::new(),
            tx_gate: SoundRequestGate::new(),
            ctrl_completion: SoundQueueCompletion::new(),
            tx_completion: SoundQueueCompletion::new(),
            dma,
            stream_started: AtomicBool::new(false),
        })
    }

    fn alloc_dma(size: usize) -> Result<(u64, u64), &'static str> {
        // For buffers > 4KB, allocate multiple contiguous frames
        let pages = (size + 4095) / 4096;
        let first_frame =
            frame_allocator::allocate_frame().ok_or("Failed to allocate DMA buffer")?;
        let phys = first_frame.start_address().as_u64();
        let phys_offset = crate::memory::physical_memory_offset();
        let virt = phys + phys_offset.as_u64();

        // Zero first page
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, 4096);
        }

        // Allocate additional pages if needed (they may not be contiguous, but for
        // our simple use case we use only the first page worth of data at a time)
        for _ in 1..pages {
            let frame = frame_allocator::allocate_frame().ok_or("Failed to allocate DMA page")?;
            let p = frame.start_address().as_u64() + phys_offset.as_u64();
            unsafe {
                core::ptr::write_bytes(p as *mut u8, 0, 4096);
            }
        }

        Ok((phys, virt))
    }

    fn send_ctrl_locked(
        &self,
        request_guard: &mut SoundRequestGuard<'_>,
        cmd_len: u32,
        resp_len: u32,
    ) -> Result<(), &'static str> {
        let completion_token = self.ctrl_completion.prepare_wait();
        let (cmd_phys, _) = self.dma.cmd;
        let (resp_phys, _) = self.dma.resp;

        let buffers = [
            (cmd_phys, cmd_len, false),  // Device reads command
            (resp_phys, resp_len, true), // Device writes response
        ];

        {
            let mut queue = self.ctrl_queue.lock();
            if queue.add_chain(&buffers).is_none() {
                self.ctrl_completion.clear();
                return Err("Control queue full");
            }
        }

        fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        if let Err(e) = self.ctrl_completion.wait_for_completion(
            completion_token,
            "Sound control command timeout",
            "Sound control command interrupted",
        ) {
            request_guard.keep_locked();
            return Err(e);
        }

        let completed_desc = match self
            .ctrl_completion
            .take_completed_desc("Sound control command woke without completion")
        {
            Ok(desc) => desc,
            Err(e) => {
                self.ctrl_completion.clear();
                return Err(e);
            }
        };

        {
            let mut queue = self.ctrl_queue.lock();
            queue.free_chain(completed_desc);
        }

        let result = self.check_response();
        self.ctrl_completion.clear();
        result
    }

    fn check_response(&self) -> Result<(), &'static str> {
        let (_, resp_virt) = self.dma.resp;
        let code = unsafe { *(resp_virt as *const u32) };
        if code != resp::OK {
            log::warn!("VirtIO sound: command failed with code {:#x}", code);
            return Err("Sound command failed");
        }
        Ok(())
    }

    #[cfg(target_arch = "x86_64")]
    fn irq_completion_available(&self) -> bool {
        crate::task::scheduler::current_thread_id().is_some()
            || x86_64::instructions::interrupts::are_enabled()
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn irq_completion_available(&self) -> bool {
        true
    }

    fn do_setup_stream(&self) -> Result<(), &'static str> {
        if self.stream_started.load(Ordering::Acquire) {
            return Ok(());
        }
        if !self.irq_completion_available() {
            return Err("Sound IRQ completion unavailable before interrupts are enabled");
        }

        let mut request_guard = self.ctrl_gate.lock()?;

        if self.stream_started.load(Ordering::Acquire) {
            return Ok(());
        }

        let (_, cmd_virt) = self.dma.cmd;
        let hdr_size = core::mem::size_of::<VirtioSndHdr>() as u32;

        // 1. SET_PARAMS
        unsafe {
            let params = cmd_virt as *mut VirtioSndPcmSetParams;
            (*params).hdr.code = cmd::SET_PARAMS;
            (*params).stream_id = 0;
            (*params).buffer_bytes = 32768;
            (*params).period_bytes = 16384;
            (*params).features = 0;
            (*params).channels = 2;
            (*params).format = pcm_format::S16;
            (*params).rate = pcm_rate::RATE_44100;
            (*params)._padding = 0;
        }
        self.send_ctrl_locked(
            &mut request_guard,
            core::mem::size_of::<VirtioSndPcmSetParams>() as u32,
            hdr_size,
        )?;

        // 2. PREPARE
        unsafe {
            let ctrl = cmd_virt as *mut VirtioSndPcmCtrl;
            (*ctrl).hdr.code = cmd::PREPARE;
            (*ctrl).stream_id = 0;
        }
        self.send_ctrl_locked(
            &mut request_guard,
            core::mem::size_of::<VirtioSndPcmCtrl>() as u32,
            hdr_size,
        )?;

        // 3. START
        unsafe {
            let ctrl = cmd_virt as *mut VirtioSndPcmCtrl;
            (*ctrl).hdr.code = cmd::START;
            (*ctrl).stream_id = 0;
        }
        self.send_ctrl_locked(
            &mut request_guard,
            core::mem::size_of::<VirtioSndPcmCtrl>() as u32,
            hdr_size,
        )?;

        self.stream_started.store(true, Ordering::Release);
        log::info!("VirtIO sound: Stream started (S16_LE, 44100 Hz, stereo)");
        Ok(())
    }

    fn do_write_pcm(&self, data: &[u8]) -> Result<usize, &'static str> {
        if !self.stream_started.load(Ordering::Acquire) {
            return Err("Stream not started");
        }
        if !self.irq_completion_available() {
            return Err("Sound IRQ completion unavailable before interrupts are enabled");
        }

        let len = core::cmp::min(data.len(), 16384);
        if len == 0 {
            return Ok(0);
        }

        let mut request_guard = self.tx_gate.lock()?;
        let completion_token = self.tx_completion.prepare_wait();
        let (xfer_phys, xfer_virt) = self.dma.tx_xfer;
        let (pcm_phys, pcm_virt) = self.dma.tx_pcm;
        let (status_phys, status_virt) = self.dma.tx_status;

        // Set up xfer header
        unsafe {
            let xfer = xfer_virt as *mut VirtioSndPcmXfer;
            (*xfer).stream_id = 0;
        }

        // Copy PCM data
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), pcm_virt as *mut u8, len);
        }

        // Clear status
        unsafe {
            core::ptr::write_bytes(
                status_virt as *mut u8,
                0,
                core::mem::size_of::<VirtioSndPcmStatus>(),
            );
        }

        // Build 3-descriptor chain on TX queue
        let buffers = [
            (
                xfer_phys,
                core::mem::size_of::<VirtioSndPcmXfer>() as u32,
                false,
            ),
            (pcm_phys, len as u32, false),
            (
                status_phys,
                core::mem::size_of::<VirtioSndPcmStatus>() as u32,
                true,
            ),
        ];

        {
            let mut queue = self.tx_queue.lock();
            if queue.add_chain(&buffers).is_none() {
                self.tx_completion.clear();
                return Err("TX queue full");
            }
        }

        fence(Ordering::SeqCst);
        self.device.notify_queue(2);

        if let Err(e) = self.tx_completion.wait_for_completion(
            completion_token,
            "Sound TX timeout",
            "Sound TX interrupted",
        ) {
            request_guard.keep_locked();
            return Err(e);
        }

        let completed_desc = match self
            .tx_completion
            .take_completed_desc("Sound TX woke without completion")
        {
            Ok(desc) => desc,
            Err(e) => {
                self.tx_completion.clear();
                return Err(e);
            }
        };

        fence(Ordering::SeqCst);
        let status = unsafe { core::ptr::read_volatile(status_virt as *const u32) };

        {
            let mut queue = self.tx_queue.lock();
            queue.free_chain(completed_desc);
        }

        self.tx_completion.clear();
        if status != resp::OK {
            log::warn!("VirtIO sound: TX failed with code {:#x}", status);
            return Err("Sound TX failed");
        }

        Ok(len)
    }

    /// Handle interrupt from the PCI sound device.
    ///
    /// CRITICAL: This function must be extremely fast. No logging, no allocations.
    pub fn handle_interrupt(&self) -> bool {
        let isr = self.device.read_isr();
        if isr == 0 {
            return false;
        }

        self.drain_ctrl_completion();
        self.drain_tx_completion();
        true
    }

    fn drain_ctrl_completion(&self) {
        if !self.ctrl_completion.has_pending() {
            return;
        }

        let Some(mut queue) = self.ctrl_queue.try_lock() else {
            return;
        };

        if let Some((completed_desc, _bytes)) = queue.get_used() {
            self.ctrl_completion.complete_desc(completed_desc);
        }
    }

    fn drain_tx_completion(&self) {
        if !self.tx_completion.has_pending() {
            return;
        }

        let Some(mut queue) = self.tx_queue.try_lock() else {
            return;
        };

        if let Some((completed_desc, _bytes)) = queue.get_used() {
            self.tx_completion.complete_desc(completed_desc);
        }
    }
}

// Global sound device
static SOUND_DEVICE: Mutex<Option<Arc<VirtioSoundDevice>>> = Mutex::new(None);

/// Initialize the VirtIO sound driver
pub fn init() -> Result<(), &'static str> {
    let devices = crate::drivers::pci::find_virtio_sound_devices();

    if devices.is_empty() {
        return Err("No VirtIO sound devices found");
    }

    log::info!("VirtIO sound: Found {} device(s)", devices.len());

    let pci_dev = &devices[0];
    let sound_dev = Arc::new(VirtioSoundDevice::new(pci_dev)?);
    *SOUND_DEVICE.lock() = Some(sound_dev);

    log::info!("VirtIO sound: Driver initialized");
    Ok(())
}

/// Get a reference to the initialized sound device.
fn get_device() -> Option<Arc<VirtioSoundDevice>> {
    SOUND_DEVICE.lock().clone()
}

/// Dispatch a pending sound interrupt, if the device is initialized.
pub fn handle_interrupt() -> bool {
    let Some(device) = get_device() else {
        return false;
    };
    device.handle_interrupt()
}

/// Return whether a PCI VirtIO sound device initialized successfully.
pub fn is_initialized() -> bool {
    get_device().is_some()
}

/// Exercise the control and TX queues once after interrupts are enabled.
pub fn test_silence() -> Result<(), &'static str> {
    log::info!("VirtIO sound test: setting up stream...");
    setup_stream()?;
    write_pcm(&SOUND_TEST_SILENCE)?;
    log::info!("VirtIO sound test: silence write successful!");
    Ok(())
}

/// Set up the PCM stream for playback
pub fn setup_stream() -> Result<(), &'static str> {
    let dev = get_device().ok_or("Sound device not initialized")?;
    dev.do_setup_stream()
}

/// Write PCM data to the sound device
pub fn write_pcm(data: &[u8]) -> Result<usize, &'static str> {
    let dev = get_device().ok_or("Sound device not initialized")?;
    dev.do_write_pcm(data)
}
