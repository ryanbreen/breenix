//! VirtIO GPU Device Driver for ARM64 (MMIO Transport)
//!
//! Implements a basic GPU/display driver using VirtIO MMIO transport.
//! Provides framebuffer functionality for simple 2D graphics.

use super::mmio::{
    device_id, VirtioMmioDevice, VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_SIZE,
};
use core::ptr::read_volatile;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64};
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;

/// Lock protecting the GPU command path (CMD_BUF, RESP_BUF, CTRL_QUEUE, GPU_DEVICE).
/// Without this, concurrent callers (e.g. render thread + particle thread) corrupt
/// the shared command/response buffers and virtqueue state.
static GPU_LOCK: Mutex<()> = Mutex::new(());

/// VirtIO GPU command types
#[allow(dead_code)]
mod cmd {
    // 2D commands
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;

    // Response types
    pub const RESP_OK_NODATA: u32 = 0x1100;
    pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
    pub const RESP_ERR_UNSPEC: u32 = 0x1200;
}

/// VirtIO GPU formats
#[allow(dead_code)]
mod format {
    pub const B8G8R8A8_UNORM: u32 = 1;
    pub const B8G8R8X8_UNORM: u32 = 2;
    pub const A8R8G8B8_UNORM: u32 = 3;
    pub const X8R8G8B8_UNORM: u32 = 4;
    pub const R8G8B8A8_UNORM: u32 = 67;
    pub const X8B8G8R8_UNORM: u32 = 68;
    pub const A8B8G8R8_UNORM: u32 = 121;
    pub const R8G8B8X8_UNORM: u32 = 134;
}

/// VirtIO GPU control header
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuCtrlHdr {
    type_: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

/// Display info for one scanout
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuDisplayOne {
    r_x: u32,
    r_y: u32,
    r_width: u32,
    r_height: u32,
    enabled: u32,
    flags: u32,
}

/// Get display info response
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuRespDisplayInfo {
    hdr: VirtioGpuCtrlHdr,
    pmodes: [VirtioGpuDisplayOne; 16], // VIRTIO_GPU_MAX_SCANOUTS = 16
}

/// Resource create 2D command
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct VirtioGpuResourceCreate2d {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

/// Set scanout command
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct VirtioGpuSetScanout {
    hdr: VirtioGpuCtrlHdr,
    r_x: u32,
    r_y: u32,
    r_width: u32,
    r_height: u32,
    scanout_id: u32,
    resource_id: u32,
}

/// Memory entry for resource attach backing
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct VirtioGpuMemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

/// Resource attach backing command
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct VirtioGpuResourceAttachBacking {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
}

/// Transfer to host 2D command
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct VirtioGpuTransferToHost2d {
    hdr: VirtioGpuCtrlHdr,
    r_x: u32,
    r_y: u32,
    r_width: u32,
    r_height: u32,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

/// Resource flush command
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct VirtioGpuResourceFlush {
    hdr: VirtioGpuCtrlHdr,
    r_x: u32,
    r_y: u32,
    r_width: u32,
    r_height: u32,
    resource_id: u32,
    padding: u32,
}

/// Virtqueue descriptor
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

/// Available ring
#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; 16],
}

/// Used ring element
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

/// Used ring
#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; 16],
}

/// Static control queue memory
#[repr(C, align(4096))]
struct CtrlQueueMemory {
    desc: [VirtqDesc; 16],
    avail: VirtqAvail,
    _padding: [u8; 4096 - 256 - 36],
    used: VirtqUsed,
}

// Static buffers
static mut CTRL_QUEUE: CtrlQueueMemory = CtrlQueueMemory {
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
    data: [u8; 512],
}

static mut CMD_BUF: CmdBuffer = CmdBuffer { data: [0; 512] };
static mut RESP_BUF: CmdBuffer = CmdBuffer { data: [0; 512] };

// Default framebuffer dimensions (used when fw_cfg doesn't specify)
const DEFAULT_FB_WIDTH: u32 = 1280;
const DEFAULT_FB_HEIGHT: u32 = 800;
// Max supported resolution: 1920x1200 @ 32bpp = ~9.2MB
const FB_MAX_WIDTH: u32 = 1920;
const FB_MAX_HEIGHT: u32 = 1200;
const FB_SIZE: usize = (FB_MAX_WIDTH * FB_MAX_HEIGHT * 4) as usize;
const BYTES_PER_PIXEL: usize = 4;
const RESOURCE_ID: u32 = 1;

// T39 bisect: dead statics intentionally added to measure whether even
// pure BSS additions to gpu_mmio.rs perturb the Parallels CPU0 guard.
// These will be referenced by future P9 turns (T40+) and are NOT
// permanent dead code — they're a bisect probe.
#[allow(dead_code)]
static MMIO_SLOT: AtomicU32 = AtomicU32::new(u32::MAX);
#[allow(dead_code)]
static MMIO_BASE: AtomicU64 = AtomicU64::new(0);
#[allow(dead_code)]
static GPU_MMIO_IRQ_COUNT: AtomicU32 = AtomicU32::new(0);
#[allow(dead_code)]
static GPU_MMIO_USED_IDX_ADVANCED: AtomicU32 = AtomicU32::new(0);
#[allow(dead_code)]
static GPU_MMIO_LAST_USED_IDX: AtomicU16 = AtomicU16::new(0);
// T40 bisect: minimal function probe — pure read of MMIO_SLOT.
// Public so it would be callable from exception.rs dispatch in a future
// turn, but NOT called from anywhere in this turn's binary. Per CLAUDE.md
// zero-warnings rule, allow(dead_code) is legitimate because this is a
// bisect probe pending T41+ wiring.
#[allow(dead_code)]
pub fn get_irq() -> Option<u32> {
    let slot = MMIO_SLOT.load(core::sync::atomic::Ordering::Acquire);
    if slot == u32::MAX {
        None
    } else {
        Some(48 + slot)
    }
}
// T41 bisect: heaviest function probe — volatile MMIO read/ack/used.idx read
// + atomic CAS. Body matches T38's handle_interrupt() verbatim to test
// whether THIS specific function body (not just any function) trips the
// Parallels CPU0 guard. NOT wired into exception.rs dispatch — pure dead
// code in this turn's binary. allow(dead_code) is legitimate per CLAUDE.md
// because this is a bisect probe pending T42+ wiring decisions.
#[allow(dead_code)]
pub fn handle_interrupt() {
    let base = MMIO_BASE.load(core::sync::atomic::Ordering::Acquire);
    if base == 0 {
        return;
    }

    let interrupt_status = unsafe { crate::arch_impl::aarch64::mmio::read32((base + 0x60) as *const u32) };
    if interrupt_status == 0 {
        return;
    }

    GPU_MMIO_IRQ_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    unsafe {
        crate::arch_impl::aarch64::mmio::write32((base + 0x64) as *mut u32, interrupt_status);
    }
    crate::arch_impl::aarch64::cpu::dsb_sy();
    fence(core::sync::atomic::Ordering::SeqCst);

    let used_idx = unsafe {
        let ptr = &raw const CTRL_QUEUE;
        read_volatile(&(*ptr).used.idx)
    };
    let previous = GPU_MMIO_LAST_USED_IDX.load(core::sync::atomic::Ordering::Acquire);
    if used_idx != previous {
        GPU_MMIO_LAST_USED_IDX.store(used_idx, core::sync::atomic::Ordering::Release);
        GPU_MMIO_USED_IDX_ADVANCED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}
// T42 bisect: last simple reader probe — pure tuple-load of the 3 counters.
// Public so it would be readable from a non-IRQ-context observer in future
// turns, but NOT called from anywhere in this turn's binary. allow(dead_code)
// is legitimate per CLAUDE.md (bisect probe pending T43+ wiring).
#[allow(dead_code)]
pub fn snapshot_counters() -> (u32, u32, u16) {
    (
        GPU_MMIO_IRQ_COUNT.load(core::sync::atomic::Ordering::Relaxed),
        GPU_MMIO_USED_IDX_ADVANCED.load(core::sync::atomic::Ordering::Relaxed),
        GPU_MMIO_LAST_USED_IDX.load(core::sync::atomic::Ordering::Relaxed),
    )
}
// T43 bisect: function body verbatim from T38's preserved diff. UNCALLED
// (no invocation from init_device or anywhere else). Tests whether the body
// alone trips Parallels CPU0, isolating it from the init-time call site
// hypothesis tested in T44. allow(dead_code) is legitimate per CLAUDE.md
// — this is a bisect probe pending T44 wiring decision.
#[allow(dead_code)]
fn record_mmio_irq_state(base: u64, slot: u32) {
    let virt_base = crate::memory::physical_memory_offset().as_u64() + base;
    let used_idx = unsafe {
        let ptr = &raw const CTRL_QUEUE;
        read_volatile(&(*ptr).used.idx)
    };

    GPU_MMIO_LAST_USED_IDX.store(used_idx, core::sync::atomic::Ordering::Release);
    MMIO_BASE.store(virt_base, core::sync::atomic::Ordering::Release);
    MMIO_SLOT.store(slot, core::sync::atomic::Ordering::Release);
}
/// Requested resolution from fw_cfg (set during init, before GPU probe)
static mut REQUESTED_WIDTH: u32 = 0;
static mut REQUESTED_HEIGHT: u32 = 0;

/// Read desired resolution from fw_cfg opt/breenix/resolution (e.g. "1728x1080")
pub fn load_resolution_from_fw_cfg() {
    if let Some(res) = crate::drivers::fw_cfg::read_string("opt/breenix/resolution") {
        if let Some(idx) = res.find('x') {
            if let (Ok(w), Ok(h)) = (res[..idx].parse::<u32>(), res[idx + 1..].parse::<u32>()) {
                if w > 0 && h > 0 && w <= FB_MAX_WIDTH && h <= FB_MAX_HEIGHT {
                    unsafe {
                        REQUESTED_WIDTH = w;
                        REQUESTED_HEIGHT = h;
                    }
                    crate::serial_println!("[virtio-gpu] fw_cfg resolution: {}x{}", w, h);
                    return;
                }
            }
        }
        crate::serial_println!("[virtio-gpu] fw_cfg resolution invalid: {:?}", res);
    }
}

/// Get the configured resolution (fw_cfg > default)
fn configured_resolution() -> (u32, u32) {
    unsafe {
        if REQUESTED_WIDTH > 0 && REQUESTED_HEIGHT > 0 {
            (REQUESTED_WIDTH, REQUESTED_HEIGHT)
        } else {
            (DEFAULT_FB_WIDTH, DEFAULT_FB_HEIGHT)
        }
    }
}

#[repr(C, align(4096))]
struct Framebuffer {
    pixels: [u8; FB_SIZE],
}

static mut FRAMEBUFFER: Framebuffer = Framebuffer {
    pixels: [0; FB_SIZE],
};

/// GPU device state
static mut GPU_DEVICE: Option<GpuDeviceState> = None;

struct GpuDeviceState {
    base: u64,
    width: u32,
    height: u32,
    resource_id: u32,
    last_used_idx: u16,
}

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    addr - crate::memory::physical_memory_offset().as_u64()
}

/// Initialize the VirtIO GPU device
pub fn init() -> Result<(), &'static str> {
    // Check if already initialized
    unsafe {
        let ptr = &raw const GPU_DEVICE;
        if (*ptr).is_some() {
            crate::serial_println!("[virtio-gpu] GPU device already initialized");
            return Ok(());
        }
    }

    crate::serial_println!("[virtio-gpu] Searching for GPU device...");

    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;
        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            if device.device_id() == device_id::GPU {
                crate::serial_println!("[virtio-gpu] Found GPU device at {:#x}", base);
                return init_device(&mut device, base, i as u32);
            }
        }
    }

    Err("No VirtIO GPU device found")
}

fn init_device(device: &mut VirtioMmioDevice, base: u64, _slot: u32) -> Result<(), &'static str> {
    let version = device.version();
    crate::serial_println!("[virtio-gpu] Device version: {}", version);

    if version == 1 {
        device.set_guest_page_size(4096);
    }

    // Initialize the device
    device.init(0)?;

    // Set up control queue (queue 0)
    device.select_queue(0);
    let queue_num_max = device.get_queue_num_max();
    crate::serial_println!("[virtio-gpu] Control queue max size: {}", queue_num_max);

    if queue_num_max == 0 {
        return Err("Control queue size is 0");
    }

    let queue_size = core::cmp::min(queue_num_max, 16);
    device.set_queue_num(queue_size);

    let queue_phys = virt_to_phys(&raw const CTRL_QUEUE as u64);

    unsafe {
        let queue_ptr = &raw mut CTRL_QUEUE;
        for i in 0..15 {
            (*queue_ptr).desc[i].next = (i + 1) as u16;
        }
        (*queue_ptr).desc[15].next = 0;
        (*queue_ptr).avail.flags = 0;
        (*queue_ptr).avail.idx = 0;
        (*queue_ptr).used.flags = 0;
        (*queue_ptr).used.idx = 0;
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

    // Mark device ready
    device.driver_ok();

    // Store initial state with configured resolution
    let (init_w, init_h) = configured_resolution();
    unsafe {
        let ptr = &raw mut GPU_DEVICE;
        *ptr = Some(GpuDeviceState {
            base,
            width: init_w,
            height: init_h,
            resource_id: RESOURCE_ID,
            last_used_idx: 0,
        });
    }

    // Determine display dimensions:
    // fw_cfg resolution takes priority over GET_DISPLAY_INFO
    let (width, height) = {
        let (req_w, req_h) = configured_resolution();
        if req_w != DEFAULT_FB_WIDTH || req_h != DEFAULT_FB_HEIGHT {
            // fw_cfg provided a custom resolution — use it
            crate::serial_println!("[virtio-gpu] Display: {}x{} (from fw_cfg)", req_w, req_h);
            (req_w, req_h)
        } else {
            // No fw_cfg override — query the GPU
            let (gpu_w, gpu_h) = get_display_info()?;
            crate::serial_println!("[virtio-gpu] Display: {}x{}", gpu_w, gpu_h);
            (gpu_w, gpu_h)
        }
    };

    // Update state with final dimensions
    unsafe {
        let ptr = &raw mut GPU_DEVICE;
        if let Some(ref mut state) = *ptr {
            state.width = width;
            state.height = height;
        }
    }

    // Create framebuffer resource and attach backing
    create_resource()?;
    attach_backing()?;
    set_scanout()?;
    flush()?;

    crate::serial_println!("[virtio-gpu] GPU device initialized successfully");
    Ok(())
}

fn get_display_info() -> Result<(u32, u32), &'static str> {
    with_device_state(|device, state| {
        // Prepare GET_DISPLAY_INFO command
        let cmd_phys = virt_to_phys(&raw const CMD_BUF as u64);
        let resp_phys = virt_to_phys(&raw const RESP_BUF as u64);

        unsafe {
            let cmd_ptr = &raw mut CMD_BUF;
            let hdr = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCtrlHdr);
            *hdr = VirtioGpuCtrlHdr {
                type_: cmd::GET_DISPLAY_INFO,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            };
        }

        // Send command
        send_command(
            device,
            state,
            cmd_phys,
            core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
            resp_phys,
            core::mem::size_of::<VirtioGpuRespDisplayInfo>() as u32,
        )?;

        // Parse response
        unsafe {
            let resp_ptr = &raw const RESP_BUF;
            let resp = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuRespDisplayInfo);

            if resp.hdr.type_ != cmd::RESP_OK_DISPLAY_INFO {
                return Err("GET_DISPLAY_INFO failed");
            }

            // Find first enabled display
            for pmode in &resp.pmodes {
                if pmode.enabled != 0 {
                    return Ok((pmode.r_width, pmode.r_height));
                }
            }

            // Default if no display found — use configured resolution
            Ok(configured_resolution())
        }
    })
}

fn send_command(
    device: &VirtioMmioDevice,
    state: &mut GpuDeviceState,
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

    // Notify device
    device.notify_queue(0);

    // Wait for response.
    // The timeout must be generous: TRANSFER_TO_HOST_2D transfers 4MB (full
    // 1280×800 framebuffer) and QEMU processes this in its event loop.
    // On SMP with 4 vCPUs, QEMU's host-side processing may be delayed by
    // vCPU scheduling. 10M iterations ≈ 50-100ms which is safe.
    let mut timeout = 10_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let ptr = &raw const CTRL_QUEUE;
            read_volatile(&(*ptr).used.idx)
        };
        if used_idx != state.last_used_idx {
            state.last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("GPU command timeout");
        }
        core::hint::spin_loop();
    }

    Ok(())
}

fn with_device_state<F, R>(f: F) -> Result<R, &'static str>
where
    F: FnOnce(&VirtioMmioDevice, &mut GpuDeviceState) -> Result<R, &'static str>,
{
    let _guard = GPU_LOCK.lock();
    let state = unsafe {
        let ptr = &raw mut GPU_DEVICE;
        (*ptr).as_mut().ok_or("GPU device not initialized")?
    };
    let device = VirtioMmioDevice::probe(state.base).ok_or("Device disappeared")?;
    f(&device, state)
}

fn framebuffer_len(state: &GpuDeviceState) -> Result<usize, &'static str> {
    let len = (state.width as usize)
        .saturating_mul(state.height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if len == 0 || len > FB_SIZE {
        return Err("Framebuffer size exceeds static buffer");
    }
    Ok(len)
}

fn send_command_expect_ok(
    device: &VirtioMmioDevice,
    state: &mut GpuDeviceState,
    cmd_len: u32,
) -> Result<(), &'static str> {
    let cmd_phys = virt_to_phys(&raw const CMD_BUF as u64);
    let resp_phys = virt_to_phys(&raw const RESP_BUF as u64);
    send_command(
        device,
        state,
        cmd_phys,
        cmd_len,
        resp_phys,
        core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
    )?;

    unsafe {
        let resp_ptr = &raw const RESP_BUF;
        let resp = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr);
        if resp.type_ != cmd::RESP_OK_NODATA {
            return Err("GPU command failed");
        }
    }
    Ok(())
}

fn create_resource() -> Result<(), &'static str> {
    with_device_state(|device, state| {
        framebuffer_len(state)?;
        unsafe {
            let cmd_ptr = &raw mut CMD_BUF;
            let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuResourceCreate2d);
            *cmd = VirtioGpuResourceCreate2d {
                hdr: VirtioGpuCtrlHdr {
                    type_: cmd::RESOURCE_CREATE_2D,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                resource_id: state.resource_id,
                format: format::B8G8R8A8_UNORM,
                width: state.width,
                height: state.height,
            };
        }
        send_command_expect_ok(
            device,
            state,
            core::mem::size_of::<VirtioGpuResourceCreate2d>() as u32,
        )
    })
}

#[repr(C)]
struct AttachBackingCmd {
    cmd: VirtioGpuResourceAttachBacking,
    entry: VirtioGpuMemEntry,
}

fn attach_backing() -> Result<(), &'static str> {
    with_device_state(|device, state| {
        let fb_len = framebuffer_len(state)? as u32;
        let fb_addr = virt_to_phys(&raw const FRAMEBUFFER as u64);
        unsafe {
            let cmd_ptr = &raw mut CMD_BUF;
            let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut AttachBackingCmd);
            *cmd = AttachBackingCmd {
                cmd: VirtioGpuResourceAttachBacking {
                    hdr: VirtioGpuCtrlHdr {
                        type_: cmd::RESOURCE_ATTACH_BACKING,
                        flags: 0,
                        fence_id: 0,
                        ctx_id: 0,
                        padding: 0,
                    },
                    resource_id: state.resource_id,
                    nr_entries: 1,
                },
                entry: VirtioGpuMemEntry {
                    addr: fb_addr,
                    length: fb_len,
                    padding: 0,
                },
            };
        }
        send_command_expect_ok(
            device,
            state,
            core::mem::size_of::<AttachBackingCmd>() as u32,
        )
    })
}

fn set_scanout() -> Result<(), &'static str> {
    with_device_state(|device, state| {
        unsafe {
            let cmd_ptr = &raw mut CMD_BUF;
            let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuSetScanout);
            *cmd = VirtioGpuSetScanout {
                hdr: VirtioGpuCtrlHdr {
                    type_: cmd::SET_SCANOUT,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                r_x: 0,
                r_y: 0,
                r_width: state.width,
                r_height: state.height,
                scanout_id: 0,
                resource_id: state.resource_id,
            };
        }
        send_command_expect_ok(
            device,
            state,
            core::mem::size_of::<VirtioGpuSetScanout>() as u32,
        )
    })
}

fn transfer_to_host(
    device: &VirtioMmioDevice,
    state: &mut GpuDeviceState,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), &'static str> {
    // The offset is the byte position in the guest's backing buffer where QEMU
    // starts reading. QEMU reads each row h at (offset + stride * h), where
    // stride = resource_width * bpp. For sub-rect transfers, offset must point
    // to (x, y) in the backing buffer so the correct pixels are transferred.
    // With offset=0, QEMU copies from row 0 of the buffer to display position
    // (x, y), producing wrong pixels for any partial flush.
    let stride = state.width as u64 * BYTES_PER_PIXEL as u64;
    let offset = y as u64 * stride + x as u64 * BYTES_PER_PIXEL as u64;

    unsafe {
        let cmd_ptr = &raw mut CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuTransferToHost2d);
        *cmd = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r_x: x,
            r_y: y,
            r_width: width,
            r_height: height,
            offset,
            resource_id: state.resource_id,
            padding: 0,
        };
    }
    send_command_expect_ok(
        device,
        state,
        core::mem::size_of::<VirtioGpuTransferToHost2d>() as u32,
    )
}

fn resource_flush(
    device: &VirtioMmioDevice,
    state: &mut GpuDeviceState,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), &'static str> {
    unsafe {
        let cmd_ptr = &raw mut CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuResourceFlush);
        *cmd = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r_x: x,
            r_y: y,
            r_width: width,
            r_height: height,
            resource_id: state.resource_id,
            padding: 0,
        };
    }
    send_command_expect_ok(
        device,
        state,
        core::mem::size_of::<VirtioGpuResourceFlush>() as u32,
    )
}

/// Flush the entire framebuffer to the display.
pub fn flush() -> Result<(), &'static str> {
    with_device_state(|device, state| {
        fence(Ordering::SeqCst);
        transfer_to_host(device, state, 0, 0, state.width, state.height)?;
        resource_flush(device, state, 0, 0, state.width, state.height)
    })
}

/// Flush a rectangular region of the framebuffer to the display.
pub fn flush_rect(x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str> {
    with_device_state(|device, state| {
        fence(Ordering::SeqCst);
        transfer_to_host(device, state, x, y, width, height)?;
        resource_flush(device, state, x, y, width, height)
    })
}

/// Flush a rectangle without waiting for the GPU lock; returns false if it is held.
///
/// For fatal paths (panic, EL1 fault) where the failing CPU may hold the lock.
pub fn try_flush_rect(x: u32, y: u32, width: u32, height: u32) -> bool {
    let Some(_guard) = GPU_LOCK.try_lock() else {
        return false;
    };
    let state = unsafe {
        let ptr = &raw mut GPU_DEVICE;
        match (*ptr).as_mut() {
            Some(state) => state,
            None => return false,
        }
    };
    let Some(device) = VirtioMmioDevice::probe(state.base) else {
        return false;
    };
    fence(Ordering::SeqCst);
    transfer_to_host(&device, state, x, y, width, height).is_ok()
        && resource_flush(&device, state, x, y, width, height).is_ok()
}

/// Get the framebuffer dimensions
pub fn dimensions() -> Option<(u32, u32)> {
    unsafe {
        let ptr = &raw const GPU_DEVICE;
        (*ptr).as_ref().map(|s| (s.width, s.height))
    }
}

/// Get a mutable reference to the framebuffer pixels
#[allow(dead_code)]
pub fn framebuffer() -> Option<&'static mut [u8]> {
    unsafe {
        let ptr = &raw mut GPU_DEVICE;
        if let Some(state) = (*ptr).as_ref() {
            let len = framebuffer_len(state).ok()?;
            let fb_ptr = &raw mut FRAMEBUFFER;
            Some(&mut (&mut (*fb_ptr).pixels)[..len])
        } else {
            None
        }
    }
}

/// Test the GPU device
pub fn test_device() -> Result<(), &'static str> {
    let (width, height) = dimensions().ok_or("GPU device not initialized")?;
    crate::serial_println!("[virtio-gpu] Device test - Display: {}x{}", width, height);
    crate::serial_println!("[virtio-gpu] Test passed!");
    Ok(())
}

// ─── GUI entry spike (scratch) ───────────────────────────────────────────────

const SPIKE_FLAG_FENCE: u32 = 1;

/// Send one transfer or flush of `rect`, fenced when `fence` is given, and return the
/// response header's (type, flags, fence_id) as the device wrote it.
fn spike_cmd(
    device: &VirtioMmioDevice,
    state: &mut GpuDeviceState,
    transfer: bool,
    rect: (u32, u32, u32, u32),
    fence: Option<u64>,
) -> Result<(u32, u32, u64), &'static str> {
    let (x, y, w, h) = rect;
    let hdr = VirtioGpuCtrlHdr {
        type_: if transfer {
            cmd::TRANSFER_TO_HOST_2D
        } else {
            cmd::RESOURCE_FLUSH
        },
        flags: if fence.is_some() { SPIKE_FLAG_FENCE } else { 0 },
        fence_id: fence.unwrap_or(0),
        ctx_id: 0,
        padding: 0,
    };
    let len = unsafe {
        let cmd_ptr = &raw mut CMD_BUF;
        if transfer {
            let stride = state.width as u64 * BYTES_PER_PIXEL as u64;
            *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuTransferToHost2d) =
                VirtioGpuTransferToHost2d {
                    hdr,
                    r_x: x,
                    r_y: y,
                    r_width: w,
                    r_height: h,
                    offset: y as u64 * stride + x as u64 * BYTES_PER_PIXEL as u64,
                    resource_id: state.resource_id,
                    padding: 0,
                };
            core::mem::size_of::<VirtioGpuTransferToHost2d>()
        } else {
            *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuResourceFlush) =
                VirtioGpuResourceFlush {
                    hdr,
                    r_x: x,
                    r_y: y,
                    r_width: w,
                    r_height: h,
                    resource_id: state.resource_id,
                    padding: 0,
                };
            core::mem::size_of::<VirtioGpuResourceFlush>()
        }
    };
    unsafe {
        let resp_ptr = &raw mut RESP_BUF;
        *((*resp_ptr).data.as_mut_ptr() as *mut VirtioGpuCtrlHdr) = VirtioGpuCtrlHdr {
            type_: 0xDEAD_BEEF,
            flags: 0xDEAD_BEEF,
            fence_id: 0xDEAD_BEEF,
            ctx_id: 0,
            padding: 0,
        };
    }
    let cmd_phys = virt_to_phys(&raw const CMD_BUF as u64);
    let resp_phys = virt_to_phys(&raw const RESP_BUF as u64);
    send_command(
        device,
        state,
        cmd_phys,
        len as u32,
        resp_phys,
        core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
    )?;
    unsafe {
        let resp_ptr = &raw const RESP_BUF;
        let r = (*resp_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr;
        Ok((
            read_volatile(&(*r).type_),
            read_volatile(&(*r).flags),
            read_volatile(&(*r).fence_id),
        ))
    }
}

/// ARM64 QEMU spike: fenced 2D transfers and flushes per serial, then read the full
/// guest backing of the scanout resource and compare it with the formula.
pub fn gui_spike_fenced_2d() {
    use crate::gui_spike::{fnv, pattern_bgrx, FNV_INIT};
    use crate::serial_println;

    let result = with_device_state(|device, state| {
        let (w, h) = (state.width, state.height);
        let len = framebuffer_len(state)?;
        serial_println!(
            "GUI_SPIKE qemu2d begin resource={} scanout=0 mode={}x{} format=B8G8R8A8 backing_bytes={}",
            state.resource_id,
            w,
            h,
            len
        );
        let fb = unsafe { &mut (&mut (*(&raw mut FRAMEBUFFER)).pixels)[..len] };

        // Control: an unfenced transfer must come back without the fence flag.
        let c = spike_cmd(device, state, true, (0, 0, 1, 1), None)?;
        serial_println!(
            "GUI_SPIKE qemu2d control unfenced transfer resp_type={:#x} flags={:#x} fence_id={}",
            c.0,
            c.1,
            c.2
        );

        let damage = |s: u32| -> ([(u32, u32, u32, u32); 3], usize) {
            match s {
                1 => ([(0, 0, w, h), (0, 0, 0, 0), (0, 0, 0, 0)], 1),
                2 => ([(100, 50, 401, 299), (w - 257, h - 129, 257, 129), (0, 0, 0, 0)], 2),
                _ => ([(0, 0, 1, 1), (0, h / 2, w, 1), (w - 1, h - 1, 1, 1)], 3),
            }
        };
        let inside = |r: (u32, u32, u32, u32), x: u32, y: u32| {
            x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
        };
        let mut next_fence: u64 = 0x51_0000;
        let mut all_ok = true;
        for serial in 1..=3u32 {
            let (rects, n) = damage(serial);
            for &r in &rects[..n] {
                for y in r.1..r.1 + r.3 {
                    for x in r.0..r.0 + r.2 {
                        let off = ((y * w + x) as usize) * BYTES_PER_PIXEL;
                        fb[off..off + 4].copy_from_slice(&pattern_bgrx(serial, x, y));
                    }
                }
            }
            fence(Ordering::SeqCst);
            for (kind, transfer) in [("transfer", true), ("flush", false)] {
                let mut answered = 0;
                let first = next_fence;
                for &r in &rects[..n] {
                    let f = next_fence;
                    next_fence += 1;
                    let (t, fl, fid) = spike_cmd(device, state, transfer, r, Some(f))?;
                    let ok = t == cmd::RESP_OK_NODATA && fl & SPIKE_FLAG_FENCE != 0 && fid == f;
                    if ok {
                        answered += 1;
                    } else {
                        all_ok = false;
                        serial_println!(
                            "GUI_SPIKE qemu2d serial={} {} rect={:?} fence={} BAD resp_type={:#x} flags={:#x} fence_id={}",
                            serial, kind, r, f, t, fl, fid
                        );
                    }
                }
                serial_println!(
                    "GUI_SPIKE qemu2d serial={} {}s={} fence_ids={}..={} answered_ok_with_echo={}/{}",
                    serial,
                    kind,
                    n,
                    first,
                    next_fence - 1,
                    answered,
                    n
                );
            }
            // Read the complete guest backing and compare it with the formula.
            fence(Ordering::SeqCst);
            let mut got = FNV_INIT;
            let mut want = FNV_INIT;
            let mut mismatched = 0u64;
            for y in 0..h {
                for x in 0..w {
                    let off = ((y * w + x) as usize) * BYTES_PER_PIXEL;
                    let px = unsafe {
                        [
                            read_volatile(&fb[off]),
                            read_volatile(&fb[off + 1]),
                            read_volatile(&fb[off + 2]),
                            read_volatile(&fb[off + 3]),
                        ]
                    };
                    let mut owner = 1;
                    for s in (1..=serial).rev() {
                        let (rs, k) = damage(s);
                        if rs[..k].iter().any(|&r| inside(r, x, y)) {
                            owner = s;
                            break;
                        }
                    }
                    let e = pattern_bgrx(owner, x, y);
                    for i in 0..4 {
                        got = fnv(got, px[i]);
                        want = fnv(want, e[i]);
                    }
                    if px != e {
                        mismatched += 1;
                    }
                }
            }
            if mismatched != 0 {
                all_ok = false;
            }
            serial_println!(
                "GUI_SPIKE qemu2d serial={} readback bytes={} fnv={:#018x} expected_fnv={:#018x} mismatched_pixels={}",
                serial,
                len,
                got,
                want,
                mismatched
            );
        }
        Ok(all_ok)
    });
    match result {
        Ok(ok) => serial_println!(
            "GUI_SPIKE qemu2d verdict={}",
            if ok { "PASS" } else { "FAIL" }
        ),
        Err(e) => serial_println!("GUI_SPIKE qemu2d verdict=ERROR {}", e),
    }
}
