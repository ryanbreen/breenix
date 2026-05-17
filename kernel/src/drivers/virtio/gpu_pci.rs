//! VirtIO GPU Device Driver for ARM64 (PCI Transport)
//!
//! Implements a basic GPU/display driver using VirtIO PCI modern transport.
//! Provides framebuffer functionality for simple 2D graphics.
//!
//! This driver reuses the same VirtIO GPU 2D protocol as `gpu_mmio.rs` but
//! communicates via the PCI transport layer (`VirtioPciDevice` from
//! `pci_transport.rs`) instead of MMIO registers.

use super::pci_transport::VirtioPciDevice;
use crate::tracing::providers::virtgpu;
use core::ptr::read_volatile;
use core::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

/// Lock protecting the GPU PCI command path (PCI_CMD_BUF, PCI_RESP_BUF,
/// PCI_CTRL_QUEUE, GPU_PCI_STATE).
/// Without this, concurrent callers corrupt the shared command/response
/// buffers and virtqueue state.
static GPU_PCI_LOCK: Mutex<()> = Mutex::new(());
static VIRTGPU_CMD_SEQ: AtomicU32 = AtomicU32::new(0);

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
    size: u32,     // size in bytes of the VirGL command buffer
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

/// Cursor queue (queue 1) — required by VirtIO GPU spec. Linux always sets up
/// both controlq and cursorq before calling driver_ok(). Not setting up the
/// cursor queue may leave the device in a partially-initialized state.
static mut PCI_CURSOR_QUEUE: PciCtrlQueueMemory = PciCtrlQueueMemory {
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
// Minimum resolution floor — with --high-resolution off, each guest pixel = 1 Mac point.
// 1280x960 gives a reasonably large VM window on Retina displays and keeps the
// per-frame DMA upload to ~4.9MB for good compositor FPS.
const MIN_FB_WIDTH: u32 = 1280;
const MIN_FB_HEIGHT: u32 = 960;
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

/// Resource ID for the compositor texture (BWM uploads pixel buffers here)
const RESOURCE_COMPOSITE_TEX_ID: u32 = 5;
/// Resource ID for the GPU cursor texture (16x80 atlas, uploaded once at init)
const RESOURCE_CURSOR_TEX_ID: u32 = 6;
/// Resource ID for the fullscreen dimmer overlay texture (4x4, B8G8R8A8, translucent black)
const RESOURCE_DIMMER_TEX_ID: u32 = 7;
/// Individual cursor shape dimensions
const CURSOR_SHAPE_W: u32 = 16;
const CURSOR_SHAPE_H: u32 = 16;
const NUM_CURSOR_SHAPES: u32 = 5;
const CURSOR_TEX_W: u32 = CURSOR_SHAPE_W;
const CURSOR_TEX_H: u32 = CURSOR_SHAPE_H * NUM_CURSOR_SHAPES; // 80

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

static mut PCI_FRAMEBUFFER: PciFramebuffer = PciFramebuffer {
    pixels: [0; FB_SIZE],
};

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
    let layout =
        alloc::alloc::Layout::from_size_align(size, 4096).expect("invalid 3D framebuffer layout");
    unsafe {
        let ptr = alloc::alloc::alloc_zeroed(layout);
        assert!(!ptr.is_null(), "failed to allocate 3D framebuffer backing");
        PCI_3D_FB_PTR = ptr;
        PCI_3D_FB_LEN = size;
    }
    let phys = virt_to_phys(unsafe { PCI_3D_FB_PTR } as u64);
    crate::serial_println!(
        "[virgl] 3D framebuffer backing: heap ptr={:#x}, phys={:#x}, size={}",
        unsafe { PCI_3D_FB_PTR } as u64,
        phys,
        size
    );
}

/// Pointer to heap-allocated vertex buffer backing (page-aligned, DMA-safe).
/// Initialized by `init_vb_backing()` during VirGL init.
#[allow(dead_code)]
static mut PCI_VB_PTR: *mut u8 = core::ptr::null_mut();
/// Actual size of the vertex buffer backing in bytes.
#[allow(dead_code)]
static mut PCI_VB_LEN: usize = 0;

/// Allocate vertex buffer backing on the heap with page alignment.
/// Currently unused: VB data is uploaded via RESOURCE_INLINE_WRITE in the batch.
/// Kept for future use if TRANSFER_TO_HOST_3D approach is needed.
#[allow(dead_code)]
fn init_vb_backing() {
    // 4KB is plenty for vertex data (128 bytes per quad × many quads)
    let size = 4096usize;
    let layout =
        alloc::alloc::Layout::from_size_align(size, 4096).expect("invalid VB backing layout");
    unsafe {
        let ptr = alloc::alloc::alloc_zeroed(layout);
        assert!(!ptr.is_null(), "failed to allocate VB backing");
        PCI_VB_PTR = ptr;
        PCI_VB_LEN = size;
    }
    let phys = virt_to_phys(unsafe { PCI_VB_PTR } as u64);
    crate::serial_println!(
        "[virgl] VB backing: heap ptr={:#x}, phys={:#x}, size={}",
        unsafe { PCI_VB_PTR } as u64,
        phys,
        size
    );
}

/// Static backing for test texture (64×64 BGRA)
#[repr(C, align(4096))]
struct TestTextureBuffer {
    pixels: [u8; TEST_TEX_BYTES],
}
static mut TEST_TEX_BUF: TestTextureBuffer = TestTextureBuffer {
    pixels: [0; TEST_TEX_BYTES],
};

/// Pointer to heap-allocated compositor texture backing (page-aligned, DMA-safe).
/// Used by `virgl_composite_frame()` to upload pixel buffers as GPU textures.
static mut COMPOSITE_TEX_PTR: *mut u8 = core::ptr::null_mut();
/// Size of the compositor texture backing in bytes.
static mut COMPOSITE_TEX_LEN: usize = 0;
/// Physical address of the first page of COMPOSITE_TEX backing.
/// Used by map_compositor_texture to MAP_SHARED into BWM's address space.
static COMPOSITE_TEX_PHYS_BASE: AtomicU64 = AtomicU64::new(0);
/// Number of pages in COMPOSITE_TEX backing.
static COMPOSITE_TEX_NUM_PAGES: AtomicU32 = AtomicU32::new(0);
/// Width of the compositor texture (set during init).
static COMPOSITE_TEX_W: AtomicU32 = AtomicU32::new(0);
/// Height of the compositor texture (set during init).
static COMPOSITE_TEX_H: AtomicU32 = AtomicU32::new(0);
/// Whether the compositor texture resource has been initialized.
static COMPOSITE_TEX_READY: AtomicBool = AtomicBool::new(false);

/// Whether the cursor GPU texture has been initialized.
static CURSOR_TEX_READY: AtomicBool = AtomicBool::new(false);
/// Whether the dimmer overlay texture has been initialized.
static DIMMER_TEX_READY: AtomicBool = AtomicBool::new(false);

/// Current cursor shape index (0=arrow, 1=NS, 2=EW, 3=NWSE, 4=NESW).
static CURSOR_SHAPE: AtomicU32 = AtomicU32::new(0);

/// Per-shape hotspot offsets (pixels from top-left of shape bitmap).
const CURSOR_HOTSPOT: [(i32, i32); 5] = [
    (0, 0), // Arrow: top-left
    (7, 7), // NS resize: center
    (7, 7), // EW resize: center
    (7, 7), // NWSE resize: center
    (7, 7), // NESW resize: center
];

// =============================================================================
// Per-Window GPU Textures
// =============================================================================

/// Base resource ID for per-window textures. Window slot N gets resource (10 + N).
const RESOURCE_WIN_TEX_BASE: u32 = 10;
/// Maximum number of per-window texture slots.
const MAX_WIN_TEX_SLOTS: usize = 8;

/// Per-slot backing buffer pointer and length.
static mut WIN_TEX_BACKING: [(*mut u8, usize); MAX_WIN_TEX_SLOTS] =
    [(core::ptr::null_mut(), 0); MAX_WIN_TEX_SLOTS];
/// Width/height of each slot's texture.
static mut WIN_TEX_DIMS: [(u32, u32); MAX_WIN_TEX_SLOTS] = [(0, 0); MAX_WIN_TEX_SLOTS];
/// Whether each slot has been initialized.
static mut WIN_TEX_INITIALIZED: [bool; MAX_WIN_TEX_SLOTS] = [false; MAX_WIN_TEX_SLOTS];

/// Create a per-window VirGL texture for GPU compositing.
///
/// Same resource creation pattern as COMPOSITE_TEX (proven working):
/// RESOURCE_CREATE_3D -> ATTACH_BACKING -> CTX_ATTACH -> TRANSFER_TO_HOST_3D
pub fn create_window_texture(slot: usize, width: u32, height: u32) -> Result<u32, &'static str> {
    use super::virgl::{format as vfmt, pipe};

    if slot >= MAX_WIN_TEX_SLOTS {
        return Err("window texture slot out of range");
    }

    let res_id = RESOURCE_WIN_TEX_BASE + slot as u32;

    // Already initialized — reuse if the requested size fits within the existing texture.
    // UV scaling in the compositor handles sub-regions correctly. Only recreate when
    // the window grows beyond the pre-allocated texture dimensions.
    if unsafe { WIN_TEX_INITIALIZED[slot] } {
        let (old_w, old_h) = unsafe { WIN_TEX_DIMS[slot] };
        if width <= old_w && height <= old_h {
            return Ok(res_id);
        }
        // Window grew beyond existing texture — destroy and recreate at new size
        crate::serial_println!(
            "[virgl-win-tex] Resize: slot={} {}x{} -> {}x{}",
            slot,
            old_w,
            old_h,
            width,
            height
        );
        with_device_state(|state| virgl_detach_backing_cmd(state, res_id)).ok();
        with_device_state(|state| virgl_resource_unref_cmd(state, res_id)).ok();
        let (old_ptr, old_size) = unsafe { WIN_TEX_BACKING[slot] };
        if !old_ptr.is_null() && old_size > 0 {
            let old_layout = alloc::alloc::Layout::from_size_align(old_size, 4096).unwrap();
            unsafe {
                alloc::alloc::dealloc(old_ptr, old_layout);
            }
        }
        unsafe {
            WIN_TEX_INITIALIZED[slot] = false;
            WIN_TEX_BACKING[slot] = (core::ptr::null_mut(), 0);
            WIN_TEX_DIMS[slot] = (0, 0);
        }
    }

    let tex_size = (width as usize) * (height as usize) * 4;
    let layout = alloc::alloc::Layout::from_size_align(tex_size, 4096)
        .map_err(|_| "invalid window texture layout")?;
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err("failed to allocate window texture backing");
    }

    // RESOURCE_CREATE_3D — same bind flags as COMPOSITE_TEX
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            res_id,
            pipe::TEXTURE_2D,
            vfmt::B8G8R8X8_UNORM,
            pipe::BIND_SAMPLER_VIEW | pipe::BIND_SCANOUT,
            width,
            height,
            1,
            1,
        )
    })?;

    // ATTACH_BACKING (paged scatter-gather)
    with_device_state(|state| virgl_attach_backing_paged(state, res_id, ptr, tex_size))?;

    // CTX_ATTACH_RESOURCE
    with_device_state(|state| virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, res_id))?;

    // Prime with TRANSFER_TO_HOST_3D
    dma_cache_clean(ptr, tex_size);
    with_device_state(|state| transfer_to_host_3d(state, res_id, 0, 0, width, height, width * 4))?;

    unsafe {
        WIN_TEX_BACKING[slot] = (ptr, tex_size);
        WIN_TEX_DIMS[slot] = (width, height);
        WIN_TEX_INITIALIZED[slot] = true;
    }

    crate::serial_println!(
        "[virgl-win-tex] Created: slot={} res_id={} {}x{} ({}B)",
        slot,
        res_id,
        width,
        height,
        tex_size
    );
    Ok(res_id)
}

/// Clear a GPU texture slot when the owning window is destroyed.
/// This prevents stale texture data from bleeding into new windows
/// that reuse the same slot index.
///
/// NOTE: Does NOT zero the backing buffer here. This function is called from
/// remove_for_pid() which runs in the process cleanup path (including from
/// context_switch.rs). Zeroing a large buffer (e.g. 750×550×4 = 1.6MB) in
/// that path corrupts other threads' contexts due to timing/memory issues.
/// The backing buffer is zeroed lazily in create_window_texture's reuse path.
pub fn clear_window_texture_slot(slot: usize) {
    if slot >= MAX_WIN_TEX_SLOTS {
        return;
    }
    unsafe {
        WIN_TEX_INITIALIZED[slot] = false;
        WIN_TEX_DIMS[slot] = (0, 0);
    }
}

/// Upload dirty window pixels to GPU texture via TRANSFER_TO_HOST_3D.
/// Copies scattered MAP_SHARED pages to contiguous backing, then uploads.
fn upload_window_texture(
    slot: usize,
    width: u32,
    height: u32,
    page_phys_addrs: &[u64],
    total_size: usize,
) -> Result<(), &'static str> {
    if slot >= MAX_WIN_TEX_SLOTS {
        return Err("slot out of range");
    }
    let (backing_ptr, backing_len) = unsafe { WIN_TEX_BACKING[slot] };
    if backing_ptr.is_null() {
        return Err("backing not allocated");
    }

    let win_bytes = (width as usize) * (height as usize) * 4;
    let copy_len = win_bytes.min(total_size).min(backing_len);

    // Copy scattered pages to contiguous backing.
    // page_phys_addrs contains PHYSICAL addresses — convert to kernel virtual.
    let mut copied = 0usize;
    for &page_phys in page_phys_addrs {
        if copied >= copy_len {
            break;
        }
        let chunk = 4096usize.min(copy_len - copied);
        let virt = phys_to_kern_virt(page_phys);
        unsafe {
            core::ptr::copy_nonoverlapping(virt as *const u8, backing_ptr.add(copied), chunk);
        }
        copied += chunk;
    }

    let res_id = RESOURCE_WIN_TEX_BASE + slot as u32;
    dma_cache_clean(backing_ptr, copy_len);
    with_device_state(|state| transfer_to_host_3d(state, res_id, 0, 0, width, height, width * 4))
}

/// Allocate and initialize the compositor texture resource for GPU compositing.
/// Creates a TEXTURE_2D resource with SAMPLER_VIEW bind, attaches heap-allocated
/// backing, and primes it with TRANSFER_TO_HOST_3D.
fn init_composite_texture(width: u32, height: u32) -> Result<(), &'static str> {
    use super::virgl::{format as vfmt, pipe};

    let size = (width as usize) * (height as usize) * 4;
    let layout = alloc::alloc::Layout::from_size_align(size, 4096)
        .expect("invalid composite texture layout");
    unsafe {
        let ptr = alloc::alloc::alloc_zeroed(layout);
        assert!(
            !ptr.is_null(),
            "failed to allocate composite texture backing"
        );
        COMPOSITE_TEX_PTR = ptr;
        COMPOSITE_TEX_LEN = size;
    }
    COMPOSITE_TEX_W.store(width, Ordering::Release);
    COMPOSITE_TEX_H.store(height, Ordering::Release);

    let phys = virt_to_phys(unsafe { COMPOSITE_TEX_PTR } as u64);
    let num_pages = (size + 4095) / 4096;
    COMPOSITE_TEX_PHYS_BASE.store(phys, Ordering::Release);
    COMPOSITE_TEX_NUM_PAGES.store(num_pages as u32, Ordering::Release);
    crate::serial_println!(
        "[virgl-composite] Texture backing: heap ptr={:#x}, phys={:#x}, {}x{} ({}B, {} pages)",
        unsafe { COMPOSITE_TEX_PTR } as u64,
        phys,
        width,
        height,
        size,
        num_pages
    );

    // Create texture resource (SAMPLER_VIEW + SCANOUT for direct display)
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_COMPOSITE_TEX_ID,
            pipe::TEXTURE_2D,
            vfmt::B8G8R8X8_UNORM,
            pipe::BIND_SAMPLER_VIEW | pipe::BIND_SCANOUT,
            width,
            height,
            1,
            1,
        )
    })?;

    // Attach backing memory (per-page scatter-gather, required by Parallels)
    with_device_state(|state| {
        virgl_attach_backing_paged(
            state,
            RESOURCE_COMPOSITE_TEX_ID,
            unsafe { COMPOSITE_TEX_PTR },
            size,
        )
    })?;

    // Attach to VirGL context
    with_device_state(|state| {
        virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_COMPOSITE_TEX_ID)
    })?;

    // Prime with TRANSFER_TO_HOST_3D (required by Parallels)
    dma_cache_clean(unsafe { COMPOSITE_TEX_PTR }, size);
    with_device_state(|state| {
        transfer_to_host_3d(
            state,
            RESOURCE_COMPOSITE_TEX_ID,
            0,
            0,
            width,
            height,
            width * 4,
        )
    })?;

    COMPOSITE_TEX_READY.store(true, Ordering::Release);
    crate::serial_println!(
        "[virgl-composite] Texture resource initialized (id={})",
        RESOURCE_COMPOSITE_TEX_ID
    );

    // Pre-allocate per-window texture pool at init time.
    // TRANSFER_TO_HOST_3D only works for resources created before first SUBMIT_3D.
    for slot in 0..MAX_WIN_TEX_SLOTS {
        let max_w: u32 = MIN_FB_WIDTH;
        let max_h: u32 = MIN_FB_HEIGHT;
        let tex_size = (max_w as usize) * (max_h as usize) * 4;
        let res_id = RESOURCE_WIN_TEX_BASE + slot as u32;

        let layout = alloc::alloc::Layout::from_size_align(tex_size, 4096)
            .map_err(|_| "invalid pre-alloc texture layout")?;
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("failed to allocate pre-alloc texture backing");
        }

        with_device_state(|state| {
            virgl_resource_create_3d_cmd(
                state,
                res_id,
                pipe::TEXTURE_2D,
                vfmt::B8G8R8A8_UNORM,
                pipe::BIND_SAMPLER_VIEW | pipe::BIND_SCANOUT,
                max_w,
                max_h,
                1,
                1,
            )
        })?;
        with_device_state(|state| virgl_attach_backing_paged(state, res_id, ptr, tex_size))?;
        with_device_state(|state| virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, res_id))?;
        dma_cache_clean(ptr, tex_size);
        with_device_state(|state| {
            transfer_to_host_3d(state, res_id, 0, 0, max_w, max_h, max_w * 4)
        })?;

        unsafe {
            WIN_TEX_BACKING[slot] = (ptr, tex_size);
            WIN_TEX_DIMS[slot] = (max_w, max_h);
            WIN_TEX_INITIALIZED[slot] = true;
        }
        crate::serial_println!(
            "[virgl-pool] Pre-allocated slot={} res_id={} {}x{}",
            slot,
            res_id,
            max_w,
            max_h
        );
    }

    // Initialize cursor GPU texture (12x18 arrow bitmap, uploaded once)
    init_cursor_texture()?;

    // Initialize dimmer overlay texture (4x4 translucent black, for launcher dimming)
    init_dimmer_texture()?;

    Ok(())
}

/// Set the active cursor shape. Called from the set_cursor_shape syscall.
/// 0=arrow, 1=NS resize, 2=EW resize, 3=NWSE resize, 4=NESW resize.
pub fn set_cursor_shape(shape: u32) {
    if shape < NUM_CURSOR_SHAPES {
        CURSOR_SHAPE.store(shape, Ordering::Release);
    }
}

/// Initialize a small GPU texture containing the cursor arrow bitmap.
///
/// The cursor is rendered as a GPU quad in `virgl_composite_single_quad()`,
/// sampling from this texture. This avoids stamping the cursor into COMPOSITE_TEX
/// (which caused ghost trails when the saved background was stale).
fn init_cursor_texture() -> Result<(), &'static str> {
    use super::virgl::{format as vfmt, pipe};

    // 5 cursor shapes, each 16x16. 0=transparent, 1=white, 2=black outline.
    // Shape 0: Arrow pointer
    // Shape 1: NS resize (vertical double arrow)
    // Shape 2: EW resize (horizontal double arrow)
    // Shape 3: NWSE resize (diagonal ↘↗)
    // Shape 4: NESW resize (diagonal ↙↗)
    const SHAPES: [[[u8; 16]; 16]; 5] = [
        // Shape 0: Arrow
        [
            [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 2, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 2, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 2, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ],
        // Shape 1: NS resize (vertical double arrow ↕)
        [
            [0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 2, 1, 2, 2, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 2, 1, 2, 2, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ],
        // Shape 2: EW resize (horizontal double arrow ↔)
        [
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0],
            [0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0],
            [0, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 0],
            [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2],
            [0, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0],
            [0, 0, 2, 1, 1, 2, 2, 2, 2, 2, 2, 1, 1, 2, 0, 0],
            [0, 0, 2, 1, 2, 0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0],
            [0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ],
        // Shape 3: NWSE resize (diagonal ↘↗)
        [
            [2, 2, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 2, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 2, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 2, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 2],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 2, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 1, 2],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2, 2, 2],
        ],
        // Shape 4: NESW resize (diagonal ↙↗)
        [
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2, 2, 2],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 1, 2],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 2],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 2, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 1, 2, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 2, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [2, 2, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ],
    ];

    let w = CURSOR_TEX_W;
    let h = CURSOR_TEX_H;
    let size = (w as usize) * (h as usize) * 4;

    // Allocate page-aligned backing (16*80*4=5120 bytes, needs 2 pages)
    let alloc_size = 8192usize;
    let layout = alloc::alloc::Layout::from_size_align(alloc_size, 4096)
        .map_err(|_| "invalid cursor texture layout")?;
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err("failed to allocate cursor texture backing");
    }

    // Rasterize all cursor shapes into BGRA pixels.
    unsafe {
        let pixels = ptr as *mut u32;
        for shape in 0..NUM_CURSOR_SHAPES as usize {
            for row in 0..CURSOR_SHAPE_H as usize {
                for col in 0..CURSOR_SHAPE_W as usize {
                    let tex_row = shape * CURSOR_SHAPE_H as usize + row;
                    let idx = tex_row * w as usize + col;
                    *pixels.add(idx) = match SHAPES[shape][row][col] {
                        1 => 0xFF_FF_FF_FF, // white
                        2 => 0xFF_00_00_00, // black with alpha=FF
                        _ => 0x00_00_00_00, // transparent
                    };
                }
            }
        }
    }

    // Create texture resource
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_CURSOR_TEX_ID,
            pipe::TEXTURE_2D,
            vfmt::B8G8R8A8_UNORM,
            pipe::BIND_SAMPLER_VIEW,
            w,
            h,
            1,
            1,
        )
    })?;

    // Attach backing memory
    with_device_state(|state| {
        virgl_attach_backing_paged(state, RESOURCE_CURSOR_TEX_ID, ptr, alloc_size)
    })?;

    // Attach to VirGL context
    with_device_state(|state| {
        virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_CURSOR_TEX_ID)
    })?;

    // Prime with TRANSFER_TO_HOST_3D
    dma_cache_clean(ptr, size);
    with_device_state(|state| {
        transfer_to_host_3d(state, RESOURCE_CURSOR_TEX_ID, 0, 0, w, h, w * 4)
    })?;

    CURSOR_TEX_READY.store(true, Ordering::Release);
    crate::serial_println!(
        "[virgl-cursor] Cursor texture initialized (id={}, {}x{}, {} shapes)",
        RESOURCE_CURSOR_TEX_ID,
        w,
        h,
        NUM_CURSOR_SHAPES
    );

    Ok(())
}

/// Initialize the dimmer overlay texture.
///
/// A tiny 4x4 B8G8R8A8_UNORM texture filled with translucent black (50% alpha).
/// Drawn as a fullscreen quad with alpha blend to dim the desktop when the
/// launcher overlay is active.
fn init_dimmer_texture() -> Result<(), &'static str> {
    use super::virgl::{format as vfmt, pipe};

    let w: u32 = 4;
    let h: u32 = 4;
    let size = (w as usize) * (h as usize) * 4; // 64 bytes
    let alloc_size = 4096; // minimum page

    let layout = alloc::alloc::Layout::from_size_align(alloc_size, 4096)
        .map_err(|_| "invalid dimmer texture layout")?;
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err("failed to allocate dimmer texture backing");
    }

    // Fill with translucent black: B8G8R8A8 = 0xB0000000 (A=0xB0 ~69%, R=G=B=0)
    unsafe {
        let px = ptr as *mut u32;
        for i in 0..(w * h) as usize {
            *px.add(i) = 0xB0000000;
        }
    }

    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_DIMMER_TEX_ID,
            pipe::TEXTURE_2D,
            vfmt::B8G8R8A8_UNORM,
            pipe::BIND_SAMPLER_VIEW,
            w,
            h,
            1,
            1,
        )
    })?;

    with_device_state(|state| {
        virgl_attach_backing_paged(state, RESOURCE_DIMMER_TEX_ID, ptr, alloc_size)
    })?;

    with_device_state(|state| {
        virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_DIMMER_TEX_ID)
    })?;

    dma_cache_clean(ptr, size);
    with_device_state(|state| {
        transfer_to_host_3d(state, RESOURCE_DIMMER_TEX_ID, 0, 0, w, h, w * 4)
    })?;

    DIMMER_TEX_READY.store(true, Ordering::Release);
    crate::serial_println!(
        "[virgl-dimmer] Dimmer texture initialized (id={}, {}x{})",
        RESOURCE_DIMMER_TEX_ID,
        w,
        h
    );

    Ok(())
}

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

/// Whether GPU command waits should yield to the scheduler instead of spinning.
/// False during init (single-threaded, scheduler may not be ready).
/// True after init completes (compositor runtime — yield lets other tasks run
/// during the ~3.4ms GPU processing time for SUBMIT_3D).
static GPU_YIELD_ENABLED: AtomicBool = AtomicBool::new(false);

/// Accumulated ticks spent sleeping (blocked) during GPU command waits.
/// Used by ksyscall-perf to distinguish sleep time from CPU time.
static GPU_SLEEP_TICKS: AtomicU64 = AtomicU64::new(0);

/// Separate sleep tick counter for gpu-phases reporting.
/// ksyscall-perf swaps GPU_SLEEP_TICKS to 0 on its own schedule;
/// gpu-phases needs its own counter to avoid interference.
static GPU_SLEEP_TICKS_PHASES: AtomicU64 = AtomicU64::new(0);

/// Thread ID of the thread currently blocked waiting for GPU command completion.
/// Set before blocking in send_command_3desc, cleared after waking.
/// The GPU interrupt handler uses this to wake the thread immediately.
static GPU_WAITING_THREAD: AtomicU64 = AtomicU64::new(0);

/// Enable yielding during GPU command waits.
/// Called after GPU init completes, when the scheduler is fully running.
pub fn enable_gpu_yield() {
    GPU_YIELD_ENABLED.store(true, Ordering::Release);

    // Enable GPU MSI-X SPI now that VirGL init is complete.
    // During init, the SPI is configured but not enabled to avoid interrupt storms.
    let irq = GPU_IRQ.load(Ordering::Relaxed);
    if irq != 0 {
        #[cfg(target_arch = "aarch64")]
        {
            use crate::arch_impl::aarch64::gic;

            // Assign VirtIO-level MSI-X vectors via modern transport common config.
            // Config change → no interrupt (0xFFFF). Controlq (0) → vector 0.
            unsafe {
                let ptr = &raw const GPU_PCI_STATE;
                if let Some(ref state) = *ptr {
                    state.device.set_config_msix_vector(0xFFFF);
                    state.device.select_queue(0);
                    let rb = state.device.set_queue_msix_vector(0);
                    if rb == 0xFFFF {
                        crate::serial_println!(
                            "[virtio-gpu-pci] MSI-X: device rejected controlq vector — disabling"
                        );
                        GPU_IRQ.store(0, Ordering::Relaxed);
                    } else {
                        // Clear any pending SPI from init/VirGL commands before enabling
                        gic::clear_spi_pending(irq);
                        gic::enable_spi(irq);
                        crate::serial_println!("[virtio-gpu-pci] MSI-X SPI {} enabled — interrupt-driven GPU wake active", irq);
                    }
                }
            }
        }
    } else {
        crate::serial_println!("[virtio-gpu-pci] GPU yield enabled (polling mode — no MSI-X)");
    }
}

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

/// Inverse of virt_to_phys: convert a physical address to a kernel-accessible
/// virtual address. Uses physical_memory_offset (HHDM base) when available.
#[inline(always)]
fn phys_to_kern_virt(phys: u64) -> u64 {
    let offset = crate::memory::physical_memory_offset().as_u64();
    offset + phys
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

/// Invalidate data cache lines covering a DMA buffer.
///
/// After the host writes to guest memory via DMA (e.g., TRANSFER_FROM_HOST),
/// the CPU cache may hold stale data. DC CIVAC cleans and invalidates each
/// cache line so subsequent CPU reads see the DMA-written values.
/// Currently unused but kept for future TRANSFER_FROM_HOST operations.
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
#[inline]
#[allow(dead_code)]
fn dma_cache_invalidate(ptr: *const u8, len: usize) {
    const CACHE_LINE: usize = 64;
    let start = ptr as usize & !(CACHE_LINE - 1);
    let end = (ptr as usize + len + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    for addr in (start..end).step_by(CACHE_LINE) {
        unsafe {
            core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

#[allow(dead_code)]
#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn dma_cache_invalidate(_ptr: *const u8, _len: usize) {
    // x86_64 has cache-coherent DMA; no explicit invalidation needed.
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

    // Step 3: Try MSI-X (what VirtIO modern devices use)
    if let Some(msix_cap) = pci_dev.find_msix_capability() {
        let table_size = pci_dev.msix_table_size(msix_cap);
        crate::serial_println!(
            "[virtio-gpu-pci] MSI-X cap at {:#x}: {} vectors",
            msix_cap,
            table_size
        );

        // Program all MSI-X table entries with same SPI (single-vector mode)
        for v in 0..table_size {
            pci_dev.configure_msix_entry(msix_cap, v, msi_address, spi);
        }

        gic::configure_spi_edge_triggered(spi);
        // Do NOT store GPU_IRQ or enable SPI here. GPU_IRQ=0 during init means
        // send_command uses spin-polling and the interrupt handler ignores GPU SPIs.
        // Both are activated after all init commands succeed (see end of init()).

        // Enable MSI-X at PCI level and disable legacy INTx
        pci_dev.enable_msix(msix_cap);
        pci_dev.disable_intx();

        crate::serial_println!(
            "[virtio-gpu-pci] MSI-X enabled: SPI {} doorbell={:#x} vectors={}",
            spi,
            msi_address,
            table_size
        );
        return spi;
    }

    // Step 4: Fall back to plain MSI
    if let Some(msi_cap) = pci_dev.find_msi_capability() {
        pci_dev.configure_msi(msi_cap, msi_address as u32, spi as u16);
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

    // Wake the compositor thread blocked in send_command_3desc.
    let waiting_tid = GPU_WAITING_THREAD.load(Ordering::Acquire);
    if waiting_tid != 0 {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.unblock(waiting_tid);
        });
        crate::task::scheduler::set_need_resched();
    }

    gic::clear_spi_pending(irq);
    gic::enable_spi(irq);
}

/// Get the GIC INTID for the GPU interrupt (for exception dispatch).
pub fn get_irq() -> Option<u32> {
    let irq = GPU_IRQ.load(Ordering::Relaxed);
    if irq != 0 {
        Some(irq)
    } else {
        None
    }
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
    let pci_dev =
        crate::drivers::pci::find_device(0x1AF4, 0x1050).ok_or("No VirtIO GPU PCI device found")?;

    // Probe VirtIO modern transport
    let mut virtio =
        VirtioPciDevice::probe(pci_dev).ok_or("VirtIO GPU PCI: no modern capabilities")?;

    // Init (reset, negotiate features).
    // VIRTIO_F_VERSION_1 is mandatory for PCI modern transport — without it,
    // Parallels's GPU device accepts the feature set but ignores subsequent
    // state-modifying commands (create_resource, attach_backing, etc.).
    let requested = VIRTIO_F_VERSION_1 | VIRTIO_GPU_F_EDID | VIRTIO_GPU_F_VIRGL;

    // Log raw device-offered features before negotiation
    let device_feats = virtio.read_device_features();
    crate::serial_println!("[virtio-gpu-pci] Device features: {:#018x}", device_feats);
    crate::serial_println!(
        "[virtio-gpu-pci] VIRGL offered: {}",
        device_feats & VIRTIO_GPU_F_VIRGL != 0
    );

    virtio.init(requested)?;

    // Check what was actually negotiated
    let negotiated = virtio.device_features() & requested;
    let virgl_on = negotiated & VIRTIO_GPU_F_VIRGL != 0;
    crate::serial_println!(
        "[virtio-gpu-pci] Negotiated: {:#018x} (VIRGL={})",
        negotiated,
        virgl_on
    );
    VIRGL_ENABLED.store(virgl_on, Ordering::Release);
    crate::serial_println!(
        "[virtio-gpu-pci] VIRGL_ENABLED stored={}, readback={}, addr={:#x}",
        virgl_on,
        VIRGL_ENABLED.load(Ordering::Acquire),
        &VIRGL_ENABLED as *const _ as usize
    );

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
    crate::serial_println!(
        "[virtio-gpu-pci] config_msix_vector: wrote={:#x} readback={:#x}",
        config_vector,
        readback
    );

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
    crate::serial_println!(
        "[virtio-gpu-pci] Queue 0 msix_vector: wrote={:#x} readback={:#x}",
        queue_vector,
        q0_readback
    );

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
        crate::serial_println!(
            "[virtio-gpu-pci] Queue 1 msix_vector: wrote={:#x} readback={:#x}",
            queue_vector,
            q1_readback
        );

        virtio.set_queue_ready(true);
        crate::serial_println!(
            "[virtio-gpu-pci] Cursor queue (q1) set up: size={}",
            cursor_queue_size
        );
    } else {
        crate::serial_println!("[virtio-gpu-pci] Cursor queue (q1) not available (max=0)");
    }

    // Mark device ready — MUST happen before sending any commands (Linux: virtio_device_ready())
    virtio.driver_ok();

    // GPU_IRQ=0 at this point (set in setup_gpu_msi but NOT stored in GPU_IRQ).
    // All init commands below use spin-polling. SPI is enabled after init succeeds.

    // Read device-specific config (Linux reads num_scanouts + num_capsets here)
    let num_scanouts = virtio.read_config_u32(GPU_CFG_NUM_SCANOUTS);
    let num_capsets = virtio.read_config_u32(GPU_CFG_NUM_CAPSETS);
    crate::serial_println!(
        "[virtio-gpu-pci] Config: num_scanouts={}, num_capsets={}",
        num_scanouts,
        num_capsets
    );

    // Check and clear pending display events (Linux: virtio_gpu_config_changed_work_func)
    let events = virtio.read_config_u32(GPU_CFG_EVENTS_READ);
    if events & VIRTIO_GPU_EVENT_DISPLAY != 0 {
        crate::serial_println!(
            "[virtio-gpu-pci] Clearing pending DISPLAY event (events_read={:#x})",
            events
        );
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
                crate::serial_println!(
                    "[virtio-gpu-pci] Capset {}: id={}, max_ver={}, max_size={}",
                    idx,
                    id,
                    max_ver,
                    max_size
                );
                // Linux always retrieves the actual capset data after GET_CAPSET_INFO.
                match get_capset(id, max_ver) {
                    Ok(resp_type) => {
                        crate::serial_println!(
                            "[virtio-gpu-pci] GET_CAPSET(id={}, ver={}): resp={:#x}",
                            id,
                            max_ver,
                            resp_type
                        );
                    }
                    Err(e) => {
                        crate::serial_println!(
                            "[virtio-gpu-pci] GET_CAPSET(id={}, ver={}) failed: {}",
                            id,
                            max_ver,
                            e
                        );
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

    // Pick the largest resolution from: GET_DISPLAY_INFO, GOP, or the minimum.
    // On Retina Macs with --high-resolution off, Parallels maps 1 guest pixel to
    // 1 Mac screen point. GET_DISPLAY_INFO may report only 1024x768 in this mode,
    // but SET_SCANOUT at a larger resolution works fine (Linux does arbitrary mode
    // changes via drmModeSetCrtc → SET_SCANOUT). We enforce a 1280x960 minimum
    // so the VM window is reasonably large on modern displays.
    let gop_w = crate::platform_config::fb_width();
    let gop_h = crate::platform_config::fb_height();
    let (disp_w, disp_h) = display_dims.unwrap_or((0, 0));
    // Pick the widest valid resolution, with 1280x960 as floor
    // NOTE: DEFAULT_FB dimensions excluded for now — compositor texture stride
    // depends on matching the BWM buffer width. Needs coordinated fix.
    let candidates: [(u32, u32); 3] = [
        (disp_w, disp_h),
        (gop_w, gop_h),
        (MIN_FB_WIDTH, MIN_FB_HEIGHT),
    ];
    let (use_width, use_height) = candidates
        .iter()
        .filter(|&&(w, h)| w >= MIN_FB_WIDTH && h > 0 && w <= FB_MAX_WIDTH && h <= FB_MAX_HEIGHT)
        .max_by_key(|&&(w, _)| w)
        .copied()
        .unwrap_or((MIN_FB_WIDTH, MIN_FB_HEIGHT));
    crate::serial_println!(
        "[virtio-gpu-pci] Resolution: {}x{} (display={}x{}, GOP={}x{}, min={}x{})",
        use_width,
        use_height,
        disp_w,
        disp_h,
        gop_w,
        gop_h,
        MIN_FB_WIDTH,
        MIN_FB_HEIGHT
    );

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

    // Store GPU_IRQ now so enable_gpu_yield() (called after virgl_init) can
    // activate interrupt-driven wake. We do NOT enable the SPI here — all
    // VirGL init commands also use spin-polling. enable_gpu_yield() handles
    // clearing pending, storing GPU_IRQ, and enabling the SPI.
    #[cfg(target_arch = "aarch64")]
    if msi_spi != 0 {
        GPU_IRQ.store(msi_spi, Ordering::Release);
        crate::serial_println!(
            "[virtio-gpu-pci] MSI-X configured (SPI={}, deferred enable after VirGL init)",
            msi_spi
        );
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

#[inline(always)]
fn virtgpu_next_trace_seq() -> u16 {
    VIRTGPU_CMD_SEQ
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1) as u16
}

#[inline(always)]
fn virtgpu_decode_command(buf: *const u8) -> (u32, u32) {
    unsafe {
        let hdr = &*(buf as *const VirtioGpuCtrlHdr);
        let cmd_type = core::ptr::read_volatile(&hdr.type_);
        let resource_id = match cmd_type {
            cmd::RESOURCE_CREATE_2D => {
                let cmd = &*(buf as *const VirtioGpuResourceCreate2d);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::RESOURCE_UNREF | cmd::RESOURCE_DETACH_BACKING => {
                let cmd = &*(buf as *const VirtioGpuCtxResource);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::SET_SCANOUT => {
                let cmd = &*(buf as *const VirtioGpuSetScanout);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::RESOURCE_FLUSH => {
                let cmd = &*(buf as *const VirtioGpuResourceFlush);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::TRANSFER_TO_HOST_2D => {
                let cmd = &*(buf as *const VirtioGpuTransferToHost2d);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::RESOURCE_ATTACH_BACKING => {
                let cmd = &*(buf as *const VirtioGpuResourceAttachBacking);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::CTX_ATTACH_RESOURCE | cmd::CTX_DETACH_RESOURCE => {
                let cmd = &*(buf as *const VirtioGpuCtxResource);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::RESOURCE_CREATE_3D => {
                let cmd = &*(buf as *const VirtioGpuResourceCreate3d);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::TRANSFER_TO_HOST_3D => {
                let cmd = &*(buf as *const VirtioGpuTransferHost3d);
                core::ptr::read_volatile(&cmd.resource_id)
            }
            cmd::SUBMIT_3D => core::ptr::read_volatile(&hdr.ctx_id),
            _ => 0,
        };
        (cmd_type, resource_id)
    }
}

#[inline(always)]
fn virtgpu_decode_pci_cmd() -> (u32, u32) {
    let cmd_ptr = &raw const PCI_CMD_BUF;
    unsafe { virtgpu_decode_command((*cmd_ptr).data.as_ptr()) }
}

#[inline(always)]
fn virtgpu_decode_3desc_command(hdr_phys: u64) -> (u32, u32) {
    let pci_cmd_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
    if hdr_phys == pci_cmd_phys {
        return virtgpu_decode_pci_cmd();
    }

    let pci_3d_phys = virt_to_phys(&raw const PCI_3D_CMD_BUF as u64);
    if hdr_phys == pci_3d_phys {
        let cmd_ptr = &raw const PCI_3D_CMD_BUF;
        return unsafe { virtgpu_decode_command((*cmd_ptr).data.as_ptr()) };
    }

    (0, 0)
}

#[inline(always)]
fn virtgpu_trace_submission(cmd_type: u32, resource_id: u32) {
    let seq = virtgpu_next_trace_seq();
    virtgpu::trace_cmd_submit(cmd_type, seq);
    virtgpu::trace_cmd_resource(resource_id);
    virtgpu_note_resource2_submit(cmd_type, resource_id);
}

#[inline(always)]
fn virtgpu_note_resource2_submit(cmd_type: u32, resource_id: u32) {
    if resource_id != RESOURCE_3D_ID {
        return;
    }

    match cmd_type {
        cmd::RESOURCE_CREATE_3D => virtgpu::VIRTGPU_R2_CREATE.increment(),
        cmd::RESOURCE_ATTACH_BACKING => virtgpu::VIRTGPU_R2_ATTACH_BACKING.increment(),
        cmd::CTX_ATTACH_RESOURCE => virtgpu::VIRTGPU_R2_CTX_ATTACH.increment(),
        cmd::TRANSFER_TO_HOST_3D => virtgpu::VIRTGPU_R2_TRANSFER.increment(),
        cmd::SET_SCANOUT => virtgpu::VIRTGPU_R2_SET_SCANOUT.increment(),
        cmd::RESOURCE_UNREF | cmd::RESOURCE_DETACH_BACKING | cmd::CTX_DETACH_RESOURCE => {
            virtgpu::VIRTGPU_R2_UNREF_OR_DETACH.increment()
        }
        _ => {}
    }
}

#[inline(always)]
fn virtgpu_note_resource2_response(cmd_type: u32, resource_id: u32, resp_type: u32) {
    if cmd_type == cmd::RESOURCE_FLUSH && resource_id == RESOURCE_3D_ID {
        if resp_type == cmd::RESP_OK_NODATA {
            virtgpu::VIRTGPU_R2_FLUSH_OK.increment();
        } else {
            virtgpu::VIRTGPU_R2_FLUSH_FAIL.increment();
        }
    }
}

#[inline(always)]
fn virtgpu_trace_used_idx() -> u16 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let q = &raw const PCI_CTRL_QUEUE;
        let used_addr = &(*q).used as *const _ as usize;
        core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }

    fence(Ordering::Acquire);
    unsafe {
        let q = &raw const PCI_CTRL_QUEUE;
        read_volatile(&(*q).used.idx)
    }
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
    // Drain stale completions from previous timed-out commands.
    // Only advance forward (diff > 0 && < 32768 in wrapping u16 space)
    // to avoid undoing a speculative advance from a prior timeout recovery.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let q = &raw const PCI_CTRL_QUEUE;
        let used_addr = &(*q).used as *const _ as usize;
        core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        fence(Ordering::Acquire);
        let cur = read_volatile(&(*q).used.idx);
        let diff = cur.wrapping_sub(state.last_used_idx);
        if diff > 0 && diff < 32768 {
            state.last_used_idx = cur;
            virtgpu::trace_stale_drain(diff, cur);
        }
    }

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
        core::ptr::write_volatile(&mut (*q).avail.ring[(idx % 16) as usize], 0);

        // Drain store buffer before cache flush — ensures the avail.ring write
        // is in cache (not still in the store buffer) when dc civac runs.
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));

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

    // Suppress device interrupts for fast 2-desc commands (spin-only).
    unsafe {
        let q = &raw mut PCI_CTRL_QUEUE;
        (*q).avail.flags = 1; // VRING_AVAIL_F_NO_INTERRUPT
        #[cfg(target_arch = "aarch64")]
        dma_cache_clean(&(*q).avail.flags as *const u16 as *const u8, 64);
    }

    // Signal that we're waiting for a completion, then notify device
    GPU_CMD_COMPLETE.store(false, Ordering::Release);
    virtgpu::trace_q_notify(0, virtgpu_trace_used_idx());
    state.device.notify_queue_fast(0);

    // Wait for used ring update — tight spin for 2-desc commands.
    // These complete in microseconds (SET_SCANOUT, RESOURCE_FLUSH, TRANSFER_TO_HOST_3D).
    // Yielding here adds ~1-2ms of context switch overhead per command, which is
    // catastrophic with 5+ commands per frame.
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
            virtgpu::trace_q_complete(0, used_idx);
            state.last_used_idx = used_idx;
            return Ok(());
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("GPU PCI command timeout");
        }
        core::hint::spin_loop();
    }
}

/// Send a command using a 3-descriptor chain (Linux format):
///   Desc 0: command header (device reads)
///   Desc 1: command payload (device reads)
///   Desc 2: response (device writes)
///
/// Returns Ok((used_len, resp_type)) on completion, Err on timeout.
fn send_command_3desc(
    state: &mut GpuPciDeviceState,
    hdr_phys: u64,
    hdr_len: u32,
    payload_phys: u64,
    payload_len: u32,
) -> Result<(u32, u32), &'static str> {
    let (cmd_type, resource_id) = virtgpu_decode_3desc_command(hdr_phys);
    virtgpu_trace_submission(cmd_type, resource_id);

    // Drain stale completions (same forward-only logic as send_command).
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let q = &raw const PCI_CTRL_QUEUE;
        let used_addr = &(*q).used as *const _ as usize;
        core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        fence(Ordering::Acquire);
        let cur = read_volatile(&(*q).used.idx);
        let diff = cur.wrapping_sub(state.last_used_idx);
        if diff > 0 && diff < 32768 {
            state.last_used_idx = cur;
            virtgpu::trace_stale_drain(diff, cur);
        }
    }

    let resp_phys = virt_to_phys(&raw const PCI_RESP_BUF as u64);
    let resp_len = core::mem::size_of::<VirtioGpuCtrlHdr>() as u32;

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

        // Use desc[0..2] with head=0 for 3-desc commands.
        // Parallels VirtIO GPU intermittently ignores non-zero head indices
        // in the avail ring, always reading desc[0] regardless.
        (*q).desc[0] = VirtqDesc {
            addr: hdr_phys,
            len: hdr_len,
            flags: DESC_F_NEXT,
            next: 1,
        };
        (*q).desc[1] = VirtqDesc {
            addr: payload_phys,
            len: payload_len,
            flags: DESC_F_NEXT,
            next: 2,
        };
        (*q).desc[2] = VirtqDesc {
            addr: resp_phys,
            len: resp_len,
            flags: DESC_F_WRITE,
            next: 0,
        };

        // Add to available ring with head=0
        let idx = (*q).avail.idx;
        core::ptr::write_volatile(&mut (*q).avail.ring[(idx % 16) as usize], 0);

        // DSB SY to drain store buffer before cache maintenance
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));

        // Flush desc + avail ring from CPU cache to RAM
        #[cfg(target_arch = "aarch64")]
        {
            let q_addr = q as *const u8;
            dma_cache_clean(q_addr, 512);
        }

        fence(Ordering::SeqCst);
        core::ptr::write_volatile(&mut (*q).avail.idx, idx.wrapping_add(1));

        // DSB before avail.idx flush
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));

        // Flush the updated avail.idx
        #[cfg(target_arch = "aarch64")]
        {
            let avail_idx_addr = &(*q).avail.idx as *const u16 as *const u8;
            dma_cache_clean(avail_idx_addr, 64);
        }

        fence(Ordering::SeqCst);
    }

    let can_yield = GPU_YIELD_ENABLED.load(Ordering::Relaxed);
    let has_msi = GPU_IRQ.load(Ordering::Relaxed) != 0;

    // When MSI-X is active: zero-spin, pure interrupt-driven wake.
    // CRITICAL: Enable interrupts and register the waiting thread BEFORE notifying
    // the device. The previous send_command (2-desc) leaves avail.flags=1 (NO_INTERRUPT).
    // If we notify first, the device sees NO_INTERRUPT, processes the command without
    // firing MSI-X, and we block forever (until the 10ms safety timeout).
    //
    // When MSI-X is not available: spin briefly then block with timer fallback.
    if !has_msi {
        // No MSI-X — suppress interrupts, we'll poll.
        unsafe {
            let q = &raw mut PCI_CTRL_QUEUE;
            (*q).avail.flags = 1; // VRING_AVAIL_F_NO_INTERRUPT
            #[cfg(target_arch = "aarch64")]
            dma_cache_clean(&(*q).avail.flags as *const u16 as *const u8, 64);
        }
    }

    #[cfg(target_arch = "aarch64")]
    if has_msi && can_yield {
        // Register thread for interrupt-driven wake BEFORE notify
        if let Some(tid) = crate::task::scheduler::current_thread_id() {
            GPU_WAITING_THREAD.store(tid, Ordering::Release);
        }

        // Enable device interrupts BEFORE notify — so the device sees avail.flags=0
        // when it processes the command and fires MSI-X on completion.
        unsafe {
            let q = &raw mut PCI_CTRL_QUEUE;
            (*q).avail.flags = 0; // Enable notifications
            dma_cache_clean(&(*q).avail.flags as *const u16 as *const u8, 64);
        }
        fence(Ordering::SeqCst);
    }

    // Notify device
    virtgpu::trace_q_notify(0, virtgpu_trace_used_idx());
    state.device.notify_queue_fast(0);

    let used_len;

    #[cfg(target_arch = "aarch64")]
    if has_msi && can_yield {
        // MSI-X path: retry blocking up to 20 times (200ms total).
        // Tolerates VM scheduling jitter where Parallels preempts the VirGL
        // GPU thread. Previous single 10ms+spin approach caused cascading
        // DEADBEEF failures on ~25% of sustained sessions.
        let sleep_start: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) sleep_start, options(nomem, nostack));
        }

        let mut completed = false;
        for _attempt in 0..20 {
            // Check if complete (race: device may finish between notify/re-block)
            let done = unsafe {
                let q = &raw const PCI_CTRL_QUEUE;
                let used_addr = &(*q).used as *const _ as usize;
                core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
                core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                fence(Ordering::Acquire);
                read_volatile(&(*q).used.idx) != state.last_used_idx
            };
            if done {
                completed = true;
                break;
            }

            // Re-register for MSI-X wake before each block
            if let Some(tid) = crate::task::scheduler::current_thread_id() {
                GPU_WAITING_THREAD.store(tid, Ordering::Release);
            }

            let (s, n) = crate::time::get_monotonic_time_ns();
            let now_ns = (s as u64) * 1_000_000_000 + (n as u64);
            let wake_ns = now_ns + 10_000_000; // 10ms per attempt

            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_compositor(wake_ns);
            });
            crate::per_cpu_aarch64::preempt_enable();
            crate::task::scheduler::yield_current();
            crate::arch_halt_with_interrupts();

            crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.blocked_in_syscall = false;
                }
            });
            crate::per_cpu_aarch64::preempt_disable();
        }

        GPU_WAITING_THREAD.store(0, Ordering::Release);

        // Suppress interrupts now that we're awake
        unsafe {
            let q = &raw mut PCI_CTRL_QUEUE;
            (*q).avail.flags = 1; // NO_INTERRUPT
            dma_cache_clean(&(*q).avail.flags as *const u16 as *const u8, 64);
        }

        let sleep_end: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) sleep_end, options(nomem, nostack));
        }
        let slept = sleep_end.saturating_sub(sleep_start);
        GPU_SLEEP_TICKS.fetch_add(slept, Ordering::Relaxed);
        GPU_SLEEP_TICKS_PHASES.fetch_add(slept, Ordering::Relaxed);

        if !completed {
            // Final check after all retries
            completed = unsafe {
                let q = &raw const PCI_CTRL_QUEUE;
                let used_addr = &(*q).used as *const _ as usize;
                core::arch::asm!("dc civac, {}", in(reg) used_addr, options(nostack));
                core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                fence(Ordering::Acquire);
                read_volatile(&(*q).used.idx) != state.last_used_idx
            };
        }

        if completed {
            virtgpu::trace_q_complete(0, virtgpu_trace_used_idx());
            used_len = unsafe {
                let q = &raw const PCI_CTRL_QUEUE;
                let elem_idx = (state.last_used_idx % 16) as usize;
                let entry_addr = &(*q).used.ring[elem_idx] as *const _ as usize;
                core::arch::asm!("dc civac, {}", in(reg) entry_addr, options(nostack));
                core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                read_volatile(&(*q).used.ring[elem_idx].len)
            };
            state.last_used_idx = state.last_used_idx.wrapping_add(1);
        } else {
            // Timeout after 200ms. Speculatively advance last_used_idx so the
            // next command's drain doesn't pick up this command's late completion
            // and cascade-fail with DEADBEEF responses.
            state.last_used_idx = state.last_used_idx.wrapping_add(1);
            return Err("GPU PCI 3-desc command timeout (200ms)");
        }
    } else {
        // Polling fallback: spin then block with timer wake.
        let mut timeout = 10_000_000u32;
        let mut spin_count = 0u32;
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
                virtgpu::trace_q_complete(0, used_idx);
                used_len = unsafe {
                    let q = &raw const PCI_CTRL_QUEUE;
                    let elem_idx = (state.last_used_idx % 16) as usize;
                    // Invalidate the specific cache line containing this entry.
                    // Entries at index >= 8 are in a different cache line from
                    // used.idx, so the earlier invalidation doesn't cover them.
                    #[cfg(target_arch = "aarch64")]
                    {
                        let entry_addr = &(*q).used.ring[elem_idx] as *const _ as usize;
                        core::arch::asm!("dc civac, {}", in(reg) entry_addr, options(nostack));
                        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                    }
                    read_volatile(&(*q).used.ring[elem_idx].len)
                };
                state.last_used_idx = used_idx;
                break;
            }
            timeout -= 1;
            if timeout == 0 {
                // Speculatively advance so the next command doesn't cascade-fail
                state.last_used_idx = state.last_used_idx.wrapping_add(1);
                return Err("GPU PCI 3-desc command timeout");
            }
            spin_count += 1;
            if can_yield && spin_count == 5_000 {
                #[cfg(target_arch = "aarch64")]
                {
                    let sleep_start: u64;
                    unsafe {
                        core::arch::asm!("mrs {}, cntvct_el0", out(reg) sleep_start, options(nomem, nostack));
                    }

                    let (s, n) = crate::time::get_monotonic_time_ns();
                    let now_ns = (s as u64) * 1_000_000_000 + (n as u64);
                    let wake_ns = now_ns + 4_000_000; // 4ms timeout

                    crate::task::scheduler::with_scheduler(|sched| {
                        sched.block_current_for_compositor(wake_ns);
                    });
                    crate::per_cpu_aarch64::preempt_enable();
                    crate::task::scheduler::yield_current();
                    crate::arch_halt_with_interrupts();
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                        }
                    });
                    crate::per_cpu_aarch64::preempt_disable();

                    let sleep_end: u64;
                    unsafe {
                        core::arch::asm!("mrs {}, cntvct_el0", out(reg) sleep_end, options(nomem, nostack));
                    }
                    let slept = sleep_end.saturating_sub(sleep_start);
                    GPU_SLEEP_TICKS.fetch_add(slept, Ordering::Relaxed);
                    GPU_SLEEP_TICKS_PHASES.fetch_add(slept, Ordering::Relaxed);
                }
                spin_count = 0;
            } else {
                core::hint::spin_loop();
            }
        }
    }

    // Invalidate response cache
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let r = &raw const PCI_RESP_BUF as usize;
        core::arch::asm!("dc civac, {}", in(reg) r, options(nostack));
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }

    let resp_type = unsafe {
        let p = &raw const PCI_RESP_BUF;
        let h = &*((*p).data.as_ptr() as *const VirtioGpuCtrlHdr);
        core::ptr::read_volatile(&h.type_)
    };

    virtgpu::trace_response(resp_type);
    virtgpu_note_resource2_response(cmd_type, resource_id, resp_type);

    Ok((used_len, resp_type))
}

/// Send a command and verify the response is RESP_OK_NODATA.
fn send_command_expect_ok(state: &mut GpuPciDeviceState, cmd_len: u32) -> Result<(), &'static str> {
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

    let (cmd_type, resource_id) = virtgpu_decode_pci_cmd();
    virtgpu_trace_submission(cmd_type, resource_id);

    // Flush command buffer and response poison from CPU cache to physical RAM.
    // On ARM64, WB-cacheable BSS may not be visible to the hypervisor's DMA
    // without explicit cache maintenance.
    dma_cache_clean(&raw const PCI_CMD_BUF as *const u8, cmd_len as usize);
    dma_cache_clean(
        &raw const PCI_RESP_BUF as *const u8,
        core::mem::size_of::<VirtioGpuCtrlHdr>(),
    );

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
    virtgpu::trace_response(resp_type);
    virtgpu_note_resource2_response(cmd_type, resource_id, resp_type);

    if resp_type != cmd::RESP_OK_NODATA {
        crate::tracing::output::trace_dump_latest(256);
        crate::tracing::output::trace_dump_counters();
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
        dma_cache_clean(
            &raw const PCI_CMD_BUF as *const u8,
            core::mem::size_of::<VirtioGpuCtrlHdr>(),
        );

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
                crate::serial_println!(
                    "[virtio-gpu-pci] GET_DISPLAY_INFO: resp_type={:#x} (expected {:#x})",
                    resp_type,
                    cmd::RESP_OK_DISPLAY_INFO
                );
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
        dma_cache_clean(
            &raw const PCI_CMD_BUF as *const u8,
            core::mem::size_of::<VirtioGpuGetCapsetInfo>(),
        );

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
                crate::serial_println!(
                    "[virtio-gpu-pci] GET_CAPSET_INFO: unexpected resp_type={:#x} (expected {:#x})",
                    resp_type,
                    cmd::RESP_OK_CAPSET_INFO
                );
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

        dma_cache_clean(
            &raw const PCI_CMD_BUF as *const u8,
            core::mem::size_of::<VirtioGpuGetCapset>(),
        );

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
        send_command_expect_ok(state, core::mem::size_of::<VirtioGpuSetScanout>() as u32)
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
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuResourceFlush>() as u32)
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
    crate::serial_println!(
        "[hex-dump] {} ({} DWORDs, {} bytes):",
        label,
        dword_count,
        byte_len
    );
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
    crate::serial_println!(
        "[hex-dump] {} ({} DWORDs, {} bytes):",
        label,
        payload_dwords,
        payload_dwords * 4
    );
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
fn virgl_ctx_create_cmd(
    state: &mut GpuPciDeviceState,
    ctx_id: u32,
    name: &[u8],
) -> Result<(), &'static str> {
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
fn virgl_resource_unref_cmd(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
) -> Result<(), &'static str> {
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCtxResource);
        *cmd = VirtioGpuCtxResource {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::RESOURCE_UNREF,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuCtxResource>() as u32)
}

fn virgl_detach_backing_cmd(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
) -> Result<(), &'static str> {
    unsafe {
        let cmd_ptr = &raw mut PCI_CMD_BUF;
        let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuCtxResource);
        *cmd = VirtioGpuCtxResource {
            hdr: VirtioGpuCtrlHdr {
                type_: cmd::RESOURCE_DETACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<VirtioGpuCtxResource>() as u32)
}

fn virgl_ctx_attach_resource_cmd(
    state: &mut GpuPciDeviceState,
    ctx_id: u32,
    resource_id: u32,
) -> Result<(), &'static str> {
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
    hex_dump_cmd_buf(
        "CTX_ATTACH_RESOURCE",
        core::mem::size_of::<VirtioGpuCtxResource>(),
    );
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
    hex_dump_cmd_buf(
        "RESOURCE_CREATE_3D",
        core::mem::size_of::<VirtioGpuResourceCreate3d>(),
    );
    send_command_expect_ok(
        state,
        core::mem::size_of::<VirtioGpuResourceCreate3d>() as u32,
    )
}

/// Attach backing memory to a 3D resource using per-page scatter-gather entries.
///
/// KEY FIX: Linux kernel sends one VirtioGpuMemEntry per 4KB page (768 entries
/// for a 1024×768×4 framebuffer). The host (Parallels) requires per-page entries
/// to properly map backing for GL texture operations and TRANSFER_FROM_HOST_3D.
/// A single-entry approach causes the host to read zeros → BLACK texture sampling.
fn virgl_attach_backing_paged(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
    backing_ptr: *const u8,
    backing_len: usize,
) -> Result<(), &'static str> {
    assert!(!backing_ptr.is_null(), "backing pointer is null");
    let fb_base_phys = virt_to_phys(backing_ptr as u64);
    let actual_len = backing_len;

    const PAGE_SIZE: usize = 4096;
    let nr_pages = (actual_len + PAGE_SIZE - 1) / PAGE_SIZE;

    crate::serial_println!(
        "[virgl] attach_backing: phys=0x{:x}, len={}, nr_pages={}",
        fb_base_phys,
        actual_len,
        nr_pages
    );

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
        let entries =
            core::slice::from_raw_parts_mut(entries_ptr as *mut VirtioGpuMemEntry, nr_pages);
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

    crate::serial_println!(
        "[virgl] attach_backing: sending 3-desc chain: hdr=0x{:x}({}B), entries=0x{:x}({}B)",
        hdr_phys,
        hdr_size,
        entries_phys,
        entries_size
    );

    let (_used_len, resp_type) = send_command_3desc(
        state,
        hdr_phys,
        hdr_size as u32,
        entries_phys,
        entries_size as u32,
    )?;

    // Free entries array (one-time during init)
    unsafe {
        alloc::alloc::dealloc(entries_ptr, entries_layout);
    }

    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!("[virgl] attach_backing FAILED: resp_type={:#x}", resp_type);
        return Err("attach_backing: device rejected paged backing");
    }

    crate::serial_println!("[virgl] attach_backing: OK ({} pages attached)", nr_pages);
    Ok(())
}

/// Attach backing memory to a resource using a single memory entry.
/// Works for small resources (VB, small textures) where a single contiguous
/// allocation fits in PCI_CMD_BUF alongside the header.
/// Currently unused: VB uses RESOURCE_INLINE_WRITE instead.
#[allow(dead_code)]
fn attach_backing_simple(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
    backing_ptr: *mut u8,
    backing_len: usize,
) -> Result<(), &'static str> {
    assert!(!backing_ptr.is_null(), "backing_ptr is null");
    let phys = virt_to_phys(backing_ptr as u64);
    crate::serial_println!(
        "[virgl] attach_backing_simple: res={}, phys=0x{:x}, len={}",
        resource_id,
        phys,
        backing_len
    );

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
                resource_id,
                nr_entries: 1,
            },
            entry: VirtioGpuMemEntry {
                addr: phys,
                length: backing_len as u32,
                padding: 0,
            },
        };
    }
    send_command_expect_ok(state, core::mem::size_of::<PciAttachBackingCmd>() as u32)?;
    crate::serial_println!("[virgl] attach_backing_simple: OK");
    Ok(())
}

/// Attach backing memory to a 3D resource using explicit per-page physical addresses.
///
/// Unlike `virgl_attach_backing_paged` which computes sequential physical addresses
/// from a contiguous kernel allocation, this takes a list of arbitrary physical page
/// addresses — used for MAP_SHARED window buffers where pages may not be contiguous.
#[allow(dead_code)]
fn virgl_attach_backing_from_pages(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
    page_phys_addrs: &[u64],
    total_len: usize,
) -> Result<(), &'static str> {
    let nr_pages = page_phys_addrs.len();
    if nr_pages == 0 {
        return Err("attach_backing_from_pages: empty page list");
    }

    crate::serial_println!(
        "[virgl] attach_backing_from_pages: res={}, pages={}, len={}",
        resource_id,
        nr_pages,
        total_len
    );

    const PAGE_SIZE: usize = 4096;

    // Heap-allocate the entries array
    let entries_size = nr_pages * core::mem::size_of::<VirtioGpuMemEntry>();
    let entries_layout = alloc::alloc::Layout::from_size_align(entries_size, 64)
        .map_err(|_| "attach_backing_from_pages: invalid entries layout")?;
    let entries_ptr = unsafe { alloc::alloc::alloc_zeroed(entries_layout) };
    if entries_ptr.is_null() {
        return Err("attach_backing_from_pages: failed to allocate entries");
    }

    // Fill each entry with the corresponding page's physical address
    unsafe {
        let entries =
            core::slice::from_raw_parts_mut(entries_ptr as *mut VirtioGpuMemEntry, nr_pages);
        let mut remaining = total_len;
        for (i, &phys) in page_phys_addrs.iter().enumerate() {
            let page_len = if remaining >= PAGE_SIZE {
                PAGE_SIZE
            } else {
                remaining
            };
            entries[i] = VirtioGpuMemEntry {
                addr: phys,
                length: page_len as u32,
                padding: 0,
            };
            remaining = remaining.saturating_sub(PAGE_SIZE);
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

    dma_cache_clean(&raw const PCI_CMD_BUF as *const u8, hdr_size);
    dma_cache_clean(entries_ptr, entries_size);

    let hdr_phys = virt_to_phys(&raw const PCI_CMD_BUF as u64);
    let entries_phys = virt_to_phys(entries_ptr as u64);

    let (_used_len, resp_type) = send_command_3desc(
        state,
        hdr_phys,
        hdr_size as u32,
        entries_phys,
        entries_size as u32,
    )?;

    unsafe {
        alloc::alloc::dealloc(entries_ptr, entries_layout);
    }

    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!(
            "[virgl] attach_backing_from_pages FAILED: resp={:#x}",
            resp_type
        );
        return Err("attach_backing_from_pages: device rejected");
    }

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
    hex_dump_cmd_buf(
        "RESOURCE_FLUSH",
        core::mem::size_of::<VirtioGpuResourceFlush>(),
    );
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
    //
    // EXCEPTION: Skip cache clean for RESOURCE_3D_ID when re-uploading after a
    // TRANSFER_FROM_HOST_3D readback. The DMA readback wrote pixels directly to
    // RAM and we invalidated cache — dma_cache_clean would push stale cached
    // data (from before the readback) back to RAM, overwriting the DMA-written
    // pixels. The render loop (virgl_render_rects/render_frame) always does
    // readback → upload, so the 3D backing data is already in RAM.
    //
    // For the priming case (virgl_init), the caller does dma_cache_clean
    // explicitly before calling transfer_to_host_3d.
    unsafe {
        if resource_id == RESOURCE_3D_ID {
            // Skip: data is already in RAM from DMA readback or explicit clean
        } else if resource_id == RESOURCE_TEX_ID {
            let tex = &raw const TEST_TEX_BUF;
            let backing_ptr = (*tex).pixels.as_ptr();
            let backing_len = TEST_TEX_BYTES;
            dma_cache_clean(backing_ptr, backing_len);
        } else if resource_id == RESOURCE_VB_ID {
            let vb_ptr = PCI_VB_PTR;
            assert!(!vb_ptr.is_null(), "VB backing not initialized");
            // For BUFFER resources: x=byte offset, w=byte width, h=1, stride=0
            let backing_len = w as usize;
            dma_cache_clean(vb_ptr.add(x as usize), backing_len);
        }
    }

    hex_dump_cmd_buf(
        "TRANSFER_TO_HOST_3D",
        core::mem::size_of::<VirtioGpuTransferHost3d>(),
    );
    send_command_expect_ok(
        state,
        core::mem::size_of::<VirtioGpuTransferHost3d>() as u32,
    )
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
    send_command_expect_ok(
        state,
        core::mem::size_of::<VirtioGpuTransferHost3d>() as u32,
    )?;

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
/// Result of a VirGL 3D command submission.
struct SubmitResult {
    /// The fence ID we requested
    submit_id: u64,
    /// The fence ID the device reported as completed in the response
    resp_fence: u64,
}

fn virgl_submit_3d_cmd(
    state: &mut GpuPciDeviceState,
    ctx_id: u32,
    cmds: &[u32],
) -> Result<SubmitResult, &'static str> {
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

    // 3-descriptor chain: header (readable) + payload (readable) + response (writable)
    let (used_len, resp_type) = send_command_3desc(
        state,
        hdr_phys,
        hdr_size as u32,
        payload_phys,
        payload_bytes as u32,
    )?;

    // Read response flags and fence_id
    let (resp_flags, resp_fence) = unsafe {
        let p = &raw const PCI_RESP_BUF;
        let h = &*((*p).data.as_ptr() as *const VirtioGpuCtrlHdr);
        (
            core::ptr::read_volatile(&h.flags),
            core::ptr::read_volatile(&h.fence_id),
        )
    };

    if resp_type != cmd::RESP_OK_NODATA {
        crate::serial_println!(
            "[virtio-gpu-pci] SUBMIT_3D failed: resp={:#x} used_len={} flags={:#x} fence={}",
            resp_type,
            used_len,
            resp_flags,
            resp_fence
        );
        return Err("SUBMIT_3D command failed");
    }
    if submit_id <= 5 {
        crate::serial_println!(
            "[virgl] SUBMIT_3D OK: id={} used_len={} resp_flags={:#x} resp_fence={}",
            submit_id,
            used_len,
            resp_flags,
            resp_fence
        );
    }
    Ok(SubmitResult {
        submit_id,
        resp_fence,
    })
}

/// Wait for the host to confirm a GPU fence has completed.
///
/// Sends NOP SUBMIT_3D commands with fences and polls until the response
/// fence_id >= target_fence_id. Uses WFI between polls to sleep until the
/// next interrupt rather than spinning, dramatically reducing CPU waste.
fn virgl_fence_sync(
    state: &mut GpuPciDeviceState,
    target_fence_id: u64,
) -> Result<(), &'static str> {
    use super::virgl::CommandBuffer;

    // Heap-allocate once outside the loop to avoid 12KB stack allocation per iteration
    let mut cmdbuf = alloc::boxed::Box::new(CommandBuffer::new());

    for round in 0..100u32 {
        // Sleep until next interrupt before polling (skip on first round).
        // The GPU completion or timer interrupt (1000Hz) will wake us.
        if round > 0 {
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }

        // Build a NOP VirGL command (set_sub_ctx is minimal)
        cmdbuf.clear();
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

        let (_used_len, resp_type) = send_command_3desc(
            state,
            hdr_phys,
            hdr_size as u32,
            payload_phys,
            payload_bytes as u32,
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
                crate::serial_println!(
                    "[virgl] fence_sync: completed after {} polls (target={}, got={})",
                    round + 1,
                    target_fence_id,
                    resp_fence
                );
            }
            return Ok(());
        }
    }

    Err("fence sync: target fence never completed after 100 polls")
}

/// Set scanout to a specific resource ID (used for 3D render targets).
fn set_scanout_resource(
    state: &mut GpuPciDeviceState,
    resource_id: u32,
) -> Result<(), &'static str> {
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

/// Get compositor texture info for MAP_SHARED into userspace.
/// Returns (phys_base, num_pages, width, height) if ready.
pub fn compositor_texture_info() -> Option<(u64, u32, u32, u32)> {
    if !COMPOSITE_TEX_READY.load(Ordering::Acquire) {
        return None;
    }
    let phys = COMPOSITE_TEX_PHYS_BASE.load(Ordering::Acquire);
    let pages = COMPOSITE_TEX_NUM_PAGES.load(Ordering::Acquire);
    let w = COMPOSITE_TEX_W.load(Ordering::Acquire);
    let h = COMPOSITE_TEX_H.load(Ordering::Acquire);
    if phys == 0 || pages == 0 {
        return None;
    }
    Some((phys, pages, w, h))
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

/// Rectangle descriptor passed from userspace for GPU rendering.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirglRect {
    /// X position in pixels (top-left)
    pub x: f32,
    /// Y position in pixels (top-left)
    pub y: f32,
    /// Width in pixels
    pub w: f32,
    /// Height in pixels
    pub h: f32,
    /// Color as [R, G, B, A] each 0.0-1.0
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
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
    use super::virgl::{pipe, CommandBuffer};

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
        crate::serial_println!(
            "[virgl] WARNING: scanout not active at frame #{} — setting now",
            frame
        );
        with_device_state(|state| set_scanout_resource(state, RESOURCE_3D_ID)).ok();
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
    cmdbuf.set_tweaks(1, 1);
    cmdbuf.set_tweaks(2, width);

    // Re-emit ALL pipeline state each frame, matching the working init batch.
    cmdbuf.bind_shader(1, pipe::SHADER_VERTEX);
    cmdbuf.bind_shader(2, pipe::SHADER_FRAGMENT);
    cmdbuf.bind_object(1, super::virgl::OBJ_BLEND);
    cmdbuf.bind_object(1, super::virgl::OBJ_DSA);
    cmdbuf.bind_object(1, super::virgl::OBJ_RASTERIZER);
    cmdbuf.bind_object(1, super::virgl::OBJ_VERTEX_ELEMENTS);
    cmdbuf.set_min_samples(1);
    cmdbuf.set_viewport(fw, fh);
    cmdbuf.set_framebuffer_state(0, &[1]); // surface_handle=1, no depth

    // Clear full screen with background color (no scissor — matches init batch)
    cmdbuf.clear_color(bg_r, bg_g, bg_b, 1.0);

    // For each ball, generate a triangle fan and draw it
    let ball_count = balls.len().min(MAX_CIRCLES);
    if verbose {
        crate::serial_println!(
            "[virgl] frame #{}: drawing {} balls (with full state re-emit)",
            frame,
            ball_count
        );
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
            1.0, 0.92388, 0.70711, 0.38268, 0.0, -0.38268, -0.70711, -0.92388, -1.0, -0.92388,
            -0.70711, -0.38268, 0.0, 0.38268, 0.70711, 0.92388, 1.0, // closing = first
        ];
        const SIN_TABLE: [f32; 17] = [
            0.0, 0.38268, 0.70711, 0.92388, 1.0, 0.92388, 0.70711, 0.38268, 0.0, -0.38268,
            -0.70711, -0.92388, -1.0, -0.92388, -0.70711, -0.38268, 0.0, // closing = first
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
            0,                       // start = 0 (relative to VB offset)
            VERTS_PER_CIRCLE as u32, // count
            pipe::PRIM_TRIANGLE_FAN,
            (VERTS_PER_CIRCLE - 1) as u32, // max_index
        );
    }

    // Submit VirGL commands to host GPU
    if verbose {
        crate::serial_println!(
            "[virgl] frame #{}: submitting {} DWORDs ({} bytes)",
            frame,
            cmdbuf.as_slice().len(),
            cmdbuf.byte_len()
        );
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

    // Linux-style display: SUBMIT_3D → RESOURCE_FLUSH (no transfers).
    with_device_state(|state| resource_flush_3d(state, RESOURCE_3D_ID))?;

    Ok(())
}

/// Render colored rectangles via VirGL DRAW_VBO + CPU-side compositing.
///
/// Submits VirGL commands (CLEAR + DRAW_VBO for each rect) to the GPU,
/// then composites CPU-side for display until VirGL readback is debugged.
pub fn virgl_render_rects(
    rects: &[VirglRect],
    bg_r: f32,
    bg_g: f32,
    bg_b: f32,
) -> Result<(), &'static str> {
    use super::virgl::{format as vfmt, pipe, CommandBuffer};

    static RECT_FRAME_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let frame = RECT_FRAME_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let verbose = frame < 3 || frame % 500 == 0;
    if verbose {
        crate::serial_println!("[virgl] render_rects #{} ({} rects)", frame, rects.len());
    }

    if !is_virgl_enabled() {
        return Err("VirGL not enabled");
    }

    if !VIRGL_SCANOUT_ACTIVE.load(Ordering::Acquire) {
        with_device_state(|state| set_scanout_resource(state, RESOURCE_3D_ID)).ok();
        VIRGL_SCANOUT_ACTIVE.store(true, Ordering::Release);
    }

    let (width, height) = match dimensions() {
        Some(d) => d,
        None => return Err("GPU not initialized"),
    };
    let fw = width as f32;
    let fh = height as f32;

    let mut cmdbuf = CommandBuffer::new();

    // CRITICAL: create_sub_ctx is REQUIRED per SUBMIT_3D batch on Parallels.
    // set_sub_ctx alone does not re-activate the GL context for rendering.
    // create_sub_ctx resets all state — surface, shaders, blend, DSA, rasterizer,
    // vertex elements must ALL be re-created in every batch.
    cmdbuf.create_sub_ctx(1);
    cmdbuf.set_sub_ctx(1);
    cmdbuf.set_tweaks(1, 1);
    cmdbuf.set_tweaks(2, width);

    // Surface wrapping the render target
    cmdbuf.create_surface(1, RESOURCE_3D_ID, vfmt::B8G8R8X8_UNORM, 0, 0);
    cmdbuf.set_framebuffer_state(0, &[1]);

    // Pipeline state objects
    cmdbuf.create_blend_simple(1);
    cmdbuf.bind_object(1, super::virgl::OBJ_BLEND);
    cmdbuf.create_dsa_default(1);
    cmdbuf.bind_object(1, super::virgl::OBJ_DSA);
    cmdbuf.create_rasterizer_default(1);
    cmdbuf.bind_object(1, super::virgl::OBJ_RASTERIZER);

    // Shaders (num_tokens=300 required by Parallels)
    let vs_text = b"VERT\nDCL IN[0]\nDCL OUT[0], POSITION\n  0: MOV OUT[0], IN[0]\n  1: END\n";
    cmdbuf.create_shader(1, pipe::SHADER_VERTEX, 300, vs_text);
    cmdbuf.bind_shader(1, pipe::SHADER_VERTEX);
    let fs_text = b"FRAG\nPROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\nDCL OUT[0], COLOR\nDCL CONST[0]\n  0: MOV OUT[0], CONST[0]\n  1: END\n";
    cmdbuf.create_shader(2, pipe::SHADER_FRAGMENT, 300, fs_text);
    cmdbuf.bind_shader(2, pipe::SHADER_FRAGMENT);

    // Vertex elements
    cmdbuf.create_vertex_elements(1, &[(0, 0, 0, vfmt::R32G32B32A32_FLOAT)]);
    cmdbuf.bind_object(1, super::virgl::OBJ_VERTEX_ELEMENTS);

    cmdbuf.set_min_samples(1);
    cmdbuf.set_viewport(fw, fh);

    // Clear full screen with background color (no scissor — matches init batch)
    cmdbuf.clear_color(bg_r, bg_g, bg_b, 1.0);

    // Draw each rectangle as a quad (triangle fan, 4 verts, 16 bytes/vert)
    // Limit to ~80 rects to stay within 3072 DW cmdbuf capacity.
    // Per rect: inline_write(~20 DW) + set_constant_buffer(~8 DW) +
    //           set_vertex_buffers(~6 DW) + draw_vbo(~13 DW) = ~47 DW
    // Budget: (3072 - ~100 overhead) / 47 ≈ 63 rects max
    let rect_count = rects.len().min(60);

    for (i, rect) in rects[..rect_count].iter().enumerate() {
        // Convert pixel coords to NDC
        let x0 = rect.x;
        let y0 = rect.y;
        let x1 = rect.x + rect.w;
        let y1 = rect.y + rect.h;

        let x0_ndc = 2.0 * x0 / fw - 1.0;
        let y0_ndc = 1.0 - 2.0 * y0 / fh; // top
        let x1_ndc = 2.0 * x1 / fw - 1.0;
        let y1_ndc = 1.0 - 2.0 * y1 / fh; // bottom

        // 4 vertices: TL, BL, BR, TR (triangle fan)
        let quad_verts: [u32; 16] = [
            x0_ndc.to_bits(),
            y0_ndc.to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(), // TL
            x0_ndc.to_bits(),
            y1_ndc.to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(), // BL
            x1_ndc.to_bits(),
            y1_ndc.to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(), // BR
            x1_ndc.to_bits(),
            y0_ndc.to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(), // TR
        ];

        let vb_offset = (i * 64) as u32; // 4 verts × 16 bytes each
        cmdbuf.resource_inline_write(RESOURCE_VB_ID, vb_offset, 64, &quad_verts);

        // Set rect color via constant buffer
        cmdbuf.set_constant_buffer(
            pipe::SHADER_FRAGMENT,
            0,
            &[
                rect.r.to_bits(),
                rect.g.to_bits(),
                rect.b.to_bits(),
                rect.a.to_bits(),
            ],
        );

        // Bind vertex buffer at this rect's offset and draw
        cmdbuf.set_vertex_buffers(&[(16, vb_offset, RESOURCE_VB_ID)]);
        cmdbuf.draw_vbo(0, 4, pipe::PRIM_TRIANGLE_FAN, 3);
    }

    // Submit VirGL commands
    if verbose {
        crate::serial_println!(
            "[virgl] render_rects #{}: submitting {} DWORDs",
            frame,
            cmdbuf.as_slice().len()
        );
    }
    virgl_submit_sync(cmdbuf.as_slice())?;

    // Linux per-frame display pattern: SUBMIT_3D → SET_SCANOUT → RESOURCE_FLUSH.
    // Linux ftrace proves SET_SCANOUT is issued every frame, not just once.
    with_device_state(|state| {
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        resource_flush_3d(state, RESOURCE_3D_ID)
    })?;

    Ok(())
}

/// Composite a CPU-rendered pixel buffer as a full-screen GPU texture.
///
/// Takes a BGRA pixel buffer (u32 per pixel), uploads it to the compositor
/// texture via TRANSFER_TO_HOST_3D, then renders it as a full-screen textured
/// quad via VirGL DRAW_VBO. This is the foundation for GPU-composited window
/// management — BWM renders each window to a pixel buffer, then composites
/// them all through the GPU.
///
/// # Arguments
/// * `pixels` - BGRA pixel data (width × height u32 values)
/// * `width` - Width of the pixel buffer in pixels
/// * `height` - Height of the pixel buffer in pixels
pub fn virgl_composite_frame(pixels: &[u32], width: u32, height: u32) -> Result<(), &'static str> {
    static COMPOSITE_FRAME_COUNT: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let frame = COMPOSITE_FRAME_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let verbose = frame < 3 || frame % 500 == 0;

    if !is_virgl_enabled() {
        return Err("VirGL not enabled");
    }

    let (display_w, display_h) = dimensions().ok_or("GPU not initialized")?;

    // Clamp source dimensions to display
    let copy_w = width.min(display_w);
    let copy_h = height.min(display_h);
    let expected_pixels = (copy_w * copy_h) as usize;
    if pixels.len() < expected_pixels {
        return Err("Pixel buffer too small");
    }

    if verbose {
        let mut first_nonzero = 0u32;
        let mut nonzero_count = 0u32;
        for i in 0..expected_pixels.min(1000) {
            if pixels[i] != 0 {
                if first_nonzero == 0 {
                    first_nonzero = pixels[i];
                }
                nonzero_count += 1;
            }
        }
        crate::serial_println!(
            "[virgl-composite] Frame #{}: {}x{} → {}x{} display (first1k: {} nonzero, first={:#010x})",
            frame, copy_w, copy_h, display_w, display_h, nonzero_count, first_nonzero
        );
    }

    // Direct blit: copy pixel data into the 3D framebuffer backing, then upload.
    // No VirGL SUBMIT_3D needed — just TRANSFER_TO_HOST_3D + SET_SCANOUT + RESOURCE_FLUSH.
    let fb_ptr = unsafe { PCI_3D_FB_PTR };
    if fb_ptr.is_null() {
        return Err("3D framebuffer not initialized");
    }
    let fb_len = unsafe { PCI_3D_FB_LEN };
    let dst_stride = display_w as usize * 4;
    let src_stride = width as usize * 4;

    unsafe {
        if copy_w == display_w {
            let copy_bytes = (copy_w as usize * copy_h as usize * 4).min(fb_len);
            core::ptr::copy_nonoverlapping(pixels.as_ptr() as *const u8, fb_ptr, copy_bytes);
        } else {
            for y in 0..copy_h as usize {
                let src_off = y * src_stride;
                let dst_off = y * dst_stride;
                let row_bytes = (copy_w as usize) * 4;
                if dst_off + row_bytes <= fb_len {
                    core::ptr::copy_nonoverlapping(
                        (pixels.as_ptr() as *const u8).add(src_off),
                        fb_ptr.add(dst_off),
                        row_bytes,
                    );
                }
            }
        }
    }

    // Cache clean the 3D FB backing before DMA upload
    let upload_bytes = (copy_w as usize * copy_h as usize * 4).min(fb_len);
    dma_cache_clean(fb_ptr, upload_bytes);

    // Upload to host via TRANSFER_TO_HOST_3D on the 3D resource
    with_device_state(|state| {
        // Need to send the transfer command manually since transfer_to_host_3d
        // skips cache clean for RESOURCE_3D_ID (we already did it above)
        let offset = 0u64;
        unsafe {
            let cmd_ptr = &raw mut PCI_CMD_BUF;
            let cmd = &mut *((*cmd_ptr).data.as_mut_ptr() as *mut VirtioGpuTransferHost3d);
            *cmd = VirtioGpuTransferHost3d {
                hdr: VirtioGpuCtrlHdr {
                    type_: cmd::TRANSFER_TO_HOST_3D,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: VIRGL_CTX_ID,
                    padding: 0,
                },
                box_x: 0,
                box_y: 0,
                box_z: 0,
                box_w: copy_w,
                box_h: copy_h,
                box_d: 1,
                offset,
                resource_id: RESOURCE_3D_ID,
                level: 0,
                stride: display_w * 4,
                layer_stride: 0,
            };
        }
        send_command_expect_ok(
            state,
            core::mem::size_of::<VirtioGpuTransferHost3d>() as u32,
        )
    })?;

    // SET_SCANOUT + RESOURCE_FLUSH
    with_device_state(|state| {
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        resource_flush_3d(state, RESOURCE_3D_ID)
    })?;

    Ok(())
}

/// GPU-composited frame via textured quad rendering.
///
/// Uploads pixel data as a VirGL texture, then renders a full-screen textured quad.
/// This is the end-goal approach for real GPU compositing (window decorations,
/// alpha blending, transforms). Currently not working on Parallels — texture
/// sampling produces black output despite successful SUBMIT_3D. Kept for future
/// debugging and enablement.
pub fn virgl_composite_frame_textured(
    pixels: &[u32],
    width: u32,
    height: u32,
) -> Result<(), &'static str> {
    use super::virgl::{format as vfmt, pipe, swizzle, CommandBuffer};

    static TEX_FRAME_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let frame = TEX_FRAME_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    if !is_virgl_enabled() {
        return Err("VirGL not enabled");
    }
    if !COMPOSITE_TEX_READY.load(Ordering::Acquire) {
        return Err("Compositor texture not initialized");
    }

    let tex_w = COMPOSITE_TEX_W.load(Ordering::Relaxed);
    let tex_h = COMPOSITE_TEX_H.load(Ordering::Relaxed);
    let (display_w, display_h) = dimensions().ok_or("GPU not initialized")?;
    let copy_w = width.min(tex_w);
    let copy_h = height.min(tex_h);
    let expected_pixels = (copy_w * copy_h) as usize;
    if pixels.len() < expected_pixels {
        return Err("Pixel buffer too small");
    }

    // Copy pixel data into the compositor texture backing
    unsafe {
        let dst = COMPOSITE_TEX_PTR;
        let copy_bytes = (copy_w as usize) * (copy_h as usize) * 4;
        core::ptr::copy_nonoverlapping(
            pixels.as_ptr() as *const u8,
            dst,
            copy_bytes.min(COMPOSITE_TEX_LEN),
        );
    }

    // Upload texture via TRANSFER_TO_HOST_3D
    let tex_bytes = (tex_w as usize) * (tex_h as usize) * 4;
    dma_cache_clean(unsafe { COMPOSITE_TEX_PTR }, tex_bytes);
    with_device_state(|state| {
        transfer_to_host_3d(
            state,
            RESOURCE_COMPOSITE_TEX_ID,
            0,
            0,
            copy_w,
            copy_h,
            tex_w * 4,
        )
    })?;

    // Build VirGL batch: full pipeline + textured quad
    // Object handles are unique per batch to avoid hash collisions in virglrenderer:
    //   10=surface, 11=blend, 12=DSA, 13=rasterizer, 14=VS, 15=FS,
    //   16=VE, 17=sampler_view, 18=sampler_state
    let mut cmdbuf = CommandBuffer::new();
    cmdbuf.create_sub_ctx(1);
    cmdbuf.set_sub_ctx(1);
    cmdbuf.set_tweaks(1, 1);
    cmdbuf.set_tweaks(2, display_w);

    cmdbuf.create_surface(10, RESOURCE_3D_ID, vfmt::B8G8R8X8_UNORM, 0, 0);
    cmdbuf.set_framebuffer_state(0, &[10]);
    cmdbuf.create_blend_simple(11);
    cmdbuf.bind_object(11, super::virgl::OBJ_BLEND);
    cmdbuf.create_dsa_default(12);
    cmdbuf.bind_object(12, super::virgl::OBJ_DSA);
    cmdbuf.create_rasterizer_default(13);
    cmdbuf.bind_object(13, super::virgl::OBJ_RASTERIZER);

    // Texture shaders (num_tokens=300 required by Parallels)
    let tex_vs = b"VERT\nDCL IN[0]\nDCL IN[1]\nDCL OUT[0], POSITION\nDCL OUT[1], GENERIC[0]\n  0: MOV OUT[0], IN[0]\n  1: MOV OUT[1], IN[1]\n  2: END\n";
    cmdbuf.create_shader(14, pipe::SHADER_VERTEX, 300, tex_vs);
    cmdbuf.bind_shader(14, pipe::SHADER_VERTEX);
    let tex_fs = b"FRAG\nPROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\nDCL IN[0], GENERIC[0], LINEAR\nDCL OUT[0], COLOR\nDCL SAMP[0]\nDCL SVIEW[0], 2D, FLOAT\n  0: TEX OUT[0], IN[0], SAMP[0], 2D\n  1: END\n";
    cmdbuf.create_shader(15, pipe::SHADER_FRAGMENT, 300, tex_fs);
    cmdbuf.bind_shader(15, pipe::SHADER_FRAGMENT);

    cmdbuf.create_vertex_elements(
        16,
        &[
            (0, 0, 0, vfmt::R32G32B32A32_FLOAT),
            (16, 0, 0, vfmt::R32G32B32A32_FLOAT),
        ],
    );
    cmdbuf.bind_object(16, super::virgl::OBJ_VERTEX_ELEMENTS);

    // Sampler view: format DWORD must include texture target in bits [24:31]
    cmdbuf.create_sampler_view(
        17,
        RESOURCE_COMPOSITE_TEX_ID,
        vfmt::B8G8R8X8_UNORM,
        pipe::TEXTURE_2D,
        0,
        0,
        0,
        0,
        swizzle::IDENTITY,
    );
    cmdbuf.create_sampler_state(
        18,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_FILTER_NEAREST,
        pipe::TEX_MIPFILTER_NONE,
        pipe::TEX_FILTER_NEAREST,
    );
    cmdbuf.set_min_samples(1);
    cmdbuf.set_viewport(display_w as f32, display_h as f32);
    cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[17]);
    cmdbuf.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[18]);
    cmdbuf.clear_color(0.0, 0.0, 0.0, 1.0);

    let u_max = copy_w as f32 / tex_w as f32;
    let v_max = copy_h as f32 / tex_h as f32;
    let quad_verts: [u32; 32] = [
        (-1.0f32).to_bits(),
        (1.0f32).to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        (-1.0f32).to_bits(),
        (-1.0f32).to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        v_max.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        (-1.0f32).to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        u_max.to_bits(),
        v_max.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        (1.0f32).to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        u_max.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
    ];
    cmdbuf.resource_inline_write(RESOURCE_VB_ID, 0, 128, &quad_verts);
    cmdbuf.set_vertex_buffers(&[(32, 0, RESOURCE_VB_ID)]);
    cmdbuf.draw_vbo(0, 4, pipe::PRIM_TRIANGLE_FAN, 3);

    if frame < 3 {
        crate::serial_println!(
            "[virgl-composite-tex] Frame #{}: {} DWORDs",
            frame,
            cmdbuf.as_slice().len()
        );
    }
    virgl_submit_sync(cmdbuf.as_slice())?;

    with_device_state(|state| {
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        resource_flush_3d(state, RESOURCE_3D_ID)
    })?;

    Ok(())
}

/// Render the composited frame: fullscreen background from COMPOSITE_TEX,
/// then per-window content quads from individual GPU textures.
///
/// COMPOSITE_TEX contains background, window frames/decorations.
/// Per-window textures contain the actual window content (pixels from clients).
/// The cursor is rendered as a GPU quad from a dedicated cursor texture (last draw).
/// When a window has no GPU texture, its content was already blitted into
/// COMPOSITE_TEX by BWM, so the background quad covers it.
fn virgl_composite_single_quad(
    windows: &[crate::syscall::graphics::WindowCompositeInfo],
) -> Result<(), &'static str> {
    use super::virgl::{format as vfmt, pipe, swizzle, CommandBuffer};

    // BWM frame decoration constants (must match bwm.rs)
    const TITLE_BAR_HEIGHT: i32 = 32;
    const BORDER_WIDTH: i32 = 2;

    // ── Build canary — detect stale binary deployment ──
    static FRAME_COUNT: AtomicU32 = AtomicU32::new(0);
    let frame = FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    if frame == 0 {
        crate::serial_println!("[BUILD-CANARY] gpu_pci.rs version=10 dimmer-fix");
    }

    let tex_w = COMPOSITE_TEX_W.load(Ordering::Relaxed);
    let tex_h = COMPOSITE_TEX_H.load(Ordering::Relaxed);
    let (display_w, display_h) = dimensions().ok_or("GPU not initialized")?;
    let dw = display_w as f32;
    let dh = display_h as f32;

    // Heap-allocate CommandBuffer (12KB) to avoid overflowing the 16KB kernel stack.
    let mut cmdbuf = alloc::boxed::Box::new(CommandBuffer::new());
    cmdbuf.create_sub_ctx(1);
    cmdbuf.set_sub_ctx(1);
    cmdbuf.set_tweaks(1, 1);
    cmdbuf.set_tweaks(2, display_w);

    cmdbuf.create_surface(10, RESOURCE_3D_ID, vfmt::B8G8R8X8_UNORM, 0, 0);
    cmdbuf.set_framebuffer_state(0, &[10]);
    cmdbuf.create_blend_simple(11);
    cmdbuf.bind_object(11, super::virgl::OBJ_BLEND);
    cmdbuf.create_blend_alpha(19); // alpha blend for chromeless windows + cursor
    cmdbuf.create_dsa_default(12);
    cmdbuf.bind_object(12, super::virgl::OBJ_DSA);
    cmdbuf.create_rasterizer_default(13);
    cmdbuf.bind_object(13, super::virgl::OBJ_RASTERIZER);

    let tex_vs = b"VERT\nDCL IN[0]\nDCL IN[1]\nDCL OUT[0], POSITION\nDCL OUT[1], GENERIC[0]\n  0: MOV OUT[0], IN[0]\n  1: MOV OUT[1], IN[1]\n  2: END\n";
    cmdbuf.create_shader(14, pipe::SHADER_VERTEX, 300, tex_vs);
    cmdbuf.bind_shader(14, pipe::SHADER_VERTEX);
    let tex_fs = b"FRAG\nPROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\nDCL IN[0], GENERIC[0], LINEAR\nDCL OUT[0], COLOR\nDCL SAMP[0]\nDCL SVIEW[0], 2D, FLOAT\n  0: TEX OUT[0], IN[0], SAMP[0], 2D\n  1: END\n";
    cmdbuf.create_shader(15, pipe::SHADER_FRAGMENT, 300, tex_fs);
    cmdbuf.bind_shader(15, pipe::SHADER_FRAGMENT);

    cmdbuf.create_vertex_elements(
        16,
        &[
            (0, 0, 0, vfmt::R32G32B32A32_FLOAT),
            (16, 0, 0, vfmt::R32G32B32A32_FLOAT),
        ],
    );
    cmdbuf.bind_object(16, super::virgl::OBJ_VERTEX_ELEMENTS);

    cmdbuf.create_sampler_state(
        18,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_WRAP_CLAMP_TO_EDGE,
        pipe::TEX_FILTER_NEAREST,
        pipe::TEX_MIPFILTER_NONE,
        pipe::TEX_FILTER_NEAREST,
    );
    cmdbuf.bind_sampler_states(pipe::SHADER_FRAGMENT, 0, &[18]);
    cmdbuf.set_min_samples(1);
    cmdbuf.set_viewport(display_w as f32, display_h as f32);

    // ── Create sampler views for all textures upfront ──
    // Handle 17: COMPOSITE_TEX (background + frames + decorations)
    cmdbuf.create_sampler_view(
        17,
        RESOURCE_COMPOSITE_TEX_ID,
        vfmt::B8G8R8X8_UNORM,
        pipe::TEXTURE_2D,
        0,
        0,
        0,
        0,
        swizzle::IDENTITY,
    );
    // Handles 40+i: per-window textures (B8G8R8X8 — no alpha)
    for (i, win) in windows.iter().enumerate() {
        if !win.virgl_initialized || win.virgl_resource_id == 0 {
            continue;
        }
        let sv_handle = 40 + i as u32;
        cmdbuf.create_sampler_view(
            sv_handle,
            win.virgl_resource_id,
            vfmt::B8G8R8X8_UNORM,
            pipe::TEXTURE_2D,
            0,
            0,
            0,
            0,
            swizzle::IDENTITY,
        );
    }

    let u_max = (tex_w.min(display_w) as f32) / (tex_w as f32);
    let v_max = (tex_h.min(display_h) as f32) / (tex_h as f32);

    // Helper: build a textured quad's 4 vertices (TRIANGLE_FAN) from pixel coords + UV
    let make_quad =
        |px0: f32, py0: f32, px1: f32, py1: f32, u0: f32, v0: f32, u1: f32, v1: f32| -> [u32; 32] {
            let nx0 = px0 / dw * 2.0 - 1.0;
            let ny0 = 1.0 - py0 / dh * 2.0;
            let nx1 = px1 / dw * 2.0 - 1.0;
            let ny1 = 1.0 - py1 / dh * 2.0;
            [
                nx0.to_bits(),
                ny0.to_bits(),
                0f32.to_bits(),
                1.0f32.to_bits(),
                u0.to_bits(),
                v0.to_bits(),
                0f32.to_bits(),
                0f32.to_bits(),
                nx0.to_bits(),
                ny1.to_bits(),
                0f32.to_bits(),
                1.0f32.to_bits(),
                u0.to_bits(),
                v1.to_bits(),
                0f32.to_bits(),
                0f32.to_bits(),
                nx1.to_bits(),
                ny1.to_bits(),
                0f32.to_bits(),
                1.0f32.to_bits(),
                u1.to_bits(),
                v1.to_bits(),
                0f32.to_bits(),
                0f32.to_bits(),
                nx1.to_bits(),
                ny0.to_bits(),
                0f32.to_bits(),
                1.0f32.to_bits(),
                u1.to_bits(),
                v0.to_bits(),
                0f32.to_bits(),
                0f32.to_bits(),
            ]
        };

    // Helper: emit a textured quad draw (inline write + draw_vbo)
    let mut vb_offset: u32 = 0;
    let mut draw_idx: u32 = 0;
    let emit_quad =
        |cmdbuf: &mut CommandBuffer, verts: &[u32; 32], vb_off: &mut u32, di: &mut u32| {
            cmdbuf.resource_inline_write(RESOURCE_VB_ID, *vb_off, 128, verts);
            cmdbuf.set_vertex_buffers(&[(32, 0, RESOURCE_VB_ID)]);
            cmdbuf.draw_vbo(*di, 4, pipe::PRIM_TRIANGLE_FAN, 3);
            *vb_off += 128;
            *di += 4;
        };

    // ── Draw 0: Fullscreen background quad from COMPOSITE_TEX ──
    // Contains background, window frames/decorations, taskbar, appbar.
    cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[17]);
    let bg_verts = make_quad(0.0, 0.0, dw, dh, 0.0, 0.0, u_max, v_max);
    emit_quad(&mut *cmdbuf, &bg_verts, &mut vb_offset, &mut draw_idx);

    // ── Per-window interleaved draws (back to front for correct z-order) ──
    // Draw order: non-chromeless windows → dimmer overlay → chromeless windows.
    // The dimmer dims the desktop (background + regular windows) under overlays.
    let tw = tex_w as f32;
    let th = tex_h as f32;
    let has_chromeless = windows.iter().any(|w| w.chromeless && w.virgl_initialized);

    // Pass 1: Non-chromeless windows (content + frame strips)
    for (i, win) in windows.iter().enumerate() {
        if !win.virgl_initialized || win.virgl_resource_id == 0 {
            continue;
        }
        if win.chromeless {
            continue;
        } // drawn in pass 2

        let cx = win.x as f32;
        let cy = win.y as f32;
        let cw = win.width as f32;
        let ch = win.height as f32;

        let fx0 = cx - BORDER_WIDTH as f32;
        let fy0 = cy - TITLE_BAR_HEIGHT as f32 - BORDER_WIDTH as f32;
        let fx1 = cx + cw + BORDER_WIDTH as f32;
        let fy1 = cy + ch + BORDER_WIDTH as f32;

        // Content quad from per-window texture
        let slot = (win.virgl_resource_id as usize).saturating_sub(RESOURCE_WIN_TEX_BASE as usize);
        let (tex_alloc_w, tex_alloc_h) = if slot < MAX_WIN_TEX_SLOTS {
            unsafe { WIN_TEX_DIMS[slot] }
        } else {
            (win.width, win.height)
        };
        let wu = win.width as f32 / tex_alloc_w as f32;
        let wv = win.height as f32 / tex_alloc_h as f32;

        let sv_handle = 40 + i as u32;
        cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[sv_handle]);
        let content_verts = make_quad(cx, cy, cx + cw, cy + ch, 0.0, 0.0, wu, wv);
        emit_quad(&mut *cmdbuf, &content_verts, &mut vb_offset, &mut draw_idx);

        // Frame strips from COMPOSITE_TEX
        cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[17]);

        // Title bar: full width of frame, from frame top to content top
        if fy0 < cy {
            let tu0 = fx0.max(0.0) / tw;
            let tv0 = fy0.max(0.0) / th;
            let tu1 = fx1.min(dw) / tw;
            let tv1 = cy.min(dh) / th;
            let title_verts = make_quad(
                fx0.max(0.0),
                fy0.max(0.0),
                fx1.min(dw),
                cy.min(dh),
                tu0,
                tv0,
                tu1,
                tv1,
            );
            emit_quad(&mut *cmdbuf, &title_verts, &mut vb_offset, &mut draw_idx);
        }
        // Left border: from content top to frame bottom
        if fx0 < cx {
            let lu0 = fx0.max(0.0) / tw;
            let lv0 = cy.max(0.0) / th;
            let lu1 = cx / tw;
            let lv1 = fy1.min(dh) / th;
            let left_verts = make_quad(
                fx0.max(0.0),
                cy.max(0.0),
                cx,
                fy1.min(dh),
                lu0,
                lv0,
                lu1,
                lv1,
            );
            emit_quad(&mut *cmdbuf, &left_verts, &mut vb_offset, &mut draw_idx);
        }
        // Right border: from content top to frame bottom
        if fx1 > cx + cw {
            let ru0 = (cx + cw) / tw;
            let rv0 = cy.max(0.0) / th;
            let ru1 = fx1.min(dw) / tw;
            let rv1 = fy1.min(dh) / th;
            let right_verts = make_quad(
                cx + cw,
                cy.max(0.0),
                fx1.min(dw),
                fy1.min(dh),
                ru0,
                rv0,
                ru1,
                rv1,
            );
            emit_quad(&mut *cmdbuf, &right_verts, &mut vb_offset, &mut draw_idx);
        }
        // Bottom border: between left and right borders
        if fy1 > cy + ch {
            let bu0 = cx / tw;
            let bv0 = (cy + ch) / th;
            let bu1 = (cx + cw) / tw;
            let bv1 = fy1.min(dh) / th;
            let bot_verts = make_quad(cx, cy + ch, cx + cw, fy1.min(dh), bu0, bv0, bu1, bv1);
            emit_quad(&mut *cmdbuf, &bot_verts, &mut vb_offset, &mut draw_idx);
        }

        if frame < 3 {
            crate::serial_println!(
                "[GPU-WIN] frame={} win[{}] res={} content=({},{})-({}x{}) frame=({:.0},{:.0})-({:.0},{:.0})",
                frame, i, win.virgl_resource_id, win.x, win.y, win.width, win.height,
                fx0, fy0, fx1, fy1
            );
        }
    }

    // ── Dimmer overlay: fullscreen translucent black quad ──
    // Drawn AFTER all non-chromeless windows to dim the entire desktop,
    // but BEFORE chromeless windows (launcher overlay) which draw on top.
    if has_chromeless && DIMMER_TEX_READY.load(Ordering::Acquire) {
        cmdbuf.bind_object(19, super::virgl::OBJ_BLEND); // alpha blend
        cmdbuf.create_sampler_view(
            21,
            RESOURCE_DIMMER_TEX_ID,
            vfmt::B8G8R8A8_UNORM,
            pipe::TEXTURE_2D,
            0,
            0,
            0,
            0,
            swizzle::IDENTITY,
        );
        cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[21]);
        let dimmer_verts = make_quad(0.0, 0.0, dw, dh, 0.0, 0.0, 1.0, 1.0);
        emit_quad(&mut *cmdbuf, &dimmer_verts, &mut vb_offset, &mut draw_idx);
        cmdbuf.bind_object(11, super::virgl::OBJ_BLEND); // restore opaque
        if frame < 3 {
            crate::serial_println!("[GPU-DIMMER] frame={} dimmer quad drawn", frame);
        }
    }

    // Pass 2: Chromeless windows (drawn opaque on top of dimmer)
    for (i, win) in windows.iter().enumerate() {
        if !win.virgl_initialized || win.virgl_resource_id == 0 {
            continue;
        }
        if !win.chromeless {
            continue;
        } // already drawn in pass 1

        let cx = win.x as f32;
        let cy = win.y as f32;
        let cw = win.width as f32;
        let ch = win.height as f32;

        let slot = (win.virgl_resource_id as usize).saturating_sub(RESOURCE_WIN_TEX_BASE as usize);
        let (tex_alloc_w, tex_alloc_h) = if slot < MAX_WIN_TEX_SLOTS {
            unsafe { WIN_TEX_DIMS[slot] }
        } else {
            (win.width, win.height)
        };
        let wu = win.width as f32 / tex_alloc_w as f32;
        let wv = win.height as f32 / tex_alloc_h as f32;

        let sv_handle = 40 + i as u32;
        cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[sv_handle]);
        let content_verts = make_quad(cx, cy, cx + cw, cy + ch, 0.0, 0.0, wu, wv);
        emit_quad(&mut *cmdbuf, &content_verts, &mut vb_offset, &mut draw_idx);

        if frame < 3 {
            crate::serial_println!(
                "[GPU-WIN] frame={} win[{}] res={} content=({},{})-({}x{}) CHROMELESS (pass 2)",
                frame,
                i,
                win.virgl_resource_id,
                win.x,
                win.y,
                win.width,
                win.height
            );
        }
    }

    // ── Draw cursor as GPU quad (rendered LAST, on top of everything) ──
    // The cursor lives in a dedicated GPU texture atlas (RESOURCE_CURSOR_TEX_ID,
    // 16x80, 5 shapes stacked vertically). UV coordinates select the active shape.
    if CURSOR_TEX_READY.load(Ordering::Acquire) {
        let (mouse_x, mouse_y) = if crate::drivers::virtio::input_mmio::is_tablet_initialized() {
            crate::drivers::virtio::input_mmio::mouse_position()
        } else {
            crate::drivers::usb::hid::mouse_position()
        };

        let shape = CURSOR_SHAPE
            .load(Ordering::Acquire)
            .min(NUM_CURSOR_SHAPES - 1);
        let (hx, hy) = CURSOR_HOTSPOT[shape as usize];
        let mx = mouse_x as f32 - hx as f32;
        let my = mouse_y as f32 - hy as f32;
        let sw = CURSOR_SHAPE_W as f32;
        let sh = CURSOR_SHAPE_H as f32;

        if (mx + sw) > 0.0 && mx < dw && (my + sh) > 0.0 && my < dh {
            cmdbuf.bind_object(19, super::virgl::OBJ_BLEND); // alpha blend (created in setup)

            cmdbuf.create_sampler_view(
                20,
                RESOURCE_CURSOR_TEX_ID,
                vfmt::B8G8R8A8_UNORM,
                pipe::TEXTURE_2D,
                0,
                0,
                0,
                0,
                swizzle::IDENTITY,
            );
            cmdbuf.set_sampler_views(pipe::SHADER_FRAGMENT, 0, &[20]);

            // UV coordinates: select the current shape from the atlas
            let v0 = (shape as f32 * sh) / CURSOR_TEX_H as f32;
            let v1 = ((shape as f32 + 1.0) * sh) / CURSOR_TEX_H as f32;
            let cursor_verts = make_quad(
                mx.max(0.0),
                my.max(0.0),
                (mx + sw).min(dw),
                (my + sh).min(dh),
                0.0,
                v0,
                1.0,
                v1,
            );
            emit_quad(&mut *cmdbuf, &cursor_verts, &mut vb_offset, &mut draw_idx);
        }
    }

    if frame < 3 {
        crate::serial_println!(
            "[composite-submit] frame={} windows={} dwords={} vb_offset={} draw_idx={}",
            frame,
            windows.len(),
            cmdbuf.as_slice().len(),
            vb_offset,
            draw_idx
        );
    }

    virgl_submit_sync(cmdbuf.as_slice())?;
    with_device_state(|state| set_scanout_resource(state, RESOURCE_3D_ID))?;
    with_device_state(|state| resource_flush_3d(state, RESOURCE_3D_ID))
}

/// Multi-window GPU compositor.
///
/// Uploads dirty textures (background + per-window), then renders all windows
/// as textured quads in a single SUBMIT_3D batch.
///
/// Phase A: Upload dirty textures (outside SUBMIT_3D)
///   - If bg_dirty: copy bg_pixels to COMPOSITE_TEX backing, cache clean, TRANSFER_TO_HOST_3D
///   - For each dirty window: cache clean MAP_SHARED pages, TRANSFER_TO_HOST_3D
///
/// Phase B: Single SUBMIT_3D batch
///   - Pipeline setup: create_sub_ctx, surface, blend, DSA, rasterizer, shaders, VE, sampler, viewport
///   - Background quad: sampler view on COMPOSITE_TEX → full-screen quad
///   - Per-window: sampler view on window tex → positioned quad + decoration rects
///
/// Phase C: SET_SCANOUT + RESOURCE_FLUSH
pub fn virgl_composite_windows(
    bg_pixels: Option<&[u32]>,
    bg_width: u32,
    bg_height: u32,
    bg_dirty: bool,
    dirty_rect: Option<(u32, u32, u32, u32)>, // (x, y, w, h) for partial upload
    windows: &[crate::syscall::graphics::WindowCompositeInfo],
) -> Result<(), &'static str> {
    static COMPOSITE_WIN_FRAME: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let frame = COMPOSITE_WIN_FRAME.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    // Performance tracing: read CNTVCT_EL0 at each phase boundary
    #[cfg(target_arch = "aarch64")]
    let t_start = {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack));
        }
        v
    };

    if !is_virgl_enabled() {
        return Err("VirGL not enabled");
    }
    if !COMPOSITE_TEX_READY.load(Ordering::Acquire) {
        return Err("Compositor texture not initialized");
    }

    if frame < 5 {
        crate::serial_println!(
            "[composite-win] frame={} bg_dirty={} windows={} bg={}x{}",
            frame,
            bg_dirty,
            windows.len(),
            bg_width,
            bg_height
        );
    }

    let tex_w = COMPOSITE_TEX_W.load(Ordering::Relaxed);
    let tex_h = COMPOSITE_TEX_H.load(Ordering::Relaxed);

    // =========================================================================
    // Phase A: Compose into COMPOSITE_TEX, then upload changed regions
    // =========================================================================

    let any_window_dirty = windows
        .iter()
        .any(|w| w.dirty && !w.page_phys_addrs.is_empty());
    let tex_stride = (tex_w as usize) * 4;

    // Step 1: Copy background into compositor texture.
    // bg_dirty=true with dirty_rect=None: full copy (original behavior).
    // bg_dirty=true with dirty_rect=Some(x,y,w,h): partial sub-rect copy.
    // BWM composites ALL windows (terminals + clients) into its buffer at correct
    // z-order. The kernel just copies and uploads — no kernel-side window blit needed.
    if bg_dirty {
        if let Some(pixels) = bg_pixels {
            let src_w = bg_width.min(tex_w) as usize;
            let src_h = bg_height.min(tex_h) as usize;
            if pixels.len() >= src_w * src_h {
                match dirty_rect {
                    Some((dx, dy, dw, dh)) => {
                        // Partial: copy only the dirty sub-rectangle row by row
                        let rx = (dx as usize).min(src_w);
                        let ry = (dy as usize).min(src_h);
                        let rw = (dw as usize).min(src_w - rx);
                        let rh = (dh as usize).min(src_h - ry);
                        let row_bytes = rw * 4;
                        for row in ry..(ry + rh) {
                            let off = row * src_w + rx;
                            let dst_off = row * (tex_w as usize) + rx;
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    (pixels.as_ptr() as *const u8).add(off * 4),
                                    COMPOSITE_TEX_PTR.add(dst_off * 4),
                                    row_bytes,
                                );
                            }
                        }
                    }
                    None => {
                        // Full copy
                        let copy_bytes = src_w * src_h * 4;
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                pixels.as_ptr() as *const u8,
                                COMPOSITE_TEX_PTR,
                                copy_bytes.min(COMPOSITE_TEX_LEN),
                            );
                        }
                    }
                }
            }
        }
    }

    // Chromeless windows like the launcher use normal per-window GPU textures
    // and render as GPU quads without frame-strip compositing.

    // ── Step 3: Cursor position tracking ────────────────────────────────────
    // Read mouse position and detect movement for early-out optimization.
    // The cursor is rendered as a GPU quad in virgl_composite_single_quad(),
    // NOT stamped into COMPOSITE_TEX (which caused ghost trails with per-window textures).
    use core::sync::atomic::AtomicI32;
    static CURSOR_PREV_X: AtomicI32 = AtomicI32::new(-1);
    static CURSOR_PREV_Y: AtomicI32 = AtomicI32::new(-1);

    let (mouse_x, mouse_y) = if crate::drivers::virtio::input_mmio::is_tablet_initialized() {
        crate::drivers::virtio::input_mmio::mouse_position()
    } else {
        crate::drivers::usb::hid::mouse_position()
    };
    let cur_x = mouse_x as i32;
    let cur_y = mouse_y as i32;
    let prev_cx = CURSOR_PREV_X.load(Ordering::Relaxed);
    let prev_cy = CURSOR_PREV_Y.load(Ordering::Relaxed);
    let cursor_moved = cur_x != prev_cx || cur_y != prev_cy;
    if cursor_moved {
        CURSOR_PREV_X.store(cur_x, Ordering::Relaxed);
        CURSOR_PREV_Y.store(cur_y, Ordering::Relaxed);
    }

    // Early out: if nothing changed (no content, no cursor movement), skip VirGL pipeline
    if !bg_dirty && !any_window_dirty && !cursor_moved {
        return Ok(());
    }

    // Step 4: Upload — full texture, partial rect, or cursor-only sub-regions
    let tex_bytes_total = (tex_w as usize) * (tex_h as usize) * 4;

    // Helper: cache-clean and upload a rectangular sub-region
    let upload_rect = |x: u32, y: u32, w: u32, h: u32| -> Result<(), &'static str> {
        let uw = w.min(tex_w.saturating_sub(x));
        let uh = h.min(tex_h.saturating_sub(y));
        if uw == 0 || uh == 0 {
            return Ok(());
        }
        let row_start = (y as usize) * tex_stride + (x as usize) * 4;
        let last_row = (y + uh).saturating_sub(1) as usize;
        let row_end = last_row * tex_stride + ((x + uw) as usize) * 4;
        let clean_len = row_end
            .saturating_sub(row_start)
            .min(tex_bytes_total.saturating_sub(row_start));
        if clean_len > 0 {
            dma_cache_clean(unsafe { COMPOSITE_TEX_PTR.add(row_start) }, clean_len);
        }
        with_device_state(|state| {
            transfer_to_host_3d(state, RESOURCE_COMPOSITE_TEX_ID, x, y, uw, uh, tex_w * 4)
        })
    };

    if bg_dirty && dirty_rect.is_some() {
        // Partial background upload — only the dirty sub-region
        let (dx, dy, dw, dh) = dirty_rect.unwrap();
        let ux = dx.min(tex_w);
        let uy = dy.min(tex_h);
        let uw = dw.min(tex_w - ux);
        let uh = dh.min(tex_h - uy);
        if uw > 0 && uh > 0 {
            upload_rect(ux, uy, uw, uh)?;
            crate::tracing::providers::counters::GPU_PARTIAL_UPLOADS.increment();
            crate::tracing::providers::counters::GPU_BYTES_UPLOADED
                .add((uw as u64) * (uh as u64) * 4);
        }
    } else if bg_dirty {
        // Full background upload
        dma_cache_clean(unsafe { COMPOSITE_TEX_PTR }, tex_bytes_total);
        with_device_state(|state| {
            transfer_to_host_3d(
                state,
                RESOURCE_COMPOSITE_TEX_ID,
                0,
                0,
                tex_w,
                tex_h,
                tex_w * 4,
            )
        })?;
        crate::tracing::providers::counters::GPU_FULL_UPLOADS.increment();
        crate::tracing::providers::counters::GPU_BYTES_UPLOADED.add(tex_bytes_total as u64);
    }
    // Note: cursor-only moves (no bg_dirty, no window_dirty) still trigger SUBMIT_3D
    // below to redraw the GPU cursor quad at the new position. No COMPOSITE_TEX upload needed.

    // Perf: timestamp after COMPOSITE_TEX upload, before per-window uploads
    #[cfg(target_arch = "aarch64")]
    let t_after_bg = {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack));
        }
        v
    };

    // =========================================================================
    // Phase A2: Upload per-window GPU textures
    // Per-window textures pre-allocated at init, TRANSFER_TO_HOST_3D proven working.
    // Uploads dirty window content from MAP_SHARED pages to GPU textures.
    // =========================================================================
    for win in windows.iter() {
        if !win.virgl_initialized || !win.dirty {
            continue;
        }
        if win.page_phys_addrs.is_empty() {
            continue;
        }
        let slot = (win.virgl_resource_id as usize).saturating_sub(RESOURCE_WIN_TEX_BASE as usize);
        if slot >= MAX_WIN_TEX_SLOTS {
            continue;
        }
        let _ = upload_window_texture(slot, win.width, win.height, &win.page_phys_addrs, win.size);
    }

    // =========================================================================
    // Phase B+C: GPU compositing + display
    // =========================================================================
    // Background + decorations from COMPOSITE_TEX, per-window content from
    // individual GPU textures, all in one SUBMIT_3D batch.

    // Perf: timestamp before display phase (after all uploads)
    #[cfg(target_arch = "aarch64")]
    let t_display = {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack));
        }
        v
    };

    virgl_composite_single_quad(windows)?;

    // Perf: end of frame
    #[cfg(target_arch = "aarch64")]
    let t_end = {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack));
        }
        v
    };

    if frame < 5 {
        crate::serial_println!("[composite-win] frame={} complete", frame);
    }

    // Performance summary every 500 WORK frames (frames that actually composited)
    // Uses a separate counter from the main frame counter since many calls hit early return.
    #[cfg(target_arch = "aarch64")]
    {
        use core::sync::atomic::AtomicU64;
        static WORK_FRAME: AtomicU32 = AtomicU32::new(0);
        static PERF_BG_UPLOAD_TICKS: AtomicU64 = AtomicU64::new(0);
        static PERF_WIN_UPLOAD_TICKS: AtomicU64 = AtomicU64::new(0);
        static PERF_SUBMIT_TICKS: AtomicU64 = AtomicU64::new(0);
        static PERF_TOTAL_TICKS: AtomicU64 = AtomicU64::new(0);

        let wf = WORK_FRAME.fetch_add(1, Ordering::Relaxed);

        let bg_upload = t_after_bg.saturating_sub(t_start);
        let win_upload = t_display.saturating_sub(t_after_bg);
        let submit = t_end.saturating_sub(t_display);
        let total = t_end.saturating_sub(t_start);

        PERF_BG_UPLOAD_TICKS.fetch_add(bg_upload, Ordering::Relaxed);
        PERF_WIN_UPLOAD_TICKS.fetch_add(win_upload, Ordering::Relaxed);
        PERF_SUBMIT_TICKS.fetch_add(submit, Ordering::Relaxed);
        PERF_TOTAL_TICKS.fetch_add(total, Ordering::Relaxed);

        if wf > 0 && wf % 500 == 0 {
            PERF_BG_UPLOAD_TICKS.store(0, Ordering::Relaxed);
            PERF_WIN_UPLOAD_TICKS.store(0, Ordering::Relaxed);
            PERF_SUBMIT_TICKS.store(0, Ordering::Relaxed);
            GPU_SLEEP_TICKS_PHASES.store(0, Ordering::Relaxed);
            PERF_TOTAL_TICKS.store(0, Ordering::Relaxed);
        }
    }

    Ok(())
}

/// Submit a VirGL command buffer for the active 3D context.
///
/// `cmds` is a slice of u32 DWORDs from a VirGL CommandBuffer.
pub fn virgl_submit(cmds: &[u32]) -> Result<u64, &'static str> {
    with_device_state(|state| virgl_submit_3d_cmd(state, VIRGL_CTX_ID, cmds).map(|r| r.submit_id))
}

/// Submit VirGL commands and wait for the fence to complete before returning.
/// This ensures the host GPU has finished processing the commands.
///
/// Optimization: if the SUBMIT_3D response already confirms the fence
/// (FLAG_FENCE means the device waited for GPU completion), skip the
/// expensive fence_sync NOP polling entirely.
pub fn virgl_submit_sync(cmds: &[u32]) -> Result<(), &'static str> {
    let result = with_device_state(|state| virgl_submit_3d_cmd(state, VIRGL_CTX_ID, cmds))?;

    // Fast path: the device response already confirmed our fence completed.
    // With VIRTIO_GPU_FLAG_FENCE, Parallels waits for GPU completion before
    // responding, so resp_fence == submit_id in almost all cases.
    if result.resp_fence >= result.submit_id {
        return Ok(());
    }

    // Slow path: fence not yet confirmed, poll with WFI between rounds
    with_device_state(|state| virgl_fence_sync(state, result.submit_id))
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
        if fb_ptr.is_null() {
            return;
        }
        let copy_len = fb_bytes.min(bar0.len()).min(fb_len);
        unsafe {
            core::ptr::copy_nonoverlapping(fb_ptr, bar0.as_mut_ptr(), copy_len);
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
#[allow(dead_code)]
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
/// Matches Linux per-frame pattern: SET_SCANOUT → RESOURCE_FLUSH.
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
    use super::virgl::{format as vfmt, pipe, CommandBuffer};

    if !is_virgl_enabled() {
        return Err("VirGL not supported");
    }

    crate::serial_println!("[virgl] Initializing VirGL 3D pipeline...");

    let (width, height) = dimensions().ok_or("GPU not initialized")?;

    // Step 1: Allocate 3D framebuffer backing on heap (DMA-safe memory).
    // BSS memory overlaps with Parallels boot stack, causing DMA failures.
    init_3d_framebuffer(width, height);

    // Step 2: Create 3D context
    with_device_state(|state| virgl_ctx_create_cmd(state, VIRGL_CTX_ID, b"breenix"))?;
    crate::serial_println!("[virgl] Step 1: context created");

    // Step 3: Create 3D render target resource
    let bind_flags =
        pipe::BIND_RENDER_TARGET | pipe::BIND_SAMPLER_VIEW | pipe::BIND_SCANOUT | pipe::BIND_SHARED;
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_3D_ID,
            pipe::TEXTURE_2D,
            vfmt::B8G8R8X8_UNORM,
            bind_flags,
            width,
            height,
            1,
            1,
        )
    })?;
    crate::serial_println!(
        "[virgl] Step 2: 3D resource created ({}x{}, bind=0x{:08x})",
        width,
        height,
        bind_flags
    );

    // Step 4: Attach backing memory (per-page scatter-gather)
    with_device_state(|state| {
        let fb_ptr = unsafe { PCI_3D_FB_PTR };
        let fb_len = unsafe { PCI_3D_FB_LEN };
        let actual_len = (state.width as usize * state.height as usize * 4).min(fb_len);
        virgl_attach_backing_paged(state, RESOURCE_3D_ID, fb_ptr, actual_len)
    })?;
    crate::serial_println!("[virgl] Step 3: backing attached");

    // Step 5: Attach resource to VirGL context
    with_device_state(|state| virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_3D_ID))?;
    crate::serial_println!("[virgl] Step 4: resource attached to context");

    // Step 5: Prime — exactly matches cornflower blue commit (e47c96b)
    {
        let fb_ptr = unsafe { PCI_3D_FB_PTR };
        let fb_len = (width * height * 4) as usize;
        dma_cache_clean(fb_ptr, fb_len);
        with_device_state(|state| {
            transfer_to_host_3d(state, RESOURCE_3D_ID, 0, 0, width, height, width * 4)
        })?;
        with_device_state(|state| set_scanout_resource(state, RESOURCE_3D_ID))?;
        with_device_state(|state| resource_flush_3d(state, RESOURCE_3D_ID))?;
        crate::serial_println!("[virgl] Step 5: resource primed");
    }

    // Step 6: Minimal VirGL clear to cornflower blue — exactly matches e47c96b
    {
        let mut cmdbuf = CommandBuffer::new();
        cmdbuf.create_sub_ctx(1);
        cmdbuf.set_sub_ctx(1);
        cmdbuf.set_tweaks(1, 1);
        cmdbuf.set_tweaks(2, width);
        cmdbuf.create_surface(1, RESOURCE_3D_ID, vfmt::B8G8R8X8_UNORM, 0, 0);
        cmdbuf.set_framebuffer_state(0, &[1]);
        cmdbuf.clear_color(0.392, 0.584, 0.929, 1.0);
        virgl_submit_sync(cmdbuf.as_slice())?;
        crate::serial_println!("[virgl] Step 6: VirGL CLEAR (cornflower blue)");
    }

    // Step 7: SET_SCANOUT + RESOURCE_FLUSH — matching Linux per-frame pattern.
    with_device_state(|state| {
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        resource_flush_3d(state, RESOURCE_3D_ID)
    })?;
    crate::serial_println!("[virgl] Step 7: SET_SCANOUT + RESOURCE_FLUSH done");

    // Step 8: Create VB resource (no backing — data via INLINE_WRITE in batch)
    with_device_state(|state| {
        virgl_resource_create_3d_cmd(
            state,
            RESOURCE_VB_ID,
            pipe::BUFFER,
            vfmt::R8_UNORM,
            pipe::BIND_VERTEX_BUFFER,
            4096,
            1,
            1,
            1,
        )
    })?;
    with_device_state(|state| virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_VB_ID))?;
    crate::serial_println!("[virgl] Step 7: VB resource created (no backing, INLINE_WRITE only)");

    // Step 8: Initialize compositor texture for GPU compositing (virgl_composite_frame)
    init_composite_texture(width, height)?;
    crate::serial_println!("[virgl] Step 8: Compositor texture initialized");

    // Step 8b removed: VB data is uploaded via RESOURCE_INLINE_WRITE in the batch below.

    // Step 9: Full pipeline + CLEAR + DRAW_VBO batch.
    // Expected: cornflower blue baseline from both CLEAR and DRAW_VBO.
    {
        let mut cmdbuf = CommandBuffer::new();
        cmdbuf.create_sub_ctx(1);
        cmdbuf.set_sub_ctx(1);

        // Surface wrapping the render target
        cmdbuf.create_surface(1, RESOURCE_3D_ID, vfmt::B8G8R8X8_UNORM, 0, 0);
        cmdbuf.set_framebuffer_state(0, &[1]);

        // Progressive test: blend + DSA + rasterizer + VS + CLEAR
        cmdbuf.create_blend_simple(1);
        cmdbuf.bind_object(1, super::virgl::OBJ_BLEND);
        cmdbuf.create_dsa_default(1);
        cmdbuf.bind_object(1, super::virgl::OBJ_DSA);
        cmdbuf.create_rasterizer_default(1);
        cmdbuf.bind_object(1, super::virgl::OBJ_RASTERIZER);

        // Shaders — num_tokens=300 REQUIRED (Parallels rejects num_tokens=0)
        let vs_text = b"VERT\nDCL IN[0]\nDCL OUT[0], POSITION\n  0: MOV OUT[0], IN[0]\n  1: END\n";
        cmdbuf.create_shader(1, pipe::SHADER_VERTEX, 300, vs_text);
        cmdbuf.bind_shader(1, pipe::SHADER_VERTEX);

        let fs_text = b"FRAG\nPROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\nDCL OUT[0], COLOR\nDCL CONST[0]\n  0: MOV OUT[0], CONST[0]\n  1: END\n";
        cmdbuf.create_shader(2, pipe::SHADER_FRAGMENT, 300, fs_text);
        cmdbuf.bind_shader(2, pipe::SHADER_FRAGMENT);

        // Vertex elements
        cmdbuf.create_vertex_elements(1, &[(0, 0, 0, vfmt::R32G32B32A32_FLOAT)]);
        cmdbuf.bind_object(1, super::virgl::OBJ_VERTEX_ELEMENTS);

        cmdbuf.set_min_samples(1);
        cmdbuf.set_viewport(width as f32, height as f32);

        // CLEAR to the documented VirGL baseline color.
        cmdbuf.clear_color(0.392, 0.584, 0.929, 1.0);

        // Upload vertex data inline
        let quad_verts: [u32; 16] = [
            (-1.0f32).to_bits(),
            (1.0f32).to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(),
            (-1.0f32).to_bits(),
            (-1.0f32).to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(),
            1.0f32.to_bits(),
            (-1.0f32).to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(),
            1.0f32.to_bits(),
            1.0f32.to_bits(),
            0f32.to_bits(),
            1.0f32.to_bits(),
        ];
        cmdbuf.resource_inline_write(RESOURCE_VB_ID, 0, 64, &quad_verts);

        // Constant buffer: cornflower blue for fragment shader.
        cmdbuf.set_constant_buffer(
            pipe::SHADER_FRAGMENT,
            0,
            &[
                0.392f32.to_bits(),
                0.584f32.to_bits(),
                0.929f32.to_bits(),
                1.0f32.to_bits(),
            ],
        );

        // Bind VB and draw
        cmdbuf.set_vertex_buffers(&[(16, 0, RESOURCE_VB_ID)]);
        cmdbuf.draw_vbo(0, 4, pipe::PRIM_TRIANGLE_FAN, 3);

        crate::serial_println!(
            "[virgl] Step 9: full pipeline+draw batch ({} DWORDs)",
            cmdbuf.len()
        );
        virgl_submit_sync(cmdbuf.as_slice())?;
    }

    // Step 10: Display VirGL-rendered content.
    // Linux per-frame pattern: SUBMIT_3D → SET_SCANOUT → RESOURCE_FLUSH.
    // SET_SCANOUT must be re-issued after every SUBMIT_3D, not just once.
    with_device_state(|state| {
        set_scanout_resource(state, RESOURCE_3D_ID)?;
        resource_flush_3d(state, RESOURCE_3D_ID)
    })?;
    crate::serial_println!("[virgl] Step 10: SET_SCANOUT + RESOURCE_FLUSH");

    VIRGL_SCANOUT_ACTIVE.store(true, Ordering::Release);
    VIRGL_HEX_DUMP_ENABLED.store(false, Ordering::Relaxed);

    crate::serial_println!("[virgl] VirGL 3D pipeline initialized successfully");

    Ok(())
}

// virgl_render_demo_pattern removed — DRAW_VBO is now in the init batch itself

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
    use super::virgl::{format as vfmt, pipe, swizzle, CommandBuffer};

    let (width, height) = dimensions().ok_or("GPU not initialized")?;
    crate::serial_println!(
        "[virgl-tex] Starting textured quad test ({}x{})...",
        width,
        height
    );

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
    crate::serial_println!(
        "[virgl-tex] Checkerboard pattern written to backing ({} bytes)",
        TEST_TEX_BYTES
    );

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
            1,
            1,
        )
    })?;
    crate::serial_println!(
        "[virgl-tex] Texture resource created (id={}, {}x{})",
        RESOURCE_TEX_ID,
        TEST_TEX_DIM,
        TEST_TEX_DIM
    );

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
    crate::serial_println!(
        "[virgl-tex] Texture backing attached (phys={:#x})",
        tex_phys
    );

    // Step 4: Attach texture to VirGL context
    with_device_state(|state| virgl_ctx_attach_resource_cmd(state, VIRGL_CTX_ID, RESOURCE_TEX_ID))?;
    crate::serial_println!("[virgl-tex] Texture attached to context");

    // Step 5: Upload texture via TRANSFER_TO_HOST_3D
    with_device_state(|state| {
        transfer_to_host_3d(
            state,
            RESOURCE_TEX_ID,
            0,
            0,
            TEST_TEX_DIM,
            TEST_TEX_DIM,
            TEST_TEX_DIM * 4,
        )
    })?;
    crate::serial_println!("[virgl-tex] Texture uploaded via TRANSFER_TO_HOST_3D");

    // Step 6: Create VirGL texture objects (FS shader, sampler view, sampler state)
    let mut cmdbuf = CommandBuffer::new();
    cmdbuf.set_sub_ctx(1);

    // Texture fragment shader: samples from SAMP[0] instead of passing vertex color
    let tex_fs = b"FRAG\nDCL IN[0], GENERIC[0], LINEAR\nDCL OUT[0], COLOR\nDCL SAMP[0]\nDCL SVIEW[0], 2D, FLOAT\n  0: TEX OUT[0], IN[0], SAMP[0], 2D\n  1: END\n";
    cmdbuf.create_shader(3, pipe::SHADER_FRAGMENT, 20, tex_fs);

    // Sampler view: bind texture resource for shader sampling
    cmdbuf.create_sampler_view(
        5,                    // handle
        RESOURCE_TEX_ID,      // resource
        vfmt::B8G8R8A8_UNORM, // format
        pipe::TEXTURE_2D,     // target (packed into format bits [24:31])
        0,
        0, // first_layer, last_layer
        0,
        0,                 // first_level, last_level
        swizzle::IDENTITY, // RGBA identity swizzle
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
    cmdbuf.bind_shader(1, pipe::SHADER_VERTEX); // existing VS (passes GENERIC[0])
    cmdbuf.bind_shader(3, pipe::SHADER_FRAGMENT); // texture FS
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
        (-1.0f32).to_bits(),
        (1.0f32).to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        // v1: bottom-left (clip: -1,-1) → texcoord (0,1)
        (-1.0f32).to_bits(),
        (-1.0f32).to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        // v2: top-right (clip: 1,1) → texcoord (1,0)
        1.0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
        // v3: bottom-right (clip: 1,-1) → texcoord (1,1)
        1.0f32.to_bits(),
        (-1.0f32).to_bits(),
        0f32.to_bits(),
        1.0f32.to_bits(),
        1.0f32.to_bits(),
        1.0f32.to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
    ];

    // Upload quad vertices inline to the vertex buffer
    cmdbuf.resource_inline_write(RESOURCE_VB_ID, 0, 128, &quad_verts);
    cmdbuf.set_vertex_buffers(&[(BYTES_PER_VERTEX as u32, 0, RESOURCE_VB_ID)]);

    // Draw triangle strip (4 vertices = 2 triangles = full screen)
    cmdbuf.draw_vbo(0, 4, pipe::PRIM_TRIANGLE_STRIP, 3);

    let fence_id = virgl_submit(cmdbuf.as_slice())?;
    crate::serial_println!(
        "[virgl-tex] Textured quad submitted ({} DWORDs, fence={})",
        cmdbuf.as_slice().len(),
        fence_id
    );

    // Wait for VirGL rendering to complete on the host GPU before displaying.
    // Without this, the display refresh can race with async VirGL execution.
    crate::serial_println!("[virgl-tex] Waiting for fence {}...", fence_id);
    with_device_state(|state| virgl_fence_sync(state, fence_id))?;
    crate::serial_println!(
        "[virgl-tex] Fence {} completed — VirGL rendering done",
        fence_id
    );

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
