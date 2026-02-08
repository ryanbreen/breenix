//! Integration test for rm coreutil with argv support (std version)
//!
//! This test verifies that rm correctly parses the file argument
//! passed via fork+exec.

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn access(path: *const u8, mode: i32) -> i32;
    fn open(path: *const u8, flags: i32, mode: i32) -> i32;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
}

const F_OK: i32 = 0;
const O_WRONLY: i32 = 0x01;
const O_CREAT: i32 = 0x40;

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

const TEST_FILE: &[u8] = b"/test_rm_argv_file\0";

fn create_test_file() -> bool {
    let fd = unsafe { open(TEST_FILE.as_ptr(), O_WRONLY | O_CREAT, 0o644) };
    if fd < 0 {
        return false;
    }
    let content = b"test content";
    unsafe {
        write(fd, content.as_ptr(), content.len());
        close(fd);
    }
    true
}

/// Fork and exec rm with file argument
fn run_rm() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec rm with file argument
        let program = b"rm\0";
        let arg0 = b"rm\0".as_ptr();
        let arg1 = b"/test_rm_argv_file\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        println!("  exec rm failed");
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
    println!("=== Rm Argv Integration Test ===");

    // Create a test file
    println!("Setup: Creating test file");
    if !create_test_file() {
        println!("RM_ARGV_TEST_FAILED: could not create test file");
        std::process::exit(1);
    }

    // Verify file exists
    if !path_exists(TEST_FILE) {
        println!("RM_ARGV_TEST_FAILED: test file not created");
        std::process::exit(1);
    }
    println!("  test file created");

    // Test: Remove file via rm with file argument
    println!("Test: rm with file argument");
    let rm_result = run_rm();
    if rm_result != 0 {
        println!("RM_ARGV_TEST_FAILED: rm returned non-zero");
        std::process::exit(1);
    }

    // Verify file was removed
    if path_exists(TEST_FILE) {
        println!("RM_ARGV_TEST_FAILED: file not removed");
        std::process::exit(1);
    }
    println!("  rm removed file successfully");

    println!("RM_ARGV_TEST_PASSED");
    std::process::exit(0);
}
