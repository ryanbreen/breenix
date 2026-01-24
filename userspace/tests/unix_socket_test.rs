//! Unix domain socket test program
//!
//! Tests the socketpair() syscall for AF_UNIX sockets using libbreenix.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::io::{self, close, fcntl_getfd, fd_flags};
use libbreenix::process;
use libbreenix::socket::{socketpair, AF_UNIX, AF_INET, SOCK_STREAM, SOCK_DGRAM, SOCK_NONBLOCK, SOCK_CLOEXEC};
use libbreenix::syscall::{nr, raw};

// Buffer size (must match kernel's UNIX_SOCKET_BUFFER_SIZE)
const UNIX_SOCKET_BUFFER_SIZE: usize = 65536;

/// Helper to write a file descriptor using raw syscall (to test sockets directly)
fn write_fd(fd: i32, data: &[u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(nr::WRITE, fd as u64, data.as_ptr() as u64, data.len() as u64)
    } as i64;

    if ret < 0 {
        Err(Errno::from_raw(-ret))
    } else {
        Ok(ret as usize)
    }
}

/// Helper to read from a file descriptor using raw syscall
fn read_fd(fd: i32, buf: &mut [u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(nr::READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;

    if ret < 0 {
        Err(Errno::from_raw(-ret))
    } else {
        Ok(ret as usize)
    }
}

/// Helper to call socketpair with a raw pointer (for testing EFAULT)
fn socketpair_raw(domain: i32, sock_type: i32, protocol: i32, sv_ptr: u64) -> Result<(), Errno> {
    let ret = unsafe {
        raw::syscall4(nr::SOCKETPAIR, domain as u64, sock_type as u64, protocol as u64, sv_ptr)
    } as i64;

    if ret < 0 {
        Err(Errno::from_raw(-ret))
    } else {
        Ok(())
    }
}

fn print_num(n: i64) {
    let mut buf = [0u8; 21];
    let mut i = 20;
    let negative = n < 0;
    let mut n = if negative { (-n) as u64 } else { n as u64 };

    if n == 0 {
        io::print("0");
        return;
    }

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }

    if negative {
        buf[i] = b'-';
        i -= 1;
    }

    if let Ok(s) = core::str::from_utf8(&buf[i + 1..]) {
        io::print(s);
    }
}

fn fail(msg: &str) -> ! {
    io::print("UNIX_SOCKET: FAIL - ");
    io::print(msg);
    io::print("\n");
    process::exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("=== Unix Socket Test ===\n");

    // Phase 1: Create socket pair
    io::print("Phase 1: Creating socket pair with socketpair()...\n");
    let (sv0, sv1) = match socketpair(AF_UNIX, SOCK_STREAM, 0) {
        Ok(pair) => pair,
        Err(e) => {
            io::print("  socketpair() returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("socketpair() failed");
        }
    };

    io::print("  Socket pair created successfully\n");
    io::print("  sv[0] = ");
    print_num(sv0 as i64);
    io::print("\n  sv[1] = ");
    print_num(sv1 as i64);
    io::print("\n");

    // Validate fd numbers are reasonable (should be >= 3 after stdin/stdout/stderr)
    if sv0 < 3 || sv1 < 3 {
        fail("Socket fds should be >= 3 (after stdin/stdout/stderr)");
    }
    if sv0 == sv1 {
        fail("Socket fds should be different");
    }
    io::print("  FD numbers are valid\n");

    // Phase 2: Write from sv[0], read from sv[1]
    io::print("Phase 2: Writing from sv[0], reading from sv[1]...\n");
    let test_data = b"Hello from sv[0]!";
    let write_ret = match write_fd(sv0, test_data) {
        Ok(n) => n,
        Err(e) => {
            io::print("  write(sv[0]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("write to sv[0] failed");
        }
    };

    io::print("  Wrote ");
    print_num(write_ret as i64);
    io::print(" bytes to sv[0]\n");

    if write_ret != test_data.len() {
        fail("Did not write expected number of bytes");
    }

    // Read from sv[1]
    let mut read_buf = [0u8; 32];
    let read_ret = match read_fd(sv1, &mut read_buf) {
        Ok(n) => n,
        Err(e) => {
            io::print("  read(sv[1]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("read from sv[1] failed");
        }
    };

    io::print("  Read ");
    print_num(read_ret as i64);
    io::print(" bytes from sv[1]\n");

    if read_ret != test_data.len() {
        fail("Did not read expected number of bytes");
    }

    // Verify data matches
    let read_slice = &read_buf[..read_ret];
    if read_slice != test_data {
        fail("Data verification failed (sv[0] -> sv[1])");
    }
    io::print("  Data verified: '");
    if let Ok(s) = core::str::from_utf8(read_slice) {
        io::print(s);
    }
    io::print("'\n");

    // Phase 3: Write from sv[1], read from sv[0] (reverse direction)
    io::print("Phase 3: Writing from sv[1], reading from sv[0]...\n");
    let test_data2 = b"Reply from sv[1]!";
    let write_ret2 = match write_fd(sv1, test_data2) {
        Ok(n) => n,
        Err(e) => {
            io::print("  write(sv[1]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("write to sv[1] failed");
        }
    };

    io::print("  Wrote ");
    print_num(write_ret2 as i64);
    io::print(" bytes to sv[1]\n");

    // Read from sv[0]
    let mut read_buf2 = [0u8; 32];
    let read_ret2 = match read_fd(sv0, &mut read_buf2) {
        Ok(n) => n,
        Err(e) => {
            io::print("  read(sv[0]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("read from sv[0] failed");
        }
    };

    io::print("  Read ");
    print_num(read_ret2 as i64);
    io::print(" bytes from sv[0]\n");

    let read_slice2 = &read_buf2[..read_ret2];
    if read_slice2 != test_data2 {
        fail("Data verification failed (sv[1] -> sv[0])");
    }
    io::print("  Bidirectional communication works!\n");

    // Phase 4: Close sv[0], verify sv[1] sees EOF
    io::print("Phase 4: Testing EOF on peer close...\n");
    let close_ret = close(sv0 as u64);
    if close_ret < 0 {
        io::print("  close(sv[0]) returned error: ");
        print_num(close_ret);
        io::print("\n");
        fail("close(sv[0]) failed");
    }
    io::print("  Closed sv[0]\n");

    // Read from sv[1] should return 0 (EOF)
    let mut eof_buf = [0u8; 8];
    let eof_ret = match read_fd(sv1, &mut eof_buf) {
        Ok(n) => n as i64,
        Err(e) => -(e as i64),
    };

    io::print("  Read from sv[1] returned: ");
    print_num(eof_ret);
    io::print("\n");

    if eof_ret != 0 {
        fail("Expected EOF (0) after peer close");
    }
    io::print("  EOF on peer close works!\n");

    // Phase 5: Close sv[1]
    io::print("Phase 5: Closing sv[1]...\n");
    let close_ret2 = close(sv1 as u64);
    if close_ret2 < 0 {
        io::print("  close(sv[1]) returned error: ");
        print_num(close_ret2);
        io::print("\n");
        fail("close(sv[1]) failed");
    }
    io::print("  Closed sv[1]\n");

    // Phase 6: Test SOCK_NONBLOCK - read should return EAGAIN when no data
    io::print("Phase 6: Testing SOCK_NONBLOCK (EAGAIN on empty read)...\n");
    let (sv_nb0, sv_nb1) = match socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(pair) => pair,
        Err(e) => {
            io::print("  socketpair(SOCK_NONBLOCK) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("socketpair(SOCK_NONBLOCK) failed");
        }
    };
    io::print("  Created non-blocking socket pair\n");
    io::print("  sv_nb[0] = ");
    print_num(sv_nb0 as i64);
    io::print(", sv_nb[1] = ");
    print_num(sv_nb1 as i64);
    io::print("\n");

    // Try to read from empty socket - should return EAGAIN
    let mut nb_buf = [0u8; 8];
    match read_fd(sv_nb1, &mut nb_buf) {
        Ok(n) => {
            io::print("  Read returned ");
            print_num(n as i64);
            io::print(" instead of EAGAIN\n");
            fail("Non-blocking read should return EAGAIN when no data available");
        }
        Err(e) => {
            io::print("  Read from empty non-blocking socket returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EAGAIN {
                io::print("  Expected EAGAIN, got different error\n");
                fail("Non-blocking read should return EAGAIN when no data available");
            }
        }
    }
    io::print("  SOCK_NONBLOCK works correctly!\n");

    // Clean up non-blocking sockets
    close(sv_nb0 as u64);
    close(sv_nb1 as u64);

    // Phase 7: Test EPIPE - write to socket after peer closed
    io::print("Phase 7: Testing EPIPE (write to closed peer)...\n");
    let (sv_pipe0, sv_pipe1) = match socketpair(AF_UNIX, SOCK_STREAM, 0) {
        Ok(pair) => pair,
        Err(_) => fail("socketpair() for EPIPE test failed"),
    };
    io::print("  Created socket pair for EPIPE test\n");

    // Close the reader end
    close(sv_pipe1 as u64);
    io::print("  Closed sv_pipe[1] (reader)\n");

    // Try to write to the socket whose peer is closed
    let pipe_data = b"This should fail";
    match write_fd(sv_pipe0, pipe_data) {
        Ok(n) => {
            io::print("  Write returned ");
            print_num(n as i64);
            io::print(" instead of EPIPE\n");
            fail("Write to closed peer should return EPIPE");
        }
        Err(e) => {
            io::print("  Write to socket with closed peer returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EPIPE {
                io::print("  Expected EPIPE, got different error\n");
                fail("Write to closed peer should return EPIPE");
            }
        }
    }
    io::print("  EPIPE works correctly!\n");

    // Clean up
    close(sv_pipe0 as u64);

    // Phase 8: Test error handling - wrong domain and type
    io::print("Phase 8: Testing error handling (invalid domain/type)...\n");

    // Test 8a: AF_INET should return EAFNOSUPPORT
    match socketpair(AF_INET, SOCK_STREAM, 0) {
        Ok(_) => fail("socketpair(AF_INET) should fail"),
        Err(e) => {
            io::print("  socketpair(AF_INET) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            // socketpair returns raw errno as i32
            if e != 97 {
                // EAFNOSUPPORT = 97
                io::print("  Expected EAFNOSUPPORT (97)\n");
                fail("socketpair(AF_INET) should return EAFNOSUPPORT");
            }
        }
    }
    io::print("  AF_INET correctly rejected with EAFNOSUPPORT\n");

    // Test 8b: SOCK_DGRAM should return EINVAL (not yet implemented)
    match socketpair(AF_UNIX, SOCK_DGRAM, 0) {
        Ok(_) => fail("socketpair(SOCK_DGRAM) should fail"),
        Err(e) => {
            io::print("  socketpair(SOCK_DGRAM) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            // socketpair returns raw errno as i32
            if e != 22 {
                // EINVAL = 22
                io::print("  Expected EINVAL (22)\n");
                fail("socketpair(SOCK_DGRAM) should return EINVAL");
            }
        }
    }
    io::print("  SOCK_DGRAM correctly rejected with EINVAL\n");

    io::print("  Error handling works correctly!\n");

    // Phase 9: Test buffer-full scenario (EAGAIN on write when buffer is full)
    io::print("Phase 9: Testing buffer-full (EAGAIN on non-blocking write)...\n");
    let (sv_buf0, sv_buf1) = match socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(pair) => pair,
        Err(_) => fail("socketpair() for buffer-full test failed"),
    };
    io::print("  Created non-blocking socket pair for buffer test\n");

    // Fill the buffer by writing chunks until EAGAIN
    let chunk = [0x42u8; 4096]; // 4KB chunks
    let mut total_written: usize = 0;
    let mut eagain_received = false;

    // Write until we get EAGAIN (buffer full)
    while total_written < UNIX_SOCKET_BUFFER_SIZE + 4096 {
        match write_fd(sv_buf0, &chunk) {
            Ok(n) => {
                total_written += n;
            }
            Err(e) => {
                if e == Errno::EAGAIN {
                    eagain_received = true;
                    io::print("  Got EAGAIN after writing ");
                    print_num(total_written as i64);
                    io::print(" bytes\n");
                    break;
                } else {
                    io::print("  Unexpected error during buffer fill: ");
                    print_num(-(e as i64));
                    io::print("\n");
                    fail("Unexpected error while filling buffer");
                }
            }
        }
    }

    if !eagain_received {
        io::print("  Wrote ");
        print_num(total_written as i64);
        io::print(" bytes without EAGAIN\n");
        fail("Expected EAGAIN when buffer is full");
    }

    // Verify we wrote at least UNIX_SOCKET_BUFFER_SIZE bytes before EAGAIN
    if total_written < UNIX_SOCKET_BUFFER_SIZE {
        io::print("  Only wrote ");
        print_num(total_written as i64);
        io::print(" bytes, expected at least ");
        print_num(UNIX_SOCKET_BUFFER_SIZE as i64);
        io::print("\n");
        fail("Buffer should hold at least UNIX_SOCKET_BUFFER_SIZE bytes");
    }
    io::print("  Buffer-full test passed!\n");

    // Clean up
    close(sv_buf0 as u64);
    close(sv_buf1 as u64);

    // Phase 10: Test NULL sv_ptr (should return EFAULT)
    io::print("Phase 10: Testing NULL sv_ptr (EFAULT)...\n");
    match socketpair_raw(AF_UNIX, SOCK_STREAM, 0, 0) {
        Ok(_) => fail("socketpair(NULL) should fail"),
        Err(e) => {
            io::print("  socketpair(NULL) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EFAULT {
                io::print("  Expected EFAULT\n");
                fail("socketpair(NULL) should return EFAULT");
            }
        }
    }
    io::print("  NULL sv_ptr correctly rejected with EFAULT\n");

    // Phase 11: Test non-zero protocol (should return EINVAL)
    io::print("Phase 11: Testing non-zero protocol (EINVAL)...\n");
    let mut sv_proto: [i32; 2] = [0, 0];
    match socketpair_raw(AF_UNIX, SOCK_STREAM, 1, sv_proto.as_mut_ptr() as u64) {
        Ok(_) => fail("socketpair(protocol=1) should fail"),
        Err(e) => {
            io::print("  socketpair(protocol=1) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EINVAL {
                io::print("  Expected EINVAL\n");
                fail("socketpair(protocol!=0) should return EINVAL");
            }
        }
    }
    io::print("  Non-zero protocol correctly rejected with EINVAL\n");

    // Phase 12: Test SOCK_CLOEXEC flag
    io::print("Phase 12: Testing SOCK_CLOEXEC flag...\n");
    let (sv_cloexec0, sv_cloexec1) = match socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0) {
        Ok(pair) => pair,
        Err(e) => {
            io::print("  socketpair(SOCK_CLOEXEC) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("socketpair(SOCK_CLOEXEC) failed");
        }
    };
    io::print("  Created socket pair with SOCK_CLOEXEC\n");

    // Verify FD_CLOEXEC is set on both fds using fcntl(F_GETFD)
    let flags0 = fcntl_getfd(sv_cloexec0 as u64);
    let flags1 = fcntl_getfd(sv_cloexec1 as u64);

    io::print("  sv_cloexec[0] flags: ");
    print_num(flags0);
    io::print(", sv_cloexec[1] flags: ");
    print_num(flags1);
    io::print("\n");

    if flags0 < 0 || flags1 < 0 {
        io::print("  fcntl(F_GETFD) failed\n");
        fail("fcntl(F_GETFD) failed on SOCK_CLOEXEC socket");
    }

    if (flags0 & fd_flags::FD_CLOEXEC as i64) == 0 {
        fail("sv_cloexec[0] should have FD_CLOEXEC set");
    }
    if (flags1 & fd_flags::FD_CLOEXEC as i64) == 0 {
        fail("sv_cloexec[1] should have FD_CLOEXEC set");
    }
    io::print("  FD_CLOEXEC correctly set on both sockets\n");

    // Clean up
    close(sv_cloexec0 as u64);
    close(sv_cloexec1 as u64);

    // All tests passed
    io::print("=== Unix Socket Test PASSED ===\n");
    io::print("UNIX_SOCKET_TEST_PASSED\n");
    process::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in unix socket test!\n");
    process::exit(1);
}
