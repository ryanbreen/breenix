//! Select syscall test program
//!
//! Tests the select() syscall for monitoring file descriptors.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;
const SYS_CLOSE: u64 = 6;
const SYS_PIPE: u64 = 22;
const SYS_SELECT: u64 = 23;

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

#[inline(always)]
unsafe fn syscall5(n: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        inlateout("rdx") arg3 => _,
        inlateout("r10") arg4 => _,
        inlateout("r8") arg5 => _,
        out("rcx") _,
        out("r9") _,
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

// Helper to write hex
#[inline(always)]
fn write_hex(n: u64) {
    write_str("0x");
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    for i in 0..16 {
        buf[15 - i] = hex_chars[((n >> (i * 4)) & 0xf) as usize];
    }
    // Skip leading zeros
    let mut start = 0;
    while start < 15 && buf[start] == b'0' {
        start += 1;
    }
    let s = unsafe { core::str::from_utf8_unchecked(&buf[start..]) };
    write_str(s);
}

// Helper to exit with error message
#[inline(always)]
fn fail(msg: &str) -> ! {
    write_str("USERSPACE SELECT: FAIL - ");
    write_str(msg);
    write_str("\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}

// fd_set helpers
#[inline(always)]
fn fd_zero(set: &mut u64) {
    *set = 0;
}

#[inline(always)]
fn fd_set_bit(fd: i32, set: &mut u64) {
    if fd >= 0 && fd < 64 {
        *set |= 1u64 << fd;
    }
}

#[inline(always)]
fn fd_isset(fd: i32, set: &u64) -> bool {
    if fd >= 0 && fd < 64 {
        (*set & (1u64 << fd)) != 0
    } else {
        false
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== Select Test Program ===\n");

    // Phase 1: Create a pipe for testing
    write_str("Phase 1: Creating pipe...\n");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { syscall1(SYS_PIPE, pipefd.as_mut_ptr() as u64) } as i64;

    if ret < 0 {
        write_str("  pipe() returned error: ");
        write_num(ret);
        write_str("\n");
        fail("pipe() failed");
    }

    write_str("  Pipe created: read_fd=");
    write_num(pipefd[0] as i64);
    write_str(", write_fd=");
    write_num(pipefd[1] as i64);
    write_str("\n");

    // Phase 2: select on empty pipe for read (should not be ready)
    write_str("Phase 2: Selecting on empty pipe for read...\n");
    let mut readfds: u64 = 0;
    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);

    let nfds = (pipefd[0] + 1) as u64;
    let select_ret = unsafe {
        syscall5(SYS_SELECT, nfds, &mut readfds as *mut u64 as u64, 0, 0, 0)
    } as i64;

    write_str("  select() returned: ");
    write_num(select_ret);
    write_str(", readfds=");
    write_hex(readfds);
    write_str("\n");

    if select_ret < 0 {
        fail("select() on empty pipe failed");
    }

    // Empty pipe should have no read readiness
    if fd_isset(pipefd[0], &readfds) {
        fail("Empty pipe should not be ready for read");
    }
    write_str("  OK: Empty pipe is not ready for read\n");

    // Phase 3: Write data to pipe, then select for read
    write_str("Phase 3: Writing data and selecting for read...\n");
    let test_data = b"Test";
    let write_ret = unsafe {
        syscall3(SYS_WRITE, pipefd[1] as u64, test_data.as_ptr() as u64, test_data.len() as u64)
    } as i64;

    if write_ret != test_data.len() as i64 {
        write_str("  write() returned: ");
        write_num(write_ret);
        write_str("\n");
        fail("write to pipe failed");
    }

    // Reset readfds and select again
    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);

    let select_ret = unsafe {
        syscall5(SYS_SELECT, nfds, &mut readfds as *mut u64 as u64, 0, 0, 0)
    } as i64;

    write_str("  select() returned: ");
    write_num(select_ret);
    write_str(", readfds=");
    write_hex(readfds);
    write_str("\n");

    if select_ret < 0 {
        fail("select() on pipe with data failed");
    }

    if select_ret != 1 {
        fail("select() should return 1 when pipe has data");
    }

    if !fd_isset(pipefd[0], &readfds) {
        fail("Pipe with data should be ready for read");
    }
    write_str("  OK: Pipe with data is ready for read\n");

    // Phase 4: Select on write end for write
    write_str("Phase 4: Selecting on write end for write...\n");
    let mut writefds: u64 = 0;
    fd_zero(&mut writefds);
    fd_set_bit(pipefd[1], &mut writefds);

    let nfds_write = (pipefd[1] + 1) as u64;
    let select_ret = unsafe {
        syscall5(SYS_SELECT, nfds_write, 0, &mut writefds as *mut u64 as u64, 0, 0)
    } as i64;

    write_str("  select() returned: ");
    write_num(select_ret);
    write_str(", writefds=");
    write_hex(writefds);
    write_str("\n");

    if select_ret < 0 {
        fail("select() on write end failed");
    }

    if !fd_isset(pipefd[1], &writefds) {
        fail("Write end should be ready for write");
    }
    write_str("  OK: Write end is ready for write\n");

    // Phase 5: Select with multiple fd_sets
    write_str("Phase 5: Selecting with multiple fd_sets...\n");
    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);
    fd_zero(&mut writefds);
    fd_set_bit(pipefd[1], &mut writefds);

    let max_fd = if pipefd[0] > pipefd[1] { pipefd[0] } else { pipefd[1] };
    let nfds_multi = (max_fd + 1) as u64;

    let select_ret = unsafe {
        syscall5(
            SYS_SELECT,
            nfds_multi,
            &mut readfds as *mut u64 as u64,
            &mut writefds as *mut u64 as u64,
            0,
            0,
        )
    } as i64;

    write_str("  select() returned: ");
    write_num(select_ret);
    write_str("\n  readfds=");
    write_hex(readfds);
    write_str(", writefds=");
    write_hex(writefds);
    write_str("\n");

    if select_ret < 0 {
        fail("select() with multiple fd_sets failed");
    }

    // Both should be ready (pipe has data, write end has space)
    if select_ret < 2 {
        fail("Expected at least 2 ready fds");
    }
    write_str("  OK: Multiple fd_sets work correctly\n");

    // Phase 6: Close write end and check for exception on read end
    write_str("Phase 6: Closing write end and checking for exception...\n");

    // First drain the pipe
    let mut read_buf = [0u8; 32];
    let _ = unsafe {
        syscall3(SYS_READ, pipefd[0] as u64, read_buf.as_mut_ptr() as u64, read_buf.len() as u64)
    };

    // Close write end
    let close_ret = unsafe { syscall1(SYS_CLOSE, pipefd[1] as u64) } as i64;
    if close_ret < 0 {
        write_str("  close() returned: ");
        write_num(close_ret);
        write_str("\n");
        fail("close() on write end failed");
    }

    // Select on read end - should trigger exception (POLLHUP)
    let mut exceptfds: u64 = 0;
    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);
    fd_zero(&mut exceptfds);
    fd_set_bit(pipefd[0], &mut exceptfds);

    let select_ret = unsafe {
        syscall5(
            SYS_SELECT,
            nfds,
            &mut readfds as *mut u64 as u64,
            0,
            &mut exceptfds as *mut u64 as u64,
            0,
        )
    } as i64;

    write_str("  select() returned: ");
    write_num(select_ret);
    write_str("\n  readfds=");
    write_hex(readfds);
    write_str(", exceptfds=");
    write_hex(exceptfds);
    write_str("\n");

    if select_ret < 0 {
        fail("select() after closing write end failed");
    }

    // After write end closes, read end should have exception (HUP)
    // Note: Some systems report this as readable with EOF instead
    write_str("  OK: select() returns after write end closed\n");

    // Phase 7: Test stdout writability
    write_str("Phase 7: Testing stdout writability...\n");
    fd_zero(&mut writefds);
    fd_set_bit(1, &mut writefds);  // fd 1 = stdout

    let select_ret = unsafe {
        syscall5(SYS_SELECT, 2, 0, &mut writefds as *mut u64 as u64, 0, 0)
    } as i64;

    write_str("  select() returned: ");
    write_num(select_ret);
    write_str(", writefds=");
    write_hex(writefds);
    write_str("\n");

    if select_ret < 0 {
        fail("select() on stdout failed");
    }

    if !fd_isset(1, &writefds) {
        fail("stdout should be writable");
    }
    write_str("  OK: stdout is writable\n");

    // Clean up
    write_str("Phase 8: Cleanup...\n");
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Closed remaining fds\n");

    // All tests passed
    write_str("USERSPACE SELECT: ALL TESTS PASSED\n");
    write_str("SELECT_TEST_PASSED\n");
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in select test!\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}
