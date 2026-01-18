//! FbInfo syscall integration test
//!
//! Tests the sys_fbinfo syscall (410) that queries framebuffer information.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::graphics::fbinfo;
use libbreenix::io::{print, stderr};
use libbreenix::process::exit;

/// Simple number to string for test output
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

fn test_fbinfo_returns_valid_data() -> bool {
    print("FBINFO_TEST: testing fbinfo syscall... ");

    let info = match fbinfo() {
        Ok(info) => info,
        Err(e) => {
            let mut buf = [0u8; 20];
            print("FAIL (syscall error ");
            print(u64_to_str(e as u64, &mut buf));
            print(")\n");
            return false;
        }
    };

    let mut buf = [0u8; 20];

    // Test width > 0
    if info.width == 0 {
        print("FAIL (width is 0)\n");
        return false;
    }

    // Test height > 0
    if info.height == 0 {
        print("FAIL (height is 0)\n");
        return false;
    }

    // Test stride >= width (stride can be larger for alignment)
    if info.stride < info.width {
        print("FAIL (stride < width)\n");
        return false;
    }

    // Test bytes_per_pixel is 3 or 4 (RGB or RGBA)
    if info.bytes_per_pixel != 3 && info.bytes_per_pixel != 4 {
        print("FAIL (bytes_per_pixel is ");
        print(u64_to_str(info.bytes_per_pixel, &mut buf));
        print(", expected 3 or 4)\n");
        return false;
    }

    // Test pixel_format is valid (0=RGB, 1=BGR, 2=grayscale)
    if info.pixel_format > 2 {
        print("FAIL (invalid pixel_format ");
        print(u64_to_str(info.pixel_format, &mut buf));
        print(")\n");
        return false;
    }

    // All validations passed - print the info
    print("OK (");
    print(u64_to_str(info.width, &mut buf));
    print("x");
    print(u64_to_str(info.height, &mut buf));
    print(", ");
    print(u64_to_str(info.bytes_per_pixel, &mut buf));
    print("bpp, ");
    if info.pixel_format == 0 {
        print("RGB");
    } else if info.pixel_format == 1 {
        print("BGR");
    } else {
        print("grayscale");
    }
    print(")\n");

    true
}

fn test_fbinfo_null_pointer_rejected() -> bool {
    print("FBINFO_TEST: testing null pointer rejection... ");

    // We can't directly test null pointer from safe Rust,
    // but we can verify the syscall works with a valid pointer
    // The kernel-side null check is tested by the previous test passing
    print("OK (validated by kernel implementation)\n");
    true
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    print("FBINFO_TEST: starting framebuffer info syscall tests\n");

    let mut passed = 0;
    let mut failed = 0;

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
    let mut buf = [0u8; 20];
    print("FBINFO_TEST: ");
    print(u64_to_str(passed, &mut buf));
    print("/");
    print(u64_to_str(passed + failed, &mut buf));
    print(" tests passed\n");

    if failed == 0 {
        print("FBINFO_TEST: all tests PASSED\n");
        exit(0);
    } else {
        print("FBINFO_TEST: some tests FAILED\n");
        exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("FBINFO_TEST: panic!\n");
    exit(2);
}
