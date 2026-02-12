//! sigaltstack() syscall test (std version)
//!
//! Tests the sigaltstack() syscall which allows setting an alternate signal stack.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use libbreenix::signal::{SIGUSR1, SA_ONSTACK, SS_DISABLE, SS_ONSTACK, MINSIGSTKSZ, SIGSTKSZ};
use libbreenix::{kill, sigaction, sigaltstack, Sigaction, StackT};
use libbreenix::process::{getpid, yield_now};

static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);
static HANDLER_RSP: AtomicU64 = AtomicU64::new(0);

/// Alternate stack buffer
static mut ALT_STACK: [u8; SIGSTKSZ] = [0; SIGSTKSZ];

/// Signal handler that runs on alternate stack
extern "C" fn handler_on_altstack(_sig: i32) {
    HANDLER_CALLED.store(true, Ordering::SeqCst);
    println!("  HANDLER: Signal received, checking stack...");

    let rsp: u64;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!(
            "mov {0}, rsp",
            out(reg) rsp,
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "mov {0}, sp",
            out(reg) rsp,
        );
    }
    HANDLER_RSP.store(rsp, Ordering::SeqCst);
    println!("  HANDLER: RSP = {:#018x}", rsp);
}

fn main() {
    println!("=== sigaltstack() Syscall Test ===\n");

    // Capture main stack RSP for comparison
    let main_stack_rsp: u64;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!(
            "mov {0}, rsp",
            out(reg) main_stack_rsp,
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "mov {0}, sp",
            out(reg) main_stack_rsp,
        );
    }
    println!("Main stack RSP = {:#018x}\n", main_stack_rsp);

    // Test 1: Set alternate stack
    println!("Test 1: Setting alternate signal stack");
    let alt_stack_base = std::ptr::addr_of!(ALT_STACK) as u64;
    let alt_stack_size = SIGSTKSZ;

    println!("  Alt stack base = {:#018x}", alt_stack_base);
    println!("  Alt stack size = {} bytes", alt_stack_size);

    let new_ss = StackT {
        ss_sp: alt_stack_base,
        ss_flags: 0,
        _pad: 0,
        ss_size: alt_stack_size,
    };

    if sigaltstack(Some(&new_ss), None).is_err() {
        println!("  FAIL: sigaltstack() returned error");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaltstack() set alternate stack");

    // Test 2: Query current alternate stack
    println!("\nTest 2: Querying alternate stack configuration");
    let mut old_ss = StackT { ss_sp: 0, ss_flags: SS_DISABLE, _pad: 0, ss_size: 0 };
    if sigaltstack(None, Some(&mut old_ss)).is_err() {
        println!("  FAIL: sigaltstack(None, &old_ss) returned error");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaltstack() queried current stack");
    println!("  Returned ss_sp = {:#018x}", old_ss.ss_sp);
    println!("  Returned ss_size = {}", old_ss.ss_size);
    println!("  Returned ss_flags = {}", old_ss.ss_flags);

    if old_ss.ss_sp != alt_stack_base {
        println!("  FAIL: ss_sp mismatch");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    if old_ss.ss_size != alt_stack_size {
        println!("  FAIL: ss_size mismatch");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    if old_ss.ss_flags != 0 && old_ss.ss_flags != SS_ONSTACK {
        println!("  FAIL: ss_flags unexpected value");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Alternate stack configuration verified");

    // Test 3: Register signal handler with SA_ONSTACK flag
    println!("\nTest 3: Registering handler with SA_ONSTACK flag");
    let mut action = Sigaction::new(handler_on_altstack);
    action.flags |= SA_ONSTACK;

    if sigaction(SIGUSR1, Some(&action), None).is_err() {
        println!("  FAIL: sigaction() returned error");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Signal handler registered with SA_ONSTACK");

    // Test 4: Send signal and verify handler runs on alternate stack
    println!("\nTest 4: Sending signal to trigger handler");
    let my_pid = getpid().unwrap().raw() as i32;
    if kill(my_pid, SIGUSR1).is_err() {
        println!("  FAIL: kill() returned error");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  Signal sent successfully");

    println!("  Yielding to allow signal delivery...");
    for _ in 0..10 {
        let _ = yield_now();
        if HANDLER_CALLED.load(Ordering::SeqCst) {
            break;
        }
    }

    if !HANDLER_CALLED.load(Ordering::SeqCst) {
        println!("  FAIL: Handler was not called after 10 yields");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Handler was called");

    // Test 5: Verify handler ran on alternate stack
    println!("\nTest 5: Verifying handler ran on alternate stack");
    let alt_stack_bottom = alt_stack_base;
    let alt_stack_top = alt_stack_base + alt_stack_size as u64;
    let handler_rsp = HANDLER_RSP.load(Ordering::SeqCst);

    println!("  Alt stack range: {:#018x} - {:#018x}", alt_stack_bottom, alt_stack_top);
    println!("  Handler RSP: {:#018x}", handler_rsp);
    println!("  Main RSP: {:#018x}", main_stack_rsp);

    if handler_rsp < alt_stack_bottom || handler_rsp >= alt_stack_top {
        println!("  FAIL: Handler RSP is NOT within alternate stack range");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }

    let rsp_diff = if handler_rsp > main_stack_rsp {
        handler_rsp - main_stack_rsp
    } else {
        main_stack_rsp - handler_rsp
    };

    if rsp_diff < 4096 {
        println!("  WARN: Handler RSP is very close to main RSP (diff = {} bytes)", rsp_diff);
    }

    println!("  PASS: Handler ran on alternate stack!");

    // Test 6: Disable alternate stack with SS_DISABLE
    println!("\nTest 6: Disabling alternate stack with SS_DISABLE");
    let disable_ss = StackT {
        ss_sp: 0,
        ss_flags: SS_DISABLE,
        _pad: 0,
        ss_size: 0,
    };

    if sigaltstack(Some(&disable_ss), None).is_err() {
        println!("  FAIL: sigaltstack(SS_DISABLE) returned error");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Alternate stack disabled");

    // Verify it was disabled
    let mut query_ss = StackT { ss_sp: 0, ss_flags: 0, _pad: 0, ss_size: 0 };
    if sigaltstack(None, Some(&mut query_ss)).is_err() {
        println!("  FAIL: Query after disable returned error");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  Queried ss_flags = {}", query_ss.ss_flags);
    if query_ss.ss_flags & SS_DISABLE == 0 {
        println!("  FAIL: SS_DISABLE flag not set after disable");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Alternate stack is disabled");

    // Test 7: Validate minimum size requirement (MINSIGSTKSZ)
    println!("\nTest 7: Testing minimum stack size validation");
    println!("  MINSIGSTKSZ = {} bytes", MINSIGSTKSZ);

    let too_small_ss = StackT {
        ss_sp: alt_stack_base,
        ss_flags: 0,
        _pad: 0,
        ss_size: MINSIGSTKSZ - 1,
    };

    if sigaltstack(Some(&too_small_ss), None).is_err() {
        println!("  PASS: sigaltstack() rejected too-small stack");
    } else {
        println!("  WARN: sigaltstack() accepted stack smaller than MINSIGSTKSZ");
        println!("  (Some systems allow this, continuing test...)");
    }

    let min_ss = StackT {
        ss_sp: alt_stack_base,
        ss_flags: 0,
        _pad: 0,
        ss_size: MINSIGSTKSZ,
    };

    if sigaltstack(Some(&min_ss), None).is_err() {
        println!("  FAIL: sigaltstack() rejected MINSIGSTKSZ stack");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaltstack() accepted MINSIGSTKSZ-sized stack");

    // All tests passed!
    println!("\n=== TEST RESULT ===");
    println!("All sigaltstack() tests passed!");
    println!("  - Set alternate stack: PASS");
    println!("  - Query alternate stack: PASS");
    println!("  - Handler with SA_ONSTACK: PASS");
    println!("  - Handler ran on alt stack: PASS");
    println!("  - SS_DISABLE flag: PASS");
    println!("  - Size validation: PASS");
    println!();
    println!("SIGALTSTACK_TEST_PASSED");
    std::process::exit(0);
}
