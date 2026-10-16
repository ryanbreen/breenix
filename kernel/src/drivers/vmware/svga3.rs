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
//! One-time initialization:
//!   1. Set up OTables (MOB, Surface, ScreenTarget)
//!   2. Create MOB for framebuffer backing
//!   3. Create GB Surface bound to the MOB
//!   4. Define Screen Target bound to the surface
//!
//! Per-frame update:
//!   1. Write pixels to MOB backing memory (CPU)
//!   2. Cache clean the dirty region
//!   3. UPDATE_GB_IMAGE — tell GPU that guest memory changed
//!   4. UPDATE_GB_SCREENTARGET — present dirty region to display
//!
//! Wire provenance: VMware-authored device interface definitions; the
//! command submission derivation and exact source anchors are in command.rs.

extern crate alloc;

use alloc::alloc::Layout;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

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
const SVGA_REG_FIFO_CAPS: u32 = 68;
const SVGA_REG_FENCE: u32 = 69;
const SVGA_FIFO_CAP_FENCE: u32 = 1 << 0;
const SVGA_CMD_FENCE: u32 = 30;

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

// ─── SVGA3D command IDs (GBObjects / STDU pipeline) ─────────────────────────

/// Set Object Table base address for a given table type.
const SVGA_3D_CMD_SET_OTABLE_BASE64: u32 = 1115;
/// Define a Guest-Backed Memory Object (64-bit PPN).
const SVGA_3D_CMD_DEFINE_GB_MOB64: u32 = 1135;
/// Destroy a MOB.
#[cfg(feature = "testing")]
const SVGA_3D_CMD_DESTROY_GB_MOB: u32 = 1094;
/// Define a Guest-Backed Surface.
const SVGA_3D_CMD_DEFINE_GB_SURFACE_V2: u32 = 1134;
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
#[cfg(feature = "testing")]
const SVGA_3D_CMD_SURFACE_COPY: u32 = 1042;
#[cfg(feature = "testing")]
const SVGA_3D_CMD_READBACK_GB_IMAGE: u32 = 1103;
#[cfg(feature = "testing")]
const SVGA_3D_CMD_DESTROY_GB_SCREENTARGET: u32 = 1125;
#[cfg(feature = "testing")]
const SVGA_3D_CMD_DESTROY_GB_SURFACE: u32 = 1098;

// ─── OTable types ───────────────────────────────────────────────────────────

const SVGA_OTABLE_MOB: u32 = 0;
const SVGA_OTABLE_SURFACE: u32 = 1;
#[allow(dead_code)]
const SVGA_OTABLE_CONTEXT: u32 = 2;
#[allow(dead_code)]
const SVGA_OTABLE_SHADER: u32 = 3;
const SVGA_OTABLE_SCREENTARGET: u32 = 4;

#[path = "tables.rs"]
mod tables;

// ─── Surface formats ────────────────────────────────────────────────────────

/// B8G8R8X8 (BGRX, 32bpp, no alpha) — matches VMware VRAM format.
const SVGA3D_B8G8R8X8_UNORM: u32 = 142;

// ─── Surface flags ──────────────────────────────────────────────────────────

/// Surface can be bound to a Screen Target.
const SVGA3D_SURFACE_SCREENTARGET: u32 = 1 << 16;

// ─── Screen Target flags ────────────────────────────────────────────────────

const SVGA_STFLAG_PRIMARY: u32 = 1;

// ─── HHDM base ──────────────────────────────────────────────────────────────

const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
const PAGE_SIZE: usize = 4096;

// ─── Command buffer structures ──────────────────────────────────────────────

#[path = "command.rs"]
mod command;
use command::Header as SvgaCbHeader;
const SVGA_CB_CONTEXT_0: u32 = 0;
const SVGA_CB_CONTEXT_DEVICE: u32 = 63;

// Breenix policy: serialize HIGH/LOW register pairs and permit one outstanding
// command. State is Off -> Ready after START_STOP_CONTEXT(enable=1) completes;
// any submission error/timeout -> Failed until a fresh device boot. No command
// is retried, and a late device write cannot resurrect a failed context.
static CB_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static CB_STATE: command::ContextState = command::ContextState::new();

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

/// Submit using VMware's published SVGACBHeader and byte-address registers.
/// Caller holds CB_LOCK. All current packets fit behind a 64-byte header in
/// one page, so translating that page preserves physical contiguity. No
/// implicit HHDM subtraction, PFN conversion or cross-page assumption.
fn submit_commands_ctx(cmds: &[u8], ctx: u32) -> Result<(), &'static str> {
    if cmds.is_empty() {
        return Ok(());
    }
    let header_size = core::mem::size_of::<SvgaCbHeader>();
    if cmds.len() > PAGE_SIZE - header_size {
        return Err("SVGA batch exceeds single DMA page");
    }
    let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).map_err(|_| "SVGA DMA layout")?;
    let storage = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if storage.is_null() {
        return Err("SVGA DMA allocation failed");
    }
    let physical = crate::platform_config::kernel_va_to_ipa(storage as u64);
    let (high, low) = match command::address_words(physical, ctx) {
        Ok(words) => words,
        Err(error) => {
            unsafe {
                alloc::alloc::dealloc(storage, layout);
            }
            return Err(error);
        }
    };
    let value = match SvgaCbHeader::new(physical + header_size as u64, cmds.len()) {
        Ok(header) => header,
        Err(error) => {
            unsafe {
                alloc::alloc::dealloc(storage, layout);
            }
            return Err(error);
        }
    };
    unsafe {
        let header = storage as *mut SvgaCbHeader;
        core::ptr::write(header, value);
        core::ptr::copy_nonoverlapping(cmds.as_ptr(), storage.add(header_size), cmds.len());
        cache_clean_range(storage as u64, header_size + cmds.len());
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        // COMMAND_HIGH latches the upper byte-address bits before COMMAND_LOW
        // publishes the aligned lower address and the six-bit context selector.
        reg_write(SVGA_REG_COMMAND_HIGH, high);
        reg_write(SVGA_REG_COMMAND_LOW, low);
        let start = crate::arch_impl::aarch64::timer::rdtsc();
        let budget = crate::arch_impl::aarch64::timer::frequency_hz() * 5;
        loop {
            cache_invalidate_range(storage as u64, header_size);
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
            let status = core::ptr::read_volatile(&(*header).status);
            if status == 1 {
                // SVGACBStatus::COMPLETED, not merely nonzero.
                alloc::alloc::dealloc(storage, layout);
                return Ok(());
            }
            if status != 0 {
                // Conservatively retain the allocation for every error status,
                // including unknown and PARTIAL_COMPLETE; no documented reset
                // has returned ownership. The wrapper stops further commands.
                CB_STATE.fail();
                return Err("SVGA device rejected command buffer");
            }
            if crate::arch_impl::aarch64::timer::rdtsc().wrapping_sub(start) >= budget {
                CB_STATE.fail();
                return Err("SVGA command buffer timed out; DMA retained");
            }
            core::hint::spin_loop();
        }
    }
}

/// SVGADCCmdStartStop: device command ID, enable DWORD, context DWORD.
/// The device context carries this control message, not SVGA3D packets.
fn start_context0() -> Result<(), &'static str> {
    let _guard = CB_LOCK.lock();
    if CB_STATE.failed() {
        return Err("SVGA context failed; device reset required");
    }
    if CB_STATE.ready() {
        return Ok(());
    }
    let words = [1u32, 1, SVGA_CB_CONTEXT_0];
    let mut bytes = [0u8; 12];
    for (index, word) in words.iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    match submit_commands_ctx(&bytes, SVGA_CB_CONTEXT_DEVICE) {
        Ok(()) => {
            CB_STATE.start()?;
            Ok(())
        }
        Err(error) => {
            CB_STATE.fail();
            Err(error)
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

/// All SVGA3D packets use the explicitly started context 0. Error recovery is
/// fail-stop: preserve the failure and device-visible storage until reset.
/// A reboot, rather than an undocumented stop/restart-and-retry, reestablishes
/// object tables and backing. Normal VMware initialization is the black-box
/// check of this routing; a non-COMPLETED status remains a failure.
fn submit_commands(cmds: &[u8]) -> Result<(), &'static str> {
    let _guard = CB_LOCK.lock();
    if CB_STATE.failed() || !CB_STATE.ready() {
        return Err("SVGA submission context is unavailable");
    }
    // svga_reg.h SVGA_CMD_FENCE (30), SVGA_REG_FENCE (69). Buffer
    // consumption alone does not prove that preceding GPU work is finished.
    // Select a value different from the register's current value; the lock
    // ensures no other Breenix submission can satisfy it prematurely.
    if unsafe { reg_read(SVGA_REG_FIFO_CAPS) } & SVGA_FIFO_CAP_FENCE == 0 {
        return Err("SVGA GPU fence capability is required");
    }
    let previous = unsafe { reg_read(SVGA_REG_FENCE) };
    let next = match previous.wrapping_add(1) {
        0 => 1,
        value => value,
    };
    let mut fenced = Vec::with_capacity(cmds.len() + 8);
    fenced.extend_from_slice(cmds);
    fenced.extend_from_slice(&SVGA_CMD_FENCE.to_le_bytes());
    fenced.extend_from_slice(&next.to_le_bytes());
    let result = submit_commands_ctx(&fenced, SVGA_CB_CONTEXT_0).and_then(|()| {
        let start = crate::arch_impl::aarch64::timer::rdtsc();
        let budget = crate::arch_impl::aarch64::timer::frequency_hz() * 5;
        loop {
            if unsafe { reg_read(SVGA_REG_FENCE) } == next {
                return Ok(());
            }
            if crate::arch_impl::aarch64::timer::rdtsc().wrapping_sub(start) >= budget {
                return Err("SVGA GPU fence timed out");
            }
            core::hint::spin_loop();
        }
    });
    if result.is_err() {
        CB_STATE.fail();
        CB_STATE.stop();
        STDU_READY.store(false, Ordering::Release);
    }
    result
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

    /// DEFINE_GB_SURFACE_V2: create a guest-backed surface.
    fn define_gb_surface_v2(
        &mut self,
        sid: u32,
        surface_flags: u32,
        format: u32,
        num_mip_levels: u32,
        multisample_count: u32,
        width: u32,
        height: u32,
        depth: u32,
        array_size: u32,
        _pad: u32,
    ) {
        // Body: sid(4) + surfaceFlags(4) + format(4) + numMipLevels(4) +
        //       multisampleCount(4) + autogenFilter(4) + size(12) +
        //       arraySize(4) + pad(4) = 44
        self.cmd_header(SVGA_3D_CMD_DEFINE_GB_SURFACE_V2, 44);
        self.push_u32(sid);
        self.push_u32(surface_flags);
        self.push_u32(format);
        self.push_u32(num_mip_levels);
        self.push_u32(multisample_count);
        self.push_u32(0); // autogenFilter = SVGA3D_TEX_FILTER_NONE
        self.push_u32(width);
        self.push_u32(height);
        self.push_u32(depth);
        self.push_u32(array_size);
        self.push_u32(_pad);
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

/// Allocate page-aligned virtual storage for DMA; callers translate every
/// page independently when a device operation spans multiple pages.
/// Returns (virtual_address, physical_address, size_in_bytes).
fn alloc_dma_pages(num_pages: usize) -> Result<(u64, u64, usize), &'static str> {
    let size = num_pages * PAGE_SIZE;
    let layout = Layout::from_size_align(size, PAGE_SIZE).map_err(|_| "Invalid DMA layout")?;
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err("Failed to allocate DMA pages");
    }
    let virt = ptr as u64;
    let phys = crate::platform_config::kernel_va_to_ipa(virt);
    Ok((virt, phys, size))
}

/// Install only the entries that this backend can index. SET_OTABLE_BASE64
/// addresses the page directly (PT64_0); it does not require DEFINE_GB_MOB64.
/// validSizeInBytes=0 means no previous host table contents are being restored.
fn setup_otable(
    enc: &mut CmdEncoder,
    table_type: u32,
    entry_bytes: usize,
    highest_id: u32,
) -> Result<(), &'static str> {
    let used_bytes = tables::table_bytes(entry_bytes, highest_id)?;
    let (virtual_address, physical, _) = alloc_dma_pages(1)?;
    unsafe {
        cache_clean_range(virtual_address, PAGE_SIZE);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    enc.set_otable_base64(table_type, physical >> 12, used_bytes, 0, tables::PT64_0);
    Ok(())
}

/// Independently construct a PT64_2 MOB from each data page's actual IPA.
/// Heap virtual contiguity is not evidence of physical contiguity. Root and
/// leaf PPNs are translated separately; no RANGE fallback is guessed.
fn compositor_page_tree(buffer: u64, pages: usize) -> Result<u64, &'static str> {
    let leaves = tables::leaf_count(pages)?;
    let (root, root_physical, _) = alloc_dma_pages(1)?;
    for index in 0..leaves {
        let (leaf, leaf_physical, _) = alloc_dma_pages(1)?;
        let first = index * 512;
        for entry in 0..(pages - first).min(512) {
            let address = buffer + ((first + entry) * PAGE_SIZE) as u64;
            let physical = crate::platform_config::kernel_va_to_ipa(address);
            if physical & (PAGE_SIZE as u64 - 1) != 0 {
                return Err("SVGA backing page is unaligned");
            }
            unsafe {
                core::ptr::write((leaf as *mut u64).add(entry), (physical >> 12).to_le());
            }
        }
        unsafe {
            core::ptr::write((root as *mut u64).add(index), (leaf_physical >> 12).to_le());
            cache_clean_range(leaf, PAGE_SIZE);
        }
    }
    unsafe {
        cache_clean_range(root, PAGE_SIZE);
        cache_clean_range(buffer, pages * PAGE_SIZE);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    Ok(root_physical >> 12)
}

/// Initialize the STDU compositing pipeline.
///
/// Sets up OTables, creates a compositor surface backed by a MOB, defines a
/// screen target, and binds everything together. After this call, pixels can
/// be composited via `composite_frame()`.
///
/// Must be called AFTER `init()` and AFTER the heap is available.
pub fn init_stdu() -> Result<(), &'static str> {
    use crate::serial_println;

    if !is_initialized() {
        return Err("SVGA3 not initialized");
    }
    if !has_gbobjects() {
        return Err("SVGA3 device does not support GBObjects (needed for STDU)");
    }

    if unsafe { CAPABILITIES } & SVGA_CAP_COMMAND_BUFFERS == 0 {
        return Err("SVGA command buffers are required for STDU");
    }
    start_context0()?;

    let width = unsafe { DISPLAY_WIDTH };
    let height = unsafe { DISPLAY_HEIGHT };
    serial_println!(
        "[svga3-stdu] Initializing STDU pipeline for {}x{}",
        width,
        height
    );

    // Phase 1: Set up OTables (MOB, Surface, ScreenTarget)
    let mut enc = CmdEncoder::new();

    // Production IDs: compositor MOB/surface 1 and primary screen 0.
    // Testing adds one offscreen copy destination at MOB/surface 2. No
    // table-backing MOB IDs are consumed. Size each profile from its last ID.
    let last_mob = COMPOSITOR_MOB_ID + u32::from(cfg!(feature = "testing"));
    let last_surface = COMPOSITOR_SURFACE_ID + u32::from(cfg!(feature = "testing"));
    setup_otable(&mut enc, SVGA_OTABLE_MOB, tables::MOB_ENTRY_BYTES, last_mob)?;
    setup_otable(
        &mut enc,
        SVGA_OTABLE_SURFACE,
        tables::SURFACE_ENTRY_BYTES,
        last_surface,
    )?;
    setup_otable(
        &mut enc,
        SVGA_OTABLE_SCREENTARGET,
        tables::SCREEN_ENTRY_BYTES,
        COMPOSITOR_STID,
    )?;

    submit_commands(&enc.finish())?;
    serial_println!("[svga3-stdu] OTables initialized (MOB, Surface, ScreenTarget)");

    // Phase 2: Allocate compositor buffer and create MOB
    let buf_size = (width as usize) * (height as usize) * 4;
    let buf_pages = (buf_size + PAGE_SIZE - 1) / PAGE_SIZE;

    let (buf_virt, buf_phys, _) = alloc_dma_pages(buf_pages)?;
    let root_ppn = compositor_page_tree(buf_virt, buf_pages)?;

    // Store compositor buffer location for later use
    COMPOSITOR_BUF_VIRT.store(buf_virt, Ordering::Release);
    COMPOSITOR_BUF_SIZE.store(buf_size as u64, Ordering::Release);

    serial_println!(
        "[svga3-stdu] Compositor buffer: virt={:#x} phys={:#x} size={:#x} ({} pages)",
        buf_virt,
        buf_phys,
        buf_size,
        buf_pages
    );

    // Define the compositor MOB using its explicit two-level page tree.
    let mut enc = CmdEncoder::new();
    enc.define_gb_mob64(COMPOSITOR_MOB_ID, tables::PT64_2, root_ppn, buf_size as u32);
    submit_commands(&enc.finish())?;
    serial_println!(
        "[svga3-stdu] Compositor MOB defined (id={}, ppn={:#x})",
        COMPOSITOR_MOB_ID,
        root_ppn
    );

    // Phase 3: Create GB Surface and bind to MOB
    let mut enc = CmdEncoder::new();
    enc.define_gb_surface_v2(
        COMPOSITOR_SURFACE_ID,
        SVGA3D_SURFACE_SCREENTARGET,
        SVGA3D_B8G8R8X8_UNORM,
        1, // numMipLevels
        0, // multisampleCount
        width,
        height,
        1, // size
        0,
        0, // arraySize, pad
    );
    enc.bind_gb_surface(COMPOSITOR_SURFACE_ID, COMPOSITOR_MOB_ID);
    submit_commands(&enc.finish())?;
    serial_println!(
        "[svga3-stdu] Surface {} created and bound to MOB {}",
        COMPOSITOR_SURFACE_ID,
        COMPOSITOR_MOB_ID
    );

    // Phase 4: Define Screen Target and bind surface
    let mut enc = CmdEncoder::new();
    enc.define_gb_screentarget(COMPOSITOR_STID, width, height, SVGA_STFLAG_PRIMARY);
    enc.bind_gb_screentarget(COMPOSITOR_STID, COMPOSITOR_SURFACE_ID);
    submit_commands(&enc.finish())?;
    serial_println!(
        "[svga3-stdu] Screen target {} defined and bound to surface {}",
        COMPOSITOR_STID,
        COMPOSITOR_SURFACE_ID
    );

    // Phase 5: Initial present (display whatever is in the buffer — should be zeros/black)
    let mut enc = CmdEncoder::new();
    enc.update_gb_image(COMPOSITOR_SURFACE_ID, 0, 0, width, height);
    enc.update_gb_screentarget(COMPOSITOR_STID, 0, 0, width, height);
    submit_commands(&enc.finish())?;
    serial_println!("[svga3-stdu] Initial present complete");

    STDU_READY.store(true, Ordering::Release);
    serial_println!("[svga3-stdu] STDU compositing pipeline ready");
    Ok(())
}

/// Testing-kernel hardware check for the otherwise unused STDU API. This is
/// an upload/readback round trip, not a command-creation assertion. Wire facts:
/// svga3d_cmd.h SVGA3dCmdReadbackGBImage, DestroyGBScreenTarget,
/// DestroyGBSurface and DestroyGBMob; svga_reg.h SVGADCCmdStartStop.
#[cfg(feature = "testing")]
pub fn test_stdu_lifecycle() -> Result<(), &'static str> {
    let registers = [
        SVGA_REG_ENABLE,
        SVGA_REG_WIDTH,
        SVGA_REG_HEIGHT,
        SVGA_REG_BITS_PER_PIXEL,
        SVGA_REG_CONFIG_DONE,
        SVGA_REG_TRACES,
    ];
    let saved = registers.map(|register| unsafe { reg_read(register) });
    let saved_stride = unsafe { reg_read(SVGA_REG_BYTES_PER_LINE) };
    let saved_offset = unsafe { reg_read(SVGA_REG_FB_OFFSET) };
    init_stdu()?;
    let width = unsafe { DISPLAY_WIDTH };
    let height = unsafe { DISPLAY_HEIGHT };
    let buffer = COMPOSITOR_BUF_VIRT.load(Ordering::Acquire);
    let bytes = COMPOSITOR_BUF_SIZE.load(Ordering::Acquire) as usize;
    let pixels = bytes / 4;
    if pixels < 2 {
        return Err("SVGA test framebuffer is empty");
    }
    let first_colour = 0x00123456u32;
    let last_colour = 0x0098abcdu32;
    unsafe {
        core::ptr::write_volatile(buffer as *mut u32, first_colour);
        core::ptr::write_volatile((buffer as *mut u32).add(pixels - 1), last_colour);
        cache_clean_range(buffer, bytes);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    let mut upload = CmdEncoder::new();
    upload.update_gb_image(COMPOSITOR_SURFACE_ID, 0, 0, width, height);
    submit_commands(&upload.finish())?;
    // VMware's published interface specifies dirty-content readback:
    // https://github.com/torvalds/linux/blob/v4.19/drivers/gpu/drm/vmwgfx/device_include/svga3d_cmd.h
    // anchors SVGA3dCmdReadbackGBImage and SVGA3dCmdSurfaceCopy. Uploading
    // alone need not dirty a host image. Make a device-side copy into separate
    // poisoned backing, then read that GPU-produced destination back.
    let destination_mob = COMPOSITOR_MOB_ID + 1;
    let destination_surface = COMPOSITOR_SURFACE_ID + 1;
    let pages = bytes.div_ceil(PAGE_SIZE);
    let (destination, _, _) = alloc_dma_pages(pages)?;
    unsafe {
        core::ptr::write_bytes(destination as *mut u8, 0xa5, bytes);
    }
    let destination_root = compositor_page_tree(destination, pages)?;
    let mut copy = CmdEncoder::new();
    copy.define_gb_mob64(
        destination_mob,
        tables::PT64_2,
        destination_root,
        bytes as u32,
    );
    copy.define_gb_surface_v2(
        destination_surface,
        0,
        SVGA3D_B8G8R8X8_UNORM,
        1,
        0,
        width,
        height,
        1,
        0,
        0,
    );
    copy.bind_gb_surface(destination_surface, destination_mob);
    // SURFACE_COPY (1042): two SurfaceImageIds (24 bytes), then one
    // CopyBox (x,y,z,w,h,d,srcx,srcy,srcz: 36 bytes), total payload 60.
    copy.cmd_header(SVGA_3D_CMD_SURFACE_COPY, 60);
    for word in [
        COMPOSITOR_SURFACE_ID,
        0,
        0,
        destination_surface,
        0,
        0,
        0,
        0,
        0,
        width,
        height,
        1,
        0,
        0,
        0,
    ] {
        copy.push_u32(word);
    }
    submit_commands(&copy.finish())?;
    let mut readback = CmdEncoder::new();
    readback.cmd_header(SVGA_3D_CMD_READBACK_GB_IMAGE, 12); // READBACK_GB_IMAGE: SurfaceImageId
    readback.push_u32(destination_surface);
    readback.push_u32(0); // face
    readback.push_u32(0); // mipmap
    submit_commands(&readback.finish())?;
    unsafe {
        cache_invalidate_range(destination, bytes);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    // B8G8R8X8 defines RGB exactly; the X byte is not an alpha channel.
    // The two distant nonzero pixels exercise the first and last MOB pages.
    let exact = (0..pixels).all(|index| {
        let expected = if index == 0 {
            first_colour
        } else if index == pixels - 1 {
            last_colour
        } else {
            0
        };
        let actual = unsafe { core::ptr::read_volatile((destination as *const u32).add(index)) };
        actual & 0x00ffffff == expected
    });
    // Remove the test display and its objects before the normal VRAM console
    // resumes. Table and page-tree allocations remain device-safe for the boot.
    let mut cleanup = CmdEncoder::new();
    cleanup.cmd_header(SVGA_3D_CMD_DESTROY_GB_SCREENTARGET, 4);
    cleanup.push_u32(COMPOSITOR_STID);
    for surface in [destination_surface, COMPOSITOR_SURFACE_ID] {
        cleanup.cmd_header(SVGA_3D_CMD_DESTROY_GB_SURFACE, 4);
        cleanup.push_u32(surface);
    }
    for mob in [destination_mob, COMPOSITOR_MOB_ID] {
        cleanup.cmd_header(SVGA_3D_CMD_DESTROY_GB_MOB, 4);
        cleanup.push_u32(mob);
    }
    submit_commands(&cleanup.finish())?;
    {
        let _guard = CB_LOCK.lock();
        let stop: [u8; 12] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        submit_commands_ctx(&stop, SVGA_CB_CONTEXT_DEVICE)?;
        CB_STATE.stop();
    }
    STDU_READY.store(false, Ordering::Release);
    COMPOSITOR_BUF_VIRT.store(0, Ordering::Release);
    COMPOSITOR_BUF_SIZE.store(0, Ordering::Release);
    // The fenced MOB destruction and completed context stop returned ownership
    // of both full-screen buffers. Keep the small table/page-tree allocations
    // until device reset, but release the data pages before resuming the boot.
    unsafe {
        let layout = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE)
            .map_err(|_| "SVGA test buffer layout")?;
        alloc::alloc::dealloc(buffer as *mut u8, layout);
        alloc::alloc::dealloc(destination as *mut u8, layout);
        reg_write(SVGA_REG_ENABLE, 0);
        for (register, value) in registers.iter().zip(saved) {
            reg_write(*register, value);
        }
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        for (register, value) in registers.iter().zip(saved) {
            if reg_read(*register) != value {
                return Err("SVGA display register restore failed");
            }
        }
        if reg_read(SVGA_REG_BYTES_PER_LINE) != saved_stride
            || reg_read(SVGA_REG_FB_OFFSET) != saved_offset
        {
            return Err("SVGA framebuffer layout restore failed");
        }
    }
    if !exact {
        return Err("SVGA STDU device readback differs from uploaded image");
    }
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

/// Get the compositor buffer info for direct-mapped access (MAP_SHARED path).
///
/// Returns (virtual_ptr, width, height) of the compositor MOB backing.
/// BWM can write pixels directly to this buffer, then call `present_rect()`
/// to display them without an intermediate copy.
pub fn compositor_buffer_info() -> Option<(*mut u32, u32, u32)> {
    if !is_stdu_ready() {
        return None;
    }
    let virt = COMPOSITOR_BUF_VIRT.load(Ordering::Acquire);
    if virt == 0 {
        return None;
    }
    let w = unsafe { DISPLAY_WIDTH };
    let h = unsafe { DISPLAY_HEIGHT };
    Some((virt as *mut u32, w, h))
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
