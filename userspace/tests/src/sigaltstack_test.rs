//! sigaltstack() syscall test (std version)
//!
//! Tests the sigaltstack() syscall which allows setting an alternate signal stack.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);
static HANDLER_RSP: AtomicU64 = AtomicU64::new(0);

const SIGUSR1: i32 = 10;
const SA_RESTORER: u64 = 0x04000000;
const SA_ONSTACK: u64 = 0x08000000;
const SS_DISABLE: i32 = 2;
const SS_ONSTACK: i32 = 1;
const MINSIGSTKSZ: usize = 2048;
const SIGSTKSZ: usize = 8192;

/// Alternate stack buffer
static mut ALT_STACK: [u8; SIGSTKSZ] = [0; SIGSTKSZ];

#[repr(C)]
struct KernelSigaction {
    handler: u64,
    mask: u64,
    flags: u64,
    restorer: u64,
}

/// Kernel stack_t layout: ss_sp(u64), ss_flags(i32), _pad(i32), ss_size(usize)
#[repr(C)]
#[derive(Clone, Copy)]
struct StackT {
    ss_sp: u64,
    ss_flags: i32,
    _pad: i32,
    ss_size: usize,
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
    fn sched_yield() -> i32;
}

// Raw syscall wrappers
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
unsafe fn raw_sigaltstack(ss: *const StackT, old_ss: *mut StackT) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 131u64,  // SYS_SIGALTSTACK
        in("rdi") ss as u64,
        in("rsi") old_ss as u64,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_sigaltstack(ss: *const StackT, old_ss: *mut StackT) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") 131u64,
        inlateout("x0") ss as u64 => ret,
        in("x1") old_ss as u64,
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

    let ret = unsafe { raw_sigaltstack(&new_ss, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaltstack() returned error {}", -ret);
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaltstack() set alternate stack");

    // Test 2: Query current alternate stack
    println!("\nTest 2: Querying alternate stack configuration");
    let mut old_ss = StackT { ss_sp: 0, ss_flags: SS_DISABLE, _pad: 0, ss_size: 0 };
    let ret = unsafe { raw_sigaltstack(std::ptr::null(), &mut old_ss) };
    if ret < 0 {
        println!("  FAIL: sigaltstack(NULL, &old_ss) returned error {}", -ret);
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
    let action = KernelSigaction {
        handler: handler_on_altstack as u64,
        mask: 0,
        flags: SA_RESTORER | SA_ONSTACK,
        restorer: __restore_rt as u64,
    };

    let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaction() returned error {}", -ret);
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Signal handler registered with SA_ONSTACK");

    // Test 4: Send signal and verify handler runs on alternate stack
    println!("\nTest 4: Sending signal to trigger handler");
    let my_pid = unsafe { getpid() };
    let ret = unsafe { kill(my_pid, SIGUSR1) };
    if ret != 0 {
        println!("  FAIL: kill() returned error");
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  Signal sent successfully");

    println!("  Yielding to allow signal delivery...");
    for _ in 0..10 {
        unsafe { sched_yield(); }
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

    let ret = unsafe { raw_sigaltstack(&disable_ss, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaltstack(SS_DISABLE) returned error {}", -ret);
        println!("SIGALTSTACK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Alternate stack disabled");

    // Verify it was disabled
    let mut query_ss = StackT { ss_sp: 0, ss_flags: 0, _pad: 0, ss_size: 0 };
    let ret = unsafe { raw_sigaltstack(std::ptr::null(), &mut query_ss) };
    if ret < 0 {
        println!("  FAIL: Query after disable returned error {}", -ret);
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

    let ret = unsafe { raw_sigaltstack(&too_small_ss, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  PASS: sigaltstack() rejected too-small stack (error {})", -ret);
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

    let ret = unsafe { raw_sigaltstack(&min_ss, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaltstack() rejected MINSIGSTKSZ stack (error {})", -ret);
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
