//! FbInfo syscall integration test (std version)
//!
//! Tests the sys_fbinfo syscall (410) that queries framebuffer information.

use std::process;

#[cfg(target_arch = "x86_64")]
use libbreenix::graphics;

#[cfg(target_arch = "x86_64")]
fn test_fbinfo_returns_valid_data() -> bool {
    print!("FBINFO_TEST: testing fbinfo syscall... ");

    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(e) => {
            print!("FAIL (syscall error {})\n", e);
            return false;
        }
    };

    // Test width > 0
    if info.width == 0 {
        print!("FAIL (width is 0)\n");
        return false;
    }

    // Test height > 0
    if info.height == 0 {
        print!("FAIL (height is 0)\n");
        return false;
    }

    // Test stride >= width (stride can be larger for alignment)
    if info.stride < info.width {
        print!("FAIL (stride < width)\n");
        return false;
    }

    // Test bytes_per_pixel is 3 or 4 (RGB or RGBA)
    if info.bytes_per_pixel != 3 && info.bytes_per_pixel != 4 {
        print!("FAIL (bytes_per_pixel is {}, expected 3 or 4)\n", info.bytes_per_pixel);
        return false;
    }

    // Test pixel_format is valid (0=RGB, 1=BGR, 2=grayscale)
    if info.pixel_format > 2 {
        print!("FAIL (invalid pixel_format {})\n", info.pixel_format);
        return false;
    }

    // All validations passed - print the info
    let format_str = match info.pixel_format {
        0 => "RGB",
        1 => "BGR",
        _ => "grayscale",
    };
    print!("OK ({}x{}, {}bpp, {})\n", info.width, info.height, info.bytes_per_pixel, format_str);

    true
}

#[cfg(target_arch = "x86_64")]
fn test_fbinfo_null_pointer_rejected() -> bool {
    print!("FBINFO_TEST: testing null pointer rejection... ");

    let ret = unsafe { libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::FBINFO, 0) } as i64;
    println!("{}", if ret == -14 { "OK" } else { "FAIL" });
    ret == -14
}

fn main() {
    print!("FBINFO_TEST: starting framebuffer info syscall tests\n");

    let mut passed: u64 = 0;
    let mut failed: u64 = 0;

    if test_fbinfo_returns_valid_data() {
        passed += 1;
    } else {
        failed += 1;
    }

    if test_fbinfo_null_pointer_rejected() {
        passed += 1;
    } else {
        failed += 1;
    }

    // Print summary
    print!("FBINFO_TEST: {}/{} tests passed\n", passed, passed + failed);

    if failed == 0 {
        print!("FBINFO_TEST: all tests PASSED\n");
        process::exit(0);
    } else {
        print!("FBINFO_TEST: some tests FAILED\n");
        process::exit(1);
    }
}

#[cfg(target_arch = "aarch64")]
fn test_fbinfo_returns_valid_data() -> bool {
    let (ret, modes) = libbreenix::gpu::modes();
    let valid = ret == 0 && modes.count > 0 && modes.count as usize <= modes.modes.len()
        && (modes.current as usize) < modes.count as usize
        && modes.modes[..modes.count as usize].iter().all(|m| m.width > 0 && m.height > 0 && m.scale > 0);
    println!("FBINFO_TEST: GPU display modes {}", if valid { "OK" } else { "FAIL" });
    valid
}

#[cfg(target_arch = "aarch64")]
fn test_fbinfo_null_pointer_rejected() -> bool {
    let ret = unsafe { libbreenix::gpu::call(libbreenix::gpu::abi::op::MODES, core::ptr::null_mut(), core::mem::size_of::<libbreenix::gpu::abi::Modes>()) };
    println!("FBINFO_TEST: GPU null output {}", if ret == -14 { "OK" } else { "FAIL" });
    ret == -14
}
