//! FIFO (Named Pipe) Test (std version)
//!
//! Tests the FIFO implementation including:
//! - mkfifo() syscall
//! - Opening FIFOs for read/write
//! - Reading and writing through FIFOs
//! - O_NONBLOCK behavior
//! - unlink() for FIFOs
//! - Error conditions

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::fs::{self, O_RDONLY, O_WRONLY, O_NONBLOCK};
use libbreenix::process::{self, ForkResult};

/// Phase 1: Basic FIFO create and open
fn test_basic_fifo() -> bool {
    println!("Phase 1: Basic FIFO create/open/close");

    let path = "/tmp/test_fifo1\0";
    match fs::mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO at /tmp/test_fifo1"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    let read_fd = match fs::open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => {
            println!("  Opened FIFO for reading: fd={}", fd.raw());
            fd
        }
        Err(e) => {
            println!("  ERROR: open for read failed: {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let write_fd = match fs::open(path, O_WRONLY) {
        Ok(fd) => {
            println!("  Opened FIFO for writing: fd={}", fd.raw());
            fd
        }
        Err(e) => {
            println!("  ERROR: open for write failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let data = b"Hello FIFO!";
    match fs::write(write_fd, data) {
        Ok(n) => {
            println!("  Wrote {} bytes to FIFO", n);
        }
        Err(e) => {
            println!("  ERROR: write failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            return false;
        }
    }

    let mut buf = [0u8; 32];
    match fs::read(read_fd, &mut buf) {
        Ok(n) => {
            println!("  Read {} bytes from FIFO", n);
            if &buf[..n] == data {
                println!("  Data matches!");
            } else {
                println!("  ERROR: Data mismatch!");
                let _ = fs::close(read_fd);
                let _ = fs::close(write_fd);
                let _ = fs::unlink(path);
                return false;
            }
        }
        Err(e) => {
            println!("  ERROR: read failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            return false;
        }
    }

    let _ = fs::close(read_fd);
    let _ = fs::close(write_fd);
    let _ = fs::unlink(path);

    println!("Phase 1: PASSED");
    true
}

/// Phase 2: EEXIST - mkfifo on existing path
fn test_eexist() -> bool {
    println!("Phase 2: EEXIST test");

    let path = "/tmp/test_fifo2\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => println!("  Created first FIFO"),
        Err(e) => {
            println!("  ERROR: first mkfifo failed: {:?}", e);
            return false;
        }
    }

    match fs::mkfifo(path, 0o644) {
        Ok(()) => {
            println!("  ERROR: second mkfifo should have failed");
            let _ = fs::unlink(path);
            return false;
        }
        Err(Error::Os(Errno::EEXIST)) => {
            println!("  Got expected EEXIST error");
        }
        Err(e) => {
            println!("  ERROR: expected EEXIST, got {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    }

    let _ = fs::unlink(path);
    println!("Phase 2: PASSED");
    true
}

/// Phase 3: ENOENT - open non-existent FIFO
fn test_enoent() -> bool {
    println!("Phase 3: ENOENT test");

    let path = "/tmp/nonexistent_fifo\0";

    match fs::open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => {
            println!("  ERROR: open should have failed, got fd={}", fd.raw());
            let _ = fs::close(fd);
            return false;
        }
        Err(Error::Os(Errno::ENOENT)) => {
            println!("  Got expected ENOENT error");
        }
        Err(e) => {
            println!("  ERROR: expected ENOENT, got {:?}", e);
            return false;
        }
    }

    println!("Phase 3: PASSED");
    true
}

/// Phase 4: O_NONBLOCK write without reader returns ENXIO
fn test_nonblock_write_no_reader() -> bool {
    println!("Phase 4: O_NONBLOCK write without reader");

    let path = "/tmp/test_fifo4\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    match fs::open(path, O_WRONLY | O_NONBLOCK) {
        Ok(fd) => {
            println!("  ERROR: open should have returned ENXIO, got fd={}", fd.raw());
            let _ = fs::close(fd);
            let _ = fs::unlink(path);
            return false;
        }
        Err(Error::Os(Errno::ENXIO)) => {
            println!("  Got expected ENXIO error");
        }
        Err(e) => {
            println!("  ERROR: expected ENXIO, got {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    }

    let _ = fs::unlink(path);
    println!("Phase 4: PASSED");
    true
}

/// Phase 5: Read from empty FIFO with O_NONBLOCK returns EAGAIN
fn test_nonblock_read_empty() -> bool {
    println!("Phase 5: O_NONBLOCK read from empty FIFO");

    let path = "/tmp/test_fifo5\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    let read_fd = match fs::open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let write_fd = match fs::open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let mut buf = [0u8; 32];
    match fs::read(read_fd, &mut buf) {
        Ok(n) => {
            println!("  ERROR: read should have returned EAGAIN, got {} bytes", n);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            return false;
        }
        Err(Error::Os(Errno::EAGAIN)) => {
            println!("  Got expected EAGAIN error");
        }
        Err(e) => {
            println!("  ERROR: expected EAGAIN, got {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            return false;
        }
    }

    let _ = fs::close(read_fd);
    let _ = fs::close(write_fd);
    let _ = fs::unlink(path);
    println!("Phase 5: PASSED");
    true
}

/// Phase 6: Multiple writes and reads
fn test_multiple_io() -> bool {
    println!("Phase 6: Multiple writes and reads");

    let path = "/tmp/test_fifo6\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => (),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    let read_fd = match fs::open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let write_fd = match fs::open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let data1 = b"First\0\0\0";
    let data2 = b"Second\0\0";
    let data3 = b"Third\0\0\0";

    for (i, data) in [(1i64, data1 as &[u8]), (2i64, data2 as &[u8]), (3i64, data3 as &[u8])].iter() {
        match fs::write(write_fd, data) {
            Ok(n) => {
                println!("  Write {}: {} bytes", i, n);
            }
            Err(e) => {
                println!("  ERROR: write failed: {:?}", e);
                let _ = fs::close(read_fd);
                let _ = fs::close(write_fd);
                let _ = fs::unlink(path);
                return false;
            }
        }
    }

    let mut buf = [0u8; 64];
    let mut total = 0usize;
    loop {
        match fs::read(read_fd, &mut buf[total..]) {
            Ok(0) => {
                break;
            }
            Ok(n) => {
                total += n;
                println!("  Read {} bytes (total: {})", n, total);
            }
            Err(Error::Os(Errno::EAGAIN)) => {
                break;
            }
            Err(e) => {
                println!("  ERROR: read failed: {:?}", e);
                let _ = fs::close(read_fd);
                let _ = fs::close(write_fd);
                let _ = fs::unlink(path);
                return false;
            }
        }
    }

    println!("  Total read: {} bytes", total);
    if total != 24 {
        println!("  ERROR: expected 24 bytes, got {}", total);
        let _ = fs::close(read_fd);
        let _ = fs::close(write_fd);
        let _ = fs::unlink(path);
        return false;
    }

    let _ = fs::close(read_fd);
    let _ = fs::close(write_fd);
    let _ = fs::unlink(path);
    println!("Phase 6: PASSED");
    true
}

/// Phase 7: Blocking read (fork test)
fn test_blocking_read() -> bool {
    println!("Phase 7: Blocking read (fork test)");

    let path = "/tmp/test_fifo_block_rd\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child process: yield then open for write
            for _ in 0..1000 {
                let _ = process::yield_now();
            }

            match fs::open(path, O_WRONLY) {
                Ok(fd) => {
                    let data = b"from_child";
                    match fs::write(fd, data) {
                        Ok(_) => {}
                        Err(e) => {
                            println!("  Child: write failed with error {:?}", e);
                            std::process::exit(1);
                        }
                    }
                    let _ = fs::close(fd);
                }
                Err(e) => {
                    println!("  Child: open for write failed with error {:?}", e);
                    std::process::exit(1);
                }
            }
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(_child_pid)) => {
            // Parent: open for read (blocking)
            println!("  Parent: opening FIFO for read (blocking)...");
            let read_fd = match fs::open(path, O_RDONLY) {
                Ok(fd) => {
                    println!("  Parent: opened for read, fd={}", fd.raw());
                    fd
                }
                Err(e) => {
                    println!("  ERROR: blocking open for read failed: {:?}", e);
                    let _ = process::waitpid(-1, core::ptr::null_mut(), 0);
                    let _ = fs::unlink(path);
                    return false;
                }
            };

            let mut buf = [0u8; 32];
            let mut total = 0usize;
            loop {
                match fs::read(read_fd, &mut buf[total..]) {
                    Ok(0) => {
                        break;
                    }
                    Ok(n) => {
                        total += n;
                    }
                    Err(Error::Os(Errno::EAGAIN)) => {
                        break;
                    }
                    Err(e) => {
                        println!("  ERROR: read failed: {:?}", e);
                        let _ = fs::close(read_fd);
                        let _ = process::waitpid(-1, core::ptr::null_mut(), 0);
                        let _ = fs::unlink(path);
                        return false;
                    }
                }
            }

            println!("  Parent: read {} bytes", total);

            let _ = fs::close(read_fd);
            let _ = process::waitpid(-1, core::ptr::null_mut(), 0);
            let _ = fs::unlink(path);

            if total == 10 {
                println!("Phase 7: PASSED");
                true
            } else {
                println!("  ERROR: expected exactly 10 bytes from child, got {}", total);
                false
            }
        }
        Err(e) => {
            println!("  ERROR: fork failed: {:?}", e);
            let _ = fs::unlink(path);
            false
        }
    }
}

/// Phase 8: EOF test
fn test_eof() -> bool {
    println!("Phase 8: EOF test (reader gets 0 when writer closes)");

    let path = "/tmp/test_fifo_eof\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    let read_fd = match fs::open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let write_fd = match fs::open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let data = b"test_data";
    let _ = fs::write(write_fd, data);
    println!("  Wrote data");

    let _ = fs::close(write_fd);
    println!("  Closed write end");

    let mut buf = [0u8; 32];
    match fs::read(read_fd, &mut buf) {
        Ok(n) => {
            println!("  First read: {} bytes", n);
        }
        Err(e) => {
            println!("  ERROR: first read failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            return false;
        }
    }

    match fs::read(read_fd, &mut buf) {
        Ok(0) => {
            println!("  Second read (after writer closed): 0");
            println!("  Got expected EOF (0)");
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            println!("Phase 8: PASSED");
            true
        }
        Ok(n) => {
            println!("  Second read (after writer closed): {}", n);
            println!("  ERROR: expected EOF (0), got {}", n);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            false
        }
        Err(Error::Os(Errno::EAGAIN)) => {
            println!("  Second read (after writer closed): EAGAIN");
            println!("  ERROR: Got EAGAIN instead of EOF!");
            println!("  FIFO must return 0 (EOF) when all writers close, not EAGAIN");
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            false
        }
        Err(e) => {
            println!("  Second read (after writer closed): {:?}", e);
            println!("  ERROR: expected EOF (0), got {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            false
        }
    }
}

/// Phase 9: EPIPE test
fn test_epipe() -> bool {
    println!("Phase 9: EPIPE test (write fails when reader closes)");

    let path = "/tmp/test_fifo_epipe\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    let read_fd = match fs::open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let write_fd = match fs::open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let data = b"test";
    match fs::write(write_fd, data) {
        Ok(n) => {
            println!("  Write with reader open: {} bytes", n);
        }
        Err(e) => {
            println!("  ERROR: write with reader open failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            return false;
        }
    }

    let _ = fs::close(read_fd);
    println!("  Closed read end");

    let data2 = b"more_data";
    match fs::write(write_fd, data2) {
        Err(Error::Os(Errno::EPIPE)) => {
            println!("  Write after reader closed: EPIPE");
            println!("  Got expected EPIPE");
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            println!("Phase 9: PASSED");
            true
        }
        Ok(n) => {
            println!("  Write after reader closed: {} bytes", n);
            println!("  ERROR: Write succeeded after reader closed!");
            println!("  FIFO must return EPIPE when all readers close, not accept data");
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            false
        }
        Err(e) => {
            println!("  Write after reader closed: {:?}", e);
            println!("  ERROR: expected EPIPE, got {:?}", e);
            let _ = fs::close(write_fd);
            let _ = fs::unlink(path);
            false
        }
    }
}

/// Phase 10: Unlink FIFO while open
fn test_unlink_while_open() -> bool {
    println!("Phase 10: Unlink FIFO while open");

    let path = "/tmp/test_fifo10\0";

    match fs::mkfifo(path, 0o644) {
        Ok(()) => (),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {:?}", e);
            return false;
        }
    }

    let read_fd = match fs::open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {:?}", e);
            let _ = fs::unlink(path);
            return false;
        }
    };

    let write_fd = match fs::open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::unlink(path);
            return false;
        }
    };

    // Unlink while both ends are open
    match fs::unlink(path) {
        Ok(()) => {
            println!("  Unlinked FIFO while open");
        }
        Err(e) => {
            println!("  ERROR: unlink failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            return false;
        }
    }

    // I/O should still work on open fds
    let data = b"After unlink";
    match fs::write(write_fd, data) {
        Ok(n) => {
            println!("  Wrote {} bytes after unlink", n);
        }
        Err(e) => {
            println!("  ERROR: write after unlink failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            return false;
        }
    }

    let mut buf = [0u8; 32];
    match fs::read(read_fd, &mut buf) {
        Ok(n) => {
            println!("  Read {} bytes after unlink", n);
            if &buf[..n] != data {
                println!("  ERROR: data mismatch");
                let _ = fs::close(read_fd);
                let _ = fs::close(write_fd);
                return false;
            }
        }
        Err(e) => {
            println!("  ERROR: read after unlink failed: {:?}", e);
            let _ = fs::close(read_fd);
            let _ = fs::close(write_fd);
            return false;
        }
    }

    let _ = fs::close(read_fd);
    let _ = fs::close(write_fd);
    println!("Phase 10: PASSED");
    true
}

fn main() {
    println!("=== FIFO (Named Pipe) Test ===");

    let mut all_passed = true;

    if !test_basic_fifo() { all_passed = false; }
    if !test_eexist() { all_passed = false; }
    if !test_enoent() { all_passed = false; }
    if !test_nonblock_write_no_reader() { all_passed = false; }
    if !test_nonblock_read_empty() { all_passed = false; }
    if !test_multiple_io() { all_passed = false; }
    if !test_blocking_read() { all_passed = false; }
    if !test_eof() { all_passed = false; }
    if !test_epipe() { all_passed = false; }
    if !test_unlink_while_open() { all_passed = false; }

    if all_passed {
        println!("=== All FIFO Tests PASSED ===");
        println!("FIFO_TEST_PASSED");
    } else {
        println!("=== Some FIFO Tests FAILED ===");
    }

    std::process::exit(if all_passed { 0 } else { 1 });
}
