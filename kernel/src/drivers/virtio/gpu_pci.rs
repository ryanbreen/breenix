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

    // Capability commands (sequential with 2D commands)
    pub const GET_CAPSET_INFO: u32 = 0x0108;
    pub const GET_CAPSET: u32 = 0x0109;

    // Response types
    pub const RESP_OK_NODATA: u32 = 0x1100;
    pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
    pub const RESP_OK_CAPSET_INFO: u32 = 0x1102;
    pub const RESP_OK_CAPSET: u32 = 0x1103;
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

/// GET_CAPSET request
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuGetCapset {
    hdr: VirtioGpuCtrlHdr,
    capset_id: u32,
    capset_version: u32,
}

/// GET_CAPSET response (header only — variable-length capset data follows)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuRespCapset {
    hdr: VirtioGpuCtrlHdr,
    // Followed by capset_size bytes of capability data
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

/// Submit 3D command buffer header (followed immediately by VirGL command data).
///
/// CRITICAL: This struct MUST be 32 bytes to match the C `sizeof(struct
/// virtio_gpu_cmd_submit)` used by the host (QEMU / Parallels). The C struct
/// inherits 8-byte alignment from `fence_id: u64` in ctrl_hdr, so the compiler
/// rounds `sizeof` from 28 to 32. The host reads exactly `sizeof(cs) = 32`
/// bytes for the header, then reads VirGL payload at offset 32. If we send a
/// 28-byte header, the host consumes 4 bytes of our payload as padding,
/// corrupting everything.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuCmdSubmit {
    hdr: VirtioGpuCtrlHdr,
    size: u32, // size in bytes of the VirGL command buffer
    _padding: u32, // matches C struct trailing alignment padding
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

/// Cursor queue (queue 1) — required by VirtIO GPU spec. Linux always sets up
/// both controlq and cursorq before calling driver_ok(). Not setting up the
/// cursor queue may leave the device in a partially-initialized state.
static mut PCI_CURSOR_QUEUE: PciCtrlQueueMemory = PciCtrlQueueMemory {
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
/// Response buffer — 1024 bytes to accommodate GET_CAPSET responses
/// (VirGL VIRGL2 capset is 768 bytes + 24 byte header = 792 bytes).
#[repr(C, align(64))]
struct PciRespBuffer {
    data: [u8; 1024],
}
static mut PCI_RESP_BUF: PciRespBuffer = PciRespBuffer { data: [0; 1024] };

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
/// Resource ID for the 2D scanout buffer (NEVER attached to VirGL context).
/// VirGL 3D context ID (used with CTX_CREATE, CTX_ATTACH_RESOURCE, SUBMIT_3D)
const VIRGL_CTX_ID: u32 = 1;
/// Maximum circles we can render per frame
const MAX_CIRCLES: usize = 16;
/// Vertices per circle (triangle fan: center + N perimeter + closing vertex)
const CIRCLE_SEGMENTS: usize = 16;
/// Vertices per circle = center + segments + 1 (close fan)
const VERTS_PER_CIRCLE: usize = CIRCLE_SEGMENTS + 2;
/// Bytes per vertex: position (4×f32) + color/texcoord (4×f32) = 32 bytes
const BYTES_PER_VERTEX: usize = 32;
/// Vertex buffer size: enough for MAX_CIRCLES circles
#[allow(dead_code)]
const VB_SIZE: usize = MAX_CIRCLES * VERTS_PER_CIRCLE * BYTES_PER_VERTEX;

/// Resource ID for test texture (textured quad proof-of-concept)
const RESOURCE_TEX_ID: u32 = 4;
/// Test texture dimensions (64×64 pixels)
const TEST_TEX_DIM: u32 = 64;
/// Test texture backing size in bytes (64×64×4 = 16KB)
const TEST_TEX_BYTES: usize = (TEST_TEX_DIM * TEST_TEX_DIM * 4) as usize;

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
///
/// CRITICAL: This backing is heap-allocated (not BSS). BSS memory on Parallels
/// overlaps with the boot stack region, causing DMA to silently fail.
/// Heap allocation ensures the backing is in DMA-safe physical memory.
/// Pointer to heap-allocated 3D framebuffer backing (page-aligned, DMA-safe).
/// Initialized by `init_3d_framebuffer()` during VirGL init.
static mut PCI_3D_FB_PTR: *mut u8 = core::ptr::null_mut();
/// Actual size of the 3D framebuffer backing in bytes.
static mut PCI_3D_FB_LEN: usize = 0;

/// Allocate the 3D framebuffer backing on the heap with page alignment.
/// Must be called before any VirGL operations that reference the backing.
fn init_3d_framebuffer(width: u32, height: u32) {
    let size = (width as usize) * (height as usize) * 4;
    let layout = alloc::alloc::Layout::from_size_align(size, 4096)
        .expect("invalid 3D framebuffer layout");
    unsafe {
        let ptr = alloc::alloc::alloc_zeroed(layout);
        assert!(!ptr.is_null(), "failed to allocate 3D framebuffer backing");
        PCI_3D_FB_PTR = ptr;
        PCI_3D_FB_LEN = size;
    }
    let phys = virt_to_phys(unsafe { PCI_3D_FB_PTR } as u64);
    crate::serial_println!(
        "[virgl] 3D framebuffer backing: heap ptr={:#x}, phys={:#x}, size={}",
        unsafe { PCI_3D_FB_PTR } as u64, phys, size
    );
}



/// Static backing for test texture (64×64 BGRA)
#[repr(C, align(4096))]
struct TestTextureBuffer {
    pixels: [u8; TEST_TEX_BYTES],
}
static mut TEST_TEX_BUF: TestTextureBuffer = TestTextureBuffer { pixels: [0; TEST_TEX_BYTES] };

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

/// Clean (flush) a range of memory from CPU caches to physical RAM.
///
/// On ARM64, CPU writes to WB-cacheable BSS memory stay in L1/L2 cache.
/// The hypervisor reads physical RAM via DMA for TRANSFER_TO_HOST operations,
/// so we must flush dirty cache lines before any DMA read. Same pattern as
/// the xHCI and AHCI drivers use on this platform.
#[cfg(target_arch = "aarch64")]
#[inline]
fn dma_cache_clean(ptr: *const u8, len: usize) {
    const CACHE_LINE: usize = 64;
    let start = ptr as usize & !(CACHE_LINE - 1);
    let end = (ptr as usize + len + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    for addr in (start..end).step_by(CACHE_LINE) {
        unsafe {
            core::arch::asm!("dc cvac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb st", options(nostack, preserves_flags));
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn dma_cache_clean(_ptr: *const u8, _len: usize) {
    // x86_64 has cache-coherent DMA; no explicit flush needed.
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

/// Set up PCI MSI-X or MSI for the VirtIO GPU through GICv2m.
///
/// VirtIO modern (PCI) devices use MSI-X (cap 0x11), not plain MSI (cap 0x05).
/// Tries MSI-X first (matching Linux's virtio_pci_modern driver), falls back
/// to plain MSI if MSI-X is not available.
///
/// Returns the allocated SPI number, or 0 if neither MSI-X nor MSI is available.
#[cfg(target_arch = "aarch64")]
fn setup_gpu_msi(pci_dev: &crate::drivers::pci::Device) -> u32 {
    use crate::arch_impl::aarch64::gic;

    // Dump PCI capabilities for diagnostic visibility
    pci_dev.dump_capabilities();

    // Step 1: Probe GICv2m (needed for both MSI-X and MSI)
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

    // Step 2: Allocate SPI
    let spi = crate::platform_config::allocate_msi_spi();
    if spi == 0 {
        crate::serial_println!("[virtio-gpu-pci] No SPIs available, using polling");
        return 0;
    }

    let msi_address = base + 0x40;

    // Step 3: Try MSI-X first (what VirtIO modern devices use)
    if let Some(msix_cap) = pci_dev.find_msix_capability() {
        let table_size = pci_dev.msix_table_size(msix_cap);
        let (table_bar, table_offset) = pci_dev.msix_table_location(msix_cap);
        crate::serial_println!("[virtio-gpu-pci] MSI-X cap found: offset={:#x} table_size={} table_bar={} table_offset={:#x}",
            msix_cap, table_size, table_bar, table_offset);

        // Check BAR validity before accessing MSI-X table
        let bar_info = &pci_dev.bars[table_bar as usize];
        crate::serial_println!("[virtio-gpu-pci] MSI-X BAR {}: addr={:#x} size={:#x} valid={}",
            table_bar, bar_info.address, bar_info.size, bar_info.is_valid());

        // DIAGNOSTIC: Skip MSI-X PCI enable to avoid interrupt interference during init.
        // We still write VirtIO MSI-X vectors to NO_VECTOR to test if vector
        // configuration (without actual MSI-X) affects VirGL activation.
        // Once VirGL works, we can re-enable MSI-X for runtime performance.
        crate::serial_println!("[virtio-gpu-pci] MSI-X cap present but skipping PCI enable (using polling)");
        // Return 0 = polling mode, VirtIO vectors will be set to NO_VECTOR below
    }

    // Step 4: Fall back to plain MSI
    if let Some(msi_cap) = pci_dev.find_msi_capability() {
        pci_dev.configure_msi(msi_cap, msi_address as u32, spi as u16);
        pci_dev.disable_intx();
        gic::configure_spi_edge_triggered(spi);
        crate::serial_println!("[virtio-gpu-pci] MSI configured: SPI={}", spi);
        return spi;
    }

    crate::serial_println!("[virtio-gpu-pci] No MSI-X or MSI capability found, using polling");
    0
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

    // Set up MSI-X/MSI BEFORE queue setup. Linux's virtio_pci_modern driver
    // enables MSI-X first, then sets up queues with MSI-X vectors. The VirtIO
    // spec requires MSI-X to be enabled at the PCI level before writing
    // queue_msix_vector in the common config.
    #[cfg(target_arch = "aarch64")]
    let msi_spi = setup_gpu_msi(virtio.pci_device());
    #[cfg(not(target_arch = "aarch64"))]
    let msi_spi = 0u32;

    // Determine the MSI-X vector number for queues. If MSI-X was configured,
    // use vector 0 for all queues. Otherwise use NO_VECTOR (0xFFFF).
    let has_msix = virtio.pci_device().find_msix_capability().is_some() && msi_spi != 0;
    let queue_vector: u16 = if has_msix { 0 } else { 0xFFFF };
    let config_vector: u16 = if has_msix { 0 } else { 0xFFFF };

    // Write config_msix_vector (Linux does this in vp_find_vqs_msix)
    let readback = virtio.set_config_msix_vector(config_vector);
    crate::serial_println!("[virtio-gpu-pci] config_msix_vector: wrote={:#x} readback={:#x}",
        config_vector, readback);

    // Log number of queues
    let num_queues = virtio.num_queues();
    crate::serial_println!("[virtio-gpu-pci] Device has {} queues", num_queues);

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

        // Flush queue memory from CPU cache to physical RAM before telling the
        // device where the queue lives. On ARM64 WB-cacheable BSS, the device
        // (hypervisor) reads physical RAM via DMA and may see stale data.
        dma_cache_clean(q as *const u8, core::mem::size_of::<PciCtrlQueueMemory>());
    }

    // Desc table at start, avail ring at +256 (16 descs * 16 bytes), used ring at +4096
    virtio.set_queue_desc(queue_phys);
    virtio.set_queue_avail(queue_phys + 256);
    virtio.set_queue_used(queue_phys + 4096);

    // Write queue MSI-X vector BEFORE enabling the queue (Linux's vp_setup_vq order)
    virtio.select_queue(0);
    let q0_readback = virtio.set_queue_msix_vector(queue_vector);
    crate::serial_println!("[virtio-gpu-pci] Queue 0 msix_vector: wrote={:#x} readback={:#x}",
        queue_vector, q0_readback);

    virtio.set_queue_ready(true);

    // Cache queue 0 notify address to avoid 2 MMIO reads per notification
    virtio.cache_queue_notify_addr(0);

    // Set up cursor queue (queue 1) — Linux always configures both queues before
    // driver_ok(). The VirtIO GPU spec requires both controlq and cursorq.
    // Without this, the device may not fully initialize its VirGL subsystem.
    virtio.select_queue(1);
    let cursor_queue_max = virtio.get_queue_num_max();
    if cursor_queue_max > 0 {
        let cursor_queue_size = core::cmp::min(cursor_queue_max, 16);
        virtio.set_queue_num(cursor_queue_size);
        let cursor_queue_phys = virt_to_phys(&raw const PCI_CURSOR_QUEUE as u64);
        unsafe {
            let q = &raw mut PCI_CURSOR_QUEUE;
            for i in 0..15 {
                (*q).desc[i].next = (i + 1) as u16;
            }
            (*q).desc[15].next = 0;
            (*q).avail.flags = 0;
            (*q).avail.idx = 0;
            (*q).used.flags = 0;
            (*q).used.idx = 0;
            dma_cache_clean(q as *const u8, core::mem::size_of::<PciCtrlQueueMemory>());
        }
        virtio.set_queue_desc(cursor_queue_phys);
        virtio.set_queue_avail(cursor_queue_phys + 256);
        virtio.set_queue_used(cursor_queue_phys + 4096);

        // Write queue 1 MSI-X vector before enabling
        virtio.select_queue(1);
        let q1_readback = virtio.set_queue_msix_vector(queue_vector);
        crate::serial_println!("[virtio-gpu-pci] Queue 1 msix_vector: wrote={:#x} readback={:#x}",
            queue_vector, q1_readback);

        virtio.set_queue_ready(true);
        crate::serial_println!("[virtio-gpu-pci] Cursor queue (q1) set up: size={}", cursor_queue_size);
    } else {
        crate::serial_println!("[virtio-gpu-pci] Cursor queue (q1) not available (max=0)");
    }

    // Mark device ready — MUST happen before sending any commands (Linux: virtio_device_ready())
    virtio.driver_ok();

    // NOTE: We do NOT store msi_spi in GPU_IRQ yet! GPU_IRQ=0 means send_command
    // uses spin-polling instead of WFI. At this early boot stage there's no timer
    // interrupt, so if an MSI-X interrupt fails to deliver, WFI would block forever.
    // We enable MSI-X interrupt delivery after all init commands succeed.
    #[cfg(target_arch = "aarch64")]
    if msi_spi != 0 {
        crate::arch_impl::aarch64::gic::enable_spi(msi_spi);
        crate::serial_println!("[virtio-gpu-pci] MSI-X SPI {} GIC-enabled (polling during init, WFI after)", msi_spi);
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

    // GET_CAPSET_INFO + GET_CAPSET for each capset (Linux does both before GET_DISPLAY_INFO).
    // The GET_CAPSET handshake may be required to fully activate VirGL rendering on the host.
    for idx in 0..num_capsets {
        match get_capset_info(idx) {
            Ok((id, max_ver, max_size)) => {
                crate::serial_println!("[virtio-gpu-pci] Capset {}: id={}, max_ver={}, max_size={}",
                    idx, id, max_ver, max_size);
                // Linux always retrieves the actual capset data after GET_CAPSET_INFO.
                match get_capset(id, max_ver) {
                    Ok(resp_type) => {
                        crate::serial_println!("[virtio-gpu-pci] GET_CAPSET(id={}, ver={}): resp={:#x}",
                            id, max_ver, resp_type);
                    }
                    Err(e) => {
                        crate::serial_println!("[virtio-gpu-pci] GET_CAPSET(id={}, ver={}) failed: {}",
                            id, max_ver, e);
                    }
                }
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

    // Use the display-reported resolution. Linux always matches its resources
    // to the display mode — gl_display.c uses drmModeGetConnector to get the
    // native mode and creates all resources at that resolution. SET_SCANOUT
    // with dimensions exceeding the display mode is silently ignored by Parallels.
    // DIAG: Force GOP resolution to test if resolution mismatch causes corruption.
    // GOP reports 1024x768, GET_DISPLAY_INFO reports 1728x1080. If 2D resource
    // must match the GOP physical display mode, using 1728x1080 would cause
    // stride mismatch in the hypervisor's display pipeline.
    // Use GOP resolution for the 2D resource + BAR0 display path.
    // VirGL will use its own resolution (from GET_DISPLAY_INFO) for 3D resources.
    let gop_w = crate::platform_config::fb_width();
    let gop_h = crate::platform_config::fb_height();
    let (use_width, use_height) = if gop_w > 0 && gop_h > 0 {
        crate::serial_println!("[virtio-gpu-pci] Using GOP resolution {}x{} (display reported {:?})",
            gop_w, gop_h, display_dims);
        (gop_w, gop_h)
    } else {
        match display_dims {
            Ok((dw, dh)) if dw > 0 && dh > 0 && dw <= FB_MAX_WIDTH && dh <= FB_MAX_HEIGHT => (dw, dh),
            _ => (DEFAULT_FB_WIDTH, DEFAULT_FB_HEIGHT),
        }
    };

    // Update state with actual dimensions
    unsafe {
        let ptr = &raw mut GPU_PCI_STATE;
        if let Some(ref mut state) = *ptr {
            state.width = use_width;
            state.height = use_height;
        }
    }

    // Always create 2D resource to establish display mode.
    // Without a 2D resource, Parallels shows the "no video signal" watermark.
    // The 2D resource + SET_SCANOUT "registers" the display dimensions.
    create_resource()?;
    attach_backing()?;
    set_scanout()?;
    crate::serial_println!("[virtio-gpu-pci] 2D resource created and scanout set");

    // All GPU setup commands succeeded — now mark as initialized.
    GPU_PCI_INITIALIZED.store(true, Ordering::Release);

    // NOW enable MSI-X interrupt-driven command completion. All init commands
    // above used spin-polling (GPU_IRQ=0). From here on, send_command will use
    // WFI to wait for MSI-X interrupts, which is more efficient for runtime.
    #[cfg(target_arch = "aarch64")]
    if msi_spi != 0 {
        GPU_IRQ.store(msi_spi, Ordering::Release);
        crate::serial_println!("[virtio-gpu-pci] MSI-X WFI mode activated (SPI={})", msi_spi);
    }

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

        // Flush desc + avail ring from CPU cache to physical RAM so device sees them.
        // ARM64 WB-cacheable BSS may not be visible to hypervisor DMA without this.
        #[cfg(target_arch = "aarch64")]
        {
            // Flush all descriptor entries (256 bytes) + avail ring (36 bytes)
            let q_addr = q as *const u8;
            dma_cache_clean(q_addr, 512);
        }

        fence(Ordering::SeqCst);
        (*q).avail.idx = idx.wrapping_add(1);

        // Flush the updated avail.idx
        #[cfg(target_arch = "aarch64")]
        {
            let avail_idx_addr = &(*q).avail.idx as *const u16 as *const u8;
            dma_cache_clean(avail_idx_addr, 64);
        }

        fence(Ordering::SeqCst);
    }

    // Signal that we're waiting for a completion, then notify device
    GPU_CMD_COMPLETE.store(false, Ordering::Release);
    state.device.notify_queue_fast(0);

    // Wait for used ring update — WFI if MSI is available, spin_loop otherwise.
    let use_msi = GPU_IRQ.load(Ordering::Relaxed) != 0;
    let mut timeout = 10_000_000u32;
    loop {
        // Invalidate used ring cache line so we see the device's DMA write
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let q = &raw const PCI_CTRL_QUEUE;
            let used_addr = &(*q).used as *const _ as usize;
            core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

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

/// Send a command using a 3-descriptor chain (Linux format):
///   Desc 0: command header (device reads)
///   Desc 1: command payload (device reads)
///   Desc 2: response (device writes)
/// Returns Ok((used_len, resp_type)) on completion, Err on timeout.
fn send_command_3desc(
    state: &mut GpuPciDeviceState,
    hdr_phys: u64,
    hdr_len: u32,
    payload_phys: u64,
    payload_len: u32,
    resp_phys: u64,
    resp_len: u32,
) -> Result<(u32, u32), &'static str> {
    // Poison response buffer to verify device actually writes a response.
    // If we read 0xDEADBEEF back after the command, the device didn't respond.
    unsafe {
        let resp_ptr = &raw mut PCI_RESP_BUF;
        let hdr = &mut *((*resp_ptr).data.as_mut_ptr() as *mut VirtioGpuCtrlHdr);
        core::ptr::write_volatile(&mut hdr.type_, 0xDEADBEEF);
        dma_cache_clean(&raw const PCI_RESP_BUF as *const u8, 64);
    }

    unsafe {
        let q = &raw mut PCI_CTRL_QUEUE;

        // Desc 0: command header (readable, chained to 1)
        (*q).desc[0] = VirtqDesc {
            addr: hdr_phys,
            len: hdr_len,
            flags: DESC_F_NEXT,
            next: 1,
        };

        // Desc 1: command payload (readable, chained to 2)
        (*q).desc[1] = VirtqDesc {
            addr: payload_phys,
            len: payload_len,
            flags: DESC_F_NEXT,
            next: 2,
        };

        // Desc 2: response (writable)
        (*q).desc[2] = VirtqDesc {
            addr: resp_phys,
            len: resp_len,
            flags: DESC_F_WRITE,
            next: 0,
        };

        // Flush all descriptor entries + avail ring from CPU cache
        #[cfg(target_arch = "aarch64")]
        {
            let q_addr = q as *const u8;
            dma_cache_clean(q_addr, 512);
        }

        // Add to available ring (head = desc 0)
        let idx = (*q).avail.idx;
        (*q).avail.ring[(idx % 16) as usize] = 0;

        fence(Ordering::SeqCst);
        (*q).avail.idx = idx.wrapping_add(1);

        // Flush the updated avail.idx
        #[cfg(target_arch = "aarch64")]
        {
            let avail_idx_addr = &(*q).avail.idx as *const u16 as *const u8;
            dma_cache_clean(avail_idx_addr, 64);
        }

        fence(Ordering::SeqCst);
    }

    // Notify device
    state.device.notify_queue_fast(0);

    // Poll for completion
    let mut timeout = 10_000_000u32;
    let used_len;
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let q = &raw const PCI_CTRL_QUEUE;
            let used_addr = &(*q).used as *const _ as usize;
            core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

        fence(Ordering::Acquire);
        let used_idx = unsafe {
            let q = &raw const PCI_CTRL_QUEUE;
            read_volatile(&(*q).used.idx)
        };
        if used_idx != state.last_used_idx {
            // Read used ring entry
            used_len = unsafe {
                let q = &raw const PCI_CTRL_QUEUE;
                let elem_idx = (state.last_used_idx % 16) as usize;
                read_volatile(&(*q).used.ring[elem_idx].len)
            };
            state.last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("GPU PCI 3-desc command timeout");
        }
        core::hint::spin_loop();
    }

    // Invalidate response cache
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Invalidate response cache line using virtual address (not physical).
        let r = &raw const PCI_RESP_BUF as usize;
        core::arch::asm!("dc civac, {}", in(reg) r, options(nostack));
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }

    let resp_type = unsafe {
        let p = &raw const PCI_RESP_BUF;
        let h = &*((*p).data.as_ptr() as *const VirtioGpuCtrlHdr);
        core::ptr::read_volatile(&h.type_)
    };

    Ok((used_len, resp_type))
}

/// Send a command and verify the response is RESP_OK_NODATA.
fn send_command_expect_ok(
    state: &mut GpuPciDeviceState,
    cmd_len: u32,
) -> Result<(), &'static str> {
    let cmd_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
    let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);

    // Poison response buffer to verify device actually writes a response.
    // If we read 0xDEADBEEF back after the command, the device didn't respond.
    unsafe {
        let resp_ptr = &raw mut PCI_RESP_BUF;
        let hdr = &mut *((*resp_ptr).data.as_mut_ptr() as *mut VirtioGpuCtrlHdr);
        core::ptr::write_volatile(&mut hdr.type_, 0xDEADBEEF);
        core::ptr::write_volatile(&mut hdr.flags, 0xDEADBEEF);
    }

    // Read command type for diagnostic
    let cmd_type = unsafe {
        let cmd_ptr = &raw const PCI_CMD_BUF;
        let hdr = &*((*cmd_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr);
        core::ptr::read_volatile(&hdr.type_)
    };

    // Flush command buffer and response poison from CPU cache to physical RAM.
    // On ARM64, WB-cacheable BSS may not be visible to the hypervisor's DMA
    // without explicit cache maintenance.
    dma_cache_clean(&raw const PCI_CMD_BUF as *const u8, cmd_len as usize);
    dma_cache_clean(&raw const PCI_RESP_BUF as *const u8, core::mem::size_of::<VirtioGpuCtrlHdr>());

    // Also flush the virtqueue descriptors and available ring
    {
        let q = &raw const PCI_CTRL_QUEUE;
        dma_cache_clean(q as *const u8, 512);
    }

    send_command(
        state,
        cmd_phys,
        cmd_len,
        resp_phys,
        core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
    )?;

    // Invalidate response cache line so we read the device's DMA write,
    // not our stale poison pattern.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let resp_addr = &raw const PCI_RESP_BUF as usize;
        // DC CIVAC: Clean and Invalidate by VA to Point of Coherency
        core::arch::asm!("dc civac, {}", in(reg) resp_addr, options(nostack));
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        core::arch::asm!("isb", options(nostack, preserves_flags));
    }

    // Also invalidate the used ring (where we check for completion)
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let q = &raw const PCI_CTRL_QUEUE;
        let used_addr = &(*q).used as *const _ as usize;
        core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }

    // Read response — use read_volatile to defeat compiler caching
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
        crate::serial_println!("[virtio-gpu-pci] cmd={:#x} FAILED: resp_type={:#x} flags={:#x} fence={} (poison=0xDEADBEEF means no device response)",
            cmd_type, resp_type, resp_flags, resp_fence);
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

        // Flush command buffer from CPU cache
        dma_cache_clean(&raw const PCI_CMD_BUF as *const u8,
            core::mem::size_of::<VirtioGpuCtrlHdr>());

        send_command(
            state,
            cmd_phys,
            core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
            resp_phys,
            core::mem::size_of::<VirtioGpuRespDisplayInfo>() as u32,
        )?;

        // Invalidate response cache so we read device's DMA write
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let resp_addr = &raw const PCI_RESP_BUF as usize;
            // Invalidate multiple cache lines covering the response
            for off in (0..core::mem::size_of::<VirtioGpuRespDisplayInfo>()).step_by(64) {
                core::arch::asm!("dc civac, {}", in(reg) resp_addr + off, options(nostack));
            }
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

        // Parse response
        unsafe {
            let resp_ptr = &raw const PCI_RESP_BUF;
            let resp = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuRespDisplayInfo);

            let resp_type = core::ptr::read_volatile(&resp.hdr.type_);
            if resp_type != cmd::RESP_OK_DISPLAY_INFO {
                crate::serial_println!("[virtio-gpu-pci] GET_DISPLAY_INFO: resp_type={:#x} (expected {:#x})",
                    resp_type, cmd::RESP_OK_DISPLAY_INFO);
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

        // Flush command buffer from CPU cache
        dma_cache_clean(&raw const PCI_CMD_BUF as *const u8,
            core::mem::size_of::<VirtioGpuGetCapsetInfo>());

        send_command(
            state,
            cmd_phys,
            core::mem::size_of::<VirtioGpuGetCapsetInfo>() as u32,
            resp_phys,
            core::mem::size_of::<VirtioGpuRespCapsetInfo>() as u32,
        )?;

        // Invalidate response cache
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let resp_addr = &raw const PCI_RESP_BUF as usize;
            core::arch::asm!("dc civac, {}", in(reg) resp_addr, options(nostack));
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

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

/// Retrieve capability set data (Linux: virtio_gpu_cmd_get_capset).
///
/// Linux always calls GET_CAPSET after GET_CAPSET_INFO for each capset.
/// The host may require this handshake before fully activating VirGL rendering.
fn get_capset(capset_id: u32, capset_version: u32) -> Result<u32, &'static str> {
    with_device_state(|state| {
        let cmd_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
        let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);

        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
            let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuGetCapset);
            *cmd = VirtioGpuGetCapset {
                hdr: VirtioGpuCtrlHdr {
                    type_: cmd::GET_CAPSET,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                capset_id,
                capset_version,
            };
        }

        dma_cache_clean(&raw const PCI_CMD_BUF as *const u8,
            core::mem::size_of::<VirtioGpuGetCapset>());

        // Response: 24-byte header + up to 768 bytes of capset data.
        // PCI_RESP_BUF is 1024 bytes, sufficient for VIRGL2 (792 bytes).
        let resp_size = 1024u32;
        send_command(
            state,
            cmd_phys,
            core::mem::size_of::<VirtioGpuGetCapset>() as u32,
            resp_phys,
            resp_size,
        )?;

        // Invalidate response cache
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let resp_addr = &raw const PCI_RESP_BUF as usize;
            // Invalidate multiple cache lines (1024 bytes / 64 = 16 lines)
            for off in (0..1024).step_by(64) {
                core::arch::asm!("dc civac, {}", in(reg) resp_addr + off, options(nostack));
            }
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }

        unsafe {
            let resp_ptr = &raw const PCI_RESP_BUF;
            let resp = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuRespCapset);
            let resp_type = core::ptr::read_volatile(&resp.hdr.type_);
            Ok(resp_type)
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
                format: format::B8G8R8X8_UNORM,
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
// 3D (VirGL) Hex Dump (for byte-comparison with Linux reference)
// =============================================================================

/// Controls whether VirGL hex dumps are emitted to serial.
/// Set to true during virgl_init(), cleared afterward.
static VIRGL_HEX_DUMP_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Hex-dump a PCI_CMD_BUF as DWORDs for byte-comparison with Linux reference.
/// Only emits when VIRGL_HEX_DUMP_ENABLED is true (during virgl_init).
fn hex_dump_cmd_buf(label: &str, byte_len: usize) {
    let enabled = VIRGL_HEX_DUMP_ENABLED.load(core::sync::atomic::Ordering::SeqCst);
    if !enabled {
        return;
    }
    let dword_count = byte_len / 4;
    crate::serial_println!("[hex-dump] {} ({} DWORDs, {} bytes):", label, dword_count, byte_len);
    unsafe {
        let buf = &raw const PCI_CMD_BUF;
        let ptr = (*buf).data.as_ptr() as *const u32;
        for i in 0..dword_count {
            let val = core::ptr::read_volatile(ptr.add(i));
            crate::serial_println!("[hex-dump] {} +{}: 0x{:08X}", label, i * 4, val);
        }
    }
    crate::serial_println!("[hex-dump] {} END", label);
}

/// Hex-dump VirGL payload from PCI_3D_CMD_BUF (skipping the 32-byte SUBMIT_3D header).
fn hex_dump_virgl_payload(label: &str, payload_dwords: usize) {
    if !VIRGL_HEX_DUMP_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let hdr_size = core::mem::size_of::<VirtioGpuCmdSubmit>();
    crate::serial_println!("[hex-dump] {} ({} DWORDs, {} bytes):", label, payload_dwords, payload_dwords * 4);
    unsafe {
        let buf = &raw const PCI_3D_CMD_BUF;
        let base = (*buf).data.as_ptr().add(hdr_size) as *const u32;
        for i in 0..payload_dwords {
            let val = core::ptr::read_volatile(base.add(i));
            crate::serial_println!("[hex-dump] {} +{}: 0x{:08X}", label, i * 4, val);
        }
    }
    crate::serial_println!("[hex-dump] {} END", label);

    // Also dump the SUBMIT_3D header itself
    crate::serial_println!("[hex-dump] SUBMIT_3D_HDR (8 DWORDs, 32 bytes):");
    unsafe {
        let buf = &raw const PCI_3D_CMD_BUF;
        let ptr = (*buf).data.as_ptr() as *const u32;
        for i in 0..8 {
            let val = core::ptr::read_volatile(ptr.add(i));
            crate::serial_println!("[hex-dump] SUBMIT_3D_HDR +{}: 0x{:08X}", i * 4, val);
        }
    }
    crate::serial_println!("[hex-dump] SUBMIT_3D_HDR END");
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
    hex_dump_cmd_buf("CTX_CREATE", core::mem::size_of::<VirtioGpuCtxCreate>());
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
    hex_dump_cmd_buf("CTX_ATTACH_RESOURCE", core::mem::size_of::<VirtioGpuCtxResource>());
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
    hex_dump_cmd_buf("RESOURCE_CREATE_3D", core::mem::size_of::<VirtioGpuResourceCreate3d>());
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuResourceCreate3d>() as u32)
}

/// Attach backing memory to a 3D resource using per-page scatter-gather entries.
///
/// Uses heap-allocated PCI_3D_FB_PTR as the backing store.
/// CRITICAL: Must NOT share backing with the 2D resource (PCI_FRAMEBUFFER).
/// Linux's Mesa/virgl creates independent GEM buffers for each resource.
///
/// KEY FIX: Linux kernel sends one VirtioGpuMemEntry per 4KB page (768 entries
/// for a 1024×768×4 framebuffer). Our previous approach sent 1 entry with the
/// entire 3MB range. The host (Parallels) may require per-page entries to
/// properly map backing for GL texture operations and TRANSFER_FROM_HOST_3D.
fn virgl_attach_backing_cmd(state: &mut GpuPciDeviceState, resource_id: u32) -> Result<(), &'static str> {
    let fb_ptr = unsafe { PCI_3D_FB_PTR };
    assert!(!fb_ptr.is_null(), "3D framebuffer not initialized");
    let fb_base_phys = virt_to_phys(fb_ptr as u64);
    let fb_len = unsafe { PCI_3D_FB_LEN };
    let actual_len = (state.width as usize * state.height as usize * 4).min(fb_len);

    const PAGE_SIZE: usize = 4096;
    let nr_pages = (actual_len + PAGE_SIZE - 1) / PAGE_SIZE;

    crate::serial_println!("[virgl] attach_backing: phys=0x{:x}, len={}, nr_pages={}",
        fb_base_phys, actual_len, nr_pages);

    // Heap-allocate the entries array (nr_pages × 16 bytes, too large for PCI_CMD_BUF)
    let entries_size = nr_pages * core::mem::size_of::<VirtioGpuMemEntry>();
    let entries_layout = alloc::alloc::Layout::from_size_align(entries_size, 64)
        .map_err(|_| "attach_backing: invalid entries layout")?;
    let entries_ptr = unsafe { alloc::alloc::alloc_zeroed(entries_layout) };
    if entries_ptr.is_null() {
        return Err("attach_backing: failed to allocate entries array");
    }

    // Fill each entry with a sequential 4KB page
    unsafe {
        let entries = core::slice::from_raw_parts_mut(
            entries_ptr as *mut VirtioGpuMemEntry, nr_pages);
        for i in 0..nr_pages {
            let page_offset = i * PAGE_SIZE;
            let page_len = if page_offset + PAGE_SIZE <= actual_len {
                PAGE_SIZE as u32
            } else {
                (actual_len - page_offset) as u32
            };
            entries[i] = VirtioGpuMemEntry {
                addr: fb_base_phys + page_offset as u64,
                length: page_len,
                padding: 0,
            };
        }
    }

    // Put the header in PCI_CMD_BUF
    let hdr_size = core::mem::size_of::<VirtioGpuResourceAttachBacking>();
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let hdr = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuResourceAttachBacking);
        *hdr = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            nr_entries: nr_pages as u32,
        };
    }

    // Flush command header and entries array from CPU cache
    dma_cache_clean(&raw const PCI_CMD_BUF as *const u8, hdr_size);
    dma_cache_clean(entries_ptr, entries_size);

    let hdr_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
    let entries_phys = virt_to_phys(entries_ptr as u64);
    let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);

    crate::serial_println!("[virgl] attach_backing: sending 3-desc chain: hdr=0x{:x}({}B), entries=0x{:x}({}B)",
        hdr_phys, hdr_size, entries_phys, entries_size);

    let (_used_len, resp_type) = send_command_3desc(
        state,
        hdr_phys, hdr_size as u32,
        entries_phys, entries_size as u32,
        resp_phys, core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
    )?;

    // Free entries array (one-time during init)
    unsafe { alloc::alloc::dealloc(entries_ptr, entries_layout); }

    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!("[virgl] attach_backing FAILED: resp_type={:#x}", resp_type);
        return Err("attach_backing: device rejected paged backing");
    }

    crate::serial_println!("[virgl] attach_backing: OK ({} pages attached)", nr_pages);
    Ok(())
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
    hex_dump_cmd_buf("RESOURCE_FLUSH", core::mem::size_of::<VirtioGpuResourceFlush>());
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuResourceFlush>() as u32)
}

/// Transfer a 3D resource from guest backing to host texture (upload).
///
/// `stride` is the row pitch in bytes of the guest-side backing buffer.
/// For display-sized resources use `width * 4`; for textures use `tex_width * 4`.
fn transfer_to_host_3d(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    stride: u32,
) -> Result<(), &'static str> {
    let offset = (y as u64) * (stride as u64) + (x as u64) * 4;

    // NOTE: TRANSFER_TO/FROM_HOST_3D uses unfenced commands (flags=0).
    // SUBMIT_3D uses VIRTIO_GPU_FLAG_FENCE (required for VirGL execution).

    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuTransferHost3d);
        *cmd = VirtioGpuTransferHost3d {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::TRANSFER_TO_HOST_3D,
                flags: 0,
                fence_id: 0,
                ctx_id: VIRGL_CTX_ID, // resource is attached to this context
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

    // Flush the backing buffer from CPU cache to physical RAM before DMA read.
    // Without this, the hypervisor reads stale zeros instead of our pixel data.
    unsafe {
        if resource_id == RESOURCE_3D_ID {
            let fb_ptr = PCI_3D_FB_PTR;
            assert!(!fb_ptr.is_null(), "3D framebuffer not initialized");
            let backing_ptr = fb_ptr.add(offset as usize);
            let backing_len = (h as usize) * (stride as usize);
            dma_cache_clean(backing_ptr, backing_len);
        } else if resource_id == RESOURCE_TEX_ID {
            let tex = &raw const TEST_TEX_BUF;
            let backing_ptr = (*tex).pixels.as_ptr();
            let backing_len = TEST_TEX_BYTES;
            dma_cache_clean(backing_ptr, backing_len);
        }
    }

    hex_dump_cmd_buf("TRANSFER_TO_HOST_3D", core::mem::size_of::<VirtioGpuTransferHost3d>());
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

    // Linux uses VIRTIO_GPU_FLAG_FENCE with transfer commands.
    // Without the fence flag, the host may not process the transfer.
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
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuTransferHost3d>() as u32)?;

    // Wait for fence completion (host DMA must finish before we read backing)
    virgl_fence_sync(state, fence_id)?;

    // CRITICAL: Invalidate CPU cache for the backing region AFTER DMA completes.
    // The host wrote pixels to physical RAM via DMA, but the CPU cache still has
    // stale zeros. Without invalidation, CPU reads will see all zeros.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let fb_ptr = PCI_3D_FB_PTR;
        assert!(!fb_ptr.is_null(), "3D framebuffer not initialized");
        let backing_start = fb_ptr.add(offset as usize);
        let backing_len = (h as usize) * (stride as usize);
        // DC CIVAC: Clean and Invalidate by VA to PoC — ensures we read DMA-written data
        for off in (0..backing_len).step_by(64) {
            let addr = backing_start.add(off) as usize;
            core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack));
        }
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        core::arch::asm!("isb", options(nostack, preserves_flags));
    }

    Ok(())
}

/// Submit a VirGL command buffer via SUBMIT_3D.
///
/// Sends VirGL commands via SUBMIT_3D using a 3-descriptor chain (Linux format):
///   Desc 0: VirtioGpuCmdSubmit header (32 bytes, readable)
///   Desc 1: VirGL command payload (N bytes, readable)
///   Desc 2: Response (24 bytes, writable)
///
/// The header and payload are written contiguously to PCI_3D_CMD_BUF but sent
/// as separate descriptors. The device reads the header from desc[0] (exactly
/// sizeof(VirtioGpuCmdSubmit) = 32 bytes) and the VirGL command data from
/// desc[1]. This matches how the Linux virtio-gpu driver sends SUBMIT_3D.
fn virgl_submit_3d_cmd(
    state: &mut GpuPciDeviceState,
    ctx_id: u32,
    cmds: &[u32],
) -> Result<u64, &'static str> {
    let payload_bytes = cmds.len() * 4;
    let hdr_size = core::mem::size_of::<VirtioGpuCmdSubmit>();
    let total_cmd_len = hdr_size + payload_bytes;

    if total_cmd_len > 16384 {
        return Err("VirGL command buffer too large");
    }

    let submit_id = state.next_fence_id;
    state.next_fence_id += 1;

    // Write header + payload contiguously to PCI_3D_CMD_BUF.
    // Descriptors will point to different offsets within this buffer.
    unsafe {
        let buf_ptr = &raw mut PCI_3D_CMD_BUF;
        let base = (*buf_ptr).data.as_mut_ptr();

        let hdr = &mut *(base as *mut VirtioGpuCmdSubmit);
        *hdr = VirtioGpuCmdSubmit {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::SUBMIT_3D,
                flags: VIRTIO_GPU_FLAG_FENCE,
                fence_id: submit_id,
                ctx_id,
                padding: 0,
            },
            size: payload_bytes as u32,
            _padding: 0,
        };

        let payload_dst = base.add(hdr_size) as *mut u32;
        core::ptr::copy_nonoverlapping(cmds.as_ptr(), payload_dst, cmds.len());
    }

    // Hex dump VirGL payload for byte comparison with Linux reference
    hex_dump_virgl_payload("SUBMIT_3D_PAYLOAD", cmds.len());

    // Flush the combined buffer from CPU cache
    dma_cache_clean(&raw const PCI_3D_CMD_BUF as *const u8, total_cmd_len);

    let hdr_phys = virt_to_phys(&raw const PCI_3D_CMD_BUF as u64);
    let payload_phys = hdr_phys + hdr_size as u64;
    let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);

    // 3-descriptor chain: header (readable) + payload (readable) + response (writable)
    let (used_len, resp_type) = send_command_3desc(
        state,
        hdr_phys,
        hdr_size as u32,
        payload_phys,
        payload_bytes as u32,
        resp_phys,
        core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
    )?;

    // Read response flags and fence_id for diagnostics
    let (resp_flags, resp_fence) = unsafe {
        let p = &raw const PCI_RESP_BUF;
        let h = &*((*p).data.as_ptr() as *const VirtioGpuCtrlHdr);
        (core::ptr::read_volatile(&h.flags), core::ptr::read_volatile(&h.fence_id))
    };

    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!("[virtio-gpu-pci] SUBMIT_3D failed: resp={:#x} used_len={} flags={:#x} fence={}",
            resp_type, used_len, resp_flags, resp_fence);
        return Err("SUBMIT_3D command failed");
    }
    if submit_id <= 5 || submit_id % 500 == 0 {
        crate::serial_println!("[virgl] SUBMIT_3D OK: id={} used_len={} resp_flags={:#x} resp_fence={}",
            submit_id, used_len, resp_flags, resp_fence);
    }
    Ok(submit_id)
}

/// Wait for the host to confirm a GPU fence has completed.
///
/// Sends NOP SUBMIT_3D commands with fences and polls until the response
/// fence_id >= target_fence_id. This ensures all prior VirGL rendering
/// commands have finished executing on the host GPU before we display.
fn virgl_fence_sync(state: &mut GpuPciDeviceState, target_fence_id: u64) -> Result<(), &'static str> {
    use super::virgl::CommandBuffer;

    for round in 0..100u32 {
        // Build a NOP VirGL command (set_sub_ctx is minimal)
        let mut cmdbuf = CommandBuffer::new();
        cmdbuf.set_sub_ctx(1);

        let payload = cmdbuf.as_slice();
        let payload_bytes = payload.len() * 4;
        let hdr_size = core::mem::size_of::<VirtioGpuCmdSubmit>();

        let fence_id = state.next_fence_id;
        state.next_fence_id += 1;

        // Write header + payload contiguously to PCI_3D_CMD_BUF
        unsafe {
            let buf_ptr = &raw mut PCI_3D_CMD_BUF;
            let base = (*buf_ptr).data.as_mut_ptr();

            let hdr = &mut *(base as *mut VirtioGpuCmdSubmit);
            *hdr = VirtioGpuCmdSubmit {
                hdr: VirtioGpuCtrlHdr {
                    type_: cmd::SUBMIT_3D,
                    flags: VIRTIO_GPU_FLAG_FENCE,
                    fence_id,
                    ctx_id: VIRGL_CTX_ID,
                    padding: 0,
                },
                size: payload_bytes as u32,
                _padding: 0,
            };

            let payload_dst = base.add(hdr_size) as *mut u32;
            core::ptr::copy_nonoverlapping(payload.as_ptr(), payload_dst, payload.len());
        }

        let total_len = hdr_size + payload_bytes;
        dma_cache_clean(&raw const PCI_3D_CMD_BUF as *const u8, total_len);

        let hdr_phys = virt_to_phys(&raw const PCI_3D_CMD_BUF as u64);
        let payload_phys = hdr_phys + hdr_size as u64;
        let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);

        let (_used_len, resp_type) = send_command_3desc(
            state,
            hdr_phys,
            hdr_size as u32,
            payload_phys,
            payload_bytes as u32,
            resp_phys,
            core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
        )?;

        if resp_type != cmd::RESP_OK_NODATA {
            crate::serial_println!("[virgl] fence_sync: NOP failed resp={:#x}", resp_type);
            return Err("fence sync NOP rejected");
        }

        // Check if the host reported our target fence as complete
        let resp_fence = unsafe {
            let resp_ptr = &raw const PCI_RESP_BUF;
            let hdr = &*((*resp_ptr).data.as_ptr() as *const VirtioGpuCtrlHdr);
            core::ptr::read_volatile(&hdr.fence_id)
        };

        if resp_fence >= target_fence_id {
            if round > 0 {
                crate::serial_println!("[virgl] fence_sync: completed after {} polls (target={}, got={})",
                    round + 1, target_fence_id, resp_fence);
            }
            return Ok(());
        }
    }

    Err("fence sync: target fence never completed after 100 polls")
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
    hex_dump_cmd_buf("SET_SCANOUT", core::mem::size_of::<VirtioGpuSetScanout>());
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
        crate::serial_println!("[virgl] WARNING: scanout not active at frame #{} — setting now", frame);
        with_device_state(|state| {
            set_scanout_resource(state, RESOURCE_3D_ID)
        }).ok();
        VIRGL_SCANOUT_ACTIVE.store(true, Ordering::Release);
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

    // Scissor clear to left half only — the right half is composited from
    // the terminal shadow buffer via TRANSFER_TO_HOST_3D.
    let half_w = width / 2;
    cmdbuf.set_scissor_state(0, 0, half_w, height);
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
        Ok(_fence_id) => {
            if verbose {
                crate::serial_println!("[virgl] frame #{}: SUBMIT_3D done", frame);
            }
        }
        Err(e) => {
            crate::serial_println!("[virgl] frame #{}: SUBMIT_3D FAILED: {}", frame, e);
            return Err(e);
        }
    }

    // Display via SET_SCANOUT + RESOURCE_FLUSH on the 3D resource.
    // VirGL renders entirely on the host GPU — no guest readback needed.
    //
    // For the terminal (right pane), we write CPU pixels to the 3D backing
    // and upload via TRANSFER_TO_HOST_3D, which pushes guest → host texture.

    // Step 1: Composite terminal (right pane) into the 3D backing + upload.
    let terminal_dirty = crate::graphics::arm64_fb::take_terminal_dirty();
    if terminal_dirty {
        if verbose {
            crate::serial_println!("[virgl] frame #{}: compositing terminal (right pane) → 3D backing", frame);
        }
        copy_terminal_to_3d_backing(width, height);
        // Upload the right half of the 3D backing to the host texture
        let half_w = width / 2;
        with_device_state(|state| {
            transfer_to_host_3d(state, RESOURCE_3D_ID, half_w, 0, width - half_w, height, width * 4)
        })?;
    }

    // Step 2: RESOURCE_FLUSH on the 3D resource
    match with_device_state(|state| {
        resource_flush_3d(state, RESOURCE_3D_ID)
    }) {
        Ok(()) => {}
        Err(e) => {
            crate::serial_println!("[virgl] frame #{}: display pipeline FAILED: {}", frame, e);
            return Err(e);
        }
    }

    Ok(())
}

/// Submit a VirGL command buffer for the active 3D context.
///
/// `cmds` is a slice of u32 DWORDs from a VirGL CommandBuffer.
pub fn virgl_submit(cmds: &[u32]) -> Result<u64, &'static str> {
    with_device_state(|state| {
        virgl_submit_3d_cmd(state, VIRGL_CTX_ID, cmds)
    })
}

/// Submit VirGL commands and wait for the fence to complete before returning.
/// This ensures the host GPU has finished processing the commands.
pub fn virgl_submit_sync(cmds: &[u32]) -> Result<(), &'static str> {
    let fence_id = with_device_state(|state| {
        virgl_submit_3d_cmd(state, VIRGL_CTX_ID, cmds)
    })?;
    with_device_state(|state| {
        virgl_fence_sync(state, fence_id)
    })
}

/// Copy 3D framebuffer backing (heap RAM) → BAR0 (display memory).
///
/// After TRANSFER_FROM_HOST_3D copies GPU-rendered pixels to the 3D backing,
/// this copies them to BAR0 so they appear on screen.
#[allow(dead_code)]
fn copy_3d_framebuffer_to_bar0(width: u32, height: u32) {
    let bar0_virt = crate::graphics::arm64_fb::gop_framebuffer();
    let fb_bytes = (width * height * 4) as usize;
    if let Some(bar0) = bar0_virt {
        let fb_ptr = unsafe { PCI_3D_FB_PTR };
        let fb_len = unsafe { PCI_3D_FB_LEN };
        if fb_ptr.is_null() { return; }
        let copy_len = fb_bytes.min(bar0.len()).min(fb_len);
        unsafe {
            core::ptr::copy_nonoverlapping(
                fb_ptr,
                bar0.as_mut_ptr(),
                copy_len,
            );
        }
    }
}

/// Copy the terminal (right pane) from the shadow buffer into the 3D framebuffer backing.
///
/// This composites the bwm terminal into the right half of the VirGL 3D
/// resource's backing store. After this, TRANSFER_TO_HOST_3D uploads the
/// right-half region to the host GPU texture, and both halves coexist in
/// the same scanout resource — no flicker.
///
/// Lock ordering: acquires SHELL_FRAMEBUFFER (via with_shadow_buffer) then
/// releases it before returning. Caller must NOT hold GPU_PCI_LOCK.
fn copy_terminal_to_3d_backing(display_w: u32, display_h: u32) {
    let divider_x = display_w / 2;
    let bpp = 4u32; // B8G8R8X8_UNORM
    let dst_stride = display_w * bpp;

    let result = crate::graphics::arm64_fb::with_shadow_buffer(|shadow, shadow_stride, _sw, sh| {
        let copy_h = (display_h as usize).min(sh);
        let right_w_pixels = display_w - divider_x;
        let copy_bytes_per_row = (right_w_pixels * bpp) as usize;
        let src_x_byte = (divider_x * bpp) as usize;

        unsafe {
            let dst_pixels = PCI_3D_FB_PTR;
            assert!(!dst_pixels.is_null(), "3D framebuffer not initialized");
            let fb_len = PCI_3D_FB_LEN;

            for y in 0..copy_h {
                let src_offset = y * shadow_stride + src_x_byte;
                let dst_offset = y * (dst_stride as usize) + src_x_byte;

                if src_offset + copy_bytes_per_row <= shadow.len()
                    && dst_offset + copy_bytes_per_row <= fb_len
                {
                    core::ptr::copy_nonoverlapping(
                        shadow.as_ptr().add(src_offset),
                        dst_pixels.add(dst_offset),
                        copy_bytes_per_row,
                    );
                }
            }

            // Draw a 4px wide dark gray divider at divider_x
            let divider_color: [u8; 4] = [0x40, 0x40, 0x40, 0xFF]; // BGRA dark gray
            for y in 0..copy_h {
                for dx in 0..4u32 {
                    let px = divider_x.saturating_sub(2) + dx;
                    if px < display_w {
                        let offset = y * (dst_stride as usize) + (px * bpp) as usize;
                        if offset + 4 <= fb_len {
                            core::ptr::copy_nonoverlapping(
                                divider_color.as_ptr(),
                                dst_pixels.add(offset),
                                4,
                            );
                        }
                    }
                }
            }
        }
        copy_h
    });
    if result.is_none() {
        crate::serial_println!("[virgl] copy_terminal_to_3d_backing: shadow buffer unavailable (lock contended or not initialized)");
    }
}

/// Flush the VirGL render target to the display.
/// After the initial priming in virgl_init(), only RESOURCE_FLUSH is needed
/// to display VirGL-rendered content. The host reads from the GPU texture.
pub fn virgl_flush() -> Result<(), &'static str> {
    if !is_virgl_enabled() {
        return Err("VirGL display not available");
    }
    with_device_state(|state| {
        resource_flush_3d(state, RESOURCE_3D_ID)
    })
}

// =============================================================================
// VirGL Initialization
// =============================================================================

/// Initialize the VirGL 3D pipeline if VIRGL was negotiated.
///
/// Creates a 3D rendering context, render target resource, pipeline state
/// objects (blend, DSA, rasterizer, shaders, vertex elements), and vertex
/// buffer. Then "primes" the resource with an initial TRANSFER_TO_HOST_3D
/// (required by Parallels before RESOURCE_FLUSH will read from the GPU
/// texture), performs a VirGL clear to cornflower blue, and flushes.
///
/// After this, the render loop only needs SUBMIT_3D + RESOURCE_FLUSH per
/// frame — no per-frame transfers for VirGL-rendered content.
pub fn virgl_init() -> Result<(), &'static str> {
    use super::virgl::{CommandBuffer, format as vfmt, pipe};

    if !is_virgl_enabled() {
        return Err("VirGL not supported");
    }

    crate::serial_println!("[virgl] Initializing VirGL 3D pipeline...");

    let (width, height) = dimensions().ok_or("GPU not initialized")?;

    // Step 1: Allocate 3D framebuffer backing on heap (DMA-safe memory).
    // BSS memory overlaps with Parallels boot stack, causing DMA failures.
    init_3d_framebuffer(width, height);

    // Step 2: Create 3D context
    with_device_state(|state| {
        virgl_ctx_create_cmd(state, VIRGL_CTX_ID, b"breenix")
    })?;
    crate::serial_println!("[virgl] Step 1: context created");

    // Step 3: Create 3D render target resource
    let bind_flags = pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW
                   | pipe::BIND_SCANOUT | pipe::BIND_SHARED;
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state, RESOURCE_3D_ID, pipe::TEXTURE_2D, vfmt::B8G8R8X8_UNORM,
            bind_flags, width, height, 1, 1,
        )
    })?;
    crate::serial_println!("[virgl] Step 2: 3D resource created ({}x{}, bind=0x{:08x})",
        width, height, bind_flags);

    // Step 4: Attach backing memory
    with_device_state(|state| {
        virgl_attach_backing_cmd(state, RESOURCE_3D_ID)
    })?;
    crate::serial_println!("[virgl] Step 3: backing attached");

    // Step 5: Attach resource to VirGL context
    with_device_state(|state| {
        virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_3D_ID)
    })?;
    crate::serial_println!("[virgl] Step 4: resource attached to context");

    // Step 6: Prime the resource with TRANSFER_TO_HOST_3D + SET_SCANOUT + FLUSH.
    // Parallels requires an initial TRANSFER_TO_HOST_3D before RESOURCE_FLUSH
    // will read from the GPU texture. This must happen BEFORE VirGL rendering.
    {
        let fb_ptr = unsafe { PCI_3D_FB_PTR };
        let fb_len = (width * height * 4) as usize;
        dma_cache_clean(fb_ptr, fb_len);
        with_device_state(|state| {
            transfer_to_host_3d(state, RESOURCE_3D_ID, 0, 0, width, height, width * 4)
        })?;
        with_device_state(|state| { set_scanout_resource(state, RESOURCE_3D_ID) })?;
        with_device_state(|state| { resource_flush_3d(state, RESOURCE_3D_ID) })?;
        crate::serial_println!("[virgl] Step 5: resource primed (TRANSFER_TO_HOST + SET_SCANOUT + FLUSH)");
    }

    // Step 7: Minimal VirGL clear to cornflower blue — matches proven B5 flow.
    // Uses only the minimal commands needed: sub_ctx, tweaks, surface, FBO, clear.
    // No shaders/blend/DSA/rasterizer needed for CLEAR operations.
    {
        let mut cmdbuf = CommandBuffer::new();
        cmdbuf.create_sub_ctx(1);
        cmdbuf.set_sub_ctx(1);
        cmdbuf.set_tweaks(1, 1);
        cmdbuf.set_tweaks(2, width);
        cmdbuf.create_surface(1, RESOURCE_3D_ID, vfmt::B8G8R8X8_UNORM, 0, 0);
        cmdbuf.set_framebuffer_state(0, &[1]);
        cmdbuf.clear_color(0.392, 0.584, 0.929, 1.0); // Cornflower blue
        virgl_submit_sync(cmdbuf.as_slice())?;
        crate::serial_println!("[virgl] Step 6: VirGL clear (cornflower blue) submitted");
    }

    // Step 8: RESOURCE_FLUSH to display the VirGL-rendered cornflower blue.
    // After priming, FLUSH reads from the GPU texture.
    with_device_state(|state| { resource_flush_3d(state, RESOURCE_3D_ID) })?;
    crate::serial_println!("[virgl] Step 7: RESOURCE_FLUSH — display should show cornflower blue");

    // Step 9: Create full pipeline state objects for the render loop.
    // Shaders, blend, DSA, rasterizer, vertex elements are needed for drawing
    // geometry (not just clears). Created after priming is confirmed working.
    {
        let mut cmdbuf = CommandBuffer::new();
        cmdbuf.set_sub_ctx(1);

        // Passthrough vertex shader: position + generic[0] (color/texcoord)
        let vs_text = b"VERT\nDCL IN[0], POSITION\nDCL IN[1], GENERIC[0]\nDCL OUT[0], POSITION\nDCL OUT[1], GENERIC[0]\n  0: MOV OUT[0], IN[0]\n  1: MOV OUT[1], IN[1]\n  2: END\n";
        cmdbuf.create_shader(1, pipe::SHADER_VERTEX, vs_text);

        // Passthrough fragment shader: color from vertex
        let fs_text = b"FRAG\nDCL IN[0], GENERIC[0], PERSPECTIVE\nDCL OUT[0], COLOR\n  0: MOV OUT[0], IN[0]\n  1: END\n";
        cmdbuf.create_shader(2, pipe::SHADER_FRAGMENT, fs_text);

        cmdbuf.create_blend_simple(1);
        cmdbuf.create_dsa_disabled(1);
        cmdbuf.create_rasterizer_default(1);
        cmdbuf.create_vertex_elements(1, &[
            (0, 0, 0, vfmt::R32G32B32A32_FLOAT),   // position at offset 0
            (16, 0, 0, vfmt::R32G32B32A32_FLOAT),  // color/texcoord at offset 16
        ]);

        virgl_submit_sync(cmdbuf.as_slice())?;
        crate::serial_println!("[virgl] Step 8: pipeline state created (shaders, blend, DSA, rasterizer, VE)");
    }

    // Step 10: Create vertex buffer resource for the render loop
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
    crate::serial_println!("[virgl] Step 9: vertex buffer created (id={}, {}B)", RESOURCE_VB_ID, VB_SIZE);

    VIRGL_SCANOUT_ACTIVE.store(true, Ordering::Release);
    VIRGL_HEX_DUMP_ENABLED.store(false, Ordering::Relaxed);

    crate::serial_println!("[virgl] VirGL 3D pipeline initialized successfully");

    Ok(())
}

// =============================================================================
// VirGL Textured Quad Test (Phase 1: prove texture sampling works)
// =============================================================================

/// Test textured quad rendering via VirGL.
///
/// Creates a 64×64 checkerboard texture (red/blue), uploads it via
/// TRANSFER_TO_HOST_3D, and renders it as a full-screen textured quad.
/// If successful, the display shows the checkerboard pattern.
#[allow(dead_code)]
fn virgl_test_textured_quad() -> Result<(), &'static str> {
    use super::virgl::{CommandBuffer, format as vfmt, pipe, swizzle};

    let (width, height) = dimensions().ok_or("GPU not initialized")?;
    crate::serial_println!("[virgl-tex] Starting textured quad test ({}x{})...", width, height);

    // Step 1: Fill test texture backing with checkerboard pattern (BGRA format)
    unsafe {
        let tex_ptr = &raw mut TEST_TEX_BUF;
        let pixels = &mut (*tex_ptr).pixels;
        for y in 0..TEST_TEX_DIM {
            for x in 0..TEST_TEX_DIM {
                let offset = ((y * TEST_TEX_DIM + x) * 4) as usize;
                let checker = ((x / 8) + (y / 8)) % 2 == 0;
                if checker {
                    // Red in BGRA: B=0, G=0, R=255, X=255
                    pixels[offset] = 0;
                    pixels[offset + 1] = 0;
                    pixels[offset + 2] = 255;
                    pixels[offset + 3] = 255;
                } else {
                    // Blue in BGRA: B=255, G=0, R=0, X=255
                    pixels[offset] = 255;
                    pixels[offset + 1] = 0;
                    pixels[offset + 2] = 0;
                    pixels[offset + 3] = 255;
                }
            }
        }
    }
    crate::serial_println!("[virgl-tex] Checkerboard pattern written to backing ({} bytes)", TEST_TEX_BYTES);

    // Step 2: Create 3D texture resource (BIND_SAMPLER_VIEW)
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_TEX_ID,
            pipe::TEXTURE_2D,
            vfmt::B8G8R8A8_UNORM,
            pipe::BIND_SAMPLER_VIEW,
            TEST_TEX_DIM,
            TEST_TEX_DIM,
            1, 1,
        )
    })?;
    crate::serial_println!("[virgl-tex] Texture resource created (id={}, {}x{})", RESOURCE_TEX_ID, TEST_TEX_DIM, TEST_TEX_DIM);

    // Step 3: Attach backing memory to texture resource
    let tex_phys = virt_to_phys(&raw const TEST_TEX_BUF as u64);
    with_device_state(|state| {
        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
            let attach = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut PciAttachBackingCmd);
            *attach = PciAttachBackingCmd {
                cmd: VirtioGpuResourceAttachBacking {
                    hdr: VirtioGpuCtrlHdr {
                        type_: cmd::RESOURCE_ATTACH_BACKING,
                        flags: 0,
                        fence_id: 0,
                        ctx_id: 0,
                        padding: 0,
                    },
                    resource_id: RESOURCE_TEX_ID,
                    nr_entries: 1,
                },
                entry: VirtioGpuMemEntry {
                    addr: tex_phys,
                    length: TEST_TEX_BYTES as u32,
                    padding: 0,
                },
            };
        }
        send_command_expect_ok(state, core::mem::size_of::<PciAttachBackingCmd>() as u32)
    })?;
    crate::serial_println!("[virgl-tex] Texture backing attached (phys={:#x})", tex_phys);

    // Step 4: Attach texture to VirGL context
    with_device_state(|state| {
        virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_TEX_ID)
    })?;
    crate::serial_println!("[virgl-tex] Texture attached to context");

    // Step 5: Upload texture via TRANSFER_TO_HOST_3D
    with_device_state(|state| {
        transfer_to_host_3d(state, RESOURCE_TEX_ID, 0, 0, TEST_TEX_DIM, TEST_TEX_DIM, TEST_TEX_DIM * 4)
    })?;
    crate::serial_println!("[virgl-tex] Texture uploaded via TRANSFER_TO_HOST_3D");

    // Step 6: Create VirGL texture objects (FS shader, sampler view, sampler state)
    let mut cmdbuf = CommandBuffer::new();
    cmdbuf.set_sub_ctx(1);

    // Texture fragment shader: samples from SAMP[0] instead of passing vertex color
    let tex_fs = b"FRAG\nDCL IN[0], GENERIC[0], LINEAR\nDCL OUT[0], COLOR\nDCL SAMP[0]\nDCL SVIEW[0], 2D, FLOAT\n  0: TEX OUT[0], IN[0], SAMP[0], 2D\n  1: END\n";
    cmdbuf.create_shader(3, pipe::SHADER_FRAGMENT, tex_fs);

    // Sampler view: bind texture resource for shader sampling
    cmdbuf.create_sampler_view(
        5,                     // handle
        RESOURCE_TEX_ID,       // resource
        vfmt::B8G8R8A8_UNORM, // format
        0,                     // first_level
        0,                     // last_level
        swizzle::IDENTITY,     // RGBA identity swizzle
    );

    // Sampler state: nearest filtering, clamp-to-edge, no mipmapping
    cmdbuf.create_sampler_state(
        6,                            // handle
        pipe::TEX_WRAP_CLAMP_TO_EDGE, // wrap_s
        pipe::TEX_WRAP_CLAMP_TO_EDGE, // wrap_t
        pipe::TEX_WRAP_CLAMP_TO_EDGE, // wrap_r
        pipe::TEX_FILTER_NEAREST,     // min_img_filter
        pipe::TEX_MIPFILTER_NONE,     // min_mip_filter
        pipe::TEX_FILTER_NEAREST,     // mag_img_filter
    );

    virgl_submit(cmdbuf.as_slice())?;
    crate::serial_println!("[virgl-tex] Texture objects created (fs=3, view=5, state=6)");

    // Step 7: Bind texture pipeline and draw a full-screen textured quad
    cmdbuf.clear();
    cmdbuf.set_sub_ctx(1);

    // Re-emit pipeline state (Parallels may reset between SUBMIT_3D batches)
    cmdbuf.bind_shader(1, pipe::SHADER_VERTEX);   // existing VS (passes GENERIC[0])
    cmdbuf.bind_shader(3, pipe::SHADER_FRAGMENT);  // texture FS
    cmdbuf.bind_object(1, super::virgl::OBJ_BLEND);
    cmdbuf.bind_object(1, super::virgl::OBJ_DSA);
    cmdbuf.bind_object(1, super::virgl::OBJ_RASTERIZER);
    cmdbuf.bind_object(1, super::virgl::OBJ_VERTEX_ELEMENTS);
    cmdbuf.set_viewport(width as f32, height as f32);
    cmdbuf.set_framebuffer_state(0, &[1]); // existing surface handle
    cmdbuf.set_scissor_state(0, 0, width, height); // full screen

    // Bind sampler view and state to fragment shader
    cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[5]);
    cmdbuf.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[6]);

    // Clear to bright green — visible confirmation that VirGL CLEAR + display work.
    // If screen shows green, CLEAR works. If checkerboard overlays green, draw works too.
    cmdbuf.clear_color(0.0, 1.0, 0.0, 1.0);

    // Full-screen quad as triangle strip: 4 vertices, each 32 bytes
    // Position (x,y,z,w) + texcoord (u,v,0,0)
    // Viewport maps clip[-1,1] → screen[0,width]; y is flipped (1→top, -1→bottom)
    let quad_verts: [u32; 32] = [
        // v0: top-left (clip: -1,1) → texcoord (0,0)
        (-1.0f32).to_bits(), (1.0f32).to_bits(), 0f32.to_bits(), 1.0f32.to_bits(),
        0f32.to_bits(), 0f32.to_bits(), 0f32.to_bits(), 0f32.to_bits(),
        // v1: bottom-left (clip: -1,-1) → texcoord (0,1)
        (-1.0f32).to_bits(), (-1.0f32).to_bits(), 0f32.to_bits(), 1.0f32.to_bits(),
        0f32.to_bits(), 1.0f32.to_bits(), 0f32.to_bits(), 0f32.to_bits(),
        // v2: top-right (clip: 1,1) → texcoord (1,0)
        1.0f32.to_bits(), 1.0f32.to_bits(), 0f32.to_bits(), 1.0f32.to_bits(),
        1.0f32.to_bits(), 0f32.to_bits(), 0f32.to_bits(), 0f32.to_bits(),
        // v3: bottom-right (clip: 1,-1) → texcoord (1,1)
        1.0f32.to_bits(), (-1.0f32).to_bits(), 0f32.to_bits(), 1.0f32.to_bits(),
        1.0f32.to_bits(), 1.0f32.to_bits(), 0f32.to_bits(), 0f32.to_bits(),
    ];

    // Upload quad vertices inline to the vertex buffer
    cmdbuf.resource_inline_write(RESOURCE_VB_ID, 0, 128, &quad_verts);
    cmdbuf.set_vertex_buffers(&[(BYTES_PER_VERTEX as u32, 0, RESOURCE_VB_ID)]);

    // Draw triangle strip (4 vertices = 2 triangles = full screen)
    cmdbuf.draw_vbo(0, 4, pipe::PRIM_TRIANGLE_STRIP, 3);

    let fence_id = virgl_submit(cmdbuf.as_slice())?;
    crate::serial_println!("[virgl-tex] Textured quad submitted ({} DWORDs, fence={})", cmdbuf.as_slice().len(), fence_id);

    // Wait for VirGL rendering to complete on the host GPU before displaying.
    // Without this, the display refresh can race with async VirGL execution.
    crate::serial_println!("[virgl-tex] Waiting for fence {}...", fence_id);
    with_device_state(|state| {
        virgl_fence_sync(state, fence_id)
    })?;
    crate::serial_println!("[virgl-tex] Fence {} completed — VirGL rendering done", fence_id);

    // Step 8: Display via TRANSFER_TO_HOST_3D → SET_SCANOUT → RESOURCE_FLUSH.
    // Linux's virtio-gpu DRM driver issues TRANSFER_TO_HOST_3D as a sync signal
    // even for VirGL-rendered content. Parallels requires this before displaying.
    with_device_state(|state| {
        let (w, h) = (state.width, state.height);
        transfer_to_host_3d(state, RESOURCE_3D_ID, 0, 0, w, h, w * 4)?;
        crate::serial_println!("[virgl-tex] TRANSFER_TO_HOST_3D OK");
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        crate::serial_println!("[virgl-tex] SET_SCANOUT OK");
        resource_flush_3d(state, RESOURCE_3D_ID)?;
        crate::serial_println!("[virgl-tex] RESOURCE_FLUSH OK");
        Ok(())
    })?;

    crate::serial_println!("[virgl-tex] Test complete — check display for green+checkerboard");

    Ok(())
}
