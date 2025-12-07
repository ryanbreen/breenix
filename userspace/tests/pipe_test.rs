//! Pipe syscall test program
//!
//! Tests the pipe() and close() syscalls for IPC.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;
const SYS_CLOSE: u64 = 6;
const SYS_PIPE: u64 = 22;

// Syscall wrappers
#[inline(always)]
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        out("rcx") _,
        out("rdx") _,
        out("rsi") _,
        out("r8") _,
        out("r9") _,
        out("r10") _,
        out("r11") _,
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        inlateout("rdx") arg3 => _,
        out("rcx") _,
        out("r8") _,
        out("r9") _,
        out("r10") _,
        out("r11") _,
    );
    ret
}

// Helper to write a string
#[inline(always)]
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
    }
}

// Helper to write a decimal number
#[inline(always)]
fn write_num(n: i64) {
    if n < 0 {
        write_str("-");
        write_num_inner(-n as u64);
    } else {
        write_num_inner(n as u64);
    }
}

#[inline(always)]
fn write_num_inner(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = 19;

    if n == 0 {
        write_str("0");
        return;
    }

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf[i + 1..]) };
    write_str(s);
}

// Helper to exit with error message
#[inline(always)]
fn fail(msg: &str) -> ! {
    write_str("USERSPACE PIPE: FAIL - ");
    write_str(msg);
    write_str("\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== Pipe Test Program ===\n");

    // Phase 1: Create a pipe
    write_str("Phase 1: Creating pipe with pipe()...\n");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        write_str("  pipe() returned error: ");
        write_num(ret);
        write_str("\n");
        fail("pipe() failed");
    }

    write_str("  Pipe created successfully\n");
    write_str("  Read fd: ");
    write_num(pipefd[0] as i64);
    write_str("\n  Write fd: ");
    write_num(pipefd[1] as i64);
    write_str("\n");

    // Validate fd numbers are reasonable (should be >= 3 after stdin/stdout/stderr)
    if pipefd[0] < 3 || pipefd[1] < 3 {
        fail("Pipe fds should be >= 3 (after stdin/stdout/stderr)");
    }
    if pipefd[0] == pipefd[1] {
        fail("Read and write fds should be different");
    }
    write_str("  FD numbers are valid\n");

    // Phase 2: Write data to pipe
    write_str("Phase 2: Writing data to pipe...\n");
    let test_data = b"Hello, Pipe!";
    let write_ret = unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;

    if write_ret < 0 {
        write_str("  write() returned error: ");
        write_num(write_ret);
        write_str("\n");
        fail("write to pipe failed");
    }

    write_str("  Wrote ");
    write_num(write_ret);
    write_str(" bytes to pipe\n");

    if write_ret != test_data.len() as i64 {
        fail("Did not write expected number of bytes");
    }

    // Phase 3: Read data from pipe
    write_str("Phase 3: Reading data from pipe...\n");
    let mut read_buf = [0u8; 32];
    let read_ret = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, read_buf.as_mut_ptr() as u64, read_buf.len() as u64)
    } as i64;

    if read_ret < 0 {
        write_str("  read() returned error: ");
        write_num(read_ret);
        write_str("\n");
        fail("read from pipe failed");
    }

    write_str("  Read ");
    write_num(read_ret);
    write_str(" bytes from pipe\n");

    if read_ret != test_data.len() as i64 {
        fail("Did not read expected number of bytes");
    }

    // Phase 4: Verify data matches
    write_str("Phase 4: Verifying data...\n");
    let read_slice = &read_buf[..read_ret as usize];

    if read_slice != test_data {
        write_str("  Data mismatch!\n");
        write_str("  Expected: ");
        if let Ok(s) = core::str::from_utf8(test_data) {
            write_str(s);
        }
        write_str("\n  Got: ");
        if let Ok(s) = core::str::from_utf8(read_slice) {
            write_str(s);
        }
        write_str("\n");
        fail("Data verification failed");
    }

    write_str("  Data verified: '");
    if let Ok(s) = core::str::from_utf8(read_slice) {
        write_str(s);
    }
    write_str("'\n");

    // Phase 5: Close the pipe ends
    write_str("Phase 5: Closing pipe file descriptors...\n");

    let close_read = unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) } as i64;
    if close_read < 0 {
        write_str("  close(read_fd) returned error: ");
        write_num(close_read);
        write_str("\n");
        fail("close(read_fd) failed");
    }
    write_str("  Closed read fd\n");

    let close_write = unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) } as i64;
    if close_write < 0 {
        write_str("  close(write_fd) returned error: ");
        write_num(close_write);
        write_str("\n");
        fail("close(write_fd) failed");
    }
    write_str("  Closed write fd\n");

    // All tests passed
    write_str("USERSPACE PIPE: ALL TESTS PASSED\n");
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in pipe test!\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}
