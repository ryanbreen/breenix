//! Integration test for cp and mv coreutils with argv support (std version)
//!
//! This test verifies that cp and mv correctly parse multiple command-line
//! arguments (source and destination) passed via fork+exec.

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn access(path: *const u8, mode: i32) -> i32;
    fn open(path: *const u8, flags: i32, mode: i32) -> i32;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn unlink(path: *const u8) -> i32;
}

const F_OK: i32 = 0;
const O_WRONLY: i32 = 0x01;
const O_CREAT: i32 = 0x40;

const TEST_FILE: &[u8] = b"/test_cp_mv_src\0";
const COPY_FILE: &[u8] = b"/test_cp_mv_copy\0";
const MOVE_FILE: &[u8] = b"/test_cp_mv_moved\0";
const TEST_CONTENT: &[u8] = b"test content for cp/mv";

/// Check if a path exists using access()
fn path_exists(path: &[u8]) -> bool {
    unsafe { access(path.as_ptr(), F_OK) == 0 }
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn create_test_file() -> bool {
    let fd = unsafe { open(TEST_FILE.as_ptr(), O_WRONLY | O_CREAT, 0o644) };
    if fd < 0 {
        return false;
    }
    unsafe {
        write(fd, TEST_CONTENT.as_ptr(), TEST_CONTENT.len());
        close(fd);
    }
    true
}

fn cleanup() {
    unsafe {
        unlink(TEST_FILE.as_ptr());
        unlink(COPY_FILE.as_ptr());
        unlink(MOVE_FILE.as_ptr());
    }
}

/// Fork and exec cp with source and dest arguments
fn run_cp() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec cp with two arguments
        let program = b"cp\0";
        let arg0 = b"cp\0".as_ptr();
        let arg1 = b"/test_cp_mv_src\0".as_ptr();
        let arg2 = b"/test_cp_mv_copy\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        println!("  exec cp failed");
        std::process::exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0) };

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        println!("  fork failed");
        -1
    }
}

/// Fork and exec mv with source and dest arguments
fn run_mv() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec mv with two arguments
        let program = b"mv\0";
        let arg0 = b"mv\0".as_ptr();
        let arg1 = b"/test_cp_mv_copy\0".as_ptr();
        let arg2 = b"/test_cp_mv_moved\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        println!("  exec mv failed");
        std::process::exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0) };

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        println!("  fork failed");
        -1
    }
}

fn main() {
    println!("=== Cp/Mv Argv Integration Test ===");

    // Cleanup any leftover files from previous runs
    cleanup();

    // Create a test file
    println!("Setup: Creating test file");
    if !create_test_file() {
        println!("CP_MV_ARGV_TEST_FAILED: could not create test file");
        std::process::exit(1);
    }

    // Test 1: Copy file via cp with source and dest arguments
    println!("Test 1: cp with source and dest arguments");
    let cp_result = run_cp();
    if cp_result != 0 {
        println!("CP_MV_ARGV_TEST_FAILED: cp returned non-zero");
        cleanup();
        std::process::exit(1);
    }

    // Verify copy was created
    if !path_exists(COPY_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: copy not created");
        cleanup();
        std::process::exit(1);
    }
    println!("  cp created copy successfully");

    // Verify original still exists
    if !path_exists(TEST_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: original deleted after cp");
        cleanup();
        std::process::exit(1);
    }

    // Test 2: Move file via mv with source and dest arguments
    println!("Test 2: mv with source and dest arguments");
    let mv_result = run_mv();
    if mv_result != 0 {
        println!("CP_MV_ARGV_TEST_FAILED: mv returned non-zero");
        cleanup();
        std::process::exit(1);
    }

    // Verify destination exists
    if !path_exists(MOVE_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: moved file not found");
        cleanup();
        std::process::exit(1);
    }
    println!("  mv created destination successfully");

    // Verify source was removed (mv removes source)
    if path_exists(COPY_FILE) {
        println!("CP_MV_ARGV_TEST_FAILED: source not removed after mv");
        cleanup();
        std::process::exit(1);
    }
    println!("  mv removed source successfully");

    // Cleanup
    cleanup();

    println!("CP_MV_ARGV_TEST_PASSED");
    std::process::exit(0);
}
