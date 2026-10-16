//! VMware SVGA3 virtual GPU driver (PCI device 15ad:0406).
//!
//! SVGA3 is VMware's ARM64-native virtual GPU, replacing the x86-only SVGA2
//! (0x0405). Key differences from SVGA2:
//!   - Registers accessed via MMIO BAR (not I/O ports)
//!   - MSI-X interrupt support
//!   - Legacy FIFO deprecated; command buffers used instead
//!   - Supports OpenGL 4.3 / ES 3.1 via SVGA3D command set
//!
//! ## GPU Compositing via STDU (Screen Target Display Unit)
//!
//! When GBObjects capability is present, the display pipeline is:
//!   Guest pixels in MOB → Surface (bound to MOB) → Screen Target → Display
//!
//! One-time initialization (`init_stdu`), on command-buffer context 0:
//!   1. Start context 0 through the device context
//!   2. Set up OTables, each over one PT64_0 page
//!   3. Create a PT64_2 MOB for the framebuffer backing
//!   4. Create a v1 GB Surface bound to the MOB
//!   5. Define Screen Target bound to the surface
//!
//! Per-frame update:
//!   1. Write pixels to MOB backing memory (CPU)
//!   2. Cache clean the dirty region
//!   3. UPDATE_GB_IMAGE — tell GPU that guest memory changed
//!   4. UPDATE_GB_SCREENTARGET — present dirty region to display
//!
//! Readback: SURFACE_COPY of the bound surface into a readback surface on the host, then
//! READBACK_GB_SURFACE of that copy into its own MOB (`compositor_readback`).
//!
//! Reference: Linux kernel drivers/gpu/drm/vmwgfx/ and device_include/svga_reg.h

extern crate alloc;

use alloc::alloc::Layout;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

// ─── PCI identification ──────────────────────────────────────────────────────

/// VMware vendor ID
pub const VMWARE_VENDOR_ID: u16 = 0x15AD;
/// SVGA3 device ID (ARM64-native GPU)
pub const SVGA3_DEVICE_ID: u16 = 0x0406;

// ─── SVGA register indices (at MMIO BAR offset = index * 4) ──────────────────

const SVGA_REG_ID: u32 = 0;
const SVGA_REG_ENABLE: u32 = 1;
const SVGA_REG_WIDTH: u32 = 2;
const SVGA_REG_HEIGHT: u32 = 3;
const SVGA_REG_BITS_PER_PIXEL: u32 = 7;
const SVGA_REG_BYTES_PER_LINE: u32 = 12;
const SVGA_REG_FB_OFFSET: u32 = 14;
const SVGA_REG_FB_SIZE: u32 = 16;
const SVGA_REG_CAPABILITIES: u32 = 17;
const SVGA_REG_MEM_SIZE: u32 = 19;
const SVGA_REG_CONFIG_DONE: u32 = 20;
const SVGA_REG_SYNC: u32 = 21;
const SVGA_REG_BUSY: u32 = 22;
const SVGA_REG_TRACES: u32 = 45;
const SVGA_REG_COMMAND_LOW: u32 = 48;
const SVGA_REG_COMMAND_HIGH: u32 = 49;
/// The last fence the device passed.
const SVGA_REG_FENCE: u32 = 69;

// ─── SVGA capability bits ───────────────────────────────────────────────────

const SVGA_CAP_CURSOR: u32 = 0x0000_0020;
#[allow(dead_code)]
const SVGA_CAP_ALPHA_CURSOR: u32 = 0x0000_0200;
const SVGA_CAP_3D: u32 = 0x0000_4000;
const SVGA_CAP_IRQMASK: u32 = 0x0004_0000;
#[allow(dead_code)]
const SVGA_CAP_SCREEN_OBJECT_2: u32 = 0x0080_0000;
const SVGA_CAP_COMMAND_BUFFERS: u32 = 0x0100_0000;
const SVGA_CAP_CMD_BUFFERS_2: u32 = 0x0400_0000;
const SVGA_CAP_GBOBJECTS: u32 = 0x0800_0000;
const SVGA_CAP_DX: u32 = 0x1000_0000;

// ─── SVGA version IDs ───────────────────────────────────────────────────────

const SVGA_ID_2: u32 = 0x0090_0002;

// ─── SVGA legacy command IDs ────────────────────────────────────────────────

#[allow(dead_code)]
const SVGA_CMD_UPDATE: u32 = 1;
#[allow(dead_code)]
const SVGA_CMD_DEFINE_SCREEN: u32 = 34;
#[allow(dead_code)]
const SVGA_CMD_DESTROY_SCREEN: u32 = 35;
#[allow(dead_code)]
const SVGA_CMD_DEFINE_GMRFB: u32 = 36;
#[allow(dead_code)]
const SVGA_CMD_BLIT_GMRFB_TO_SCREEN: u32 = 37;
#[allow(dead_code)]
const SVGA_CMD_ANNOTATION_FILL: u32 = 39;
/// FENCE: when the device reaches it, SVGA_REG_FENCE takes its value.
const SVGA_CMD_FENCE: u32 = 30;
/// Device-context command: start or stop a command-buffer context.
const SVGA_DC_CMD_START_STOP_CONTEXT: u32 = 1;

// ─── SVGA3D command IDs (GBObjects / STDU pipeline) ─────────────────────────

/// Set Object Table base address for a given table type.
const SVGA_3D_CMD_SET_OTABLE_BASE64: u32 = 1115;
/// Define a Guest-Backed Memory Object (64-bit PPN).
const SVGA_3D_CMD_DEFINE_GB_MOB64: u32 = 1135;
/// Destroy a MOB.
#[allow(dead_code)]
const SVGA_3D_CMD_DESTROY_GB_MOB: u32 = 1094;
/// Define a Guest-Backed Surface (v1); the v2 form failed with COMMAND_ERROR on VMware Fusion.
const SVGA_3D_CMD_DEFINE_GB_SURFACE: u32 = 1097;
/// Bind a surface to a MOB (surface data is in the MOB).
const SVGA_3D_CMD_BIND_GB_SURFACE: u32 = 1099;
/// Tell GPU that guest memory backing a surface image has been updated.
const SVGA_3D_CMD_UPDATE_GB_IMAGE: u32 = 1101;
/// Define a Screen Target (modern display output).
const SVGA_3D_CMD_DEFINE_GB_SCREENTARGET: u32 = 1124;
/// Bind a surface to a Screen Target.
const SVGA_3D_CMD_BIND_GB_SCREENTARGET: u32 = 1126;
/// Present a dirty region of the Screen Target to the display.
const SVGA_3D_CMD_UPDATE_GB_SCREENTARGET: u32 = 1127;
/// Copy boxes of one surface into another, on the host.
const SVGA_3D_CMD_SURFACE_COPY: u32 = 1042;
/// Write a guest-backed surface's host contents into its MOB.
const SVGA_3D_CMD_READBACK_GB_SURFACE: u32 = 1104;

// ─── OTable types ───────────────────────────────────────────────────────────

const SVGA_OTABLE_MOB: u32 = 0;
const SVGA_OTABLE_SURFACE: u32 = 1;
const SVGA_OTABLE_CONTEXT: u32 = 2;
const SVGA_OTABLE_SHADER: u32 = 3;
const SVGA_OTABLE_SCREENTARGET: u32 = 4;

// ─── MOB format (ptDepth) ───────────────────────────────────────────────────

/// MOB format: the base is the page itself.
const SVGA3D_MOBFMT_PT64_0: u32 = 4;
/// MOB format: two levels of 64-bit page tables.
const SVGA3D_MOBFMT_PT64_2: u32 = 6;

// ─── Surface formats ────────────────────────────────────────────────────────

/// SVGA3D surface format X8R8G8B8 (B, G, R, X bytes in memory).
const SVGA3D_X8R8G8B8: u32 = 1;

// ─── Surface flags ──────────────────────────────────────────────────────────

/// Surface can be bound to a Screen Target.
const SVGA3D_SURFACE_SCREENTARGET: u32 = 1 << 16;

// ─── Screen Target flags ────────────────────────────────────────────────────

const SVGA_STFLAG_PRIMARY: u32 = 1;

// ─── HHDM base ──────────────────────────────────────────────────────────────

const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
const PAGE_SIZE: usize = 4096;

// ─── Command buffer structures ──────────────────────────────────────────────

/// Command buffer status codes (volatile, written by device)
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CbStatus {
    None = 0,
    Completed = 1,
    #[allow(dead_code)]
    QueueFull = 2,
    #[allow(dead_code)]
    CommandError = 3,
    #[allow(dead_code)]
    HeaderError = 4,
    #[allow(dead_code)]
    Preempted = 5,
    #[allow(dead_code)]
    SubmissionError = 6,
}

/// Command buffer context IDs (encoded in lower 6 bits of COMMAND_LOW)
const SVGA_CB_CONTEXT_0: u32 = 0x0;
/// Device-level context for OTable, MOB, Surface, ScreenTarget management
const SVGA_CB_CONTEXT_DEVICE: u32 = 0x3F;

/// SVGACBHeader — 56 bytes, must be 64-byte aligned in memory.
#[repr(C)]
struct SvgaCbHeader {
    status: u32,
    error_offset: u32,
    id: u64,
    flags: u32,
    length: u32,
    pa: u64,
    offset: u32,
    dx_context: u32,
    must_be_zero: [u32; 6],
}

// ─── Driver state ───────────────────────────────────────────────────────────

static INITIALIZED: AtomicBool = AtomicBool::new(false);
/// Whether the STDU compositing pipeline is initialized and ready.
static STDU_READY: AtomicBool = AtomicBool::new(false);

/// Cached MMIO register base (virtual address via HHDM)
static mut RMMIO_BASE: u64 = 0;
/// Cached VRAM base (virtual address via HHDM)
static mut VRAM_BASE: u64 = 0;
/// VRAM physical base
static mut VRAM_PHYS: u64 = 0;
/// VRAM size in bytes
static mut VRAM_SIZE: u64 = 0;
/// Device capabilities
static mut CAPABILITIES: u32 = 0;
/// Current display width
static mut DISPLAY_WIDTH: u32 = 0;
/// Current display height
static mut DISPLAY_HEIGHT: u32 = 0;
/// Bytes per scanline
static mut BYTES_PER_LINE: u32 = 0;

// ─── STDU compositor state ──────────────────────────────────────────────────

/// Virtual address of the compositor texture backing (HHDM-mapped heap memory).
/// BWM writes pixels here, then UPDATE_GB_IMAGE + UPDATE_GB_SCREENTARGET
/// tells the GPU to read and display them.
static COMPOSITOR_BUF_VIRT: AtomicU64 = AtomicU64::new(0);
/// Size of the compositor buffer in bytes.
static COMPOSITOR_BUF_SIZE: AtomicU64 = AtomicU64::new(0);

/// MOB ID for the compositor surface backing.
const COMPOSITOR_MOB_ID: u32 = 1;
/// Surface ID for the compositor surface.
const COMPOSITOR_SURFACE_ID: u32 = 1;
/// Screen Target ID (primary display).
const COMPOSITOR_STID: u32 = 0;

/// The surface the wrong-resource fault binds the screen target to, over the same MOB.
const ALT_SURFACE_ID: u32 = 2;
/// The surface the readback copies the screen target's surface into, on its own MOB.
const READBACK_SURFACE_ID: u32 = 3;
const READBACK_MOB_ID: u32 = 2;
/// The readback surface's MOB, once defined.
static READBACK_BUF_VIRT: AtomicU64 = AtomicU64::new(0);
/// The screen target's size.
static STDU_W: AtomicU32 = AtomicU32::new(0);
static STDU_H: AtomicU32 = AtomicU32::new(0);
/// The surface the screen target is bound to.
static BOUND_SURFACE: AtomicU32 = AtomicU32::new(0);
/// Whether `ALT_SURFACE_ID` is defined.
static ALT_DEFINED: AtomicBool = AtomicBool::new(false);
/// The next fence the compositor puts in the command stream.
static NEXT_FENCE: AtomicU32 = AtomicU32::new(1);

// ─── Register access ────────────────────────────────────────────────────────

#[inline]
unsafe fn reg_read(index: u32) -> u32 {
    let addr = RMMIO_BASE + (index as u64) * 4;
    core::ptr::read_volatile(addr as *const u32)
}

#[inline]
unsafe fn reg_write(index: u32, value: u32) {
    let addr = RMMIO_BASE + (index as u64) * 4;
    core::ptr::write_volatile(addr as *mut u32, value);
}

// ─── Command buffer submission ──────────────────────────────────────────────

/// Flag: don't generate an IRQ on command buffer completion.
const SVGA_CB_FLAG_NO_IRQ: u32 = 1 << 0;

/// The device-visible address of a kernel virtual address. VMware's RAM starts at IPA
/// 0x80000000, so a heap address minus the HHDM base is not an address the device can use.
fn device_address(virt: u64) -> u64 {
    crate::platform_config::kernel_va_to_ipa(virt)
}

/// Submit a command buffer on context `ctx` and wait for the device to finish it.
///
/// The header's device-visible byte address goes in COMMAND_HIGH/LOW with the context in
/// bits 5:0, as vmwgfx does. Returns the header's status and error offset. A buffer the
/// device never finishes is leaked, since the device may still write it.
fn submit_commands_ctx(cmds: &[u8], ctx: u32) -> Result<(u32, u32), &'static str> {
    if cmds.is_empty() {
        return Ok((CbStatus::Completed as u32, 0));
    }

    const HEADER_SIZE: usize = 64;
    let total_size = HEADER_SIZE + cmds.len();
    let layout = Layout::from_size_align(total_size, PAGE_SIZE)
        .map_err(|_| "Invalid layout for command buffer")?;
    let buf_ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if buf_ptr.is_null() {
        return Err("Failed to allocate command buffer");
    }
    let buf_virt = buf_ptr as u64;
    let buf_phys = device_address(buf_virt);
    let header = buf_ptr as *mut SvgaCbHeader;

    unsafe {
        (*header).status = CbStatus::None as u32;
        (*header).flags = SVGA_CB_FLAG_NO_IRQ;
        (*header).length = cmds.len() as u32;
        (*header).pa = buf_phys + HEADER_SIZE as u64;
        core::ptr::copy_nonoverlapping(cmds.as_ptr(), buf_ptr.add(HEADER_SIZE), cmds.len());

        // The device reads physical memory, not the CPU cache.
        cache_clean_range(buf_virt, total_size);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));

        reg_write(SVGA_REG_COMMAND_HIGH, (buf_phys >> 32) as u32);
        reg_write(SVGA_REG_COMMAND_LOW, (buf_phys as u32) | (ctx & 0x3F));

        let mut spins = 0u32;
        loop {
            cache_invalidate_range(buf_virt, 64);
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
            let status = core::ptr::read_volatile(&(*header).status);
            if status != CbStatus::None as u32 {
                let err_off = core::ptr::read_volatile(&(*header).error_offset);
                alloc::alloc::dealloc(buf_ptr, layout);
                return Ok((status, err_off));
            }
            spins += 1;
            if spins > 20_000_000 {
                return Err("Command buffer timeout");
            }
            core::hint::spin_loop();
        }
    }
}

/// Clean (write-back) CPU cache for a memory range so device can read it.
#[inline]
unsafe fn cache_clean_range(virt: u64, size: usize) {
    let mut addr = virt & !63; // Cache line aligned
    let end = virt + size as u64;
    while addr < end {
        core::arch::asm!("dc cvac, {}", in(reg) addr, options(nostack, preserves_flags));
        addr += 64;
    }
}

/// Invalidate CPU cache for a memory range so CPU sees device writes.
#[inline]
unsafe fn cache_invalidate_range(virt: u64, size: usize) {
    let mut addr = virt & !63;
    let end = virt + size as u64;
    while addr < end {
        core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack, preserves_flags));
        addr += 64;
    }
}

/// Start (`enable`) or stop context 0 through the device context.
fn start_stop_context0(enable: bool) -> Result<(), &'static str> {
    let mut enc = CmdEncoder::new();
    enc.push_u32(SVGA_DC_CMD_START_STOP_CONTEXT);
    enc.push_u32(u32::from(enable));
    enc.push_u32(SVGA_CB_CONTEXT_0);
    match submit_commands_ctx(&enc.finish(), SVGA_CB_CONTEXT_DEVICE)? {
        (status, _) if status == CbStatus::Completed as u32 => Ok(()),
        _ => Err("device context refused START_STOP_CONTEXT"),
    }
}

/// Submit SVGA3D commands on context 0. A command error halts the context, so it is stopped
/// and restarted, as vmwgfx does, and the error is returned.
fn submit_commands(cmds: &[u8]) -> Result<(), &'static str> {
    let (status, err_off) = submit_commands_ctx(cmds, SVGA_CB_CONTEXT_0)?;
    if status == CbStatus::Completed as u32 {
        return Ok(());
    }
    crate::serial_println!(
        "[svga3] CB error: status={} err_off={} len={}",
        status,
        err_off,
        cmds.len()
    );
    let _ = start_stop_context0(false);
    let _ = start_stop_context0(true);
    Err("Command buffer error")
}

// ─── SVGA3D command encoding ────────────────────────────────────────────────

/// Command encoder — appends SVGA3D commands to a byte buffer.
struct CmdEncoder {
    buf: Vec<u8>,
}

impl CmdEncoder {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(1024),
        }
    }

    fn push_u32(&mut self, val: u32) {
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    fn push_i32(&mut self, val: i32) {
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    fn push_u64(&mut self, val: u64) {
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    /// Emit command header: { id: u32, size: u32 }
    fn cmd_header(&mut self, cmd_id: u32, body_size: u32) {
        self.push_u32(cmd_id);
        self.push_u32(body_size);
    }

    /// SET_OTABLE_BASE64: register an Object Table with the device.
    fn set_otable_base64(
        &mut self,
        table_type: u32,
        ppn64: u64,
        size_bytes: u32,
        valid_size: u32,
        pt_depth: u32,
    ) {
        // Body: type(4) + baseAddress(8) + sizeInBytes(4) + validSizeInBytes(4) + ptDepth(4) = 24
        self.cmd_header(SVGA_3D_CMD_SET_OTABLE_BASE64, 24);
        self.push_u32(table_type);
        self.push_u64(ppn64);
        self.push_u32(size_bytes);
        self.push_u32(valid_size);
        self.push_u32(pt_depth);
    }

    /// DEFINE_GB_MOB64: create a guest-backed memory object.
    fn define_gb_mob64(&mut self, mobid: u32, pt_depth: u32, base_ppn64: u64, size_bytes: u32) {
        // Body: mobid(4) + ptDepth(4) + base(8) + sizeInBytes(4) = 20
        self.cmd_header(SVGA_3D_CMD_DEFINE_GB_MOB64, 20);
        self.push_u32(mobid);
        self.push_u32(pt_depth);
        self.push_u64(base_ppn64);
        self.push_u32(size_bytes);
    }

    /// BIND_GB_SURFACE: bind a surface to a MOB (surface pixels live in the MOB).
    fn bind_gb_surface(&mut self, sid: u32, mobid: u32) {
        // Body: sid(4) + mobid(4) = 8
        self.cmd_header(SVGA_3D_CMD_BIND_GB_SURFACE, 8);
        self.push_u32(sid);
        self.push_u32(mobid);
    }

    /// UPDATE_GB_IMAGE: tell GPU that guest memory for a surface region changed.
    fn update_gb_image(&mut self, sid: u32, x: u32, y: u32, w: u32, h: u32) {
        // Body: image(12) + box(24) = 36
        self.cmd_header(SVGA_3D_CMD_UPDATE_GB_IMAGE, 36);
        // SVGA3dSurfaceImageId: sid(4) + face(4) + mipmap(4)
        self.push_u32(sid);
        self.push_u32(0); // face
        self.push_u32(0); // mipmap
                          // SVGA3dBox: x(4) + y(4) + z(4) + w(4) + h(4) + d(4)
        self.push_u32(x);
        self.push_u32(y);
        self.push_u32(0); // z
        self.push_u32(w);
        self.push_u32(h);
        self.push_u32(1); // d
    }

    /// DEFINE_GB_SCREENTARGET: create a screen target for display output.
    fn define_gb_screentarget(&mut self, stid: u32, width: u32, height: u32, flags: u32) {
        // Body: stid(4) + width(4) + height(4) + xRoot(4) + yRoot(4) + flags(4) + dpi(4) = 28
        self.cmd_header(SVGA_3D_CMD_DEFINE_GB_SCREENTARGET, 28);
        self.push_u32(stid);
        self.push_u32(width);
        self.push_u32(height);
        self.push_i32(0); // xRoot
        self.push_i32(0); // yRoot
        self.push_u32(flags);
        self.push_u32(0); // dpi
    }

    /// BIND_GB_SCREENTARGET: bind a surface to a screen target.
    fn bind_gb_screentarget(&mut self, stid: u32, sid: u32) {
        // Body: stid(4) + image(12) = 16
        self.cmd_header(SVGA_3D_CMD_BIND_GB_SCREENTARGET, 16);
        self.push_u32(stid);
        // SVGA3dSurfaceImageId: sid(4) + face(4) + mipmap(4)
        self.push_u32(sid);
        self.push_u32(0); // face
        self.push_u32(0); // mipmap
    }

    /// UPDATE_GB_SCREENTARGET: present a dirty rectangle to the display.
    fn update_gb_screentarget(&mut self, stid: u32, x: u32, y: u32, w: u32, h: u32) {
        // Body: stid(4) + rect(16) = 20
        self.cmd_header(SVGA_3D_CMD_UPDATE_GB_SCREENTARGET, 20);
        self.push_u32(stid);
        // SVGASignedRect: left(4) + top(4) + right(4) + bottom(4)
        self.push_i32(x as i32);
        self.push_i32(y as i32);
        self.push_i32((x + w) as i32);
        self.push_i32((y + h) as i32);
    }

    /// SURFACE_COPY: copy the `w` x `h` box at the origin of `src` into `dst`, on the host.
    fn surface_copy(&mut self, src: u32, dst: u32, w: u32, h: u32) {
        // Body: src image(12) + dest image(12) + one copy box(36) = 60
        self.cmd_header(SVGA_3D_CMD_SURFACE_COPY, 60);
        for sid in [src, dst] {
            self.push_u32(sid);
            self.push_u32(0); // face
            self.push_u32(0); // mipmap
        }
        // SVGA3dCopyBox: x, y, z, w, h, d, srcx, srcy, srcz
        for v in [0, 0, 0, w, h, 1, 0, 0, 0] {
            self.push_u32(v);
        }
    }

    /// READBACK_GB_SURFACE: write the host's contents of surface `sid` into its MOB.
    fn readback_gb_surface(&mut self, sid: u32) {
        self.cmd_header(SVGA_3D_CMD_READBACK_GB_SURFACE, 4);
        self.push_u32(sid);
    }

    fn finish(self) -> Vec<u8> {
        self.buf
    }
}

// ─── Initialization ─────────────────────────────────────────────────────────

/// Initialize the SVGA3 driver (device detection, register setup, VRAM traces).
pub fn init() -> Result<(), &'static str> {
    use crate::serial_println;

    let dev = crate::drivers::pci::find_device(VMWARE_VENDOR_ID, SVGA3_DEVICE_ID)
        .ok_or("SVGA3 device not found")?;

    serial_println!(
        "[svga3] Found VMware SVGA3 at {:02x}:{:02x}.{}",
        dev.bus,
        dev.device,
        dev.function
    );

    let bar0 = &dev.bars[0];
    if !bar0.is_valid() || bar0.is_io {
        return Err("SVGA3 BAR0 is not a valid MMIO BAR");
    }
    let rmmio_phys = bar0.address;

    let bar2 = &dev.bars[2];
    if !bar2.is_valid() || bar2.is_io {
        return Err("SVGA3 BAR2 (VRAM) is not a valid MMIO BAR");
    }
    let vram_phys = bar2.address;
    let vram_size = bar2.size;

    serial_println!(
        "[svga3] BAR0 (regs): phys={:#x} size={:#x}",
        rmmio_phys,
        bar0.size
    );
    serial_println!(
        "[svga3] BAR2 (VRAM): phys={:#x} size={:#x}",
        vram_phys,
        vram_size
    );

    let rmmio_virt = HHDM_BASE + rmmio_phys;
    let vram_virt = HHDM_BASE + vram_phys;

    unsafe {
        RMMIO_BASE = rmmio_virt;
        VRAM_BASE = vram_virt;
        VRAM_PHYS = vram_phys;
        VRAM_SIZE = vram_size;
    }

    unsafe {
        reg_write(SVGA_REG_ID, SVGA_ID_2);
        let id = reg_read(SVGA_REG_ID);
        serial_println!("[svga3] Device version: {:#x}", id);
    }

    let caps = unsafe { reg_read(SVGA_REG_CAPABILITIES) };
    unsafe {
        CAPABILITIES = caps;
    }

    serial_println!("[svga3] Capabilities: {:#010x}", caps);
    serial_println!(
        "[svga3]   3D={} CmdBuf={} CmdBuf2={} GB={} DX={} IRQ={} Cursor={}",
        if caps & SVGA_CAP_3D != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_COMMAND_BUFFERS != 0 {
            "Y"
        } else {
            "n"
        },
        if caps & SVGA_CAP_CMD_BUFFERS_2 != 0 {
            "Y"
        } else {
            "n"
        },
        if caps & SVGA_CAP_GBOBJECTS != 0 {
            "Y"
        } else {
            "n"
        },
        if caps & SVGA_CAP_DX != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_IRQMASK != 0 {
            "Y"
        } else {
            "n"
        },
        if caps & SVGA_CAP_CURSOR != 0 {
            "Y"
        } else {
            "n"
        }
    );

    if caps & SVGA_CAP_COMMAND_BUFFERS == 0 {
        return Err("SVGA3 device does not support command buffers");
    }

    let width = unsafe { reg_read(SVGA_REG_WIDTH) };
    let height = unsafe { reg_read(SVGA_REG_HEIGHT) };
    serial_println!("[svga3] Using GOP resolution {}x{}", width, height);

    let bpp = unsafe { reg_read(SVGA_REG_BITS_PER_PIXEL) };
    let bpl = unsafe { reg_read(SVGA_REG_BYTES_PER_LINE) };
    let fb_size = unsafe { reg_read(SVGA_REG_FB_SIZE) };
    let fb_offset = unsafe { reg_read(SVGA_REG_FB_OFFSET) };
    let mem_size = unsafe { reg_read(SVGA_REG_MEM_SIZE) };

    unsafe {
        DISPLAY_WIDTH = width;
        DISPLAY_HEIGHT = height;
        BYTES_PER_LINE = bpl;
    }

    serial_println!(
        "[svga3] Display: {}x{} @ {}bpp, stride={} bytes",
        width,
        height,
        bpp,
        bpl
    );
    serial_println!(
        "[svga3] FB offset={:#x} size={:#x} mem_size={:#x}",
        fb_offset,
        fb_size,
        mem_size
    );

    unsafe {
        reg_write(SVGA_REG_ENABLE, 1);
        reg_write(SVGA_REG_TRACES, 1);
        reg_write(SVGA_REG_CONFIG_DONE, 1);
    }

    let traces_val = unsafe { reg_read(SVGA_REG_TRACES) };
    serial_println!(
        "[svga3] VRAM traces: {}",
        if traces_val != 0 {
            "enabled"
        } else {
            "disabled"
        }
    );

    INITIALIZED.store(true, Ordering::Release);
    serial_println!("[svga3] SVGA3 driver initialized successfully");
    Ok(())
}

/// Check if the SVGA3 driver is initialized.
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

/// Check if the device has 3D acceleration capability.
pub fn has_3d() -> bool {
    unsafe { CAPABILITIES & SVGA_CAP_3D != 0 }
}

/// Check if GBObjects capability is available (needed for STDU).
pub fn has_gbobjects() -> bool {
    unsafe { CAPABILITIES & SVGA_CAP_GBOBJECTS != 0 }
}

/// Check if the STDU compositing pipeline is ready for use.
pub fn is_stdu_ready() -> bool {
    STDU_READY.load(Ordering::Acquire)
}

/// Get current display dimensions.
pub fn dimensions() -> Option<(u32, u32)> {
    if !is_initialized() {
        return None;
    }
    unsafe { Some((DISPLAY_WIDTH, DISPLAY_HEIGHT)) }
}

/// Synchronize — wait for all pending SVGA commands to complete.
pub fn sync() {
    if !is_initialized() {
        return;
    }
    unsafe {
        reg_write(SVGA_REG_SYNC, 1);
        while reg_read(SVGA_REG_BUSY) != 0 {
            core::hint::spin_loop();
        }
    }
}

// ─── STDU Compositing Pipeline ──────────────────────────────────────────────

/// Allocate a page-aligned, contiguous block of memory for DMA.
/// Returns (virtual_address, device_address, size_in_bytes).
fn alloc_dma_pages(num_pages: usize) -> Result<(u64, u64, usize), &'static str> {
    let size = num_pages * PAGE_SIZE;
    let layout = Layout::from_size_align(size, PAGE_SIZE).map_err(|_| "Invalid DMA layout")?;
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err("Failed to allocate DMA pages");
    }
    let virt = ptr as u64;
    Ok((virt, device_address(virt), size))
}

/// Register one OTable over a single zeroed page (`SVGA3D_MOBFMT_PT64_0`), as vmwgfx does.
fn set_otable(enc: &mut CmdEncoder, table_type: u32) -> Result<(), &'static str> {
    let (virt, phys, size) = alloc_dma_pages(1)?;
    unsafe { cache_clean_range(virt, size) };
    enc.set_otable_base64(table_type, phys >> 12, size as u32, 0, SVGA3D_MOBFMT_PT64_0);
    Ok(())
}

/// Define v1 guest-backed surface `sid`: X8R8G8B8, one mip level, `width` x `height`.
fn define_surface(sid: u32, flags: u32, width: u32, height: u32) -> Vec<u8> {
    let mut enc = CmdEncoder::new();
    enc.cmd_header(SVGA_3D_CMD_DEFINE_GB_SURFACE, 36);
    enc.push_u32(sid);
    enc.push_u32(flags);
    enc.push_u32(SVGA3D_X8R8G8B8);
    enc.push_u32(1); // numMipLevels
    enc.push_u32(0); // multisampleCount
    enc.push_u32(0); // autogenFilter
    enc.push_u32(width);
    enc.push_u32(height);
    enc.push_u32(1); // depth
    enc.finish()
}

/// Define MOB `mob` over zeroed contiguous pages under a two-level page table (PT64_2), and
/// screen-target surface `sid` of `width` x `height` bound to it. Returns the pages' address.
fn backed_surface(mob: u32, sid: u32, width: u32, height: u32) -> Result<u64, &'static str> {
    let bytes = width as usize * height as usize * 4;
    let pages = bytes.div_ceil(PAGE_SIZE);
    let tables = pages.div_ceil(PAGE_SIZE / 8);
    let (data_virt, _, _) = alloc_dma_pages(pages)?;
    let (dir_virt, dir_phys, _) = alloc_dma_pages(1)?;
    let (pt_virt, _, _) = alloc_dma_pages(tables)?;
    unsafe {
        for i in 0..pages {
            let ppn = device_address(data_virt + (i * PAGE_SIZE) as u64) >> 12;
            core::ptr::write_volatile((pt_virt as *mut u64).add(i), ppn);
        }
        for t in 0..tables {
            let ppn = device_address(pt_virt + (t * PAGE_SIZE) as u64) >> 12;
            core::ptr::write_volatile((dir_virt as *mut u64).add(t), ppn);
        }
        cache_clean_range(pt_virt, tables * PAGE_SIZE);
        cache_clean_range(dir_virt, PAGE_SIZE);
        cache_clean_range(data_virt, bytes);
    }
    let mut enc = CmdEncoder::new();
    enc.define_gb_mob64(mob, SVGA3D_MOBFMT_PT64_2, dir_phys >> 12, bytes as u32);
    submit_commands(&enc.finish())?;
    // Each object in its own command buffer, so an error names its command.
    submit_commands(&define_surface(sid, SVGA3D_SURFACE_SCREENTARGET, width, height))?;
    let mut enc = CmdEncoder::new();
    enc.bind_gb_surface(sid, mob);
    submit_commands(&enc.finish())?;
    Ok(data_virt)
}

/// Initialize the STDU pipeline at `width` x `height`: start context 0, register the
/// OTables, define a MOB for the compositor's backing, a screen-target surface bound to it,
/// and primary screen target `COMPOSITOR_STID` bound to the surface. After this the device
/// shows the MOB's pixels through `UPDATE_GB_IMAGE` and `UPDATE_GB_SCREENTARGET`.
pub fn init_stdu(width: u32, height: u32) -> Result<(), &'static str> {
    use crate::serial_println;

    if is_stdu_ready() {
        return Ok(());
    }
    if !is_initialized() {
        return Err("SVGA3 not initialized");
    }
    if !has_gbobjects() {
        return Err("SVGA3 device does not support GBObjects (needed for STDU)");
    }

    start_stop_context0(true)?;

    let mut enc = CmdEncoder::new();
    for table in [
        SVGA_OTABLE_MOB,
        SVGA_OTABLE_SURFACE,
        SVGA_OTABLE_CONTEXT,
        SVGA_OTABLE_SHADER,
        SVGA_OTABLE_SCREENTARGET,
    ] {
        set_otable(&mut enc, table)?;
    }
    submit_commands(&enc.finish())?;

    let data_virt = backed_surface(COMPOSITOR_MOB_ID, COMPOSITOR_SURFACE_ID, width, height)?;
    let bytes = width as usize * height as usize * 4;
    let mut enc = CmdEncoder::new();
    enc.define_gb_screentarget(COMPOSITOR_STID, width, height, SVGA_STFLAG_PRIMARY);
    submit_commands(&enc.finish())?;
    let mut enc = CmdEncoder::new();
    enc.bind_gb_screentarget(COMPOSITOR_STID, COMPOSITOR_SURFACE_ID);
    submit_commands(&enc.finish())?;

    COMPOSITOR_BUF_VIRT.store(data_virt, Ordering::Release);
    COMPOSITOR_BUF_SIZE.store(bytes as u64, Ordering::Release);
    STDU_W.store(width, Ordering::Release);
    STDU_H.store(height, Ordering::Release);
    BOUND_SURFACE.store(COMPOSITOR_SURFACE_ID, Ordering::Release);
    NEXT_FENCE.store(unsafe { reg_read(SVGA_REG_FENCE) }.wrapping_add(1).max(1), Ordering::Release);
    STDU_READY.store(true, Ordering::Release);
    serial_println!(
        "[svga3-stdu] Screen target {} at {}x{} bound to surface {} on MOB {}",
        COMPOSITOR_STID,
        width,
        height,
        COMPOSITOR_SURFACE_ID,
        COMPOSITOR_MOB_ID
    );
    Ok(())
}

/// Composite a pixel buffer to the display via VRAM direct writes.
///
/// Copies pixels to the VRAM framebuffer. The SVGA3 device monitors writes
/// to VRAM via its trace mechanism and updates the display automatically.
pub fn composite_frame(pixels: &[u32], width: u32, height: u32) -> Result<(), &'static str> {
    if !is_initialized() {
        return Err("SVGA3 not initialized");
    }

    let vram = unsafe { VRAM_BASE } as *mut u8;
    let bpl = unsafe { BYTES_PER_LINE } as usize;
    let display_w = unsafe { DISPLAY_WIDTH } as usize;
    let display_h = unsafe { DISPLAY_HEIGHT } as usize;
    let copy_w = (width as usize).min(display_w);
    let copy_h = (height as usize).min(display_h);
    let row_bytes = copy_w * 4;

    let src = pixels.as_ptr() as *const u8;
    let src_stride = (width as usize) * 4;

    for row in 0..copy_h {
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.add(row * src_stride),
                vram.add(row * bpl),
                row_bytes,
            );
        }
    }

    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    Ok(())
}

/// Composite a dirty rectangle from a pixel buffer to the display via VRAM.
///
/// Only copies and updates the specified region, reducing bandwidth.
pub fn composite_frame_rect(
    pixels: &[u32],
    src_w: u32,
    _src_h: u32,
    dx: u32,
    dy: u32,
    dw: u32,
    dh: u32,
) -> Result<(), &'static str> {
    if !is_initialized() {
        return Err("SVGA3 not initialized");
    }

    let vram = unsafe { VRAM_BASE } as *mut u8;
    let bpl = unsafe { BYTES_PER_LINE } as usize;
    let display_w = unsafe { DISPLAY_WIDTH };
    let display_h = unsafe { DISPLAY_HEIGHT };

    let bx = dx.min(display_w);
    let by = dy.min(display_h);
    let bw = dw.min(display_w - bx);
    let bh = dh.min(display_h - by);
    if bw == 0 || bh == 0 {
        return Ok(());
    }

    let src_stride = src_w as usize * 4;
    let src = pixels.as_ptr() as *const u8;
    let row_bytes = bw as usize * 4;

    for row in by..(by + bh) {
        let src_off = (row as usize) * src_stride + (bx as usize) * 4;
        let dst_off = (row as usize) * bpl + (bx as usize) * 4;
        if src_off + row_bytes <= pixels.len() * 4 {
            unsafe {
                core::ptr::copy_nonoverlapping(src.add(src_off), vram.add(dst_off), row_bytes);
            }
        }
    }

    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    Ok(())
}

/// Get compositor texture info for MAP_SHARED into userspace.
/// Returns (phys_base, num_pages, width, height) — same signature as gpu_pci's version.
///
/// Maps the VRAM framebuffer directly — the SVGA3 device monitors writes to VRAM
/// via its trace mechanism and updates the display automatically.
pub fn compositor_texture_info() -> Option<(u64, u32, u32, u32)> {
    if !is_initialized() {
        return None;
    }
    let w = unsafe { DISPLAY_WIDTH };
    let h = unsafe { DISPLAY_HEIGHT };
    let phys = unsafe { VRAM_PHYS };
    if w == 0 || h == 0 || phys == 0 {
        return None;
    }
    let size = (w as usize) * (h as usize) * 4;
    let num_pages = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as u32;
    Some((phys, num_pages, w, h))
}

/// The compositor's backing: the MOB of the surface bound to the screen target, rows of
/// `width * 4` bytes, X8R8G8B8 (B, G, R, X in memory).
pub fn compositor_backing() -> Option<&'static mut [u8]> {
    if !is_stdu_ready() {
        return None;
    }
    let virt = COMPOSITOR_BUF_VIRT.load(Ordering::Acquire);
    let len = COMPOSITOR_BUF_SIZE.load(Ordering::Acquire) as usize;
    (virt != 0).then(|| unsafe { core::slice::from_raw_parts_mut(virt as *mut u8, len) })
}

/// The surface the compositor composes into.
pub fn compositor_resource() -> u64 {
    COMPOSITOR_SURFACE_ID as u64
}

/// The most recent fence the compositor put in the command stream.
pub fn last_fence() -> u64 {
    NEXT_FENCE.load(Ordering::Acquire).wrapping_sub(1) as u64
}

/// A new fence for the command stream; never 0.
fn next_fence() -> u32 {
    let f = NEXT_FENCE.fetch_add(1, Ordering::AcqRel);
    if f == 0 {
        NEXT_FENCE.fetch_add(1, Ordering::AcqRel)
    } else {
        f
    }
}

/// Wait for `SVGA_REG_FENCE` to pass `goal`.
fn wait_fence(goal: u32) -> bool {
    for _ in 0..20_000_000u32 {
        if unsafe { reg_read(SVGA_REG_FENCE) }.wrapping_sub(goal) as i32 >= 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Present `rects` of the backing: per rectangle `UPDATE_GB_IMAGE`,
/// `UPDATE_GB_SCREENTARGET` and `SVGA_CMD_FENCE`, then wait for `SVGA_REG_FENCE` to reach
/// every fence. `extra` is updated too, unfenced and unrecorded. `skip` leaves out the first
/// rectangle, `wrong` binds the screen target to another surface over the same MOB, and
/// `stale` gives the first rectangle that fence instead of a new one (the planted faults of
/// BreenGL §20.3). Returns the surface bound to the screen target and one completion per
/// fenced rectangle.
pub fn compositor_present(
    rects: &[breengel_abi::Rect],
    extra: Option<breengel_abi::Rect>,
    skip: bool,
    wrong: bool,
    stale: Option<u64>,
) -> Result<(u64, Vec<breengel_abi::Completion>), &'static str> {
    let backing = compositor_backing().ok_or("STDU not ready")?;
    let (w, h) = (STDU_W.load(Ordering::Acquire), STDU_H.load(Ordering::Acquire));
    let bound = if wrong { ALT_SURFACE_ID } else { COMPOSITOR_SURFACE_ID };
    if wrong && !ALT_DEFINED.load(Ordering::Acquire) {
        submit_commands(&define_surface(ALT_SURFACE_ID, SVGA3D_SURFACE_SCREENTARGET, w, h))?;
        let mut enc = CmdEncoder::new();
        enc.bind_gb_surface(ALT_SURFACE_ID, COMPOSITOR_MOB_ID);
        submit_commands(&enc.finish())?;
        ALT_DEFINED.store(true, Ordering::Release);
    }
    let mut enc = CmdEncoder::new();
    if BOUND_SURFACE.load(Ordering::Acquire) != bound {
        enc.bind_gb_screentarget(COMPOSITOR_STID, bound);
    }
    let mut fences = Vec::new();
    for (i, r) in rects.iter().chain(extra.iter()).enumerate() {
        if (skip && i == 0) || r.w <= 0 || r.h <= 0 {
            continue;
        }
        let (x, y, rw, rh) = (r.x as u32, r.y as u32, r.w as u32, r.h as u32);
        if x + rw > w || y + rh > h {
            return Err("damage outside the screen target");
        }
        let row = w as usize * 4;
        let start = y as usize * row + x as usize * 4;
        let end = (y + rh - 1) as usize * row + (x + rw) as usize * 4;
        unsafe { cache_clean_range(backing[start..end].as_ptr() as u64, end - start) };
        enc.update_gb_image(COMPOSITOR_SURFACE_ID, x, y, rw, rh);
        enc.update_gb_screentarget(COMPOSITOR_STID, x, y, rw, rh);
        if i < rects.len() {
            let fence = match stale {
                Some(f) if i == 0 => f as u32,
                _ => next_fence(),
            };
            enc.push_u32(SVGA_CMD_FENCE);
            enc.push_u32(fence);
            fences.push(fence);
        }
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    submit_commands(&enc.finish())?;
    BOUND_SURFACE.store(bound, Ordering::Release);
    let mut done = Vec::new();
    for fence in fences {
        if !wait_fence(fence) {
            return Err("SVGA_REG_FENCE did not reach the composite's fence");
        }
        done.push(breengel_abi::Completion {
            fence: fence as u64,
            kind: SVGA_3D_CMD_UPDATE_GB_SCREENTARGET,
            status: CbStatus::Completed as u32,
        });
    }
    Ok((bound as u64, done))
}

/// Read the device's copy of the surface bound to the screen target into `out` (rows of
/// `width * 4` bytes, B, G, R, X). The device writes nothing back for a surface whose host
/// contents came only from guest uploads, so the host first copies the bound surface into the
/// readback surface (`SURFACE_COPY`), and `READBACK_GB_SURFACE` writes that copy into the
/// readback surface's own MOB, fenced. The MOB is cleared to X 0 before, and every published
/// pixel has X 0xFF, so a pixel the device did not write is refused. Returns the readback's
/// answered completion.
pub fn compositor_readback(out: &mut [u8]) -> Result<breengel_abi::Completion, &'static str> {
    if !is_stdu_ready() {
        return Err("STDU not ready");
    }
    let (w, h) = (STDU_W.load(Ordering::Acquire), STDU_H.load(Ordering::Acquire));
    let len = w as usize * h as usize * 4;
    if out.len() < len {
        return Err("readback destination too small");
    }
    let mut virt = READBACK_BUF_VIRT.load(Ordering::Acquire);
    if virt == 0 {
        virt = backed_surface(READBACK_MOB_ID, READBACK_SURFACE_ID, w, h)?;
        READBACK_BUF_VIRT.store(virt, Ordering::Release);
    }
    let copy = unsafe { core::slice::from_raw_parts_mut(virt as *mut u8, len) };
    copy.fill(0);
    unsafe { cache_clean_range(virt, len) };
    let fence = next_fence();
    let mut enc = CmdEncoder::new();
    enc.surface_copy(BOUND_SURFACE.load(Ordering::Acquire), READBACK_SURFACE_ID, w, h);
    enc.readback_gb_surface(READBACK_SURFACE_ID);
    enc.push_u32(SVGA_CMD_FENCE);
    enc.push_u32(fence);
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    submit_commands(&enc.finish())?;
    if !wait_fence(fence) {
        return Err("SVGA_REG_FENCE did not reach the readback's fence");
    }
    unsafe { cache_invalidate_range(virt, len) };
    if copy.chunks_exact(4).any(|px| px[3] != 0xFF) {
        return Err("readback left pixels the device did not write");
    }
    out[..len].copy_from_slice(copy);
    Ok(breengel_abi::Completion {
        fence: fence as u64,
        kind: SVGA_3D_CMD_READBACK_GB_SURFACE,
        status: CbStatus::Completed as u32,
    })
}

/// Present a dirty rectangle from the compositor buffer (VRAM) to the display.
///
/// With VRAM traces enabled, the device monitors writes automatically.
/// This just ensures the CPU writes are flushed to memory.
pub fn present_rect(_x: u32, _y: u32, _w: u32, _h: u32) -> Result<(), &'static str> {
    if !is_initialized() {
        return Err("SVGA3 not initialized");
    }
    // VRAM traces handle display updates automatically.
    // DSB ensures all CPU writes to VRAM are visible.
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    Ok(())
}

// ─── VRAM Cursor Drawing ────────────────────────────────────────────────────

/// Previous cursor position (for erase-before-draw).
static CURSOR_PREV_X: AtomicI32 = AtomicI32::new(-1);
static CURSOR_PREV_Y: AtomicI32 = AtomicI32::new(-1);

const CURSOR_W: usize = 12;
const CURSOR_H: usize = 18;

// Arrow cursor bitmap: 1=white, 2=black outline, 0=transparent (12x18)
const CURSOR_BITMAP: [[u8; 12]; 18] = [
    [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0],
    [2, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 0],
    [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 1, 1, 2, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 2, 1, 1, 2, 0, 0, 0, 0],
    [2, 2, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0],
    [2, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 0],
];

/// Saved background pixels under the cursor.
static mut CURSOR_SAVED_BG: [u32; CURSOR_W * CURSOR_H] = [0; CURSOR_W * CURSOR_H];

/// Draw/update the mouse cursor in VRAM. Erases the old cursor position (restoring
/// saved background), saves background under the new position, and draws the cursor.
/// Returns true if any VRAM pixels were modified.
pub fn update_cursor() -> bool {
    if !is_initialized() {
        return false;
    }

    let (mx, my, _) = crate::drivers::usb::hid::mouse_state();
    let cur_x = mx as i32;
    let cur_y = my as i32;

    let prev_cx = CURSOR_PREV_X.load(Ordering::Relaxed);
    let prev_cy = CURSOR_PREV_Y.load(Ordering::Relaxed);

    if cur_x == prev_cx && cur_y == prev_cy {
        return false;
    }

    let vram = unsafe { VRAM_BASE } as *mut u32;
    let tw = unsafe { DISPLAY_WIDTH } as usize;
    let th = unsafe { DISPLAY_HEIGHT } as usize;
    if tw == 0 || th == 0 {
        return false;
    }

    // Erase old cursor (restore saved background)
    if prev_cx >= 0 && prev_cy >= 0 {
        for row in 0..CURSOR_H {
            let py = prev_cy as usize + row;
            if py >= th {
                break;
            }
            for col in 0..CURSOR_W {
                let px = prev_cx as usize + col;
                if px >= tw {
                    break;
                }
                if CURSOR_BITMAP[row][col] != 0 {
                    unsafe {
                        let saved = CURSOR_SAVED_BG[row * CURSOR_W + col];
                        *vram.add(py * tw + px) = saved;
                    }
                }
            }
        }
    }

    // Save background under new cursor position, then draw
    let draw = cur_x >= 0 && cur_y >= 0 && (cur_x as u32) < tw as u32 && (cur_y as u32) < th as u32;
    if draw {
        // Save
        for row in 0..CURSOR_H {
            let py = cur_y as usize + row;
            if py >= th {
                break;
            }
            for col in 0..CURSOR_W {
                let px = cur_x as usize + col;
                if px >= tw {
                    break;
                }
                if CURSOR_BITMAP[row][col] != 0 {
                    unsafe {
                        CURSOR_SAVED_BG[row * CURSOR_W + col] = *vram.add(py * tw + px);
                    }
                }
            }
        }
        // Draw
        for row in 0..CURSOR_H {
            let py = cur_y as usize + row;
            if py >= th {
                break;
            }
            for col in 0..CURSOR_W {
                let px = cur_x as usize + col;
                if px >= tw {
                    break;
                }
                match CURSOR_BITMAP[row][col] {
                    1 => unsafe {
                        *vram.add(py * tw + px) = 0x00FFFFFF;
                    }, // white
                    2 => unsafe {
                        *vram.add(py * tw + px) = 0x00000000;
                    }, // black
                    _ => {}
                }
            }
        }
    }

    CURSOR_PREV_X.store(cur_x, Ordering::Relaxed);
    CURSOR_PREV_Y.store(cur_y, Ordering::Relaxed);

    // Flush CPU writes to VRAM
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }

    true
}

// ─── Legacy VRAM Drawing ────────────────────────────────────────────────────

/// Fill VRAM with a solid color (legacy path, VRAM traces mode).
fn fill_rect_vram(x: u32, y: u32, w: u32, h: u32, r: u8, g: u8, b: u8) {
    let width = unsafe { DISPLAY_WIDTH };
    let height = unsafe { DISPLAY_HEIGHT };
    let bpl = unsafe { BYTES_PER_LINE } as usize;
    let vram = unsafe { VRAM_BASE as *mut u8 };

    let x2 = (x + w).min(width);
    let y2 = (y + h).min(height);

    for row in y..y2 {
        let row_base = (row as usize) * bpl;
        for col in x..x2 {
            let offset = row_base + (col as usize) * 4;
            unsafe {
                core::ptr::write_volatile(vram.add(offset), b);
                core::ptr::write_volatile(vram.add(offset + 1), g);
                core::ptr::write_volatile(vram.add(offset + 2), r);
                core::ptr::write_volatile(vram.add(offset + 3), 0);
            }
        }
    }

    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Draw a test pattern to prove the SVGA3 driver works.
pub fn draw_test_pattern() -> Result<(), &'static str> {
    use crate::serial_println;

    if !is_initialized() {
        return Err("SVGA3 not initialized");
    }

    let w = unsafe { DISPLAY_WIDTH };
    let h = unsafe { DISPLAY_HEIGHT };

    serial_println!("[svga3] Drawing test pattern on {}x{} display...", w, h);

    fill_rect_vram(0, 0, w, h, 20, 30, 50);

    let rects: [(u32, u32, u32, u32, u8, u8, u8); 6] = [
        (50, 50, 200, 150, 220, 40, 40),
        (300, 80, 200, 150, 40, 200, 40),
        (550, 110, 200, 150, 40, 80, 220),
        (100, 300, 250, 120, 220, 220, 40),
        (400, 350, 200, 130, 200, 50, 200),
        (700, 280, 180, 160, 40, 200, 200),
    ];

    for &(x, y, rw, rh, r, g, b) in &rects {
        fill_rect_vram(x, y, rw, rh, r, g, b);
    }

    fill_rect_vram(0, 0, w, 30, 60, 80, 120);

    serial_println!("[svga3] Test pattern drawn successfully");
    Ok(())
}
