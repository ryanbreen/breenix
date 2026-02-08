//! VirtIO GPU Device Driver for ARM64 (MMIO Transport)
//!
//! Implements a basic GPU/display driver using VirtIO MMIO transport.
//! Provides framebuffer functionality for simple 2D graphics.

use super::mmio::{VirtioMmioDevice, device_id, VIRTIO_MMIO_BASE, VIRTIO_MMIO_SIZE, VIRTIO_MMIO_COUNT};
use core::ptr::read_volatile;
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
    pmodes: [VirtioGpuDisplayOne; 16],  // VIRTIO_GPU_MAX_SCANOUTS = 16
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
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; 16],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; 16] },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed { flags: 0, idx: 0, ring: [VirtqUsedElem { id: 0, len: 0 }; 16] },
};

// Command/response buffers
#[repr(C, align(64))]
struct CmdBuffer {
    data: [u8; 512],
}

static mut CMD_BUF: CmdBuffer = CmdBuffer { data: [0; 512] };
static mut RESP_BUF: CmdBuffer = CmdBuffer { data: [0; 512] };

// Framebuffer - 1280x800 @ 32bpp = 4MB
const FB_WIDTH: u32 = 1280;
const FB_HEIGHT: u32 = 800;
const FB_SIZE: usize = (FB_WIDTH * FB_HEIGHT * 4) as usize;
const BYTES_PER_PIXEL: usize = 4;
const RESOURCE_ID: u32 = 1;

#[repr(C, align(4096))]
struct Framebuffer {
    pixels: [u8; FB_SIZE],
}

static mut FRAMEBUFFER: Framebuffer = Framebuffer { pixels: [0; FB_SIZE] };

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
                return init_device(&mut device, base);
            }
        }
    }

    Err("No VirtIO GPU device found")
}

fn init_device(device: &mut VirtioMmioDevice, base: u64) -> Result<(), &'static str> {
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

    // Store initial state
    unsafe {
        let ptr = &raw mut GPU_DEVICE;
        *ptr = Some(GpuDeviceState {
            base,
            width: FB_WIDTH,
            height: FB_HEIGHT,
            resource_id: RESOURCE_ID,
            last_used_idx: 0,
        });
    }

    // Get display info
    let (width, height) = get_display_info()?;
    crate::serial_println!("[virtio-gpu] Display: {}x{}", width, height);

    // Update state with actual dimensions
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

            // Default if no display found
            Ok((FB_WIDTH, FB_HEIGHT))
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
        send_command_expect_ok(device, state, core::mem::size_of::<AttachBackingCmd>() as u32)
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
            offset: 0,
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
