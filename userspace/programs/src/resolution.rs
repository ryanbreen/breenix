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

/// Display: the GPU core's current display mode (MODES), whose scanout targets are BGRX, four
/// bytes a pixel, rows of the mode's width.
fn main() {
    #[cfg(target_arch = "x86_64")]
    {
        let (status, caps) = libbreenix::gpu::caps();
        if status != 0 || caps.status != libbreenix::gpu::abi::status::ADMITTED {
            eprintln!("resolution: GPU admission failed: result {status}, status {}", caps.status);
            std::process::exit(1);
        }
    }
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
