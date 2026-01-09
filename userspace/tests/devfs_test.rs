//! devfs test - tests /dev/null, /dev/zero, /dev/console
//!
//! Tests:
//! - /dev/null: write succeeds, read returns EOF
//! - /dev/zero: read returns zeros
//! - /dev/console: write outputs to serial
//!
//! Emits DEVFS_TEST_PASSED on success

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs::{open, O_RDONLY, O_RDWR, O_WRONLY};
use libbreenix::io::{close, print, println, read, write};
use libbreenix::process::exit;

fn fail(msg: &str) -> ! {
    println(msg);
    exit(1);
}

fn print_num(n: i64) {
    if n < 0 {
        print("-");
        print_num(-n);
        return;
    }
    if n >= 10 {
        print_num(n / 10);
    }
    let digit = (n % 10) as u8 + b'0';
    let s = [digit];
    print(unsafe { core::str::from_utf8_unchecked(&s) });
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("devfs test starting...");

    // Test 1: Open and write to /dev/null
    println("\nTest 1: /dev/null write");
    let fd = match open("/dev/null\0", O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            print("FAILED: open(/dev/null) failed with error: ");
            print_num(e as i64);
            print("\n");
            exit(1);
        }
    };

    let test_data = b"This data goes to /dev/null";
    let result = write(fd, test_data);
    if result < 0 {
        print("FAILED: write to /dev/null failed with error: ");
        print_num(result);
        print("\n");
        exit(1);
    }
    let n = result as usize;
    print("  Wrote ");
    print_num(n as i64);
    print(" bytes to /dev/null\n");
    if n != test_data.len() {
        fail("FAILED: write returned wrong byte count");
    }
    close(fd);
    println("  /dev/null write test passed");

    // Test 2: Read from /dev/null returns EOF (0 bytes)
    println("\nTest 2: /dev/null read (should return EOF)");
    let fd = match open("/dev/null\0", O_RDONLY) {
        Ok(fd) => fd,
        Err(e) => {
            print("FAILED: open(/dev/null) for read failed: ");
            print_num(e as i64);
            print("\n");
            exit(1);
        }
    };

    let mut buf = [0u8; 64];
    let result = read(fd, &mut buf);
    if result < 0 {
        print("FAILED: read from /dev/null failed: ");
        print_num(result);
        print("\n");
        exit(1);
    }
    let n = result as usize;
    if n != 0 {
        print("FAILED: read from /dev/null returned ");
        print_num(n as i64);
        print(" bytes, expected 0 (EOF)\n");
        exit(1);
    }
    println("  Read returned 0 bytes (EOF) as expected");
    close(fd);
    println("  /dev/null read test passed");

    // Test 3: Read from /dev/zero returns zeros
    println("\nTest 3: /dev/zero read (should return zeros)");
    let fd = match open("/dev/zero\0", O_RDONLY) {
        Ok(fd) => fd,
        Err(e) => {
            print("FAILED: open(/dev/zero) failed: ");
            print_num(e as i64);
            print("\n");
            exit(1);
        }
    };

    // Initialize with non-zero values to ensure they get overwritten
    let mut buf = [0xFFu8; 32];
    let result = read(fd, &mut buf);
    if result < 0 {
        print("FAILED: read from /dev/zero failed: ");
        print_num(result);
        print("\n");
        exit(1);
    }
    let n = result as usize;
    print("  Read ");
    print_num(n as i64);
    print(" bytes from /dev/zero\n");
    if n != buf.len() {
        print("FAILED: read returned ");
        print_num(n as i64);
        print(" bytes, expected ");
        print_num(buf.len() as i64);
        print("\n");
        exit(1);
    }
    // Verify all bytes are zero
    for (i, &byte) in buf.iter().enumerate() {
        if byte != 0 {
            print("FAILED: byte ");
            print_num(i as i64);
            print(" is ");
            print_num(byte as i64);
            print(", expected 0\n");
            exit(1);
        }
    }
    println("  All bytes are zero as expected");
    close(fd);
    println("  /dev/zero read test passed");

    // Test 4: Write to /dev/zero (should succeed, discarding data)
    println("\nTest 4: /dev/zero write (should discard data)");
    let fd = match open("/dev/zero\0", O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            print("FAILED: open(/dev/zero) for write failed: ");
            print_num(e as i64);
            print("\n");
            exit(1);
        }
    };

    let test_data = b"Data written to /dev/zero";
    let result = write(fd, test_data);
    if result < 0 {
        print("FAILED: write to /dev/zero failed: ");
        print_num(result);
        print("\n");
        exit(1);
    }
    let n = result as usize;
    if n != test_data.len() {
        fail("FAILED: write to /dev/zero returned wrong count");
    }
    println("  /dev/zero write succeeded (data discarded)");
    close(fd);

    // Test 5: Write to /dev/console
    println("\nTest 5: /dev/console write");
    let fd = match open("/dev/console\0", O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            print("FAILED: open(/dev/console) failed: ");
            print_num(e as i64);
            print("\n");
            exit(1);
        }
    };

    let console_msg = b"  [direct console write via /dev/console]\n";
    let result = write(fd, console_msg);
    if result < 0 {
        print("FAILED: write to /dev/console failed: ");
        print_num(result);
        print("\n");
        exit(1);
    }
    let n = result as usize;
    if n != console_msg.len() {
        fail("FAILED: write to /dev/console returned wrong count");
    }
    println("  /dev/console write succeeded");
    close(fd);

    // Test 6: Open /dev/tty
    println("\nTest 6: /dev/tty write");
    let fd = match open("/dev/tty\0", O_RDWR) {
        Ok(fd) => fd,
        Err(e) => {
            print("FAILED: open(/dev/tty) failed: ");
            print_num(e as i64);
            print("\n");
            exit(1);
        }
    };

    let tty_msg = b"  [direct tty write via /dev/tty]\n";
    let result = write(fd, tty_msg);
    if result < 0 {
        print("FAILED: write to /dev/tty failed: ");
        print_num(result);
        print("\n");
        exit(1);
    }
    let n = result as usize;
    if n != tty_msg.len() {
        fail("FAILED: write to /dev/tty returned wrong count");
    }
    println("  /dev/tty write succeeded");
    close(fd);

    println("\nAll devfs tests passed!");
    println("DEVFS_TEST_PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print("PANIC in devfs_test!\n");
    exit(2);
}
