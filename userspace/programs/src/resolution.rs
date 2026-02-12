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

use libbreenix::graphics;

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
