//! VMware SVGA3 virtual GPU driver (PCI device 15ad:0406).
//!
//! SVGA3 is VMware's ARM64-native virtual GPU, replacing the x86-only SVGA2
//! (0x0405). Key differences from SVGA2:
//!   - Registers accessed via MMIO BAR (not I/O ports)
//!   - MSI-X interrupt support
//!   - Legacy FIFO deprecated; command buffers used instead
//!   - Supports OpenGL 4.3 / ES 3.1 via SVGA3D command set
//!
//! Command buffer submission:
//!   1. Allocate 64-byte-aligned buffer in guest RAM
//!   2. Write SVGACBHeader (56 bytes) at start
//!   3. Fill SVGA commands after the header
//!   4. Set header.ptr.pa = physical address of command data
//!   5. Write header phys high to SVGA_REG_COMMAND_HIGH
//!   6. Write (header phys low | context) to SVGA_REG_COMMAND_LOW
//!   7. Poll header.status until != NONE
//!
//! Reference: Linux kernel drivers/gpu/drm/vmwgfx/ and device_include/svga_reg.h

extern crate alloc;

use alloc::alloc::Layout;
use core::sync::atomic::{AtomicBool, Ordering};

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

// ─── SVGA FIFO/command IDs ──────────────────────────────────────────────────

// Command IDs for future command buffer submission (currently using VRAM traces)
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

// ─── Command buffer structures ──────────────────────────────────────────────
// These are used for SVGA3D command submission. Currently unused because
// ARM64 cache coherency for DMA needs fixing. VRAM traces are used instead.

/// Command buffer status codes (volatile, written by device)
#[allow(dead_code)]
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CbStatus {
    None = 0,
    Completed = 1,
    QueueFull = 2,
    CommandError = 3,
    HeaderError = 4,
    Preempted = 5,
    SubmissionError = 6,
}

/// Command buffer context IDs (encoded in lower 6 bits of COMMAND_LOW)
#[allow(dead_code)]
const SVGA_CB_CONTEXT_0: u32 = 0x0;

/// SVGACBHeader — 56 bytes, must be 64-byte aligned in memory.
///
/// Layout matches Linux kernel's svga_reg.h exactly.
/// All fields are naturally aligned (no padding needed), so repr(C) gives
/// correct layout without repr(packed). The u64 fields at offsets 0x08 and
/// 0x18 are 8-byte aligned within the 64-byte-aligned allocation.
#[allow(dead_code)]
#[repr(C)]
struct SvgaCbHeader {
    status: u32,          // 0x00: volatile, written by device
    error_offset: u32,    // 0x04: offset of error in command data
    id: u64,              // 0x08: identifier (for driver tracking)
    flags: u32,           // 0x10: SVGACBFlags
    length: u32,          // 0x14: length of command data in bytes
    pa: u64,              // 0x18: physical address of command data
    offset: u32,          // 0x20: first valid byte (0 unless prepending)
    dx_context: u32,      // 0x24: DX context (0 if not DX)
    must_be_zero: [u32; 6], // 0x28–0x3F: reserved, must be zero
}

// ─── Driver state ───────────────────────────────────────────────────────────

static INITIALIZED: AtomicBool = AtomicBool::new(false);

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

#[allow(dead_code)]
/// Submit a command buffer to the SVGA3 device.
///
/// `cmds` is a slice of raw command bytes. This function:
/// 1. Allocates a 64-byte-aligned buffer for header + commands
/// 2. Copies commands after the header
/// 3. Submits via SVGA_REG_COMMAND_LOW/HIGH
/// 4. Polls until completion
///
/// Returns Ok(()) on success, Err on submission error.
fn submit_commands(cmds: &[u8]) -> Result<(), &'static str> {
    use crate::serial_println;

    if cmds.is_empty() {
        return Ok(());
    }

    // Header is 56 bytes, but must be 64-byte aligned. We place command data
    // right after the header at offset 64 (next 64-byte boundary).
    const HEADER_SIZE: usize = 64; // padded to alignment
    let total_size = HEADER_SIZE + cmds.len();

    // Allocate 64-byte aligned buffer
    let layout = Layout::from_size_align(total_size, 64)
        .map_err(|_| "Invalid layout for command buffer")?;
    let buf_ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if buf_ptr.is_null() {
        return Err("Failed to allocate command buffer");
    }

    // We need physical address. Since heap is identity-mapped via HHDM,
    // phys = virt - HHDM_BASE.
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let buf_virt = buf_ptr as u64;
    let buf_phys = buf_virt - HHDM_BASE;

    // Fill the header
    let header = buf_ptr as *mut SvgaCbHeader;
    let cmd_phys = buf_phys + HEADER_SIZE as u64;

    unsafe {
        (*header).status = CbStatus::None as u32;
        (*header).error_offset = 0;
        (*header).id = 0;
        (*header).flags = 0; // SVGA_CB_FLAG_NONE
        (*header).length = cmds.len() as u32;
        (*header).pa = cmd_phys;
        (*header).offset = 0;
        (*header).dx_context = 0;
        (*header).must_be_zero = [0; 6];

        // Copy command data after header
        let cmd_dst = buf_ptr.add(HEADER_SIZE);
        core::ptr::copy_nonoverlapping(cmds.as_ptr(), cmd_dst, cmds.len());

        // Ensure writes are visible before submitting to device
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));

        // Submit: write physical address of header to device registers.
        // Lower 6 bits of COMMAND_LOW encode the context ID (header is 64-byte
        // aligned so those bits are zero — we OR in the context).
        let header_phys_low = (buf_phys & 0xFFFF_FFFF) as u32;
        let header_phys_high = (buf_phys >> 32) as u32;

        reg_write(SVGA_REG_COMMAND_HIGH, header_phys_high);
        // Writing COMMAND_LOW triggers submission
        reg_write(SVGA_REG_COMMAND_LOW, header_phys_low | SVGA_CB_CONTEXT_0);

        // Poll for completion
        let mut spins = 0u32;
        loop {
            let status = core::ptr::read_volatile(&(*header).status);
            if status != CbStatus::None as u32 {
                if status == CbStatus::Completed as u32 {
                    break;
                }
                let err_off = core::ptr::read_volatile(&(*header).error_offset);
                serial_println!("[svga3] Command buffer error: status={} error_offset={}",
                    status, err_off);
                alloc::alloc::dealloc(buf_ptr, layout);
                return Err("Command buffer error");
            }
            spins += 1;
            if spins > 10_000_000 {
                serial_println!("[svga3] Command buffer timeout (status still NONE)");
                alloc::alloc::dealloc(buf_ptr, layout);
                return Err("Command buffer timeout");
            }
            core::hint::spin_loop();
        }

        alloc::alloc::dealloc(buf_ptr, layout);
    }

    Ok(())
}

// ─── Command encoding helpers ───────────────────────────────────────────────

#[allow(dead_code)]
/// Encode an SVGA_CMD_UPDATE command (tell device a screen region changed).
fn encode_cmd_update(buf: &mut [u8], offset: &mut usize, x: u32, y: u32, w: u32, h: u32) {
    let o = *offset;
    let bytes = buf;
    bytes[o..o+4].copy_from_slice(&SVGA_CMD_UPDATE.to_le_bytes());
    bytes[o+4..o+8].copy_from_slice(&x.to_le_bytes());
    bytes[o+8..o+12].copy_from_slice(&y.to_le_bytes());
    bytes[o+12..o+16].copy_from_slice(&w.to_le_bytes());
    bytes[o+16..o+20].copy_from_slice(&h.to_le_bytes());
    *offset += 20;
}

// ─── Initialization ─────────────────────────────────────────────────────────

/// Initialize the SVGA3 driver.
pub fn init() -> Result<(), &'static str> {
    use crate::serial_println;

    let dev = crate::drivers::pci::find_device(VMWARE_VENDOR_ID, SVGA3_DEVICE_ID)
        .ok_or("SVGA3 device not found")?;

    serial_println!("[svga3] Found VMware SVGA3 at {:02x}:{:02x}.{}",
        dev.bus, dev.device, dev.function);

    // BAR0 = MMIO register space
    let bar0 = &dev.bars[0];
    if !bar0.is_valid() || bar0.is_io {
        return Err("SVGA3 BAR0 is not a valid MMIO BAR");
    }
    let rmmio_phys = bar0.address;

    // BAR2 = VRAM (framebuffer)
    let bar2 = &dev.bars[2];
    if !bar2.is_valid() || bar2.is_io {
        return Err("SVGA3 BAR2 (VRAM) is not a valid MMIO BAR");
    }
    let vram_phys = bar2.address;
    let vram_size = bar2.size;

    serial_println!("[svga3] BAR0 (regs): phys={:#x} size={:#x}", rmmio_phys, bar0.size);
    serial_println!("[svga3] BAR2 (VRAM): phys={:#x} size={:#x}", vram_phys, vram_size);

    // Map via HHDM
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let rmmio_virt = HHDM_BASE + rmmio_phys;
    let vram_virt = HHDM_BASE + vram_phys;

    unsafe {
        RMMIO_BASE = rmmio_virt;
        VRAM_BASE = vram_virt;
        VRAM_PHYS = vram_phys;
        VRAM_SIZE = vram_size;
    }

    // Negotiate device version
    unsafe {
        reg_write(SVGA_REG_ID, SVGA_ID_2);
        let id = reg_read(SVGA_REG_ID);
        serial_println!("[svga3] Device version: {:#x}", id);
    }

    // Read capabilities
    let caps = unsafe { reg_read(SVGA_REG_CAPABILITIES) };
    unsafe { CAPABILITIES = caps; }

    serial_println!("[svga3] Capabilities: {:#010x}", caps);
    serial_println!("[svga3]   3D={} CmdBuf={} CmdBuf2={} GB={} DX={} IRQ={} Cursor={}",
        if caps & SVGA_CAP_3D != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_COMMAND_BUFFERS != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_CMD_BUFFERS_2 != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_GBOBJECTS != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_DX != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_IRQMASK != 0 { "Y" } else { "n" },
        if caps & SVGA_CAP_CURSOR != 0 { "Y" } else { "n" });

    if caps & SVGA_CAP_COMMAND_BUFFERS == 0 {
        return Err("SVGA3 device does not support command buffers");
    }

    // Use the resolution already set by the UEFI GOP driver. Overriding it
    // would desync the SVGA scanout stride from the kernel framebuffer stride
    // (GOP configures both the SVGA mode and the kernel's arm64-fb metadata).
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

    serial_println!("[svga3] Display: {}x{} @ {}bpp, stride={} bytes",
        width, height, bpp, bpl);
    serial_println!("[svga3] FB offset={:#x} size={:#x} mem_size={:#x}",
        fb_offset, fb_size, mem_size);

    // Enable the device and VRAM tracing
    unsafe {
        reg_write(SVGA_REG_ENABLE, 1);
        // Enable VRAM traces — the device auto-detects VRAM writes and
        // redraws modified regions without explicit UPDATE commands.
        reg_write(SVGA_REG_TRACES, 1);
        reg_write(SVGA_REG_CONFIG_DONE, 1);
    }

    let traces_val = unsafe { reg_read(SVGA_REG_TRACES) };
    serial_println!("[svga3] VRAM traces: {}", if traces_val != 0 { "enabled" } else { "disabled" });

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

/// Get current display dimensions.
pub fn dimensions() -> Option<(u32, u32)> {
    if !is_initialized() { return None; }
    unsafe { Some((DISPLAY_WIDTH, DISPLAY_HEIGHT)) }
}

/// Synchronize — wait for all pending SVGA commands to complete.
pub fn sync() {
    if !is_initialized() { return; }
    unsafe {
        reg_write(SVGA_REG_SYNC, 1);
        while reg_read(SVGA_REG_BUSY) != 0 {
            core::hint::spin_loop();
        }
    }
}

// ─── Drawing ────────────────────────────────────────────────────────────────

/// Fill VRAM with a solid color.
///
/// Writes pixels directly to BAR2 (VRAM). With SVGA_REG_TRACES enabled,
/// the device automatically detects modified regions and updates the display.
fn fill_rect_vram(x: u32, y: u32, w: u32, h: u32, r: u8, g: u8, b: u8) {
    let width = unsafe { DISPLAY_WIDTH };
    let height = unsafe { DISPLAY_HEIGHT };
    let bpl = unsafe { BYTES_PER_LINE } as usize;
    let vram = unsafe { VRAM_BASE as *mut u8 };

    // Clamp to screen bounds
    let x2 = (x + w).min(width);
    let y2 = (y + h).min(height);

    // Write BGRA pixels to VRAM (VMware uses BGRX format at 32bpp)
    // VRAM is in device memory (BAR2), so writes are uncached — no
    // cache maintenance needed.
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

    // DSB ensures all VRAM writes are committed before any further operations
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }
}

/// Draw a test pattern to prove the SVGA3 driver works.
///
/// Fills the screen with colored rectangles and issues UPDATE commands.
pub fn draw_test_pattern() -> Result<(), &'static str> {
    use crate::serial_println;

    if !is_initialized() {
        return Err("SVGA3 not initialized");
    }

    let w = unsafe { DISPLAY_WIDTH };
    let h = unsafe { DISPLAY_HEIGHT };

    serial_println!("[svga3] Drawing test pattern on {}x{} display...", w, h);

    // Dark blue background
    fill_rect_vram(0, 0, w, h, 20, 30, 50);

    // Colored rectangles
    let rects: [(u32, u32, u32, u32, u8, u8, u8); 6] = [
        // x, y, w, h, r, g, b
        (50,  50,  200, 150, 220, 40,  40),   // Red
        (300, 80,  200, 150, 40,  200, 40),   // Green
        (550, 110, 200, 150, 40,  80,  220),  // Blue
        (100, 300, 250, 120, 220, 220, 40),   // Yellow
        (400, 350, 200, 130, 200, 50,  200),  // Magenta
        (700, 280, 180, 160, 40,  200, 200),  // Cyan
    ];

    for &(x, y, rw, rh, r, g, b) in &rects {
        fill_rect_vram(x, y, rw, rh, r, g, b);
    }

    // Title bar area at top
    fill_rect_vram(0, 0, w, 30, 60, 80, 120);

    serial_println!("[svga3] Test pattern drawn successfully");
    Ok(())
}
