//! resolution - display framebuffer resolution
//!
//! Usage: resolution
//!
//! Queries the kernel framebuffer and displays:
//! - Resolution (width x height)
//! - Stride (pixels per scanline)
//! - Bytes per pixel
//! - Pixel format (RGB/BGR)
//! - Framebuffer size in KB

#[cfg(target_arch = "x86_64")]
use libbreenix::graphics;

/// x86-64: the firmware framebuffer's geometry.
#[cfg(target_arch = "x86_64")]
fn main() {
    match graphics::fbinfo() {
        Ok(info) => {
            println!("Resolution: {}x{}", info.width, info.height);
            println!("Stride: {} pixels per scanline", info.stride);
            println!("Bytes per pixel: {}", info.bytes_per_pixel);

            let format_str = match info.pixel_format {
                0 => "RGB".to_string(),
                1 => "BGR".to_string(),
                2 => "Grayscale".to_string(),
                other => format!("Unknown ({})", other),
            };
            println!("Pixel format: {}", format_str);

            let fb_size = info.stride * info.height * info.bytes_per_pixel;
            println!("Framebuffer size: {} KB", fb_size / 1024);

            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("resolution: error getting framebuffer info: {}", e);
            std::process::exit(1);
        }
    }
}

/// ARM64: the GPU core's current display mode (MODES), whose scanout targets are BGRX, four
/// bytes a pixel, rows of the mode's width.
#[cfg(target_arch = "aarch64")]
fn main() {
    let (r, modes) = libbreenix::gpu::modes();
    let mode = (r == 0).then(|| modes.modes.get(modes.current as usize).copied()).flatten();
    match mode {
        Some(mode) => {
            println!("Resolution: {}x{}", mode.width, mode.height);
            println!("Stride: {} pixels per scanline", mode.width);
            println!("Bytes per pixel: 4");
            println!("Pixel format: BGR");
            println!("UI scale: {}", mode.scale);
            println!("Framebuffer size: {} KB", mode.width as u64 * mode.height as u64 * 4 / 1024);
            std::process::exit(0);
        }
        None => {
            eprintln!("resolution: error getting the GPU display mode: errno {}", -r);
            std::process::exit(1);
        }
    }
}
