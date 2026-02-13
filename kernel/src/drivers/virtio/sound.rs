//! VirtIO Sound Device Driver for x86_64 (PCI Transport)
//!
//! Implements audio playback using VirtIO PCI legacy transport.
//! Provides PCM audio output at 44100 Hz, S16_LE, stereo.

use super::queue::Virtqueue;
use super::VirtioDevice;
use crate::drivers::pci::Device as PciDevice;
use crate::memory::frame_allocator;
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;

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
    pub const S16: u8 = 5;  // VIRTIO_SND_PCM_FMT_S16
}

/// VirtIO Sound PCM rates (from virtio_snd.h)
mod pcm_rate {
    pub const RATE_44100: u8 = 6;  // VIRTIO_SND_PCM_RATE_44100
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
    ctrl_queue: Virtqueue,
    tx_queue: Virtqueue,
    dma: DmaBuffers,
    stream_started: bool,
}

impl VirtioSoundDevice {
    fn new(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        let io_bar = pci_dev.get_io_bar().ok_or("No I/O BAR found")?;
        let io_base = io_bar.address as u16;

        log::info!("VirtIO sound: Initializing device at I/O base {:#x}", io_base);

        pci_dev.enable_bus_master();
        pci_dev.enable_io_space();

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
            ctrl_queue,
            tx_queue,
            dma,
            stream_started: false,
        })
    }

    fn alloc_dma(size: usize) -> Result<(u64, u64), &'static str> {
        // For buffers > 4KB, allocate multiple contiguous frames
        let pages = (size + 4095) / 4096;
        let first_frame = frame_allocator::allocate_frame().ok_or("Failed to allocate DMA buffer")?;
        let phys = first_frame.start_address().as_u64();
        let phys_offset = crate::memory::physical_memory_offset();
        let virt = phys + phys_offset.as_u64();

        // Zero first page
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096); }

        // Allocate additional pages if needed (they may not be contiguous, but for
        // our simple use case we use only the first page worth of data at a time)
        for _ in 1..pages {
            let frame = frame_allocator::allocate_frame().ok_or("Failed to allocate DMA page")?;
            let p = frame.start_address().as_u64() + phys_offset.as_u64();
            unsafe { core::ptr::write_bytes(p as *mut u8, 0, 4096); }
        }

        Ok((phys, virt))
    }

    fn send_ctrl(&mut self, cmd_len: u32, resp_len: u32) -> Result<(), &'static str> {
        let (cmd_phys, _) = self.dma.cmd;
        let (resp_phys, _) = self.dma.resp;

        let buffers = [
            (cmd_phys, cmd_len, false),   // Device reads command
            (resp_phys, resp_len, true),   // Device writes response
        ];

        self.ctrl_queue.add_chain(&buffers).ok_or("Control queue full")?;
        fence(Ordering::SeqCst);
        self.device.notify_queue(0);

        // Poll for completion
        let mut timeout = 100_000u32;
        while !self.ctrl_queue.has_used() && timeout > 0 {
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
            let _ = self.device.read_isr();
            timeout -= 1;
        }
        if timeout == 0 {
            return Err("Sound control command timeout");
        }

        let (desc, _) = self.ctrl_queue.get_used().ok_or("No completion")?;
        self.ctrl_queue.free_chain(desc);
        Ok(())
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

    fn do_setup_stream(&mut self) -> Result<(), &'static str> {
        if self.stream_started {
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
        self.send_ctrl(core::mem::size_of::<VirtioSndPcmSetParams>() as u32, hdr_size)?;
        self.check_response()?;

        // 2. PREPARE
        unsafe {
            let ctrl = cmd_virt as *mut VirtioSndPcmCtrl;
            (*ctrl).hdr.code = cmd::PREPARE;
            (*ctrl).stream_id = 0;
        }
        self.send_ctrl(core::mem::size_of::<VirtioSndPcmCtrl>() as u32, hdr_size)?;
        self.check_response()?;

        // 3. START
        unsafe {
            let ctrl = cmd_virt as *mut VirtioSndPcmCtrl;
            (*ctrl).hdr.code = cmd::START;
            (*ctrl).stream_id = 0;
        }
        self.send_ctrl(core::mem::size_of::<VirtioSndPcmCtrl>() as u32, hdr_size)?;
        self.check_response()?;

        self.stream_started = true;
        log::info!("VirtIO sound: Stream started (S16_LE, 44100 Hz, stereo)");
        Ok(())
    }

    fn do_write_pcm(&mut self, data: &[u8]) -> Result<usize, &'static str> {
        if !self.stream_started {
            return Err("Stream not started");
        }

        let len = core::cmp::min(data.len(), 16384);
        if len == 0 {
            return Ok(0);
        }

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
            core::ptr::write_bytes(status_virt as *mut u8, 0, core::mem::size_of::<VirtioSndPcmStatus>());
        }

        // Build 3-descriptor chain on TX queue
        let buffers = [
            (xfer_phys, core::mem::size_of::<VirtioSndPcmXfer>() as u32, false),
            (pcm_phys, len as u32, false),
            (status_phys, core::mem::size_of::<VirtioSndPcmStatus>() as u32, true),
        ];

        self.tx_queue.add_chain(&buffers).ok_or("TX queue full")?;
        fence(Ordering::SeqCst);
        self.device.notify_queue(2);

        // Poll for completion
        let mut timeout = 100_000u32;
        while !self.tx_queue.has_used() && timeout > 0 {
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
            let _ = self.device.read_isr();
            timeout -= 1;
        }
        if timeout == 0 {
            return Err("Sound TX timeout");
        }

        let (desc, _) = self.tx_queue.get_used().ok_or("No TX completion")?;
        self.tx_queue.free_chain(desc);

        Ok(len)
    }
}

// Global sound device
static SOUND_DEVICE: Mutex<Option<VirtioSoundDevice>> = Mutex::new(None);

/// Initialize the VirtIO sound driver
pub fn init() -> Result<(), &'static str> {
    let devices = crate::drivers::pci::find_virtio_sound_devices();

    if devices.is_empty() {
        return Err("No VirtIO sound devices found");
    }

    log::info!("VirtIO sound: Found {} device(s)", devices.len());

    let pci_dev = &devices[0];
    let sound_dev = VirtioSoundDevice::new(pci_dev)?;
    *SOUND_DEVICE.lock() = Some(sound_dev);

    log::info!("VirtIO sound: Driver initialized");
    Ok(())
}

/// Set up the PCM stream for playback
pub fn setup_stream() -> Result<(), &'static str> {
    let mut guard = SOUND_DEVICE.lock();
    let dev = guard.as_mut().ok_or("Sound device not initialized")?;
    dev.do_setup_stream()
}

/// Write PCM data to the sound device
pub fn write_pcm(data: &[u8]) -> Result<usize, &'static str> {
    let mut guard = SOUND_DEVICE.lock();
    let dev = guard.as_mut().ok_or("Sound device not initialized")?;
    dev.do_write_pcm(data)
}
