//! UEFI GOP (Graphics Output Protocol) framebuffer discovery.
//!
//! Queries UEFI boot services for a GOP framebuffer and populates the
//! HardwareConfig with resolution, stride, pixel format, and base address.
//! Enumerates available modes and selects the highest resolution.
//! Must be called before exit_boot_services().

use crate::hw_config::HardwareConfig;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};

// =============================================================================
// Raw UART output (bypasses UEFI ConOut, goes directly to serial)
// =============================================================================

/// Writer that sends bytes directly to the PL011 UART data register.
/// Used to log GOP mode info to serial, since UEFI ConOut goes to the
/// display (which gets reset when the kernel takes over).
struct UartWriter(u64);

impl UartWriter {
    fn putc(&self, c: u8) {
        if self.0 == 0 {
            return;
        }
        unsafe {
            let dr = self.0 as *mut u32;
            core::ptr::write_volatile(dr, c as u32);
        }
    }
}

impl core::fmt::Write for UartWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.bytes() {
            if c == b'\n' {
                self.putc(b'\r');
            }
            self.putc(c);
        }
        Ok(())
    }
}

/// Discover GOP framebuffer and populate HardwareConfig.
///
/// Enumerates all available GOP modes and selects the highest resolution
/// that uses a supported pixel format (RGB or BGR, not BltOnly).
/// On Parallels Desktop, GOP provides a linear framebuffer that the
/// hypervisor composites to the VM window.
///
/// Returns Ok(()) if a framebuffer was found, Err if GOP is not available.
/// Failure is non-fatal — the kernel will boot without display output.
pub fn discover_gop(config: &mut HardwareConfig) -> Result<(), &'static str> {
    use core::fmt::Write;

    // Raw UART writer for serial output (UEFI log goes to display, not serial)
    let mut uart = UartWriter(config.uart_base_phys);

    // Find the GOP handle
    let handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>()
        .map_err(|_| "No GOP handle found")?;

    // Open the protocol
    let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle)
        .map_err(|_| "Failed to open GOP protocol")?;

    // Enumerate all available modes and find the best one.
    // "Best" = highest pixel count with a supported pixel format.
    let mut best_mode: Option<uefi::proto::console::gop::Mode> = None;
    let mut best_pixels: usize = 0;
    let mut mode_count: usize = 0;

    log::info!("[gop] Enumerating display modes...");
    let _ = write!(uart, "[gop-serial] Enumerating GOP display modes:\n");

    for mode in gop.modes() {
        let info = mode.info();
        let (w, h) = info.resolution();
        let pf = info.pixel_format();
        let pixels = w * h;

        log::info!("[gop]   Mode {}: {}x{} format={:?}", mode_count, w, h, pf);
        let _ = write!(uart, "[gop-serial]   Mode {}: {}x{} format={}\n",
            mode_count, w, h,
            match pf {
                PixelFormat::Rgb => "RGB",
                PixelFormat::Bgr => "BGR",
                PixelFormat::Bitmask => "Bitmask",
                PixelFormat::BltOnly => "BltOnly",
            });

        mode_count += 1;

        // Skip BltOnly modes — we need a linear framebuffer
        if pf == PixelFormat::BltOnly {
            continue;
        }

        if pixels > best_pixels {
            best_pixels = pixels;
            best_mode = Some(mode);
        }
    }

    let _ = write!(uart, "[gop-serial] {} modes total, best={} pixels\n", mode_count, best_pixels);

    // Set the best mode if it's different from the current one
    if let Some(ref mode) = best_mode {
        let info = mode.info();
        let (bw, bh) = info.resolution();
        let (cw, ch) = gop.current_mode_info().resolution();

        if bw != cw || bh != ch {
            log::info!("[gop] Switching from {}x{} to {}x{}", cw, ch, bw, bh);
            let _ = write!(uart, "[gop-serial] Switching from {}x{} to {}x{}\n", cw, ch, bw, bh);
            gop.set_mode(mode).map_err(|_| "Failed to set GOP mode")?;
            log::info!("[gop] Mode set successfully");
            let _ = write!(uart, "[gop-serial] Mode switch successful\n");
        } else {
            log::info!("[gop] Already at best mode: {}x{}", cw, ch);
            let _ = write!(uart, "[gop-serial] Already at best mode: {}x{}\n", cw, ch);
        }
    } else {
        let _ = write!(uart, "[gop-serial] WARNING: No usable GOP mode found!\n");
    }

    // Read the (potentially updated) mode info
    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride();
    let pixel_format = mode_info.pixel_format();

    // Get framebuffer base address and size
    let mut fb = gop.frame_buffer();
    let fb_base = fb.as_mut_ptr() as u64;
    let fb_size = fb.size() as u64;

    log::info!(
        "[gop] Framebuffer: {}x{} stride={} format={:?} base={:#x} size={:#x}",
        width, height, stride, pixel_format, fb_base, fb_size,
    );
    let _ = write!(uart, "[gop-serial] Selected: {}x{} stride={} base={:#x} size={:#x}\n",
        width, height, stride, fb_base, fb_size);

    // Map UEFI pixel format to our convention: 0 = RGB, 1 = BGR
    let pf = match pixel_format {
        PixelFormat::Bgr => 1,
        PixelFormat::Rgb => 0,
        // Bitmask and BltOnly are uncommon; treat as RGB and hope for the best
        _ => 0,
    };

    // Populate config
    config.has_framebuffer = 1;
    config.framebuffer.base = fb_base;
    config.framebuffer.size = fb_size;
    config.framebuffer.width = width as u32;
    config.framebuffer.height = height as u32;
    config.framebuffer.stride = stride as u32;
    config.framebuffer.pixel_format = pf;

    Ok(())
}
