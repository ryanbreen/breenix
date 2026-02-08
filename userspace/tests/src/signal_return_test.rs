//! Signal handler return test (std version)
//!
//! This test proves the signal trampoline works end-to-end:
//! 1. Register a handler for SIGUSR1
//! 2. Send SIGUSR1 to self
//! 3. Handler executes and returns
//! 4. Trampoline calls sigreturn to restore pre-signal context
//! 5. Execution resumes where it was interrupted

use std::sync::atomic::{AtomicBool, Ordering};

// Flags to track execution flow
static BEFORE_SIGNAL: AtomicBool = AtomicBool::new(false);
static HANDLER_RAN: AtomicBool = AtomicBool::new(false);
static AFTER_SIGNAL: AtomicBool = AtomicBool::new(false);

const SIGUSR1: i32 = 10;
const SA_RESTORER: u64 = 0x04000000;

#[repr(C)]
struct KernelSigaction {
    handler: u64,
    mask: u64,
    flags: u64,
    restorer: u64,
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
    fn sched_yield() -> i32;
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 13u64,
        in("rdi") sig as u64,
        in("rsi") act as u64,
        in("rdx") oldact as u64,
        in("r10") 8u64,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") 13u64,
        inlateout("x0") sig as u64 => ret,
        in("x1") act as u64,
        in("x2") oldact as u64,
        in("x3") 8u64,
        options(nostack),
    );
    ret as i64
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov rax, 15",
        "int 0x80",
        "ud2",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov x8, 15",
        "svc #0",
        "brk #1",
    )
}

/// Simple signal handler that just sets a flag and returns
extern "C" fn sigusr1_handler(_sig: i32) {
    HANDLER_RAN.store(true, Ordering::SeqCst);
}

fn main() {
    println!("=== Signal Return Test ===");
    println!("Testing signal handler return via trampoline\n");

    let my_pid = unsafe { getpid() };

    // Register handler for SIGUSR1
    println!("Step 1: Registering SIGUSR1 handler");
    let action = KernelSigaction {
        handler: sigusr1_handler as u64,
        mask: 0,
        flags: SA_RESTORER,
        restorer: __restore_rt as u64,
    };

    let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaction returned error");
        std::process::exit(1);
    }
    println!("  Handler registered successfully");

    // Set flag before sending signal
    println!("\nStep 2: Setting BEFORE_SIGNAL flag");
    BEFORE_SIGNAL.store(true, Ordering::SeqCst);
    println!("  BEFORE_SIGNAL = true");

    // Send signal to self
    println!("\nStep 3: Sending SIGUSR1 to self");
    let ret = unsafe { kill(my_pid, SIGUSR1) };
    if ret != 0 {
        println!("  FAIL: kill returned error");
        std::process::exit(1);
    }
    println!("  Signal sent successfully");

    // Yield to allow signal delivery
    println!("\nStep 4: Yielding to allow signal delivery");
    for i in 0..100 {
        unsafe { sched_yield(); }

        if HANDLER_RAN.load(Ordering::SeqCst) {
            println!("  Signal delivered and handler executed");
            break;
        }

        if i % 20 == 19 {
            println!("  Still waiting for signal delivery...");
        }
    }

    // If we reach here, the handler MUST have returned successfully
    println!("\nStep 5: Execution resumed after handler");
    AFTER_SIGNAL.store(true, Ordering::SeqCst);
    println!("  AFTER_SIGNAL = true");

    // Verify all flags are set correctly
    println!("\n=== Verification ===");
    let before = BEFORE_SIGNAL.load(Ordering::SeqCst);
    let handler = HANDLER_RAN.load(Ordering::SeqCst);
    let after = AFTER_SIGNAL.load(Ordering::SeqCst);

    print!("BEFORE_SIGNAL: ");
    if before { println!("true"); } else { println!("false (ERROR)"); }

    print!("HANDLER_RAN:   ");
    if handler { println!("true"); } else { println!("false (handler never executed)"); }

    print!("AFTER_SIGNAL:  ");
    if after { println!("true"); } else { println!("false (execution didn't resume after handler)"); }

    // Final verdict
    println!("\n=== Result ===");
    if before && handler && after {
        println!("SIGNAL_RETURN_WORKS");
        println!("\nThe signal trampoline successfully:");
        println!("  1. Delivered the signal");
        println!("  2. Executed the handler");
        println!("  3. Called sigreturn via trampoline");
        println!("  4. Restored pre-signal execution context");
        println!("  5. Resumed execution after the signal");
        std::process::exit(0);
    } else {
        println!("SIGNAL_RETURN_FAILED");
        println!("\nThe trampoline did not work correctly:");
        if !handler {
            println!("  - Handler never executed (signal not delivered)");
        }
        if !after {
            println!("  - Execution didn't resume (sigreturn failed)");
        }
        std::process::exit(1);
    }
}
