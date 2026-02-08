//! Signal exec check program (std version)
//!
//! This program is exec'd by signal_exec_test to verify that signal
//! handlers are reset to SIG_DFL after exec().
//!
//! POSIX requires that signals with custom handlers be reset to SIG_DFL
//! after exec, while ignored signals (SIG_IGN) may remain ignored.

// Signal constants
const SIGUSR1: i32 = 10;
const SIG_DFL: u64 = 0;
const SIG_IGN: u64 = 1;

// Syscall number
const SYS_SIGACTION: u64 = 13;

#[repr(C)]
struct KernelSigaction {
    handler: u64,
    mask: u64,
    flags: u64,
    restorer: u64,
}

// Use raw syscall for sigaction to match exactly what the kernel expects
#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") SYS_SIGACTION,
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
        in("x8") SYS_SIGACTION,
        inlateout("x0") sig as u64 => ret,
        in("x1") act as u64,
        in("x2") oldact as u64,
        in("x3") 8u64,
        options(nostack),
    );
    ret as i64
}

fn main() {
    println!("=== Signal Exec Check (after exec) ===");
    println!("This program was exec'd - checking if signal handlers are reset to SIG_DFL\n");

    // Query the current handler for SIGUSR1
    // If exec reset handlers properly, this should return SIG_DFL (0)
    println!("Querying SIGUSR1 handler state...");

    let mut old_action = KernelSigaction {
        handler: 0,
        mask: 0,
        flags: 0,
        restorer: 0,
    };

    // sigaction with act=null queries current handler without changing it
    let ret = unsafe { raw_sigaction(SIGUSR1, std::ptr::null(), &mut old_action) };

    if ret >= 0 {
        println!("  sigaction query succeeded");
        println!("  Handler value: {}", old_action.handler);

        if old_action.handler == SIG_DFL {
            println!("  PASS: Handler is SIG_DFL (correctly reset after exec)");
            println!("\nSIGNAL_EXEC_RESET_VERIFIED");
            std::process::exit(0);
        } else if old_action.handler == SIG_IGN {
            println!("  INFO: Handler is SIG_IGN (may be acceptable per POSIX)");
            // This is technically acceptable for POSIX but we want SIG_DFL
            println!("\nSIGNAL_EXEC_RESET_PARTIAL");
            std::process::exit(1);
        } else {
            println!("  FAIL: Handler is NOT SIG_DFL - it was inherited from pre-exec!");
            println!("\nSIGNAL_EXEC_RESET_FAILED");
            std::process::exit(2);
        }
    } else {
        println!("  FAIL: sigaction query returned error {}", -ret);
        println!("\nSIGNAL_EXEC_RESET_FAILED");
        std::process::exit(3);
    }
}
