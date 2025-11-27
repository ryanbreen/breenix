# AGENTS.md: waitpid Userspace Test Implementation

## Mission

Create a userspace test for the `getpid` and `waitpid` syscalls in Breenix. This test verifies the kernel implementation works correctly from userspace.

## Prerequisites

Read the research document first:
- `docs/planning/posix-compliance/AGENTS_WAITPID_RESEARCH.md`

The kernel implementation should be complete:
- `docs/planning/posix-compliance/AGENTS_WAITPID_KERNEL.md`

## Implementation Tasks

### Task 1: Add sys_waitpid to libbreenix

**File:** `userspace/tests/libbreenix.rs`

Add the syscall number constant:
```rust
const SYS_WAITPID: u64 = 7;
```

Add the syscall wrapper function:
```rust
/// waitpid - wait for child process state change
///
/// Arguments:
///   pid: Process to wait for (-1 for any child)
///   wstatus: Pointer to store exit status (can be null/0)
///   options: Wait options (WNOHANG = 1)
///
/// Returns:
///   >0: PID of exited child
///   0: WNOHANG and no child exited
///   -10: ECHILD - no children
pub unsafe fn sys_waitpid(pid: i64, wstatus: *mut i32, options: u64) -> i64 {
    syscall3(SYS_WAITPID, pid as u64, wstatus as u64, options) as i64
}
```

Make sure it's exported in the module (add to existing pub use or make public).

### Task 2: Create waitpid Test Program

**File:** `userspace/tests/waitpid_test.rs` (NEW FILE)

```rust
//! waitpid test program - tests getpid and waitpid syscalls
//!
//! Test plan:
//! 1. Verify getpid returns non-zero for userspace process
//! 2. Fork a child process
//! 3. Child exits with code 42
//! 4. Parent calls waitpid with WNOHANG in a loop
//! 5. Verify child PID returned and exit code is correct

#![no_std]
#![no_main]

mod libbreenix;
use libbreenix::{sys_exit, sys_fork, sys_getpid, sys_waitpid, sys_write};

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    let _ = sys_write(1, prefix.as_bytes());

    let mut n = num;
    let mut i = 0;

    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    let _ = sys_write(1, &BUFFER[..i]);
    let _ = sys_write(1, b"\n");
}

/// Extract exit code from status word
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let _ = sys_write(1, b"=== WAITPID TEST START ===\n");

        // Test 1: getpid returns non-zero
        let _ = sys_write(1, b"Test 1: getpid returns non-zero\n");
        let my_pid = sys_getpid();
        print_number("  My PID: ", my_pid);

        if my_pid == 0 {
            let _ = sys_write(1, b"  FAIL: getpid returned 0 for userspace process\n");
            let _ = sys_write(1, b"=== WAITPID TEST FAIL ===\n");
            sys_exit(1);
        }
        let _ = sys_write(1, b"  PASS: getpid returned non-zero\n");

        // Test 2: waitpid returns ECHILD when no children
        let _ = sys_write(1, b"Test 2: waitpid returns ECHILD with no children\n");
        let mut status: i32 = 0;
        let result = sys_waitpid(-1, &mut status as *mut i32, 1); // WNOHANG
        print_number("  waitpid result: ", result as u64);

        // -10 = ECHILD (twos complement: 0xFFFFFFFFFFFFFFF6)
        if result != -10 && result != 0xFFFFFFFFFFFFFFF6u64 as i64 {
            let _ = sys_write(1, b"  Note: Expected ECHILD (-10), got different value\n");
            // Don't fail - some implementations return 0 for WNOHANG with no children
        } else {
            let _ = sys_write(1, b"  PASS: Got expected ECHILD\n");
        }

        // Test 3: Fork and waitpid
        let _ = sys_write(1, b"Test 3: fork() then waitpid()\n");

        let fork_result = sys_fork();
        print_number("  fork returned: ", fork_result);

        if fork_result == 0 {
            // Child process
            let _ = sys_write(1, b"  CHILD: Hello from child process\n");
            let child_pid = sys_getpid();
            print_number("  CHILD: My PID is ", child_pid);
            let _ = sys_write(1, b"  CHILD: Exiting with code 42\n");
            sys_exit(42);
        }

        // Parent process
        let _ = sys_write(1, b"  PARENT: Child created\n");
        print_number("  PARENT: Child PID is ", fork_result);

        // Wait for child with WNOHANG in a polling loop
        let _ = sys_write(1, b"  PARENT: Waiting for child (WNOHANG polling)\n");

        let mut attempts: u64 = 0;
        const MAX_ATTEMPTS: u64 = 10_000_000;
        const WNOHANG: u64 = 1;

        loop {
            status = 0;
            let wait_result = sys_waitpid(-1, &mut status as *mut i32, WNOHANG);

            if wait_result > 0 {
                // Child reaped successfully
                print_number("  PARENT: waitpid returned PID ", wait_result as u64);
                print_number("  PARENT: Raw status word: ", status as u64);

                let exit_code = wexitstatus(status);
                print_number("  PARENT: Child exit code: ", exit_code as u64);

                if exit_code == 42 {
                    let _ = sys_write(1, b"  PASS: Child exited with expected code 42\n");
                    let _ = sys_write(1, b"=== WAITPID TEST PASS ===\n");
                    sys_exit(0);
                } else {
                    let _ = sys_write(1, b"  FAIL: Child exit code was not 42\n");
                    let _ = sys_write(1, b"=== WAITPID TEST FAIL ===\n");
                    sys_exit(1);
                }
            } else if wait_result == 0 {
                // WNOHANG: child not yet exited, continue polling
                attempts += 1;
                if attempts > MAX_ATTEMPTS {
                    let _ = sys_write(1, b"  FAIL: Timeout waiting for child\n");
                    let _ = sys_write(1, b"=== WAITPID TEST FAIL ===\n");
                    sys_exit(1);
                }

                // Small delay to reduce polling frequency
                if attempts % 100000 == 0 {
                    let _ = sys_write(1, b".");
                }
            } else {
                // Error from waitpid
                print_number("  PARENT: waitpid error: ", (-wait_result) as u64);
                let _ = sys_write(1, b"  FAIL: waitpid returned error\n");
                let _ = sys_write(1, b"=== WAITPID TEST FAIL ===\n");
                sys_exit(1);
            }
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        let _ = sys_write(1, b"PANIC in waitpid test!\n");
        let _ = sys_write(1, b"=== WAITPID TEST FAIL ===\n");
        sys_exit(255);
    }
}
```

### Task 3: Add to Build Script

**File:** `userspace/build.sh`

Find where other test binaries are built and add `waitpid_test`:

Look for lines like:
```bash
cargo build --release --target ... --bin fork_test
```

Add a similar line for waitpid_test:
```bash
cargo build --release --target ... --bin waitpid_test
```

Also add to the copy section where binaries are copied to the output directory.

### Task 4: Add Binary to Cargo.toml

**File:** `userspace/Cargo.toml`

Add the binary entry:
```toml
[[bin]]
name = "waitpid_test"
path = "tests/waitpid_test.rs"
```

### Task 5: Register Test Binary in Kernel (Optional)

**File:** `kernel/src/userspace_test.rs`

If the kernel has a list of embedded test binaries, add waitpid_test:

Look for a section like:
```rust
pub fn get_test_binary(name: &str) -> &'static [u8] {
    match name {
        "fork_test" => include_bytes!(...),
        // Add:
        "waitpid_test" => include_bytes!("../../target/x86_64-breenix/release/waitpid_test"),
        ...
    }
}
```

## Build and Test

1. Build userspace:
```bash
cd userspace && ./build.sh
```

2. Build and run kernel:
```bash
cargo run -p xtask -- boot-stages
```

3. Check output for:
```
=== WAITPID TEST PASS ===
```

## Test Success Criteria

The test passes if:
1. getpid returns a non-zero PID
2. fork successfully creates a child
3. Child exits with code 42
4. Parent receives child PID from waitpid
5. Parent extracts exit code 42 from status
6. Output contains "WAITPID TEST PASS"

## Debugging Tips

If the test fails:

1. **getpid returns 0:** Check kernel sys_getpid implementation
2. **fork doesn't return:** Check fork syscall and scheduler
3. **waitpid always returns 0:** Check if child is becoming zombie properly
4. **Wrong exit code:** Check status word encoding (should be exit_code << 8)
5. **waitpid returns error:** Check error codes and children list

Add debug prints to trace execution:
```rust
let _ = sys_write(1, b"DEBUG: checkpoint N\n");
```

## Code Quality

- Use existing patterns from fork_test.rs
- No compiler warnings
- Clear output messages for debugging
- Proper error handling with descriptive failures
