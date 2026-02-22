//! VirtIO GPU Device Driver for ARM64 (PCI Transport)
//!
//! Implements a basic GPU/display driver using VirtIO PCI modern transport.
//! Provides framebuffer functionality for simple 2D graphics.
//!
//! This driver reuses the same VirtIO GPU 2D protocol as `gpu_mmio.rs` but
//! communicates via the PCI transport layer (`VirtioPciDevice` from
//! `pci_transport.rs`) instead of MMIO registers.

use super::pci_transport::VirtioPciDevice;
use core::ptr::read_volatile;
use core::sync::atomic::{fence, AtomicBool, Ordering};
use spin::Mutex;

/// Lock protecting the GPU PCI command path (PCI_CMD_BUF, PCI_RESP_BUF,
/// PCI_CTRL_QUEUE, GPU_PCI_STATE).
/// Without this, concurrent callers corrupt the shared command/response
/// buffers and virtqueue state.
static GPU_PCI_LOCK: Mutex<()> = Mutex::new(());

// =============================================================================
// VirtIO GPU Protocol (same as gpu_mmio.rs)
// =============================================================================

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

// =============================================================================
// GPU Protocol Structures
// =============================================================================

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
struct VirtioGpuMemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

/// Resource attach backing command
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceAttachBacking {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
}

/// Transfer to host 2D command
#[repr(C)]
#[derive(Clone, Copy, Default)]
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
struct VirtioGpuResourceFlush {
    hdr: VirtioGpuCtrlHdr,
    r_x: u32,
    r_y: u32,
    r_width: u32,
    r_height: u32,
    resource_id: u32,
    padding: u32,
}

// =============================================================================
// Virtqueue Structures
// =============================================================================

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

/// Static control queue memory (page-aligned for used ring)
#[repr(C, align(4096))]
struct PciCtrlQueueMemory {
    desc: [VirtqDesc; 16],
    avail: VirtqAvail,
    _padding: [u8; 4096 - 256 - 36],
    used: VirtqUsed,
}

// =============================================================================
// Static Buffers (prefixed with PCI_ to avoid conflicts with gpu_mmio.rs)
// =============================================================================

static mut PCI_CTRL_QUEUE: PciCtrlQueueMemory = PciCtrlQueueMemory {
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; 16],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; 16] },
    _padding: [0; 4096 - 256 - 36],
    used: VirtqUsed {
        flags: 0,
        idx: 0,
        ring: [VirtqUsedElem { id: 0, len: 0 }; 16],
    },
};

/// Command/response buffers
#[repr(C, align(64))]
struct PciCmdBuffer {
    data: [u8; 512],
}

static mut PCI_CMD_BUF: PciCmdBuffer = PciCmdBuffer { data: [0; 512] };
static mut PCI_RESP_BUF: PciCmdBuffer = PciCmdBuffer { data: [0; 512] };

// Default framebuffer dimensions (Parallels: set_scanout configures display mode)
// 2560x1600 is the max that fits in the ~16MB GOP BAR0 region on Parallels.
// On a Retina Mac, Parallels 2x-scales this to ~1280x800 window points.
const DEFAULT_FB_WIDTH: u32 = 2560;
const DEFAULT_FB_HEIGHT: u32 = 1600;
// Max supported resolution: 2560x1600 @ 32bpp = ~16.4MB
const FB_MAX_WIDTH: u32 = 2560;
const FB_MAX_HEIGHT: u32 = 1600;
const FB_SIZE: usize = (FB_MAX_WIDTH * FB_MAX_HEIGHT * 4) as usize;
const BYTES_PER_PIXEL: usize = 4;
const RESOURCE_ID: u32 = 1;

// VirtIO standard feature bits
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
// VirtIO GPU feature bits (requested but not required)
#[allow(dead_code)]
const VIRTIO_GPU_F_EDID: u64 = 1 << 1;

#[repr(C, align(4096))]
struct PciFramebuffer {
    pixels: [u8; FB_SIZE],
}

static mut PCI_FRAMEBUFFER: PciFramebuffer = PciFramebuffer { pixels: [0; FB_SIZE] };

// =============================================================================
// GPU PCI Device State
// =============================================================================

/// Combined GPU PCI device state (transport + GPU state)
struct GpuPciDeviceState {
    device: VirtioPciDevice,
    width: u32,
    height: u32,
    resource_id: u32,
    last_used_idx: u16,
}

static mut GPU_PCI_STATE: Option<GpuPciDeviceState> = None;
static GPU_PCI_INITIALIZED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Helpers
// =============================================================================

/// Convert a kernel virtual address to a physical address.
///
/// On QEMU, statics live in the HHDM range (>= 0xFFFF_0000_0000_0000),
/// so phys = virt - HHDM_BASE.
/// On Parallels, the kernel may be identity-mapped via TTBR0, so statics are
/// at their physical addresses already.
#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    if addr >= HHDM_BASE {
        addr - HHDM_BASE
    } else {
        addr // Already a physical address (identity-mapped kernel on Parallels)
    }
}

/// Check if the GPU PCI driver has been initialized.
pub fn is_initialized() -> bool {
    GPU_PCI_INITIALIZED.load(Ordering::Acquire)
}

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the VirtIO GPU PCI device.
///
/// Discovers a VirtIO GPU device on the PCI bus, negotiates features,
/// sets up the control virtqueue, and configures the framebuffer.
pub fn init() -> Result<(), &'static str> {
    // Check if already initialized
    if is_initialized() {
        crate::serial_println!("[virtio-gpu-pci] GPU device already initialized");
        return Ok(());
    }

    crate::serial_println!("[virtio-gpu-pci] Searching for GPU PCI device...");

    // Find VirtIO GPU PCI device (device_id 0x1050 = 0x1040 + 16)
    let pci_dev = crate::drivers::pci::find_device(0x1AF4, 0x1050)
        .ok_or("No VirtIO GPU PCI device found")?;

    crate::serial_println!(
        "[virtio-gpu-pci] Found GPU at PCI {:02x}:{:02x}.{:x}",
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function
    );

    // Probe VirtIO modern transport
    let mut virtio = VirtioPciDevice::probe(pci_dev)
        .ok_or("VirtIO GPU PCI: no modern capabilities")?;

    // Init (reset, negotiate features).
    // VIRTIO_F_VERSION_1 is mandatory for PCI modern transport — without it,
    // Parallels's GPU device accepts the feature set but ignores subsequent
    // state-modifying commands (create_resource, attach_backing, etc.).
    let requested = VIRTIO_F_VERSION_1 | VIRTIO_GPU_F_EDID;
    virtio.init(requested)?;
    let dev_feats = virtio.device_features();
    let negotiated = dev_feats & requested;
    crate::serial_println!(
        "[virtio-gpu-pci] Features: device={:#x} requested={:#x} negotiated={:#x}",
        dev_feats, requested, negotiated
    );

    // Set up control queue (queue 0)
    virtio.select_queue(0);
    let queue_max = virtio.get_queue_num_max();
    crate::serial_println!("[virtio-gpu-pci] Control queue max size: {}", queue_max);

    if queue_max == 0 {
        return Err("Control queue size is 0");
    }

    let queue_size = core::cmp::min(queue_max, 16);
    virtio.set_queue_num(queue_size);

    // Set up queue memory using separate physical addresses (PCI modern transport)
    let queue_phys = virt_to_phys(&raw const PCI_CTRL_QUEUE as u64);

    // Initialize descriptor chain
    unsafe {
        let q = &raw mut PCI_CTRL_QUEUE;
        for i in 0..15 {
            (*q).desc[i].next = (i + 1) as u16;
        }
        (*q).desc[15].next = 0;
        (*q).avail.flags = 0;
        (*q).avail.idx = 0;
        (*q).used.flags = 0;
        (*q).used.idx = 0;
    }

    // Desc table at start, avail ring at +256 (16 descs * 16 bytes), used ring at +4096
    virtio.set_queue_desc(queue_phys);
    virtio.set_queue_avail(queue_phys + 256);
    virtio.set_queue_used(queue_phys + 4096);
    virtio.set_queue_ready(true);

    // Mark device ready
    virtio.driver_ok();

    // Store initial state with default dimensions (will be updated after display query)
    unsafe {
        let ptr = &raw mut GPU_PCI_STATE;
        *ptr = Some(GpuPciDeviceState {
            device: virtio,
            width: DEFAULT_FB_WIDTH,
            height: DEFAULT_FB_HEIGHT,
            resource_id: RESOURCE_ID,
            last_used_idx: 0,
        });
    }
    // Don't set GPU_PCI_INITIALIZED yet — the GPU commands below can fail.
    // If create_resource/attach_backing/set_scanout/flush time out, leaving
    // the flag true would mislead other code into thinking the device is usable.

    // Log physical addresses for diagnostics (DMA correctness)
    let fb_virt = &raw const PCI_FRAMEBUFFER as u64;
    let fb_phys = virt_to_phys(fb_virt);
    let cmd_virt = &raw const PCI_CMD_BUF as u64;
    let cmd_phys = virt_to_phys(cmd_virt);
    let queue_virt = &raw const PCI_CTRL_QUEUE as u64;
    let queue_phys = virt_to_phys(queue_virt);
    crate::serial_println!(
        "[virtio-gpu-pci] DMA addrs: fb={:#x}->{:#x} cmd={:#x}->{:#x} queue={:#x}->{:#x}",
        fb_virt, fb_phys, cmd_virt, cmd_phys, queue_virt, queue_phys
    );

    // Query display info for diagnostics.
    match get_display_info() {
        Ok((w, h)) => {
            crate::serial_println!("[virtio-gpu-pci] Display reports: {}x{}", w, h);
        }
        Err(e) => {
            crate::serial_println!("[virtio-gpu-pci] get_display_info failed: {}", e);
        }
    }

    // Override to our desired resolution.
    // On Parallels, VirtIO GPU set_scanout controls the display MODE (stride,
    // resolution) but actual pixels are read from BAR0 (the GOP address at
    // 0x10000000). We use VirtIO GPU purely to configure a higher resolution
    // than the GOP-reported 1024x768.
    let (use_width, use_height) = (DEFAULT_FB_WIDTH, DEFAULT_FB_HEIGHT);
    crate::serial_println!("[virtio-gpu-pci] Requesting resolution: {}x{}", use_width, use_height);

    // Update state with actual dimensions
    unsafe {
        let ptr = &raw mut GPU_PCI_STATE;
        if let Some(ref mut state) = *ptr {
            state.width = use_width;
            state.height = use_height;
        }
    }

    // Create framebuffer resource and attach backing
    create_resource()?;
    attach_backing()?;
    set_scanout()?;
    flush()?;

    // All GPU setup commands succeeded — now mark as initialized.
    GPU_PCI_INITIALIZED.store(true, Ordering::Release);

    crate::serial_println!("[virtio-gpu-pci] Initialized: {}x{}", use_width, use_height);
    Ok(())
}

// =============================================================================
// Device State Access
// =============================================================================

/// Execute a closure with exclusive access to the GPU PCI device state.
fn with_device_state<F, R>(f: F) -> Result<R, &'static str>
where
    F: FnOnce(&mut GpuPciDeviceState) -> Result<R, &'static str>,
{
    let _guard = GPU_PCI_LOCK.lock();
    let state = unsafe {
        let ptr = &raw mut GPU_PCI_STATE;
        (*ptr).as_mut().ok_or("GPU PCI not initialized")?
    };
    f(state)
}

fn framebuffer_len(state: &GpuPciDeviceState) -> Result<usize, &'static str> {
    let len = (state.width as usize)
        .saturating_mul(state.height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if len == 0 || len > FB_SIZE {
        return Err("Framebuffer size exceeds static buffer");
    }
    Ok(len)
}

// =============================================================================
// Command Submission
// =============================================================================

/// Submit a 2-descriptor command/response chain via the control queue.
///
/// Descriptor 0: command (device reads)
/// Descriptor 1: response (device writes)
/// Then notify the device via PCI transport and spin-wait for completion.
fn send_command(
    state: &mut GpuPciDeviceState,
    cmd_phys: u64,
    cmd_len: u32,
    resp_phys: u64,
    resp_len: u32,
) -> Result<(), &'static str> {
    unsafe {
        let q = &raw mut PCI_CTRL_QUEUE;

        // Descriptor 0: command (device reads)
        (*q).desc[0] = VirtqDesc {
            addr: cmd_phys,
            len: cmd_len,
            flags: DESC_F_NEXT,
            next: 1,
        };

        // Descriptor 1: response (device writes)
        (*q).desc[1] = VirtqDesc {
            addr: resp_phys,
            len: resp_len,
            flags: DESC_F_WRITE,
            next: 0,
        };

        // Add to available ring
        let idx = (*q).avail.idx;
        (*q).avail.ring[(idx % 16) as usize] = 0;
        fence(Ordering::SeqCst);
        (*q).avail.idx = idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify device via PCI transport
    state.device.notify_queue(0);

    // Spin-wait for used ring.
    // The timeout must be generous: TRANSFER_TO_HOST_2D transfers up to 4MB
    // (full framebuffer) and QEMU processes this in its event loop.
    // 10M iterations is safe.
    let mut timeout = 10_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let q = &raw const PCI_CTRL_QUEUE;
            read_volatile(&(*q).used.idx)
        };
        if used_idx != state.last_used_idx {
            state.last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("GPU PCI command timeout");
        }
        core::hint::spin_loop();
    }

    Ok(())
}

/// Send a command and verify the response is RESP_OK_NODATA.
fn send_command_expect_ok(
    state: &mut GpuPciDeviceState,
    cmd_len: u32,
) -> Result<(), &'static str> {
    let cmd_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
    let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);
    send_command(
        state,
        cmd_phys,
        cmd_len,
        resp_phys,
        core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
    )?;

    // Read response — use read_volatile to defeat caching (DMA coherency)
    let resp_type = unsafe {
        let resp_ptr = &raw const PCI_RESP_BUF;
        core::ptr::read_volatile(&(*((*resp_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr)).type_)
    };
    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!("[virtio-gpu-pci] Command failed: resp_type={:#x}", resp_type);
        return Err("GPU PCI command failed");
    }
    Ok(())
}

// =============================================================================
// GPU Commands
// =============================================================================

fn get_display_info() -> Result<(u32, u32), &'static str> {
    with_device_state(|state| {
        let cmd_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
        let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);

        // Prepare GET_DISPLAY_INFO command
        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
            let hdr = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCtrlHdr);
            *hdr = VirtioGpuCtrlHdr {
                type_: cmd::GET_DISPLAY_INFO,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            };
        }

        send_command(
            state,
            cmd_phys,
            core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
            resp_phys,
            core::mem::size_of::<VirtioGpuRespDisplayInfo>() as u32,
        )?;

        // Parse response
        unsafe {
            let resp_ptr = &raw const PCI_RESP_BUF;
            let resp = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuRespDisplayInfo);

            if resp.hdr.type_ != cmd::RESP_OK_DISPLAY_INFO {
                return Err("GET_DISPLAY_INFO failed");
            }

            // Log ALL scanouts for diagnostics
            let mut first_enabled = None;
            for (i, pmode) in resp.pmodes.iter().enumerate() {
                if pmode.r_width > 0 || pmode.r_height > 0 || pmode.enabled != 0 {
                    crate::serial_println!(
                        "[virtio-gpu-pci] Scanout {}: {}x{} enabled={} flags={:#x}",
                        i, pmode.r_width, pmode.r_height, pmode.enabled, pmode.flags
                    );
                    if pmode.enabled != 0 && first_enabled.is_none() {
                        first_enabled = Some((pmode.r_width, pmode.r_height));
                    }
                }
            }

            // Use first enabled scanout, or default
            Ok(first_enabled.unwrap_or((DEFAULT_FB_WIDTH, DEFAULT_FB_HEIGHT)))
        }
    })
}

fn create_resource() -> Result<(), &'static str> {
    with_device_state(|state| {
        framebuffer_len(state)?;
        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
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
            state,
            core::mem::size_of::<VirtioGpuResourceCreate2d>() as u32,
        )
    })
}

#[repr(C)]
struct PciAttachBackingCmd {
    cmd: VirtioGpuResourceAttachBacking,
    entry: VirtioGpuMemEntry,
}

fn attach_backing() -> Result<(), &'static str> {
    with_device_state(|state| {
        let fb_len = framebuffer_len(state)? as u32;
        let fb_addr = virt_to_phys(&raw const PCI_FRAMEBUFFER as u64);
        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
            let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut PciAttachBackingCmd);
            *cmd = PciAttachBackingCmd {
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
        send_command_expect_ok(state, core::mem::size_of::<PciAttachBackingCmd>() as u32)
    })
}

fn set_scanout() -> Result<(), &'static str> {
    with_device_state(|state| {
        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
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
            state,
            core::mem::size_of::<VirtioGpuSetScanout>() as u32,
        )
    })
}

fn transfer_to_host(
    state: &mut GpuPciDeviceState,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), &'static str> {
    // The offset is the byte position in the guest's backing buffer where QEMU
    // starts reading. QEMU reads each row h at (offset + stride * h), where
    // stride = resource_width * bpp. For sub-rect transfers, offset must point
    // to (x, y) in the backing buffer so the correct pixels are transferred.
    let stride = state.width as u64 * BYTES_PER_PIXEL as u64;
    let offset = y as u64 * stride + x as u64 * BYTES_PER_PIXEL as u64;

    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
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
        state,
        core::mem::size_of::<VirtioGpuTransferToHost2d>() as u32,
    )
}

fn resource_flush_cmd(
    state: &mut GpuPciDeviceState,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), &'static str> {
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
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
        state,
        core::mem::size_of::<VirtioGpuResourceFlush>() as u32,
    )
}

// =============================================================================
// Public API
// =============================================================================

/// Flush the entire framebuffer to the display.
pub fn flush() -> Result<(), &'static str> {
    with_device_state(|state| {
        fence(Ordering::SeqCst);
        transfer_to_host(state, 0, 0, state.width, state.height)?;
        resource_flush_cmd(state, 0, 0, state.width, state.height)
    })
}

/// Flush a rectangular region of the framebuffer to the display.
pub fn flush_rect(x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str> {
    with_device_state(|state| {
        fence(Ordering::SeqCst);
        transfer_to_host(state, x, y, width, height)?;
        resource_flush_cmd(state, x, y, width, height)
    })
}

/// Get the framebuffer dimensions.
pub fn dimensions() -> Option<(u32, u32)> {
    unsafe {
        let ptr = &raw const GPU_PCI_STATE;
        (*ptr).as_ref().map(|s| (s.width, s.height))
    }
}

/// Get a mutable reference to the framebuffer pixels.
#[allow(dead_code)]
pub fn framebuffer() -> Option<&'static mut [u8]> {
    unsafe {
        let ptr = &raw mut GPU_PCI_STATE;
        if let Some(state) = (*ptr).as_ref() {
            let len = framebuffer_len(state).ok()?;
            let fb_ptr = &raw mut PCI_FRAMEBUFFER;
            Some(&mut (&mut (*fb_ptr).pixels)[..len])
        } else {
            None
        }
    }
}
