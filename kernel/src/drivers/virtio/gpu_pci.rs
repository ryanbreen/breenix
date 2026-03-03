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
use core::sync::atomic::{fence, AtomicBool, AtomicU32, Ordering};
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

    // 3D commands (VirGL)
    pub const CTX_CREATE: u32 = 0x0200;
    pub const CTX_DESTROY: u32 = 0x0201;
    pub const CTX_ATTACH_RESOURCE: u32 = 0x0202;
    pub const CTX_DETACH_RESOURCE: u32 = 0x0203;
    pub const RESOURCE_CREATE_3D: u32 = 0x0204;
    pub const TRANSFER_TO_HOST_3D: u32 = 0x0205;
    pub const TRANSFER_FROM_HOST_3D: u32 = 0x0206;
    pub const SUBMIT_3D: u32 = 0x0207;

    // Capability commands
    pub const GET_CAPSET_INFO: u32 = 0x0110;

    // Response types
    pub const RESP_OK_NODATA: u32 = 0x1100;
    pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
    pub const RESP_OK_CAPSET_INFO: u32 = 0x1102;
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

/// GET_CAPSET_INFO request
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuGetCapsetInfo {
    hdr: VirtioGpuCtrlHdr,
    capset_index: u32,
    padding: u32,
}

/// GET_CAPSET_INFO response
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuRespCapsetInfo {
    hdr: VirtioGpuCtrlHdr,
    capset_id: u32,
    capset_max_version: u32,
    capset_max_size: u32,
    padding: u32,
}

// VirtIO GPU config space offsets
const GPU_CFG_EVENTS_READ: usize = 0;
const GPU_CFG_EVENTS_CLEAR: usize = 4;
const GPU_CFG_NUM_SCANOUTS: usize = 8;
const GPU_CFG_NUM_CAPSETS: usize = 12;
const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;

// =============================================================================
// VirtIO GPU 3D (VirGL) Protocol Structures
// =============================================================================

/// Create a 3D rendering context
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuCtxCreate {
    hdr: VirtioGpuCtrlHdr,
    nlen: u32,
    context_init: u32, // 0 for VirGL
    debug_name: [u8; 64],
}

/// Attach/detach a resource to/from a 3D context
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuCtxResource {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    padding: u32,
}

/// Create a 3D resource (texture, render target, buffer)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceCreate3d {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    target: u32,
    format: u32,
    bind: u32,
    width: u32,
    height: u32,
    depth: u32,
    array_size: u32,
    last_level: u32,
    nr_samples: u32,
    flags: u32,
    padding: u32,
}

/// Transfer host 3D command — used for both TRANSFER_TO_HOST_3D (0x0205)
/// and TRANSFER_FROM_HOST_3D (0x0206). Copies between guest backing and
/// host-side texture. Linux's DRM driver calls TRANSFER_TO_HOST_3D before
/// RESOURCE_FLUSH even for VirGL-rendered content — it serves as a
/// synchronization point that tells the host the resource is display-ready.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuTransferHost3d {
    hdr: VirtioGpuCtrlHdr,
    box_x: u32,
    box_y: u32,
    box_z: u32,
    box_w: u32,
    box_h: u32,
    box_d: u32,
    offset: u64,
    resource_id: u32,
    level: u32,
    stride: u32,
    layer_stride: u32,
}

/// Submit 3D command buffer header (followed immediately by VirGL command data)
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuCmdSubmit {
    hdr: VirtioGpuCtrlHdr,
    size: u32, // size in bytes of the VirGL command buffer
    // NO padding — VirGL data follows immediately at offset 28
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

/// Larger command buffer for 3D submissions (Submit3D header + VirGL payload).
/// 16KB accommodates shader text + inline vertex data for 12 circle draws.
#[repr(C, align(64))]
struct Pci3dCmdBuffer {
    data: [u8; 16384],
}
static mut PCI_3D_CMD_BUF: Pci3dCmdBuffer = Pci3dCmdBuffer { data: [0; 16384] };


// Default framebuffer dimensions (Parallels: set_scanout configures display mode)
// 1728x1080 matches the QEMU resolution for consistent performance comparison.
const DEFAULT_FB_WIDTH: u32 = 1728;
const DEFAULT_FB_HEIGHT: u32 = 1080;
// Max supported resolution: 2560x1600 @ 32bpp = ~16.4MB
const FB_MAX_WIDTH: u32 = 2560;
const FB_MAX_HEIGHT: u32 = 1600;
const FB_SIZE: usize = (FB_MAX_WIDTH * FB_MAX_HEIGHT * 4) as usize;
const BYTES_PER_PIXEL: usize = 4;
const RESOURCE_ID: u32 = 1;
/// Resource ID for the VirGL 3D render target (with BIND_SCANOUT)
const RESOURCE_3D_ID: u32 = 2;
/// Resource ID for the VirGL vertex buffer
const RESOURCE_VB_ID: u32 = 3;
/// VirGL 3D context ID
const VIRGL_CTX_ID: u32 = 1;
/// Maximum circles we can render per frame
const MAX_CIRCLES: usize = 16;
/// Vertices per circle (triangle fan: center + N perimeter + closing vertex)
const CIRCLE_SEGMENTS: usize = 16;
/// Vertices per circle = center + segments + 1 (close fan)
const VERTS_PER_CIRCLE: usize = CIRCLE_SEGMENTS + 2;
/// Bytes per vertex: position (4×f32) + color (4×f32) = 32 bytes
const BYTES_PER_VERTEX: usize = 32;
/// Vertex buffer size: enough for MAX_CIRCLES circles
const VB_SIZE: usize = MAX_CIRCLES * VERTS_PER_CIRCLE * BYTES_PER_VERTEX;

// VirtIO standard feature bits
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
// VirtIO GPU feature bits (requested but not required)
#[allow(dead_code)]
const VIRTIO_GPU_F_EDID: u64 = 1 << 1;
// VirtIO GPU 3D (VirGL) acceleration
const VIRTIO_GPU_F_VIRGL: u64 = 1 << 0;

/// Whether VirGL 3D acceleration was successfully negotiated with the device.
static VIRGL_ENABLED: AtomicBool = AtomicBool::new(false);
/// Whether SET_SCANOUT has been issued for the 3D resource.
/// Deferred until the first virgl_render_frame call so that the mmap/GOP
/// path keeps working until userspace explicitly opts into VirGL.
static VIRGL_SCANOUT_ACTIVE: AtomicBool = AtomicBool::new(false);


#[repr(C, align(4096))]
struct PciFramebuffer {
    pixels: [u8; FB_SIZE],
}

static mut PCI_FRAMEBUFFER: PciFramebuffer = PciFramebuffer { pixels: [0; FB_SIZE] };

/// Separate backing for 3D resource — NOT shared with the 2D resource.
/// Linux's Mesa/virgl creates independent GEM buffers for each resource.
/// Sharing backing between 2D and 3D resources may cause the hypervisor
/// to handle SET_SCANOUT incorrectly.
const FB_3D_SIZE: usize = (DEFAULT_FB_WIDTH * DEFAULT_FB_HEIGHT * 4) as usize;

#[repr(C, align(4096))]
struct Pci3dFramebuffer {
    pixels: [u8; FB_3D_SIZE],
}
static mut PCI_3D_FRAMEBUFFER: Pci3dFramebuffer = Pci3dFramebuffer { pixels: [0; FB_3D_SIZE] };

// =============================================================================
// GPU PCI Device State
// =============================================================================

/// VirtIO GPU fence flag — tells the host to signal completion via fence_id.
const VIRTIO_GPU_FLAG_FENCE: u32 = 1;

/// Combined GPU PCI device state (transport + GPU state)
struct GpuPciDeviceState {
    device: VirtioPciDevice,
    width: u32,
    height: u32,
    resource_id: u32,
    last_used_idx: u16,
    /// Monotonically increasing fence counter for GPU synchronization.
    /// Each fenced command gets a unique fence_id; the host signals completion
    /// by echoing this ID in the response. Required for TRANSFER_FROM_HOST_3D
    /// to ensure DMA writes complete before reading backing memory.
    next_fence_id: u64,
}

static mut GPU_PCI_STATE: Option<GpuPciDeviceState> = None;
static GPU_PCI_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// GIC INTID (SPI number) allocated for GPU MSI. 0 = polling mode.
static GPU_IRQ: AtomicU32 = AtomicU32::new(0);

/// Set by the interrupt handler to wake the WFI loop in send_command().
static GPU_CMD_COMPLETE: AtomicBool = AtomicBool::new(false);

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

/// Check if VirGL 3D acceleration was negotiated with the device.
pub fn is_virgl_enabled() -> bool {
    VIRGL_ENABLED.load(Ordering::Acquire)
}

/// Disable VirGL for display purposes (e.g., Parallels can't route VirGL output to screen).
pub fn disable_virgl() {
    VIRGL_ENABLED.store(false, Ordering::Release);
}

// =============================================================================
// MSI Interrupt Support
// =============================================================================

/// Set up PCI MSI for the VirtIO GPU through GICv2m.
///
/// Follows the same pattern as xHCI MSI setup: find MSI cap, probe GICv2m,
/// allocate SPI, program MSI registers, configure GIC.
///
/// Returns the allocated SPI number, or 0 if MSI is unavailable.
#[cfg(target_arch = "aarch64")]
fn setup_gpu_msi(pci_dev: &crate::drivers::pci::Device) -> u32 {
    use crate::arch_impl::aarch64::gic;

    // Step 1: Find MSI capability
    let msi_cap = match pci_dev.find_msi_capability() {
        Some(offset) => offset,
        None => {
            crate::serial_println!("[virtio-gpu-pci] No MSI capability found, using polling");
            return 0;
        }
    };

    // Step 2: Ensure GICv2m is probed
    const PARALLELS_GICV2M_BASE: u64 = 0x0225_0000;
    let gicv2m_base = crate::platform_config::gicv2m_base_phys();
    let base = if gicv2m_base != 0 {
        gicv2m_base
    } else if crate::platform_config::probe_gicv2m(PARALLELS_GICV2M_BASE) {
        PARALLELS_GICV2M_BASE
    } else {
        crate::serial_println!("[virtio-gpu-pci] GICv2m not available, using polling");
        return 0;
    };

    // Step 3: Allocate SPI
    let spi = crate::platform_config::allocate_msi_spi();
    if spi == 0 {
        crate::serial_println!("[virtio-gpu-pci] No SPIs available, using polling");
        return 0;
    }

    // Step 4: Program PCI MSI registers
    let msi_address = (base + 0x40) as u32;
    let msi_data = spi as u16;
    pci_dev.configure_msi(msi_cap, msi_address, msi_data);
    pci_dev.disable_intx();

    // Step 5: Configure GIC for this SPI (edge-triggered)
    gic::configure_spi_edge_triggered(spi);

    crate::serial_println!("[virtio-gpu-pci] MSI configured: SPI={}", spi);
    spi
}

/// Handle GPU MSI interrupt — called from exception.rs IRQ dispatch.
///
/// Wakes the WFI loop in send_command() by setting GPU_CMD_COMPLETE.
/// Follows the xHCI pattern: disable SPI, clear pending, ack, re-enable.
#[cfg(target_arch = "aarch64")]
pub fn handle_interrupt() {
    use crate::arch_impl::aarch64::gic;

    let irq = GPU_IRQ.load(Ordering::Relaxed);
    if irq == 0 {
        return;
    }

    gic::disable_spi(irq);
    gic::clear_spi_pending(irq);

    // Read ISR to auto-acknowledge the VirtIO interrupt condition
    if GPU_PCI_INITIALIZED.load(Ordering::Acquire) {
        unsafe {
            let ptr = &raw const GPU_PCI_STATE;
            if let Some(ref state) = *ptr {
                state.device.read_interrupt_status();
            }
        }
    }

    GPU_CMD_COMPLETE.store(true, Ordering::Release);

    gic::clear_spi_pending(irq);
    gic::enable_spi(irq);
}

/// Get the GIC INTID for the GPU interrupt (for exception dispatch).
pub fn get_irq() -> Option<u32> {
    let irq = GPU_IRQ.load(Ordering::Relaxed);
    if irq != 0 { Some(irq) } else { None }
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

    // Find VirtIO GPU PCI device (device_id 0x1050 = 0x1040 + 16)
    let pci_dev = crate::drivers::pci::find_device(0x1AF4, 0x1050)
        .ok_or("No VirtIO GPU PCI device found")?;

    // Probe VirtIO modern transport
    let mut virtio = VirtioPciDevice::probe(pci_dev)
        .ok_or("VirtIO GPU PCI: no modern capabilities")?;

    // Init (reset, negotiate features).
    // VIRTIO_F_VERSION_1 is mandatory for PCI modern transport — without it,
    // Parallels's GPU device accepts the feature set but ignores subsequent
    // state-modifying commands (create_resource, attach_backing, etc.).
    let requested = VIRTIO_F_VERSION_1 | VIRTIO_GPU_F_EDID | VIRTIO_GPU_F_VIRGL;

    // Log raw device-offered features before negotiation
    let device_feats = virtio.read_device_features();
    crate::serial_println!("[virtio-gpu-pci] Device features: {:#018x}", device_feats);
    crate::serial_println!("[virtio-gpu-pci] VIRGL offered: {}", device_feats & VIRTIO_GPU_F_VIRGL != 0);

    virtio.init(requested)?;

    // Check what was actually negotiated
    let negotiated = virtio.device_features() & requested;
    let virgl_on = negotiated & VIRTIO_GPU_F_VIRGL != 0;
    crate::serial_println!("[virtio-gpu-pci] Negotiated: {:#018x} (VIRGL={})", negotiated, virgl_on);
    VIRGL_ENABLED.store(virgl_on, Ordering::Release);
    crate::serial_println!("[virtio-gpu-pci] VIRGL_ENABLED stored={}, readback={}, addr={:#x}",
        virgl_on, VIRGL_ENABLED.load(Ordering::Acquire),
        &VIRGL_ENABLED as *const _ as usize);

    // Set up control queue (queue 0)
    virtio.select_queue(0);
    let queue_max = virtio.get_queue_num_max();

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

    // Cache queue 0 notify address to avoid 2 MMIO reads per notification
    virtio.cache_queue_notify_addr(0);

    // Set up MSI interrupt before driver_ok so the device can signal completions
    #[cfg(target_arch = "aarch64")]
    let msi_spi = setup_gpu_msi(virtio.pci_device());
    #[cfg(not(target_arch = "aarch64"))]
    let msi_spi = 0u32;

    // Mark device ready — MUST happen before sending any commands (Linux: virtio_device_ready())
    virtio.driver_ok();

    // Enable the MSI SPI after driver_ok so the device can actually fire interrupts
    #[cfg(target_arch = "aarch64")]
    if msi_spi != 0 {
        GPU_IRQ.store(msi_spi, Ordering::Release);
        crate::arch_impl::aarch64::gic::enable_spi(msi_spi);
        crate::serial_println!("[virtio-gpu-pci] MSI SPI {} enabled", msi_spi);
    }

    // Read device-specific config (Linux reads num_scanouts + num_capsets here)
    let num_scanouts = virtio.read_config_u32(GPU_CFG_NUM_SCANOUTS);
    let num_capsets = virtio.read_config_u32(GPU_CFG_NUM_CAPSETS);
    crate::serial_println!("[virtio-gpu-pci] Config: num_scanouts={}, num_capsets={}", num_scanouts, num_capsets);

    // Check and clear pending display events (Linux: virtio_gpu_config_changed_work_func)
    let events = virtio.read_config_u32(GPU_CFG_EVENTS_READ);
    if events & VIRTIO_GPU_EVENT_DISPLAY != 0 {
        crate::serial_println!("[virtio-gpu-pci] Clearing pending DISPLAY event (events_read={:#x})", events);
        virtio.write_config_u32(GPU_CFG_EVENTS_CLEAR, events & VIRTIO_GPU_EVENT_DISPLAY);
    }

    // Store initial state with default dimensions (will be updated after display query)
    unsafe {
        let ptr = &raw mut GPU_PCI_STATE;
        *ptr = Some(GpuPciDeviceState {
            device: virtio,
            width: DEFAULT_FB_WIDTH,
            height: DEFAULT_FB_HEIGHT,
            resource_id: RESOURCE_ID,
            last_used_idx: 0,
            next_fence_id: 1,
        });
    }
    // Don't set GPU_PCI_INITIALIZED yet — the GPU commands below can fail.

    // GET_CAPSET_INFO for each capset (Linux does this before GET_DISPLAY_INFO)
    for idx in 0..num_capsets {
        match get_capset_info(idx) {
            Ok((id, max_ver, max_size)) => {
                crate::serial_println!("[virtio-gpu-pci] Capset {}: id={}, max_ver={}, max_size={}",
                    idx, id, max_ver, max_size);
            }
            Err(e) => {
                crate::serial_println!("[virtio-gpu-pci] GET_CAPSET_INFO[{}] failed: {}", idx, e);
            }
        }
    }

    // Query display info to see what Parallels reports as native resolution.
    let display_dims = get_display_info();
    match display_dims {
        Ok((dw, dh)) => crate::serial_println!("[virtio-gpu-pci] Display reports: {}x{}", dw, dh),
        Err(e) => crate::serial_println!("[virtio-gpu-pci] GET_DISPLAY_INFO failed: {}", e),
    }

    // Always use our configured resolution. GET_DISPLAY_INFO reports the
    // Parallels native display (e.g. 2560x1600 on Retina), but we want to
    // control the rendering resolution for performance. The VirtIO GPU
    // SET_SCANOUT will tell Parallels to use our chosen resolution.
    let (use_width, use_height) = (DEFAULT_FB_WIDTH, DEFAULT_FB_HEIGHT);

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

    // Signal that we're waiting for a completion, then notify device
    GPU_CMD_COMPLETE.store(false, Ordering::Release);
    state.device.notify_queue_fast(0);

    // Wait for used ring update — WFI if MSI is available, spin_loop otherwise.
    let use_msi = GPU_IRQ.load(Ordering::Relaxed) != 0;
    let mut timeout = 10_000_000u32;
    loop {
        fence(Ordering::Acquire);
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
        if use_msi {
            // WFI halts the vCPU until an interrupt arrives. The hypervisor
            // processes the VirtIO command while the guest is halted, then
            // delivers the MSI interrupt to wake us.
            #[cfg(target_arch = "aarch64")]
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
            #[cfg(not(target_arch = "aarch64"))]
            core::hint::spin_loop();
        } else {
            core::hint::spin_loop();
        }
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
    let (resp_type, resp_flags, resp_fence) = unsafe {
        let resp_ptr = &raw const PCI_RESP_BUF;
        let hdr = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr);
        (
            core::ptr::read_volatile(&hdr.type_),
            core::ptr::read_volatile(&hdr.flags),
            core::ptr::read_volatile(&hdr.fence_id),
        )
    };
    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!("[virtio-gpu-pci] Command failed: resp_type={:#x} flags={:#x} fence={}",
            resp_type, resp_flags, resp_fence);
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

            let mut first_enabled = None;
            for (_i, pmode) in resp.pmodes.iter().enumerate() {
                if pmode.enabled != 0 && first_enabled.is_none() {
                    first_enabled = Some((pmode.r_width, pmode.r_height));
                }
            }

            // Use first enabled scanout, or default
            Ok(first_enabled.unwrap_or((DEFAULT_FB_WIDTH, DEFAULT_FB_HEIGHT)))
        }
    })
}

/// Query capability set info (Linux: virtio_gpu_get_capsets).
/// Returns (capset_id, max_version, max_size).
fn get_capset_info(capset_index: u32) -> Result<(u32, u32, u32), &'static str> {
    with_device_state(|state| {
        let cmd_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
        let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);

        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
            let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuGetCapsetInfo);
            *cmd = VirtioGpuGetCapsetInfo {
                hdr: VirtioGpuCtrlHdr {
                    type_: cmd::GET_CAPSET_INFO,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                capset_index,
                padding: 0,
            };
        }

        send_command(
            state,
            cmd_phys,
            core::mem::size_of::<VirtioGpuGetCapsetInfo>() as u32,
            resp_phys,
            core::mem::size_of::<VirtioGpuRespCapsetInfo>() as u32,
        )?;

        unsafe {
            let resp_ptr = &raw const PCI_RESP_BUF;
            let resp = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuRespCapsetInfo);
            let resp_type = core::ptr::read_volatile(&resp.hdr.type_);

            if resp_type != cmd::RESP_OK_CAPSET_INFO {
                crate::serial_println!("[virtio-gpu-pci] GET_CAPSET_INFO: unexpected resp_type={:#x} (expected {:#x})",
                    resp_type, cmd::RESP_OK_CAPSET_INFO);
                return Err("GET_CAPSET_INFO failed");
            }

            Ok((
                core::ptr::read_volatile(&resp.capset_id),
                core::ptr::read_volatile(&resp.capset_max_version),
                core::ptr::read_volatile(&resp.capset_max_size),
            ))
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
// 3D (VirGL) Command Helpers
// =============================================================================

/// Create a VirGL 3D rendering context.
fn virgl_ctx_create_cmd(state: &mut GpuPciDeviceState, ctx_id: u32, name: &[u8]) -> Result<(), &'static str> {
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCtxCreate);
        let mut debug_name = [0u8; 64];
        let copy_len = name.len().min(63);
        debug_name[..copy_len].copy_from_slice(&name[..copy_len]);
        *cmd = VirtioGpuCtxCreate {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::CTX_CREATE,
                flags: 0,
                fence_id: 0,
                ctx_id,
                padding: 0,
            },
            nlen: copy_len as u32,
            context_init: 0, // VirGL context
            debug_name,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuCtxCreate>() as u32)
}

/// Attach a resource to a VirGL context.
fn virgl_ctx_attach_resource_cmd(state: &mut GpuPciDeviceState, ctx_id: u32, resource_id: u32) -> Result<(), &'static str> {
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCtxResource);
        *cmd = VirtioGpuCtxResource {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::CTX_ATTACH_RESOURCE,
                flags: 0,
                fence_id: 0,
                ctx_id,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuCtxResource>() as u32)
}

/// Create a 3D resource (texture / render target / buffer).
fn virgl_resource_create_3d_cmd(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
    target: u32,
    fmt: u32,
    bind: u32,
    width: u32,
    height: u32,
    depth: u32,
    array_size: u32,
) -> Result<(), &'static str> {
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuResourceCreate3d);
        *cmd = VirtioGpuResourceCreate3d {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::RESOURCE_CREATE_3D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            target,
            format: fmt,
            bind,
            width,
            height,
            depth,
            array_size,
            last_level: 0,
            nr_samples: 0,
            flags: 0,
            padding: 0,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuResourceCreate3d>() as u32)
}

/// Attach backing memory to a 3D resource.
///
/// Uses PCI_3D_FRAMEBUFFER (separate BSS RAM) as the backing store.
/// CRITICAL: Must NOT share backing with the 2D resource (PCI_FRAMEBUFFER).
/// Linux's Mesa/virgl creates independent GEM buffers for each resource.
/// Sharing backing may cause the hypervisor to mishandle SET_SCANOUT.
fn virgl_attach_backing_cmd(state: &mut GpuPciDeviceState, resource_id: u32) -> Result<(), &'static str> {
    let fb_addr = virt_to_phys(&raw const PCI_3D_FRAMEBUFFER as u64);
    let actual_len = (state.width * state.height * 4).min(FB_3D_SIZE as u32);
    crate::serial_println!("[virgl] attach_backing: 3D RAM phys=0x{:x}, len={} (SEPARATE from 2D)", fb_addr, actual_len);
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
                resource_id,
                nr_entries: 1,
            },
            entry: VirtioGpuMemEntry {
                addr: fb_addr,
                length: actual_len,
                padding: 0,
            },
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<PciAttachBackingCmd>() as u32)
}

/// Flush a specific resource to the display (SET_SCANOUT must point at it first).
fn resource_flush_3d(state: &mut GpuPciDeviceState, resource_id: u32) -> Result<(), &'static str> {
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
            r_x: 0,
            r_y: 0,
            r_width: state.width,
            r_height: state.height,
            resource_id,
            padding: 0,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuResourceFlush>() as u32)
}

/// Transfer a 3D resource from guest backing to host texture (upload).
///
/// NOTE: Not used for VirGL-rendered 3D resources — the host GPU already has
/// the rendered data. Kept for potential future use with CPU-written resources.
#[allow(dead_code)]
fn transfer_to_host_3d(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<(), &'static str> {
    let stride = state.width * 4;
    let offset = (y as u64) * (stride as u64) + (x as u64) * 4;

    let fence_id = state.next_fence_id;
    state.next_fence_id += 1;

    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuTransferHost3d);
        *cmd = VirtioGpuTransferHost3d {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::TRANSFER_TO_HOST_3D,
                flags: VIRTIO_GPU_FLAG_FENCE,
                fence_id,
                ctx_id: VIRGL_CTX_ID,
                padding: 0,
            },
            box_x: x,
            box_y: y,
            box_z: 0,
            box_w: w,
            box_h: h,
            box_d: 1,
            offset,
            resource_id,
            level: 0,
            stride,
            layer_stride: 0,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuTransferHost3d>() as u32)
}

/// Transfer a 3D resource from host texture to guest backing (readback/download).
///
/// After VirGL renders to the host GPU texture, this copies the rendered pixels
/// back into the resource's guest-side backing memory. If the backing is BAR0,
/// this is a host-side DMA that writes directly to the display framebuffer —
/// bypassing the 6 MB/s guest CPU MMIO bottleneck entirely.
#[allow(dead_code)]
fn transfer_from_host_3d(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<(), &'static str> {
    let stride = state.width * 4;
    let offset = (y as u64) * (stride as u64) + (x as u64) * 4;

    let fence_id = state.next_fence_id;
    state.next_fence_id += 1;

    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuTransferHost3d);
        *cmd = VirtioGpuTransferHost3d {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::TRANSFER_FROM_HOST_3D,
                flags: VIRTIO_GPU_FLAG_FENCE,
                fence_id,
                ctx_id: VIRGL_CTX_ID,
                padding: 0,
            },
            box_x: x,
            box_y: y,
            box_z: 0,
            box_w: w,
            box_h: h,
            box_d: 1,
            offset,
            resource_id,
            level: 0,
            stride,
            layer_stride: 0,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuTransferHost3d>() as u32)
}

/// Submit a VirGL command buffer via SUBMIT_3D.
///
/// Uses a 3-descriptor chain matching the Linux kernel driver layout:
///   Desc 0: VirtioGpuCmdSubmit header (device-readable)
///   Desc 1: VirGL command data (device-readable)
///   Desc 2: Response header (device-writable)
fn virgl_submit_3d_cmd(
    state: &mut GpuPciDeviceState,
    ctx_id: u32,
    cmds: &[u32],
) -> Result<(), &'static str> {
    let payload_bytes = cmds.len() * 4;

    if payload_bytes > 16384 {
        return Err("VirGL command buffer too large");
    }

    // Allocate a fence ID for this submission so the host signals completion
    let fence_id = state.next_fence_id;
    state.next_fence_id += 1;

    // Write the Submit3D header into PCI_CMD_BUF
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let hdr = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCmdSubmit);
        *hdr = VirtioGpuCmdSubmit {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::SUBMIT_3D,
                flags: VIRTIO_GPU_FLAG_FENCE,
                fence_id,
                ctx_id,
                padding: 0,
            },
            size: payload_bytes as u32,
        };
    }

    // Copy VirGL command data into PCI_3D_CMD_BUF
    unsafe {
        let buf_ptr = &raw mut PCI_3D_CMD_BUF;
        let dst = (*buf_ptr).data.as_mut_ptr() as *mut u32;
        core::ptr::copy_nonoverlapping(cmds.as_ptr(), dst, cmds.len());
    }

    let hdr_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
    let data_phys = virt_to_phys(&raw const PCI_3D_CMD_BUF as u64);
    let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);
    let hdr_len = core::mem::size_of::<VirtioGpuCmdSubmit>() as u32;
    let resp_len = core::mem::size_of::<VirtioGpuCtrlHdr>() as u32;

    // Build a 3-descriptor chain
    unsafe {
        let q = &raw mut PCI_CTRL_QUEUE;

        // Desc 0: Submit3D header (device reads)
        (*q).desc[0] = VirtqDesc {
            addr: hdr_phys,
            len: hdr_len,
            flags: DESC_F_NEXT,
            next: 1,
        };

        // Desc 1: VirGL command payload (device reads)
        (*q).desc[1] = VirtqDesc {
            addr: data_phys,
            len: payload_bytes as u32,
            flags: DESC_F_NEXT,
            next: 2,
        };

        // Desc 2: Response (device writes)
        (*q).desc[2] = VirtqDesc {
            addr: resp_phys,
            len: resp_len,
            flags: DESC_F_WRITE,
            next: 0,
        };

        // Add to available ring
        let idx = (*q).avail.idx;
        (*q).avail.ring[(idx % 16) as usize] = 0; // head of chain = desc 0
        fence(Ordering::SeqCst);
        (*q).avail.idx = idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }

    // Notify and wait for completion — WFI if MSI available
    GPU_CMD_COMPLETE.store(false, Ordering::Release);
    state.device.notify_queue_fast(0);

    let use_msi = GPU_IRQ.load(Ordering::Relaxed) != 0;
    let mut timeout = 10_000_000u32;
    loop {
        fence(Ordering::Acquire);
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
            return Err("SUBMIT_3D timeout");
        }
        if use_msi {
            #[cfg(target_arch = "aarch64")]
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
            #[cfg(not(target_arch = "aarch64"))]
            core::hint::spin_loop();
        } else {
            core::hint::spin_loop();
        }
    }

    // Check response — read fence info to verify the host echoed our fence_id
    let (resp_type, resp_flags, resp_fence) = unsafe {
        let resp_ptr = &raw const PCI_RESP_BUF;
        let hdr = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr);
        (
            core::ptr::read_volatile(&hdr.type_),
            core::ptr::read_volatile(&hdr.flags),
            core::ptr::read_volatile(&hdr.fence_id),
        )
    };
    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!("[virtio-gpu-pci] SUBMIT_3D failed: resp={:#x} flags={:#x} fence={}",
            resp_type, resp_flags, resp_fence);
        return Err("SUBMIT_3D command failed");
    }
    // Log fence acknowledgement periodically (init + every 500th frame)
    if fence_id <= 5 || fence_id % 500 == 0 {
        crate::serial_println!("[virgl] SUBMIT_3D OK: sent fence={} resp_flags={:#x} resp_fence={}",
            fence_id, resp_flags, resp_fence);
    }
    Ok(())
}

/// Wait for the host to confirm a GPU fence has completed.
///
/// Parallels returns SUBMIT_3D responses immediately (resp_flags=0x0) before
/// the GPU work finishes. The actual fence completion is reported in
/// subsequent command responses via resp_fence. This function sends NOP
/// SUBMIT_3D commands and polls until resp_fence >= target_fence_id.
#[allow(dead_code)]
fn virgl_fence_sync(state: &mut GpuPciDeviceState, target_fence_id: u64) -> Result<(), &'static str> {
    use super::virgl::CommandBuffer;

    // Try up to 100 rounds of polling (each takes ~50-100us on Parallels)
    for _ in 0..100 {
        let mut cmdbuf = CommandBuffer::new();
        cmdbuf.set_sub_ctx(1); // NOP — just re-sets the active sub-context

        let payload = cmdbuf.as_slice();
        let payload_bytes = payload.len() * 4;

        let fence_id = state.next_fence_id;
        state.next_fence_id += 1;

        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
            let hdr = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCmdSubmit);
            *hdr = VirtioGpuCmdSubmit {
                hdr: VirtioGpuCtrlHdr {
                    type_: cmd::SUBMIT_3D,
                    flags: VIRTIO_GPU_FLAG_FENCE,
                    fence_id,
                    ctx_id: VIRGL_CTX_ID,
                    padding: 0,
                },
                size: payload_bytes as u32,
            };

            let buf_ptr = &raw mut PCI_3D_CMD_BUF;
            let dst = (*buf_ptr).data.as_mut_ptr() as *mut u32;
            core::ptr::copy_nonoverlapping(payload.as_ptr(), dst, payload.len());
        }

        let hdr_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
        let data_phys = virt_to_phys(&raw const PCI_3D_CMD_BUF as u64);
        let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);
        let hdr_len = core::mem::size_of::<VirtioGpuCmdSubmit>() as u32;
        let resp_len = core::mem::size_of::<VirtioGpuCtrlHdr>() as u32;

        unsafe {
            let q = &raw mut PCI_CTRL_QUEUE;
            (*q).desc[0] = VirtqDesc { addr: hdr_phys, len: hdr_len, flags: DESC_F_NEXT, next: 1 };
            (*q).desc[1] = VirtqDesc { addr: data_phys, len: payload_bytes as u32, flags: DESC_F_NEXT, next: 2 };
            (*q).desc[2] = VirtqDesc { addr: resp_phys, len: resp_len, flags: DESC_F_WRITE, next: 0 };
            let idx = (*q).avail.idx;
            (*q).avail.ring[(idx % 16) as usize] = 0;
            fence(Ordering::SeqCst);
            (*q).avail.idx = idx.wrapping_add(1);
            fence(Ordering::SeqCst);
        }

        state.device.notify_queue(0);

        // Spin-wait for response
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
            if timeout == 0 { return Err("fence sync timeout"); }
            core::hint::spin_loop();
        }

        // Check if the host reported our target fence as complete
        let resp_fence = unsafe {
            let resp_ptr = &raw const PCI_RESP_BUF;
            let hdr = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr);
            core::ptr::read_volatile(&hdr.fence_id)
        };

        if resp_fence >= target_fence_id {
            return Ok(());
        }
    }

    Err("fence sync: target fence never completed")
}

/// Set scanout to a specific resource ID (used for 3D render targets).
fn set_scanout_resource(state: &mut GpuPciDeviceState, resource_id: u32) -> Result<(), &'static str> {
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
            resource_id,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuSetScanout>() as u32)
}

// =============================================================================
// Public API (2D)
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

/// Send only a RESOURCE_FLUSH command without TRANSFER_TO_HOST_2D.
///
/// Used in GOP hybrid mode where pixels are already in BAR0 (the display
/// scanout memory). The RESOURCE_FLUSH tells Parallels which region changed
/// so it can update the host window, without the overhead of a DMA transfer
/// from PCI_FRAMEBUFFER (which isn't used in hybrid mode).
pub fn resource_flush_only(x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str> {
    with_device_state(|state| {
        fence(Ordering::SeqCst);
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

/// Get a mutable reference to the PCI_FRAMEBUFFER pixels.
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

// =============================================================================
// Public API (3D / VirGL)
// =============================================================================

/// Ball descriptor passed from userspace for GPU rendering.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirglBall {
    /// X position in pixels (f32 bits)
    pub x: f32,
    /// Y position in pixels (f32 bits)
    pub y: f32,
    /// Radius in pixels (f32 bits)
    pub radius: f32,
    /// Color as [R, G, B, A] each 0.0-1.0
    pub color: [f32; 4],
}

/// Render a frame with the VirGL GPU pipeline.
///
/// Clears to background color, draws circles for each ball, submits to host
/// GPU, then issues RESOURCE_FLUSH to display the result.
pub fn virgl_render_frame(
    balls: &[VirglBall],
    bg_r: f32,
    bg_g: f32,
    bg_b: f32,
) -> Result<(), &'static str> {
    use super::virgl::{CommandBuffer, pipe};

    static FRAME_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let frame = FRAME_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let verbose = frame < 3 || frame % 500 == 0;
    if verbose {
        crate::serial_println!("[virgl] render_frame #{} ({} balls)", frame, balls.len());
    }

    if !is_virgl_enabled() {
        return Err("VirGL not enabled");
    }

    if !VIRGL_SCANOUT_ACTIVE.load(Ordering::Acquire) {
        VIRGL_SCANOUT_ACTIVE.store(true, Ordering::Release);
        crate::serial_println!("[virgl] first VirGL frame #{}", frame);
    }

    let (width, height) = match dimensions() {
        Some(d) => d,
        None => return Err("GPU not initialized"),
    };
    let fw = width as f32;
    let fh = height as f32;

    let mut cmdbuf = CommandBuffer::new();
    if verbose {
        crate::serial_println!("[virgl] frame #{}: cmdbuf created, setting FB state", frame);
    }

    cmdbuf.set_sub_ctx(1);

    // Re-emit ALL pipeline state each frame. Mesa's virgl driver re-emits
    // dirty state before every draw; Parallels may reset context state between
    // SUBMIT_3D batches, so we must not assume anything persists.
    cmdbuf.bind_shader(1, pipe::SHADER_VERTEX);
    cmdbuf.bind_shader(2, pipe::SHADER_FRAGMENT);
    cmdbuf.bind_object(1, super::virgl::OBJ_BLEND);
    cmdbuf.bind_object(1, super::virgl::OBJ_DSA);
    cmdbuf.bind_object(1, super::virgl::OBJ_RASTERIZER);
    cmdbuf.bind_object(1, super::virgl::OBJ_VERTEX_ELEMENTS);
    cmdbuf.set_viewport(fw, fh);
    cmdbuf.set_framebuffer_state(0, &[1]); // surface_handle=1, no depth

    // Clear to background color
    cmdbuf.clear_color(bg_r, bg_g, bg_b, 1.0);

    // For each ball, generate a triangle fan and draw it
    let ball_count = balls.len().min(MAX_CIRCLES);
    if verbose {
        crate::serial_println!("[virgl] frame #{}: drawing {} balls (with full state re-emit)", frame, ball_count);
    }

    for (i, ball) in balls[..ball_count].iter().enumerate() {
        let cx = ball.x;
        let cy = ball.y;
        let r = ball.radius;
        let [cr, cg, cb, ca] = ball.color;

        // Convert pixel coords to NDC: x_ndc = (2*x/width - 1), y_ndc = (1 - 2*y/height)
        // VirGL with our viewport transform already maps clip coords to screen pixels,
        // but the vertex shader outputs POSITION in clip space. With our viewport of
        // (width/2, -height/2) scale + (width/2, height/2) translate, clip space
        // [-1,1] maps to [0,width] and [1,-1] maps to [0,height].
        let cx_ndc = 2.0 * cx / fw - 1.0;
        let cy_ndc = 1.0 - 2.0 * cy / fh;
        let rx_ndc = 2.0 * r / fw;
        let ry_ndc = 2.0 * r / fh;

        // Build triangle fan: center + CIRCLE_SEGMENTS perimeter + 1 closing vertex
        let mut verts = [0u32; VERTS_PER_CIRCLE * 8]; // 8 u32 per vertex (pos4 + col4)

        // Center vertex
        verts[0] = cx_ndc.to_bits();
        verts[1] = cy_ndc.to_bits();
        verts[2] = 0f32.to_bits(); // z = 0
        verts[3] = 1.0f32.to_bits(); // w = 1
        verts[4] = cr.to_bits();
        verts[5] = cg.to_bits();
        verts[6] = cb.to_bits();
        verts[7] = ca.to_bits();

        // Perimeter vertices + closing vertex
        // Precomputed cos/sin for 16-segment circle (2π/16 = π/8 increments)
        const COS_TABLE: [f32; 17] = [
            1.0, 0.92388, 0.70711, 0.38268, 0.0,
            -0.38268, -0.70711, -0.92388, -1.0,
            -0.92388, -0.70711, -0.38268, 0.0,
            0.38268, 0.70711, 0.92388, 1.0, // closing = first
        ];
        const SIN_TABLE: [f32; 17] = [
            0.0, 0.38268, 0.70711, 0.92388, 1.0,
            0.92388, 0.70711, 0.38268, 0.0,
            -0.38268, -0.70711, -0.92388, -1.0,
            -0.92388, -0.70711, -0.38268, 0.0, // closing = first
        ];
        for seg in 0..=CIRCLE_SEGMENTS {
            let cos_a = COS_TABLE[seg];
            let sin_a = SIN_TABLE[seg];
            let vx = cx_ndc + rx_ndc * cos_a;
            let vy = cy_ndc + ry_ndc * sin_a;
            let base = (seg + 1) * 8;
            verts[base] = vx.to_bits();
            verts[base + 1] = vy.to_bits();
            verts[base + 2] = 0f32.to_bits();
            verts[base + 3] = 1.0f32.to_bits();
            verts[base + 4] = cr.to_bits();
            verts[base + 5] = cg.to_bits();
            verts[base + 6] = cb.to_bits();
            verts[base + 7] = ca.to_bits();
        }

        let vb_offset = (i * VERTS_PER_CIRCLE * BYTES_PER_VERTEX) as u32;
        let vb_bytes = (VERTS_PER_CIRCLE * BYTES_PER_VERTEX) as u32;

        // Upload vertex data inline
        cmdbuf.resource_inline_write(
            RESOURCE_VB_ID,
            vb_offset,
            vb_bytes,
            &verts[..VERTS_PER_CIRCLE * 8],
        );

        // Bind vertex buffer with correct offset for this circle
        cmdbuf.set_vertex_buffers(&[(BYTES_PER_VERTEX as u32, vb_offset, RESOURCE_VB_ID)]);

        // Draw triangle fan
        cmdbuf.draw_vbo(
            0,                          // start = 0 (relative to VB offset)
            VERTS_PER_CIRCLE as u32,    // count
            pipe::PRIM_TRIANGLE_FAN,
            (VERTS_PER_CIRCLE - 1) as u32, // max_index
        );
    }

    // Submit VirGL commands to host GPU
    if verbose {
        crate::serial_println!("[virgl] frame #{}: submitting {} DWORDs ({} bytes)",
            frame, cmdbuf.as_slice().len(), cmdbuf.byte_len());
    }
    match virgl_submit(cmdbuf.as_slice()) {
        Ok(()) => {
            if verbose {
                crate::serial_println!("[virgl] frame #{}: SUBMIT_3D done", frame);
            }
        }
        Err(e) => {
            crate::serial_println!("[virgl] frame #{}: SUBMIT_3D FAILED: {}", frame, e);
            return Err(e);
        }
    }

    // SET_SCANOUT only on first frame (scanout target doesn't change between frames).
    // RESOURCE_FLUSH every frame to tell the hypervisor to re-scan the texture.
    static SCANOUT_SET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    match with_device_state(|state| {
        if !SCANOUT_SET.load(core::sync::atomic::Ordering::Relaxed) {
            set_scanout_resource(state, RESOURCE_3D_ID)?;
            SCANOUT_SET.store(true, core::sync::atomic::Ordering::Relaxed);
        }
        resource_flush_3d(state, RESOURCE_3D_ID)
    }) {
        Ok(()) => {}
        Err(e) => {
            crate::serial_println!("[virgl] frame #{}: SET_SCANOUT/FLUSH FAILED: {}", frame, e);
            return Err(e);
        }
    }

    Ok(())
}

/// Submit a VirGL command buffer for the active 3D context.
///
/// `cmds` is a slice of u32 DWORDs from a VirGL CommandBuffer.
pub fn virgl_submit(cmds: &[u32]) -> Result<(), &'static str> {
    with_device_state(|state| {
        virgl_submit_3d_cmd(state, VIRGL_CTX_ID, cmds)
    })
}

/// Copy PCI_3D_FRAMEBUFFER (RAM) → BAR0 (display memory).
///
/// After TRANSFER_FROM_HOST_3D copies GPU-rendered pixels to PCI_3D_FRAMEBUFFER,
/// this copies them to BAR0 so they appear on screen.
#[allow(dead_code)]
fn copy_3d_framebuffer_to_bar0(width: u32, height: u32) {
    let bar0_virt = crate::graphics::arm64_fb::gop_framebuffer();
    let fb_bytes = (width * height * 4) as usize;
    if let Some(bar0) = bar0_virt {
        let copy_len = fb_bytes.min(bar0.len()).min(FB_3D_SIZE);
        unsafe {
            let src = &raw const PCI_3D_FRAMEBUFFER;
            core::ptr::copy_nonoverlapping(
                (*src).pixels.as_ptr(),
                bar0.as_mut_ptr(),
                copy_len,
            );
        }
    }
}

/// Flush the VirGL render target to the display.
/// SET_SCANOUT + RESOURCE_FLUSH — matching Linux's display path.
pub fn virgl_flush() -> Result<(), &'static str> {
    if !is_virgl_enabled() {
        return Err("VirGL display not available");
    }
    with_device_state(|state| {
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        resource_flush_3d(state, RESOURCE_3D_ID)
    })
}

// =============================================================================
// VirGL Initialization (Phase 3: proof-of-pipeline)
// =============================================================================

/// Initialize the VirGL 3D pipeline if VIRGL was negotiated.
///
/// Creates a 3D rendering context, a render target resource matching the
/// scanout dimensions, submits a clear command, and sets scanout to the
/// 3D resource. If successful, the screen shows a solid color rendered
/// entirely by the host GPU — no BAR0 pixel writes.
pub fn virgl_init() -> Result<(), &'static str> {
    use super::virgl::{CommandBuffer, format as vfmt, pipe};

    crate::serial_println!("[virgl_init] VIRGL_ENABLED={} addr={:#x}",
        VIRGL_ENABLED.load(Ordering::Acquire),
        &VIRGL_ENABLED as *const _ as usize);
    if !is_virgl_enabled() {
        return Err("VirGL not supported");
    }

    crate::serial_println!("[virtio-gpu-pci] Initializing VirGL 3D pipeline (v10: separate backing + fence sync + capset init)...");

    let (width, height) = dimensions().ok_or("GPU not initialized")?;

    // Step 1: Create 3D context
    with_device_state(|state| {
        virgl_ctx_create_cmd(state, VIRGL_CTX_ID, b"breenix")
    })?;
    crate::serial_println!("[virgl] Step 1: context created (ctx_id={})", VIRGL_CTX_ID);

    // Step 2: Create 3D resource with bind flags matching Linux Mesa/virgl.
    // Linux strace shows bind=0x0014000a = RENDER_TARGET|SAMPLER_VIEW|SCANOUT|SHARED.
    // CRITICAL: Must use B8G8R8X8_UNORM (XRGB8888) — ARGB8888 causes EINVAL.
    let bind_flags = pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW
                   | pipe::BIND_SCANOUT | pipe::BIND_SHARED;
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_3D_ID,
            pipe::TEXTURE_2D,
            vfmt::B8G8R8X8_UNORM,
            bind_flags,
            width,
            height,
            1,  // depth
            1,  // array_size
        )
    })?;
    crate::serial_println!("[virgl] Step 2: 3D resource created (id={}, {}x{}, B8G8R8X8_UNORM, bind=0x{:08x})", RESOURCE_3D_ID, width, height, bind_flags);

    // Step 3: Attach SEPARATE backing memory (PCI_3D_FRAMEBUFFER, NOT shared with 2D resource)
    with_device_state(|state| {
        virgl_attach_backing_cmd(state, RESOURCE_3D_ID)
    })?;
    crate::serial_println!("[virgl] Step 3: separate backing attached");

    // Step 4: Attach 3D resource to VirGL context
    with_device_state(|state| {
        virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_3D_ID)
    })?;
    crate::serial_println!("[virgl] Step 4: 3D resource attached to context");

    // Step 5: Create sub-context + pipeline state objects
    let mut cmdbuf = CommandBuffer::new();
    cmdbuf.create_sub_ctx(1);
    cmdbuf.set_sub_ctx(1);

    let vs_text = b"VERT\nDCL IN[0], POSITION\nDCL IN[1], GENERIC[0]\nDCL OUT[0], POSITION\nDCL OUT[1], GENERIC[0]\n  0: MOV OUT[0], IN[0]\n  1: MOV OUT[1], IN[1]\n  2: END\n";
    cmdbuf.create_shader(1, pipe::SHADER_VERTEX, vs_text);
    let fs_text = b"FRAG\nDCL IN[0], GENERIC[0], PERSPECTIVE\nDCL OUT[0], COLOR\n  0: MOV OUT[0], IN[0]\n  1: END\n";
    cmdbuf.create_shader(2, pipe::SHADER_FRAGMENT, fs_text);

    cmdbuf.create_blend_simple(1);
    cmdbuf.create_dsa_disabled(1);
    cmdbuf.create_rasterizer_default(1);
    cmdbuf.create_vertex_elements(1, &[
        (0, 0, 0, vfmt::R32G32B32A32_FLOAT),
        (16, 0, 0, vfmt::R32G32B32A32_FLOAT),
    ]);

    virgl_submit(cmdbuf.as_slice())?;
    crate::serial_println!("[virgl] Step 5: pipeline state created");

    // Step 6: Bind state, create surface on 3D resource, clear to cornflower blue
    cmdbuf.clear();
    cmdbuf.set_sub_ctx(1);
    cmdbuf.bind_shader(1, pipe::SHADER_VERTEX);
    cmdbuf.bind_shader(2, pipe::SHADER_FRAGMENT);
    cmdbuf.bind_object(1, super::virgl::OBJ_BLEND);
    cmdbuf.bind_object(1, super::virgl::OBJ_DSA);
    cmdbuf.bind_object(1, super::virgl::OBJ_RASTERIZER);
    cmdbuf.bind_object(1, super::virgl::OBJ_VERTEX_ELEMENTS);
    cmdbuf.set_viewport(width as f32, height as f32);

    let surface_handle = 1u32;
    cmdbuf.create_surface(surface_handle, RESOURCE_3D_ID, vfmt::B8G8R8X8_UNORM, 0, 0);
    cmdbuf.set_framebuffer_state(0, &[surface_handle]);
    cmdbuf.clear_color(0.392, 0.584, 0.929, 1.0);

    virgl_submit(cmdbuf.as_slice())?;
    crate::serial_println!("[virgl] Step 6: VirGL clear submitted to host GPU");

    // Step 7: Parallels processes SUBMIT_3D synchronously — used ring completion
    // means the GPU work is done. No fence sync needed (Parallels returns
    // resp_fence=0 for all responses, so virgl_fence_sync doesn't work).
    crate::serial_println!("[virgl] Step 7: SUBMIT_3D sync completed (Parallels processes synchronously)");

    // Step 8: SKIPPED — Green pixel fill removed.
    // PCI_3D_FRAMEBUFFER in BSS overlaps with the Parallels boot stack at phys
    // 0x42000000. Writing 7.5MB of pixel data overwrites the stack frames and
    // corrupts return addresses. The VirGL clear in Step 6 already put cornflower
    // blue in the host GPU texture. SET_SCANOUT + RESOURCE_FLUSH should display
    // that if 3D resource scanout works on Parallels.
    crate::serial_println!("[virgl] Step 8: skipped green fill (BSS overlaps Parallels boot stack)");

    // Step 9: Switch display to 3D resource.
    // First disable current scanout (resource_id=0), then enable with 3D resource.
    // This mimics Linux DRM modesetting which does a full scanout reconfiguration.
    with_device_state(|state| {
        // Disable current scanout
        crate::serial_println!("[virgl] Step 9: disabling current scanout (resource_id=0)...");
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
                r_width: 0,
                r_height: 0,
                scanout_id: 0,
                resource_id: 0,  // disable scanout
            };
        }
        send_command_expect_ok(state, core::mem::size_of::<VirtioGpuSetScanout>() as u32)?;
        crate::serial_println!("[virgl] Step 9: scanout disabled");

        // Enable scanout with 3D resource
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        crate::serial_println!("[virgl] Step 9: scanout set to 3D resource (id={})", RESOURCE_3D_ID);

        resource_flush_3d(state, RESOURCE_3D_ID)?;
        crate::serial_println!("[virgl] Step 9: RESOURCE_FLUSH done");
        Ok(())
    })?;

    // Step 10: VirGL clear rendered cornflower blue to host texture.
    // SET_SCANOUT (Step 9) pointed display at the 3D resource.
    crate::serial_println!("[virgl] Step 10: display configured (cornflower blue if SET_SCANOUT works)");

    // Step 12: Create vertex buffer resource
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_VB_ID,
            pipe::BUFFER,
            vfmt::R8G8B8A8_UNORM,
            pipe::BIND_VERTEX_BUFFER,
            VB_SIZE as u32,
            1, 1, 1,
        )
    })?;
    with_device_state(|state| {
        virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_VB_ID)
    })?;
    crate::serial_println!("[virgl] Step 12: vertex buffer created (id={}, {}B)", RESOURCE_VB_ID, VB_SIZE);

    crate::serial_println!("[virgl] VirGL 3D pipeline initialized (v10b: TRANSFER_TO_HOST_3D green test)");
    crate::serial_println!("[virgl_init] END: VIRGL_ENABLED={}", VIRGL_ENABLED.load(Ordering::Acquire));

    Ok(())
}
