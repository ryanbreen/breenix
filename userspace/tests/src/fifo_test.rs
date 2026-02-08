//! FIFO (Named Pipe) Test (std version)
//!
//! Tests the FIFO implementation including:
//! - mkfifo() syscall
//! - Opening FIFOs for read/write
//! - Reading and writing through FIFOs
//! - O_NONBLOCK behavior
//! - unlink() for FIFOs
//! - Error conditions

// Error codes
const ENOENT: i32 = 2;
const ENXIO: i32 = 6;
const EAGAIN: i32 = 11;
const EEXIST: i32 = 17;
const EPIPE: i32 = 32;

// Open flags
const O_RDONLY: i32 = 0;
const O_WRONLY: i32 = 1;
const O_NONBLOCK: i32 = 0x800;

// S_IFIFO for mknod
const S_IFIFO: u32 = 0o010000;

extern "C" {
    fn open(path: *const u8, flags: i32, mode: u32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn unlink(path: *const u8) -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn sched_yield() -> i32;
    static mut ERRNO: i32;
}

// Raw syscall for mkfifo (uses MKNOD under the hood)
#[cfg(target_arch = "x86_64")]
unsafe fn raw_mknod(path: *const u8, mode: u32, dev: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 133u64,  // SYS_MKNOD
        in("rdi") path as u64,
        in("rsi") mode as u64,
        in("rdx") dev,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_mknod(path: *const u8, mode: u32, dev: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") 133u64,
        inlateout("x0") path as u64 => ret,
        in("x1") mode as u64,
        in("x2") dev,
        options(nostack),
    );
    ret as i64
}

fn get_errno() -> i32 {
    unsafe { ERRNO }
}

fn do_mkfifo(path: &str, mode: u32) -> Result<(), i32> {
    let ret = unsafe { raw_mknod(path.as_ptr(), S_IFIFO | mode, 0) };
    if ret < 0 {
        Err((-ret) as i32)
    } else {
        Ok(())
    }
}

fn do_open(path: &str, flags: i32) -> Result<i32, i32> {
    let ret = unsafe { open(path.as_ptr(), flags, 0) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(ret)
    }
}

fn do_close(fd: i32) {
    unsafe { close(fd); }
}

fn do_unlink(path: &str) {
    unsafe { unlink(path.as_ptr()); }
}

fn do_read(fd: i32, buf: &mut [u8]) -> i64 {
    let ret = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
    if ret < 0 {
        -(get_errno() as i64)
    } else {
        ret as i64
    }
}

fn do_write(fd: i32, data: &[u8]) -> i64 {
    let ret = unsafe { write(fd, data.as_ptr(), data.len()) };
    if ret < 0 {
        -(get_errno() as i64)
    } else {
        ret as i64
    }
}

/// Phase 1: Basic FIFO create and open
fn test_basic_fifo() -> bool {
    println!("Phase 1: Basic FIFO create/open/close");

    let path = "/tmp/test_fifo1\0";
    match do_mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO at /tmp/test_fifo1"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    let read_fd = match do_open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => {
            println!("  Opened FIFO for reading: fd={}", fd);
            fd
        }
        Err(e) => {
            println!("  ERROR: open for read failed: {}", e);
            do_unlink(path);
            return false;
        }
    };

    let write_fd = match do_open(path, O_WRONLY) {
        Ok(fd) => {
            println!("  Opened FIFO for writing: fd={}", fd);
            fd
        }
        Err(e) => {
            println!("  ERROR: open for write failed: {}", e);
            do_close(read_fd);
            do_unlink(path);
            return false;
        }
    };

    let data = b"Hello FIFO!";
    let write_ret = do_write(write_fd, data);
    if write_ret < 0 {
        println!("  ERROR: write failed: {}", -write_ret);
        do_close(read_fd);
        do_close(write_fd);
        do_unlink(path);
        return false;
    }
    println!("  Wrote {} bytes to FIFO", write_ret);

    let mut buf = [0u8; 32];
    let read_ret = do_read(read_fd, &mut buf);
    if read_ret < 0 {
        println!("  ERROR: read failed: {}", -read_ret);
        do_close(read_fd);
        do_close(write_fd);
        do_unlink(path);
        return false;
    }
    println!("  Read {} bytes from FIFO", read_ret);
    if &buf[..read_ret as usize] == data {
        println!("  Data matches!");
    } else {
        println!("  ERROR: Data mismatch!");
        do_close(read_fd);
        do_close(write_fd);
        do_unlink(path);
        return false;
    }

    do_close(read_fd);
    do_close(write_fd);
    do_unlink(path);

    println!("Phase 1: PASSED");
    true
}

/// Phase 2: EEXIST - mkfifo on existing path
fn test_eexist() -> bool {
    println!("Phase 2: EEXIST test");

    let path = "/tmp/test_fifo2\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => println!("  Created first FIFO"),
        Err(e) => {
            println!("  ERROR: first mkfifo failed: {}", e);
            return false;
        }
    }

    match do_mkfifo(path, 0o644) {
        Ok(()) => {
            println!("  ERROR: second mkfifo should have failed");
            do_unlink(path);
            return false;
        }
        Err(e) => {
            if e == EEXIST {
                println!("  Got expected EEXIST error");
            } else {
                println!("  ERROR: expected EEXIST, got {}", e);
                do_unlink(path);
                return false;
            }
        }
    }

    do_unlink(path);
    println!("Phase 2: PASSED");
    true
}

/// Phase 3: ENOENT - open non-existent FIFO
fn test_enoent() -> bool {
    println!("Phase 3: ENOENT test");

    let path = "/tmp/nonexistent_fifo\0";

    match do_open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => {
            println!("  ERROR: open should have failed, got fd={}", fd);
            do_close(fd);
            return false;
        }
        Err(e) => {
            if e == ENOENT {
                println!("  Got expected ENOENT error");
            } else {
                println!("  ERROR: expected ENOENT, got {}", e);
                return false;
            }
        }
    }

    println!("Phase 3: PASSED");
    true
}

/// Phase 4: O_NONBLOCK write without reader returns ENXIO
fn test_nonblock_write_no_reader() -> bool {
    println!("Phase 4: O_NONBLOCK write without reader");

    let path = "/tmp/test_fifo4\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    match do_open(path, O_WRONLY | O_NONBLOCK) {
        Ok(fd) => {
            println!("  ERROR: open should have returned ENXIO, got fd={}", fd);
            do_close(fd);
            do_unlink(path);
            return false;
        }
        Err(e) => {
            if e == ENXIO {
                println!("  Got expected ENXIO error");
            } else {
                println!("  ERROR: expected ENXIO, got {}", e);
                do_unlink(path);
                return false;
            }
        }
    }

    do_unlink(path);
    println!("Phase 4: PASSED");
    true
}

/// Phase 5: Read from empty FIFO with O_NONBLOCK returns EAGAIN
fn test_nonblock_read_empty() -> bool {
    println!("Phase 5: O_NONBLOCK read from empty FIFO");

    let path = "/tmp/test_fifo5\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    let read_fd = match do_open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {}", e);
            do_unlink(path);
            return false;
        }
    };

    let write_fd = match do_open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {}", e);
            do_close(read_fd);
            do_unlink(path);
            return false;
        }
    };

    let mut buf = [0u8; 32];
    let read_ret = do_read(read_fd, &mut buf);
    if read_ret >= 0 {
        println!("  ERROR: read should have returned EAGAIN, got {} bytes", read_ret);
        do_close(read_fd);
        do_close(write_fd);
        do_unlink(path);
        return false;
    }
    if -read_ret == EAGAIN as i64 {
        println!("  Got expected EAGAIN error");
    } else {
        println!("  ERROR: expected EAGAIN, got {}", -read_ret);
        do_close(read_fd);
        do_close(write_fd);
        do_unlink(path);
        return false;
    }

    do_close(read_fd);
    do_close(write_fd);
    do_unlink(path);
    println!("Phase 5: PASSED");
    true
}

/// Phase 6: Multiple writes and reads
fn test_multiple_io() -> bool {
    println!("Phase 6: Multiple writes and reads");

    let path = "/tmp/test_fifo6\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => (),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    let read_fd = match do_open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {}", e);
            do_unlink(path);
            return false;
        }
    };

    let write_fd = match do_open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {}", e);
            do_close(read_fd);
            do_unlink(path);
            return false;
        }
    };

    let data1 = b"First\0\0\0";
    let data2 = b"Second\0\0";
    let data3 = b"Third\0\0\0";

    for (i, data) in [(1i64, data1 as &[u8]), (2i64, data2 as &[u8]), (3i64, data3 as &[u8])].iter() {
        let write_ret = do_write(write_fd, data);
        if write_ret < 0 {
            println!("  ERROR: write failed: {}", -write_ret);
            do_close(read_fd);
            do_close(write_fd);
            do_unlink(path);
            return false;
        }
        println!("  Write {}: {} bytes", i, write_ret);
    }

    let mut buf = [0u8; 64];
    let mut total = 0usize;
    loop {
        let read_ret = do_read(read_fd, &mut buf[total..]);
        if read_ret == 0 {
            break;
        } else if read_ret < 0 {
            if -read_ret == EAGAIN as i64 {
                break;
            }
            println!("  ERROR: read failed: {}", -read_ret);
            do_close(read_fd);
            do_close(write_fd);
            do_unlink(path);
            return false;
        }
        total += read_ret as usize;
        println!("  Read {} bytes (total: {})", read_ret, total);
    }

    println!("  Total read: {} bytes", total);
    if total != 24 {
        println!("  ERROR: expected 24 bytes, got {}", total);
        do_close(read_fd);
        do_close(write_fd);
        do_unlink(path);
        return false;
    }

    do_close(read_fd);
    do_close(write_fd);
    do_unlink(path);
    println!("Phase 6: PASSED");
    true
}

/// Phase 7: Blocking read (fork test)
fn test_blocking_read() -> bool {
    println!("Phase 7: Blocking read (fork test)");

    let path = "/tmp/test_fifo_block_rd\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    let pid = unsafe { fork() };
    if pid < 0 {
        println!("  ERROR: fork failed");
        do_unlink(path);
        return false;
    }

    if pid == 0 {
        // Child process: yield then open for write
        for _ in 0..1000 {
            unsafe { sched_yield(); }
        }

        match do_open(path, O_WRONLY) {
            Ok(fd) => {
                let data = b"from_child";
                let ret = do_write(fd, data);
                if ret < 0 {
                    println!("  Child: write failed with error {}", -ret);
                    std::process::exit(1);
                }
                do_close(fd);
            }
            Err(e) => {
                println!("  Child: open for write failed with error {}", e);
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }

    // Parent: open for read (blocking)
    println!("  Parent: opening FIFO for read (blocking)...");
    let read_fd = match do_open(path, O_RDONLY) {
        Ok(fd) => {
            println!("  Parent: opened for read, fd={}", fd);
            fd
        }
        Err(e) => {
            println!("  ERROR: blocking open for read failed: {}", e);
            unsafe { waitpid(pid, std::ptr::null_mut(), 0); }
            do_unlink(path);
            return false;
        }
    };

    let mut buf = [0u8; 32];
    let mut total = 0i64;
    loop {
        let ret = do_read(read_fd, &mut buf[total as usize..]);
        if ret == 0 {
            break;
        } else if ret < 0 {
            if -ret == EAGAIN as i64 {
                break;
            }
            println!("  ERROR: read failed: {}", -ret);
            do_close(read_fd);
            unsafe { waitpid(pid, std::ptr::null_mut(), 0); }
            do_unlink(path);
            return false;
        }
        total += ret;
    }

    println!("  Parent: read {} bytes", total);

    do_close(read_fd);
    unsafe { waitpid(pid, std::ptr::null_mut(), 0); }
    do_unlink(path);

    if total == 10 {
        println!("Phase 7: PASSED");
        true
    } else {
        println!("  ERROR: expected exactly 10 bytes from child, got {}", total);
        false
    }
}

/// Phase 8: EOF test
fn test_eof() -> bool {
    println!("Phase 8: EOF test (reader gets 0 when writer closes)");

    let path = "/tmp/test_fifo_eof\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    let read_fd = match do_open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {}", e);
            do_unlink(path);
            return false;
        }
    };

    let write_fd = match do_open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {}", e);
            do_close(read_fd);
            do_unlink(path);
            return false;
        }
    };

    let data = b"test_data";
    let _ = do_write(write_fd, data);
    println!("  Wrote data");

    do_close(write_fd);
    println!("  Closed write end");

    let mut buf = [0u8; 32];
    let ret = do_read(read_fd, &mut buf);
    if ret < 0 {
        println!("  ERROR: first read failed: {}", -ret);
        do_close(read_fd);
        do_unlink(path);
        return false;
    }
    println!("  First read: {} bytes", ret);

    let ret2 = do_read(read_fd, &mut buf);
    println!("  Second read (after writer closed): {}", ret2);

    do_close(read_fd);
    do_unlink(path);

    if ret2 == 0 {
        println!("  Got expected EOF (0)");
        println!("Phase 8: PASSED");
        true
    } else if ret2 < 0 && -ret2 == EAGAIN as i64 {
        println!("  ERROR: Got EAGAIN instead of EOF!");
        println!("  FIFO must return 0 (EOF) when all writers close, not EAGAIN");
        false
    } else {
        println!("  ERROR: expected EOF (0), got {}", ret2);
        false
    }
}

/// Phase 9: EPIPE test
fn test_epipe() -> bool {
    println!("Phase 9: EPIPE test (write fails when reader closes)");

    let path = "/tmp/test_fifo_epipe\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => println!("  Created FIFO"),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    let read_fd = match do_open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {}", e);
            do_unlink(path);
            return false;
        }
    };

    let write_fd = match do_open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {}", e);
            do_close(read_fd);
            do_unlink(path);
            return false;
        }
    };

    let data = b"test";
    let ret = do_write(write_fd, data);
    if ret < 0 {
        println!("  ERROR: write with reader open failed: {}", -ret);
        do_close(read_fd);
        do_close(write_fd);
        do_unlink(path);
        return false;
    }
    println!("  Write with reader open: {} bytes", ret);

    do_close(read_fd);
    println!("  Closed read end");

    let data2 = b"more_data";
    let ret2 = do_write(write_fd, data2);
    println!("  Write after reader closed: {}", ret2);

    do_close(write_fd);
    do_unlink(path);

    if ret2 < 0 && -ret2 == EPIPE as i64 {
        println!("  Got expected EPIPE");
        println!("Phase 9: PASSED");
        true
    } else if ret2 >= 0 {
        println!("  ERROR: Write succeeded after reader closed!");
        println!("  FIFO must return EPIPE when all readers close, not accept data");
        println!("  This would cause data loss in real applications");
        false
    } else {
        println!("  ERROR: expected EPIPE (32), got {}", -ret2);
        false
    }
}

/// Phase 10: Unlink FIFO while open
fn test_unlink_while_open() -> bool {
    println!("Phase 10: Unlink FIFO while open");

    let path = "/tmp/test_fifo10\0";

    match do_mkfifo(path, 0o644) {
        Ok(()) => (),
        Err(e) => {
            println!("  ERROR: mkfifo failed: {}", e);
            return false;
        }
    }

    let read_fd = match do_open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for read failed: {}", e);
            do_unlink(path);
            return false;
        }
    };

    let write_fd = match do_open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  ERROR: open for write failed: {}", e);
            do_close(read_fd);
            do_unlink(path);
            return false;
        }
    };

    // Unlink while both ends are open
    let ret = unsafe { unlink(path.as_ptr()) };
    if ret < 0 {
        println!("  ERROR: unlink failed");
        do_close(read_fd);
        do_close(write_fd);
        return false;
    }
    println!("  Unlinked FIFO while open");

    // I/O should still work on open fds
    let data = b"After unlink";
    let write_ret = do_write(write_fd, data);
    if write_ret < 0 {
        println!("  ERROR: write after unlink failed: {}", -write_ret);
        do_close(read_fd);
        do_close(write_fd);
        return false;
    }
    println!("  Wrote {} bytes after unlink", write_ret);

    let mut buf = [0u8; 32];
    let read_ret = do_read(read_fd, &mut buf);
    if read_ret < 0 {
        println!("  ERROR: read after unlink failed: {}", -read_ret);
        do_close(read_fd);
        do_close(write_fd);
        return false;
    }
    println!("  Read {} bytes after unlink", read_ret);
    if &buf[..read_ret as usize] != data {
        println!("  ERROR: data mismatch");
        do_close(read_fd);
        do_close(write_fd);
        return false;
    }

    do_close(read_fd);
    do_close(write_fd);
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
