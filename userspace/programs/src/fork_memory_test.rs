//! Fork memory isolation test program (std version)
//!
//! Tests that fork() correctly implements copy-on-write (CoW) semantics,
//! ensuring that parent and child have isolated memory spaces.
//!
//! This test verifies:
//! 1. Stack isolation - child inherits but is isolated from parent's stack
//! 2. Heap isolation (sbrk) - child has separate heap memory
//! 3. Global/static data isolation - child has copy of parent's globals

use libbreenix::process::{fork, getpid, waitpid, yield_now, wifexited, wexitstatus, ForkResult};
use libbreenix::memory::sbrk;

/// Global variable for memory isolation test
static mut GLOBAL_VALUE: u64 = 0xDEADBEEF;

/// Test 1: Stack memory isolation
///
/// Parent writes 0xDEADBEEF to a stack variable, forks, then modifies
/// it to 0xCAFEBABE. Child should still see the original value.
fn test_stack_isolation() -> bool {
    println!("\n=== Test 1: Stack Memory Isolation ===");

    // Stack variable with known initial value
    let mut stack_value: u64 = 0xDEADBEEF;
    println!("Parent: Initial stack value: {:#018X}", stack_value);

    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            // Small delay to ensure parent has time to modify its value
            for _ in 0..50 {
                let _ = yield_now();
            }

            // Child should see the ORIGINAL value (0xDEADBEEF)
            println!("Child: Reading stack value: {:#018X}", stack_value);

            if stack_value == 0xDEADBEEF {
                println!("Child: Stack value is ORIGINAL (0xDEADBEEF) - CORRECT!");
                println!("FORK_STACK_ISOLATION_PASSED");
                std::process::exit(0); // Success
            } else if stack_value == 0xCAFEBABE {
                println!("Child: Stack value is MODIFIED (0xCAFEBABE) - ISOLATION FAILED!");
                println!("FORK_STACK_ISOLATION_FAILED");
                std::process::exit(1); // Failure
            } else {
                println!("Child: Stack value is UNEXPECTED: {:#018X}", stack_value);
                println!("FORK_STACK_ISOLATION_FAILED");
                std::process::exit(1);
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            // Immediately modify the stack value
            stack_value = 0xCAFEBABE;
            println!("Parent: Modified stack value to: {:#018X}", stack_value);

            // Wait for child to complete
            let mut status: i32 = 0;
            let result = waitpid(child_pid.raw() as i32, &mut status, 0).unwrap();

            if result.raw() as i32 != child_pid.raw() as i32 {
                println!("Parent: waitpid failed");
                return false;
            }

            // Check if child exited successfully (exit code 0)
            if wifexited(status) && wexitstatus(status) == 0 {
                println!("Parent: Child verified stack isolation - TEST PASSED");
                return true;
            } else {
                println!("Parent: Child exit code: {}", wexitstatus(status));
                println!("Parent: Stack isolation test FAILED");
                return false;
            }
        }
        Err(_) => {
            println!("fork() failed");
            return false;
        }
    }
}

/// Test 2: Heap memory isolation (using sbrk)
///
/// Parent allocates heap memory, writes 0xDEADBEEF, forks, then modifies
/// it to 0xCAFEBABE. Child should still see the original value.
fn test_heap_isolation() -> bool {
    println!("\n=== Test 2: Heap Memory Isolation (sbrk) ===");

    // Allocate 8 bytes on the heap
    let heap_ptr = sbrk(8) as *mut u64;

    if heap_ptr.is_null() || heap_ptr as usize == usize::MAX {
        println!("Parent: sbrk failed - cannot allocate heap memory");
        return false;
    }

    println!("Parent: Allocated heap at address: {:#018X}", heap_ptr as u64);

    // Write initial value to heap
    unsafe { *heap_ptr = 0xDEADBEEF; }
    println!("Parent: Initial heap value: {:#018X}", unsafe { *heap_ptr });

    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            // Small delay to ensure parent has time to modify its value
            for _ in 0..50 {
                let _ = yield_now();
            }

            // Child should see the ORIGINAL value (0xDEADBEEF)
            let child_value = unsafe { *heap_ptr };
            println!("Child: Reading heap value: {:#018X}", child_value);

            if child_value == 0xDEADBEEF {
                println!("Child: Heap value is ORIGINAL (0xDEADBEEF) - CORRECT!");
                println!("FORK_HEAP_ISOLATION_PASSED");
                std::process::exit(0); // Success
            } else if child_value == 0xCAFEBABE {
                println!("Child: Heap value is MODIFIED (0xCAFEBABE) - ISOLATION FAILED!");
                println!("FORK_HEAP_ISOLATION_FAILED");
                std::process::exit(1); // Failure
            } else {
                println!("Child: Heap value is UNEXPECTED: {:#018X}", child_value);
                println!("FORK_HEAP_ISOLATION_FAILED");
                std::process::exit(1);
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            // Immediately modify the heap value
            unsafe { *heap_ptr = 0xCAFEBABE; }
            println!("Parent: Modified heap value to: {:#018X}", unsafe { *heap_ptr });

            // Wait for child to complete
            let mut status: i32 = 0;
            let result = waitpid(child_pid.raw() as i32, &mut status, 0).unwrap();

            if result.raw() as i32 != child_pid.raw() as i32 {
                println!("Parent: waitpid failed");
                return false;
            }

            // Check if child exited successfully (exit code 0)
            if wifexited(status) && wexitstatus(status) == 0 {
                println!("Parent: Child verified heap isolation - TEST PASSED");
                return true;
            } else {
                println!("Parent: Child exit code: {}", wexitstatus(status));
                println!("Parent: Heap isolation test FAILED");
                return false;
            }
        }
        Err(_) => {
            println!("fork() failed");
            return false;
        }
    }
}

/// Test 3: Global/static memory isolation
///
/// Uses a global static variable to verify isolation across fork.
fn test_global_isolation() -> bool {
    println!("\n=== Test 3: Global/Static Memory Isolation ===");

    // Global variable is already initialized to 0xDEADBEEF
    println!("Parent: Initial global value: {:#018X}", unsafe { GLOBAL_VALUE });

    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            // Small delay to ensure parent has time to modify its value
            for _ in 0..50 {
                let _ = yield_now();
            }

            // Child should see the ORIGINAL value (0xDEADBEEF)
            let value = unsafe { GLOBAL_VALUE };
            println!("Child: Reading global value: {:#018X}", value);

            if value == 0xDEADBEEF {
                println!("Child: Global value is ORIGINAL (0xDEADBEEF) - CORRECT!");
                println!("FORK_GLOBAL_ISOLATION_PASSED");
                std::process::exit(0); // Success
            } else if value == 0xCAFEBABE {
                println!("Child: Global value is MODIFIED (0xCAFEBABE) - ISOLATION FAILED!");
                println!("FORK_GLOBAL_ISOLATION_FAILED");
                std::process::exit(1); // Failure
            } else {
                println!("Child: Global value is UNEXPECTED: {:#018X}", value);
                println!("FORK_GLOBAL_ISOLATION_FAILED");
                std::process::exit(1);
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            // Immediately modify the global value
            unsafe { GLOBAL_VALUE = 0xCAFEBABE; }
            println!("Parent: Modified global value to: {:#018X}", unsafe { GLOBAL_VALUE });

            // Wait for child to complete
            let mut status: i32 = 0;
            let result = waitpid(child_pid.raw() as i32, &mut status, 0).unwrap();

            if result.raw() as i32 != child_pid.raw() as i32 {
                println!("Parent: waitpid failed");
                return false;
            }

            // Check if child exited successfully (exit code 0)
            if wifexited(status) && wexitstatus(status) == 0 {
                println!("Parent: Child verified global isolation - TEST PASSED");
                return true;
            } else {
                println!("Parent: Child exit code: {}", wexitstatus(status));
                println!("Parent: Global isolation test FAILED");
                return false;
            }
        }
        Err(_) => {
            println!("fork() failed");
            return false;
        }
    }
}

fn main() {
    println!("=== Fork Memory Isolation Test Suite ===");
    println!("Verifying copy-on-write (CoW) semantics for fork()");
    println!("Parent PID: {}", getpid().unwrap().raw() as i32);

    let mut all_passed = true;

    // Run all isolation tests
    if !test_stack_isolation() {
        all_passed = false;
    }

    if !test_heap_isolation() {
        all_passed = false;
    }

    if !test_global_isolation() {
        all_passed = false;
    }

    // Final summary
    println!("\n=== Fork Memory Isolation Test Summary ===");
    if all_passed {
        println!("All memory isolation tests PASSED!");
        println!("FORK_MEMORY_ISOLATION_PASSED");
        std::process::exit(0);
    } else {
        println!("Some memory isolation tests FAILED!");
        println!("FORK_MEMORY_ISOLATION_FAILED");
        std::process::exit(1);
    }
}
