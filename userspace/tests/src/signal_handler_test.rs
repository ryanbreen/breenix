//! Signal handler test program (std version)
//!
//! Tests that signal handlers actually execute:
//! 1. Register a signal handler using sigaction
//! 2. Send a signal to self using kill
//! 3. Verify the handler was called
//! 4. Print boot stage marker for validation

use std::sync::atomic::{AtomicBool, Ordering};

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

// Signal constants (Breenix uses Linux signal numbers)
const SIGUSR1: i32 = 10;

// Sigaction struct matching the kernel layout (all u64 fields)
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

// Use raw syscall for sigaction since the libc sigaction has a different struct layout
// and we want to match exactly what the kernel expects
#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 13u64,  // SYS_SIGACTION
        in("rdi") sig as u64,
        in("rsi") act as u64,
        in("rdx") oldact as u64,
        in("r10") 8u64,   // sigsetsize
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
        in("x8") 13u64,  // SYS_SIGACTION
        inlateout("x0") sig as u64 => ret,
        in("x1") act as u64,
        in("x2") oldact as u64,
        in("x3") 8u64,   // sigsetsize
        options(nostack),
    );
    ret as i64
}

// Signal restorer trampoline
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov rax, 15",  // SYS_rt_sigreturn
        "int 0x80",
        "ud2",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov x8, 15",  // SYS_rt_sigreturn
        "svc #0",
        "brk #1",
    )
}

// SA_RESTORER flag
const SA_RESTORER: u64 = 0x04000000;

/// Signal handler for SIGUSR1
extern "C" fn sigusr1_handler(_sig: i32) {
    HANDLER_CALLED.store(true, Ordering::SeqCst);
    println!("  HANDLER: SIGUSR1 received and executed!");
}

fn main() {
    println!("=== Signal Handler Test ===");

    let my_pid = unsafe { getpid() };
    println!("My PID: {}", my_pid);

    // Test 1: Register signal handler using sigaction
    println!("\nTest 1: Register SIGUSR1 handler");
    let action = KernelSigaction {
        handler: sigusr1_handler as u64,
        mask: 0,
        flags: SA_RESTORER,
        restorer: __restore_rt as u64,
    };

    let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaction returned error {}", -ret);
        println!("SIGNAL_HANDLER_NOT_EXECUTED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered handler");

    // Test 2: Send SIGUSR1 to self
    println!("\nTest 2: Send SIGUSR1 to self using kill");
    let ret = unsafe { kill(my_pid, SIGUSR1) };
    if ret != 0 {
        println!("  FAIL: kill() returned error");
        println!("SIGNAL_HANDLER_NOT_EXECUTED");
        std::process::exit(1);
    }
    println!("  PASS: kill() succeeded");

    // Test 3: Yield to allow signal delivery
    println!("\nTest 3: Yielding to allow signal delivery...");
    for i in 0..10 {
        unsafe { sched_yield(); }

        if HANDLER_CALLED.load(Ordering::SeqCst) {
            println!("  Handler called after {} yields", i + 1);
            break;
        }
    }

    // Test 4: Verify handler was called
    println!("\nTest 4: Verify handler execution");
    if HANDLER_CALLED.load(Ordering::SeqCst) {
        println!("  PASS: Handler was called!");
        println!();
        println!("SIGNAL_HANDLER_EXECUTED");
        std::process::exit(0);
    } else {
        println!("  FAIL: Handler was NOT called after 10 yields");
        println!();
        println!("SIGNAL_HANDLER_NOT_EXECUTED");
        std::process::exit(1);
    }
}
