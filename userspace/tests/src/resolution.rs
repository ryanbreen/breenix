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

/// Framebuffer information structure.
/// Must match kernel's FbInfo layout (all u64 fields, C repr).
#[repr(C)]
struct FbInfo {
    width: u64,
    height: u64,
    stride: u64,
    bytes_per_pixel: u64,
    pixel_format: u64,
}

/// Call the Breenix FBINFO syscall (number 410) via inline assembly.
/// This syscall is not exposed through libc, so we invoke it directly.
fn fbinfo() -> Result<FbInfo, i64> {
    let mut info = FbInfo {
        width: 0,
        height: 0,
        stride: 0,
        bytes_per_pixel: 0,
        pixel_format: 0,
    };

    let result: i64;

    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") 410u64,
            in("rdi") &mut info as *mut FbInfo as u64,
            lateout("rax") result,
            options(nostack, preserves_flags),
        );
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") 410u64,
            inlateout("x0") &mut info as *mut FbInfo as u64 => result,
            options(nostack),
        );
    }

    if result < 0 {
        Err(-result)
    } else {
        Ok(info)
    }
}

fn print_info(info: &FbInfo) {
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
}

fn main() {
    match fbinfo() {
        Ok(info) => {
            print_info(&info);
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("resolution: error getting framebuffer info: {}", e);
            std::process::exit(1);
        }
    }
}
