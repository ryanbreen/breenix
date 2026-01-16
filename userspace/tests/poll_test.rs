//! Poll syscall test program
//!
//! Tests the poll() syscall for monitoring file descriptors.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;
const SYS_CLOSE: u64 = 6;
const SYS_POLL: u64 = 7;
const SYS_PIPE: u64 = 22;
const SYS_SOCKET: u64 = 41;
const SYS_CONNECT: u64 = 42;
const SYS_BIND: u64 = 49;
const SYS_LISTEN: u64 = 50;

// Poll event constants
const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
#[allow(dead_code)]  // Part of POSIX poll API, not used in this test but available for completeness
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
const POLLNVAL: i16 = 0x0020;

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;

// pollfd structure
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: addr,
            sin_zero: [0; 8],
        }
    }
}

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
unsafe fn syscall2(n: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        out("rcx") _,
        out("rdx") _,
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

// Helper to write hex
#[inline(always)]
fn write_hex(n: i16) {
    write_str("0x");
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 4];
    buf[0] = hex_chars[((n >> 12) & 0xf) as usize];
    buf[1] = hex_chars[((n >> 8) & 0xf) as usize];
    buf[2] = hex_chars[((n >> 4) & 0xf) as usize];
    buf[3] = hex_chars[(n & 0xf) as usize];
    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    write_str(s);
}

// Helper to exit with error message
#[inline(always)]
fn fail(msg: &str) -> ! {
    write_str("USERSPACE POLL: FAIL - ");
    write_str(msg);
    write_str("\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== Poll Test Program ===\n");

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

    // Phase 2: Poll empty pipe for POLLIN (should not be ready)
    write_str("Phase 2: Polling empty pipe for POLLIN...\n");
    let mut fds = [PollFd {
        fd: pipefd[0],
        events: POLLIN,
        revents: 0,
    }];

    let poll_ret = unsafe {
        syscall3(SYS_POLL, fds.as_mut_ptr() as u64, 1, 0)
    } as i64;

    write_str("  poll() returned: ");
    write_num(poll_ret);
    write_str(", revents=");
    write_hex(fds[0].revents);
    write_str("\n");

    if poll_ret < 0 {
        fail("poll() on empty pipe failed");
    }

    // Empty pipe should have revents=0 (no data available)
    if fds[0].revents & POLLIN != 0 {
        fail("Empty pipe should not have POLLIN set");
    }
    write_str("  OK: Empty pipe has no POLLIN\n");

    // Phase 3: Write data to pipe, then poll for POLLIN
    write_str("Phase 3: Writing data and polling for POLLIN...\n");
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

    // Reset revents and poll again
    fds[0].revents = 0;
    let poll_ret = unsafe {
        syscall3(SYS_POLL, fds.as_mut_ptr() as u64, 1, 0)
    } as i64;

    write_str("  poll() returned: ");
    write_num(poll_ret);
    write_str(", revents=");
    write_hex(fds[0].revents);
    write_str("\n");

    if poll_ret < 0 {
        fail("poll() on pipe with data failed");
    }

    if poll_ret != 1 {
        fail("poll() should return 1 when pipe has data");
    }

    if fds[0].revents & POLLIN == 0 {
        fail("Pipe with data should have POLLIN set");
    }
    write_str("  OK: Pipe with data has POLLIN\n");

    // Phase 4: Poll write end for POLLOUT
    write_str("Phase 4: Polling write end for POLLOUT...\n");
    fds[0] = PollFd {
        fd: pipefd[1],
        events: POLLOUT,
        revents: 0,
    };

    let poll_ret = unsafe {
        syscall3(SYS_POLL, fds.as_mut_ptr() as u64, 1, 0)
    } as i64;

    write_str("  poll() returned: ");
    write_num(poll_ret);
    write_str(", revents=");
    write_hex(fds[0].revents);
    write_str("\n");

    if poll_ret < 0 {
        fail("poll() on write end failed");
    }

    if fds[0].revents & POLLOUT == 0 {
        fail("Write end should have POLLOUT set");
    }
    write_str("  OK: Write end has POLLOUT\n");

    // Phase 5: Poll invalid fd
    write_str("Phase 5: Polling invalid fd...\n");
    fds[0] = PollFd {
        fd: 999,
        events: POLLIN,
        revents: 0,
    };

    let poll_ret = unsafe {
        syscall3(SYS_POLL, fds.as_mut_ptr() as u64, 1, 0)
    } as i64;

    write_str("  poll() returned: ");
    write_num(poll_ret);
    write_str(", revents=");
    write_hex(fds[0].revents);
    write_str("\n");

    if poll_ret < 0 {
        fail("poll() on invalid fd should not return error");
    }

    if poll_ret != 1 {
        fail("poll() should return 1 when fd has POLLNVAL");
    }

    if fds[0].revents & POLLNVAL == 0 {
        fail("Invalid fd should have POLLNVAL set");
    }
    write_str("  OK: Invalid fd has POLLNVAL\n");

    // Phase 6: Close write end and check for POLLHUP on read end
    write_str("Phase 6: Closing write end and checking for POLLHUP...\n");

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

    // Poll read end - should have POLLHUP
    fds[0] = PollFd {
        fd: pipefd[0],
        events: POLLIN,
        revents: 0,
    };

    let poll_ret = unsafe {
        syscall3(SYS_POLL, fds.as_mut_ptr() as u64, 1, 0)
    } as i64;

    write_str("  poll() returned: ");
    write_num(poll_ret);
    write_str(", revents=");
    write_hex(fds[0].revents);
    write_str("\n");

    if poll_ret < 0 {
        fail("poll() after closing write end failed");
    }

    if fds[0].revents & POLLHUP == 0 {
        fail("Read end should have POLLHUP after write end closed");
    }
    write_str("  OK: Read end has POLLHUP after write end closed\n");

    // Phase 7: Poll multiple fds
    write_str("Phase 7: Polling multiple fds...\n");
    let mut multi_fds = [
        PollFd { fd: 0, events: POLLIN, revents: 0 },   // stdin
        PollFd { fd: 1, events: POLLOUT, revents: 0 },  // stdout
        PollFd { fd: 999, events: POLLIN, revents: 0 }, // invalid
    ];

    let poll_ret = unsafe {
        syscall3(SYS_POLL, multi_fds.as_mut_ptr() as u64, 3, 0)
    } as i64;

    write_str("  poll() returned: ");
    write_num(poll_ret);
    write_str("\n");
    write_str("  stdin revents=");
    write_hex(multi_fds[0].revents);
    write_str(", stdout revents=");
    write_hex(multi_fds[1].revents);
    write_str(", invalid revents=");
    write_hex(multi_fds[2].revents);
    write_str("\n");

    if poll_ret < 0 {
        fail("poll() on multiple fds failed");
    }

    // stdout should have POLLOUT
    if multi_fds[1].revents & POLLOUT == 0 {
        fail("stdout should have POLLOUT");
    }

    // invalid fd should have POLLNVAL
    if multi_fds[2].revents & POLLNVAL == 0 {
        fail("invalid fd should have POLLNVAL");
    }
    write_str("  OK: Multiple fds poll works correctly\n");

    // Phase 8: Poll TCP listener for first-call readiness
    write_str("Phase 8: Polling TCP listener for POLLIN...\n");
    let server_fd = unsafe { syscall3(SYS_SOCKET, AF_INET as u64, SOCK_STREAM as u64, 0) } as i64;
    if server_fd < 0 {
        write_str("  socket() returned: ");
        write_num(server_fd);
        write_str("\n");
        fail("socket() failed");
    }
    let server_fd = server_fd as i32;

    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9091);
    let bind_ret = unsafe {
        syscall3(
            SYS_BIND,
            server_fd as u64,
            &server_addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    } as i64;
    if bind_ret < 0 {
        write_str("  bind() returned: ");
        write_num(bind_ret);
        write_str("\n");
        fail("bind() failed");
    }

    let listen_ret = unsafe { syscall2(SYS_LISTEN, server_fd as u64, 16) } as i64;
    if listen_ret < 0 {
        write_str("  listen() returned: ");
        write_num(listen_ret);
        write_str("\n");
        fail("listen() failed");
    }

    let client_fd = unsafe { syscall3(SYS_SOCKET, AF_INET as u64, SOCK_STREAM as u64, 0) } as i64;
    if client_fd < 0 {
        write_str("  client socket() returned: ");
        write_num(client_fd);
        write_str("\n");
        fail("client socket() failed");
    }
    let client_fd = client_fd as i32;

    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 9091);
    let connect_ret = unsafe {
        syscall3(
            SYS_CONNECT,
            client_fd as u64,
            &loopback_addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    } as i64;
    if connect_ret < 0 {
        write_str("  connect() returned: ");
        write_num(connect_ret);
        write_str("\n");
        fail("connect() failed");
    }

    let mut listen_fds = [PollFd {
        fd: server_fd,
        events: POLLIN,
        revents: 0,
    }];
    let poll_ret = unsafe {
        syscall3(SYS_POLL, listen_fds.as_mut_ptr() as u64, 1, 0)
    } as i64;

    write_str("  poll() returned: ");
    write_num(poll_ret);
    write_str(", revents=");
    write_hex(listen_fds[0].revents);
    write_str("\n");

    if poll_ret < 0 {
        fail("poll() on listener failed");
    }
    if poll_ret != 1 {
        fail("poll() should return 1 for listener readiness");
    }
    if listen_fds[0].revents & POLLIN == 0 {
        fail("Listener should have POLLIN set on first poll");
    }
    write_str("  OK: Listener POLLIN set on first poll\n");

    unsafe { syscall1(SYS_CLOSE, client_fd as u64) };
    unsafe { syscall1(SYS_CLOSE, server_fd as u64) };

    // Clean up
    write_str("Phase 9: Cleanup...\n");
    unsafe { syscall1(SYS_CLOSE, pipefd[0] as u64) };
    write_str("  Closed remaining fds\n");

    // All tests passed
    write_str("USERSPACE POLL: ALL TESTS PASSED\n");
    write_str("POLL_TEST_PASSED\n");
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in poll test!\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}
