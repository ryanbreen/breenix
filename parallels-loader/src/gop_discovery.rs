//! UEFI GOP (Graphics Output Protocol) framebuffer discovery.
//!
//! Queries UEFI boot services for a GOP framebuffer and populates the
//! HardwareConfig with resolution, stride, pixel format, and base address.
//! Must be called before exit_boot_services().

use crate::hw_config::HardwareConfig;
use uefi::proto::console::gop::GraphicsOutput;

/// Discover GOP framebuffer and populate HardwareConfig.
///
/// This queries UEFI's GraphicsOutput protocol for the current display mode.
/// On Parallels Desktop, GOP provides a linear framebuffer that the hypervisor
/// composites to the VM window.
///
/// Returns Ok(()) if a framebuffer was found, Err if GOP is not available.
/// Failure is non-fatal — the kernel will boot without display output.
pub fn discover_gop(config: &mut HardwareConfig) -> Result<(), &'static str> {
    // Find the GOP handle
    let handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>()
        .map_err(|_| "No GOP handle found")?;

    // Open the protocol
    let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle)
        .map_err(|_| "Failed to open GOP protocol")?;

    // Read current mode info
    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride();
    let pixel_format = mode_info.pixel_format();

    // Get framebuffer base address and size
    let mut fb = gop.frame_buffer();
    let fb_base = fb.as_mut_ptr() as u64;
    let fb_size = fb.size() as u64;

    // Map UEFI pixel format to our convention: 0 = RGB, 1 = BGR
    let pf = match pixel_format {
        uefi::proto::console::gop::PixelFormat::Bgr => 1,
        uefi::proto::console::gop::PixelFormat::Rgb => 0,
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
