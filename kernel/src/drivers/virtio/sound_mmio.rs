//! VirtIO Sound Device Driver for ARM64 (MMIO Transport)
//!
//! Implements a basic audio playback driver using VirtIO MMIO transport.
//! Provides PCM audio output at 44100 Hz, S16_LE, stereo.

use super::mmio::{VirtioMmioDevice, device_id, VIRTIO_MMIO_BASE, VIRTIO_MMIO_SIZE, VIRTIO_MMIO_COUNT};
use core::ptr::read_volatile;
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;

/// Lock protecting the sound command path.
static SOUND_LOCK: Mutex<()> = Mutex::new(());

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
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; 16],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; 16] },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed { flags: 0, idx: 0, ring: [VirtqUsedElem { id: 0, len: 0 }; 16] },
};

// TX queue (queue 2)
static mut TX_QUEUE: QueueMemory = QueueMemory {
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; 16],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; 16] },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed { flags: 0, idx: 0, ring: [VirtqUsedElem { id: 0, len: 0 }; 16] },
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
static mut TX_STATUS: VirtioSndPcmStatus = VirtioSndPcmStatus { status: 0, latency_bytes: 0 };

// PCM data buffer (16KB)
const PCM_BUF_SIZE: usize = 16384;
#[repr(C, align(64))]
struct PcmBuffer {
    data: [u8; PCM_BUF_SIZE],
}
static mut PCM_BUF: PcmBuffer = PcmBuffer { data: [0; PCM_BUF_SIZE] };

/// Sound device state
static mut SOUND_DEVICE: Option<SoundDeviceState> = None;

struct SoundDeviceState {
    base: u64,
    ctrl_last_used_idx: u16,
    tx_last_used_idx: u16,
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
                return init_device(&mut device, base);
            }
        }
    }

    Err("No VirtIO Sound device found")
}

fn init_device(device: &mut VirtioMmioDevice, base: u64) -> Result<(), &'static str> {
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

    unsafe {
        let ptr = &raw mut SOUND_DEVICE;
        *ptr = Some(SoundDeviceState {
            base,
            ctrl_last_used_idx: 0,
            tx_last_used_idx: 0,
            stream_started: false,
        });
    }

    crate::serial_println!("[virtio-sound] Sound device initialized");
    Ok(())
}

fn setup_queue(device: &mut VirtioMmioDevice, queue_idx: u32, version: u32, queue: *mut QueueMemory) -> Result<(), &'static str> {
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

fn with_device_state<F, R>(f: F) -> Result<R, &'static str>
where
    F: FnOnce(&VirtioMmioDevice, &mut SoundDeviceState) -> Result<R, &'static str>,
{
    let _guard = SOUND_LOCK.lock();
    let state = unsafe {
        let ptr = &raw mut SOUND_DEVICE;
        match (*ptr).as_mut() {
            Some(s) => s,
            None => {
                crate::serial_println!("[virtio-sound] ERROR: Sound device not initialized");
                return Err("Sound device not initialized");
            }
        }
    };
    let device = VirtioMmioDevice::probe(state.base).ok_or("Device disappeared")?;
    f(&device, state)
}

fn send_ctrl_command(
    device: &VirtioMmioDevice,
    state: &mut SoundDeviceState,
    cmd_phys: u64,
    cmd_len: u32,
    resp_phys: u64,
    resp_len: u32,
) -> Result<(), &'static str> {
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

    // Log queue state before notify
    let avail_idx_before = unsafe {
        let ptr = &raw const CTRL_QUEUE;
        read_volatile(&(*ptr).avail.idx)
    };
    let used_idx_before = unsafe {
        let ptr = &raw const CTRL_QUEUE;
        read_volatile(&(*ptr).used.idx)
    };
    crate::serial_println!("[virtio-sound] Notifying queue 0: avail_idx={}, used_idx={}, last_used={}, cmd_phys={:#x}, resp_phys={:#x}",
        avail_idx_before, used_idx_before, state.ctrl_last_used_idx, cmd_phys, resp_phys);

    // Notify device (queue 0 = controlq)
    device.notify_queue(0);

    // Wait for response
    let mut timeout = 10_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let ptr = &raw const CTRL_QUEUE;
            read_volatile(&(*ptr).used.idx)
        };
        if used_idx != state.ctrl_last_used_idx {
            state.ctrl_last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            let used_idx = unsafe {
                let ptr = &raw const CTRL_QUEUE;
                read_volatile(&(*ptr).used.idx)
            };
            crate::serial_println!("[virtio-sound] Control command timeout! ctrl_last_used={}, used_idx={}", state.ctrl_last_used_idx, used_idx);
            return Err("Sound control command timeout");
        }
        core::hint::spin_loop();
    }

    Ok(())
}

/// Set up the PCM stream for playback (S16_LE, 44100 Hz, stereo)
pub fn setup_stream() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-sound] setup_stream() called");
    with_device_state(|device, state| {
        if state.stream_started {
            crate::serial_println!("[virtio-sound] Stream already started");
            return Ok(());
        }

        crate::serial_println!("[virtio-sound] Setting up stream (format={}, rate={}, ch=2)", pcm_format::S16, pcm_rate::RATE_44100);
        let cmd_phys = virt_to_phys(&raw const CMD_BUF as u64);
        let resp_phys = virt_to_phys(&raw const RESP_BUF as u64);

        // 1. SET_PARAMS
        unsafe {
            let cmd_ptr = &raw mut CMD_BUF;
            let params = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioSndPcmSetParams);
            *params = VirtioSndPcmSetParams {
                hdr: VirtioSndHdr { code: cmd::SET_PARAMS },
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
            device, state, cmd_phys,
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
            device, state, cmd_phys,
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
            device, state, cmd_phys,
            core::mem::size_of::<VirtioSndPcmCtrl>() as u32,
            resp_phys,
            core::mem::size_of::<VirtioSndHdr>() as u32,
        )?;
        check_response("START")?;

        state.stream_started = true;
        crate::serial_println!("[virtio-sound] Stream started (S16_LE, 44100 Hz, stereo)");
        Ok(())
    })
}

fn check_response(cmd_name: &str) -> Result<(), &'static str> {
    unsafe {
        let resp_ptr = &raw const RESP_BUF;
        let hdr = &*((*resp_ptr).data.as_ptr() as *const VirtioSndHdr);
        if hdr.code != resp::OK {
            crate::serial_println!("[virtio-sound] {} failed with code {:#x}", cmd_name, hdr.code);
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

    let len = core::cmp::min(data.len(), PCM_BUF_SIZE);

    with_device_state(|device, state| {
        if !state.stream_started {
            return Err("Stream not started");
        }

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
        device.notify_queue(2);

        // Wait for completion
        let mut timeout = 10_000_000u32;
        loop {
            fence(Ordering::SeqCst);
            let used_idx = unsafe {
                let ptr = &raw const TX_QUEUE;
                read_volatile(&(*ptr).used.idx)
            };
            if used_idx != state.tx_last_used_idx {
                state.tx_last_used_idx = used_idx;
                break;
            }
            timeout -= 1;
            if timeout == 0 {
                return Err("Sound TX timeout");
            }
            core::hint::spin_loop();
        }

        Ok(len)
    })
}
