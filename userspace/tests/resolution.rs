//! resolution - display framebuffer resolution
//!
//! Usage: resolution
//!
//! Queries the kernel framebuffer and displays:
//! - Resolution (width x height)
//! - Stride (pixels per scanline)
//! - Bytes per pixel
//! - Pixel format (RGB/BGR)

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::graphics::{fbinfo, FbInfo};
use libbreenix::io::{print, stderr};
use libbreenix::process::exit;

/// Simple number to string conversion for u64
fn u64_to_str(mut n: u64, buf: &mut [u8]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }

    let mut i = buf.len();
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i..]) }
}

fn print_info(info: &FbInfo) {
    let mut buf = [0u8; 20];

    // Resolution
    print("Resolution: ");
    print(u64_to_str(info.width, &mut buf));
    print("x");
    print(u64_to_str(info.height, &mut buf));
    print("\n");

    // Stride
    print("Stride: ");
    print(u64_to_str(info.stride, &mut buf));
    print(" pixels per scanline\n");

    // Bytes per pixel
    print("Bytes per pixel: ");
    print(u64_to_str(info.bytes_per_pixel, &mut buf));
    print("\n");

    // Pixel format
    print("Pixel format: ");
    if info.is_bgr() {
        print("BGR");
    } else if info.is_rgb() {
        print("RGB");
    } else if info.is_grayscale() {
        print("Grayscale");
    } else {
        print("Unknown (");
        print(u64_to_str(info.pixel_format, &mut buf));
        print(")");
    }
    print("\n");

    // Total framebuffer size
    let fb_size = info.stride * info.height * info.bytes_per_pixel;
    print("Framebuffer size: ");
    print(u64_to_str(fb_size / 1024, &mut buf));
    print(" KB\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    match fbinfo() {
        Ok(info) => {
            print_info(&info);
            exit(0);
        }
        Err(e) => {
            let mut buf = [0u8; 20];
            let _ = stderr().write_str("resolution: error getting framebuffer info: ");
            let _ = stderr().write_str(u64_to_str(e as u64, &mut buf));
            let _ = stderr().write_str("\n");
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("resolution: panic!\n");
    exit(2);
}
