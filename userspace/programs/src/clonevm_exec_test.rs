//! Runtime proof that exec detaches a former CLONE_VM process cleanly.
//!
//! ARM64 first proves that a live CLONE_VM sibling prevents exec. Both
//! architectures then release that sibling, exec, and exercise both futex
//! entry points with keys derived from the post-exec thread group.

use libbreenix::errno::Errno;
#[cfg(target_arch = "aarch64")]
use libbreenix::error::Error;
use libbreenix::memory;
use libbreenix::process;
use libbreenix::syscall::{nr, raw};
use std::ptr;

const SHARED_ALIVE_OFFSET: usize = 0;
const SHARED_COMMAND_OFFSET: usize = 8;
const SHARED_TID_OFFSET: usize = 16;

const CHILD_STACK_SIZE: usize = 64 * 1024;
const SPIN_LIMIT: u64 = 2_000_000;
const LIVE_CHILD_WATCHDOG_LIMIT: u64 = 10_000;
const CLONE_FLAGS: u64 = 0x00000100 | 0x00000400 | 0x00200000 | 0x01000000;
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;

const PROGRAM: &[u8] = b"/usr/local/test/bin/clonevm_exec_test\0";
const ARG0: &[u8] = b"clonevm_exec_test\0";
const SECOND_STAGE_ARG: &[u8] = b"--second-stage\0";

extern "C" {
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

unsafe fn raw_msg(msg: &[u8]) {
    write(2, msg.as_ptr(), msg.len());
}

unsafe fn sys_yield() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("int 0x80", in("rax") 24u64, options(nostack));
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("svc #0", in("x8") 124u64, in("x0") 0u64, options(nostack));
}

unsafe fn thread_exit(code: u64) -> ! {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "int 0x80",
        "2:",
        "pause",
        "jmp 2b",
        in("rax") 60u64,
        in("rdi") code,
        options(noreturn),
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        "2:",
        "yield",
        "b 2b",
        in("x8") 93u64,
        in("x0") code,
        options(noreturn),
    );
}

unsafe fn futex_wait(word: *mut u32, expected: u32) -> i64 {
    raw::syscall6(nr::FUTEX, word as u64, FUTEX_WAIT, expected as u64, 0, 0, 0) as i64
}

unsafe fn futex_wake(word: *mut u32, count: u32) -> i64 {
    raw::syscall6(nr::FUTEX, word as u64, FUTEX_WAKE, count as u64, 0, 0, 0) as i64
}

unsafe fn clone_vm_child(
    stack: *mut u8,
    child_fn: extern "C" fn(*mut u8) -> *mut u8,
    child_arg: *mut u8,
    tid_addr: *mut u32,
) -> i64 {
    let stack_top = (stack as usize + CHILD_STACK_SIZE) & !0xF;
    let ret: i64;

    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "int 0x80",
        in("rax") 56u64,
        in("rdi") CLONE_FLAGS,
        in("rsi") stack_top as u64,
        in("rdx") child_fn as u64,
        in("r10") child_arg as u64,
        in("r8") tid_addr as u64,
        lateout("rax") ret,
        options(nostack),
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") 220u64,
        inlateout("x0") CLONE_FLAGS => ret,
        in("x1") stack_top as u64,
        in("x2") child_fn as u64,
        in("x3") child_arg as u64,
        in("x4") tid_addr as u64,
        options(nostack),
    );

    ret
}

unsafe fn map_region(size: usize, error: &[u8]) -> *mut u8 {
    match memory::mmap(core::ptr::null_mut(), size, 3, 0x22, -1, 0) {
        Ok(mapped) => mapped,
        Err(map_error) => {
            drop(map_error);
            raw_msg(error);
            std::process::exit(1);
        }
    }
}

unsafe fn spin_until_nonzero_u64(address: *mut u64) -> bool {
    let mut iteration = 0;
    while iteration < SPIN_LIMIT {
        if core::ptr::read_volatile(address) != 0 {
            return true;
        }
        sys_yield();
        iteration += 1;
    }

    false
}

unsafe fn wait_for_nonzero_u64(address: *mut u64, error: &[u8]) {
    if spin_until_nonzero_u64(address) {
        return;
    }

    raw_msg(error);
    std::process::exit(1);
}

unsafe fn spin_until_u32(address: *mut u32, expected: u32) -> bool {
    let mut iteration = 0;
    while iteration < SPIN_LIMIT {
        if core::ptr::read_volatile(address) == expected {
            return true;
        }
        sys_yield();
        iteration += 1;
    }

    false
}

unsafe fn wait_for_zero_u32(address: *mut u32, error: &[u8]) {
    if spin_until_u32(address, 0) {
        return;
    }

    raw_msg(error);
    std::process::exit(1);
}

unsafe fn prove_child_still_live(alive: *mut u64, tid_addr: *mut u32) {
    let mut consecutive_observations = 0;
    let mut iteration = 0;
    while iteration < SPIN_LIMIT {
        if core::ptr::read_volatile(alive) != 0 && core::ptr::read_volatile(tid_addr) != 0 {
            consecutive_observations += 1;
            if consecutive_observations == 2 {
                return;
            }
        } else {
            consecutive_observations = 0;
        }
        sys_yield();
        iteration += 1;
    }

    raw_msg(b"CLONEVM_EXEC_TEST: ERROR child was not live before exec probe\n");
    std::process::exit(1);
}

extern "C" fn phase_one_child(arg: *mut u8) -> *mut u8 {
    unsafe {
        let alive = arg.add(SHARED_ALIVE_OFFSET) as *mut u64;
        let command = arg.add(SHARED_COMMAND_OFFSET) as *mut u64;
        core::ptr::write_volatile(alive, 1);

        let mut iteration = 0;
        while iteration < LIVE_CHILD_WATCHDOG_LIMIT {
            if core::ptr::read_volatile(command) == 2 {
                thread_exit(0);
            }
            sys_yield();
            iteration += 1;
        }

        raw_msg(b"CLONEVM_EXEC_TEST: ERROR live child release timeout\n");
        thread_exit(1);
    }
}

fn second_stage_argv() -> [*const u8; 3] {
    [ARG0.as_ptr(), SECOND_STAGE_ARG.as_ptr(), ptr::null()]
}

#[cfg(target_arch = "aarch64")]
unsafe fn probe_live_sibling_exec() {
    let argv = second_stage_argv();
    match process::execv(PROGRAM, argv.as_ptr()) {
        Err(Error::Os(Errno::EAGAIN)) => {
            raw_msg(b"CLONEVM_EXEC_TEST: live sibling refused exec\n");
        }
        Err(error) => {
            drop(error);
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR live-sibling exec returned unexpected errno\n");
            std::process::exit(1);
        }
        Ok(infallible) => match infallible {},
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn probe_live_sibling_exec() {
    raw_msg(b"CLONEVM_EXEC_TEST: SKIP live-sibling probe (no x86 guard, see #468)\n");
}

unsafe fn second_stage() -> ! {
    // These nonblocking calls exercise both futex entry points on the post-exec
    // row at two addresses, so each group key is derived from that row's
    // post-exec identity. They deliberately do not prove a cross-thread
    // rendezvous because #584 tracks an enqueue-to-block lost-wake window.
    raw_msg(b"CLONEVM_EXEC_TEST: second stage\n");

    let shared = map_region(
        4096,
        b"CLONEVM_EXEC_TEST: ERROR post-exec shared mmap failed\n",
    );
    let first_word = shared as *mut u32;
    let second_word = shared.add(core::mem::size_of::<u32>()) as *mut u32;
    core::ptr::write_volatile(first_word, 0);
    core::ptr::write_volatile(second_word, 0);

    if futex_wake(first_word, 1) != 0 {
        raw_msg(b"CLONEVM_EXEC_TEST: ERROR first post-exec futex wake returned nonzero\n");
        std::process::exit(1);
    }
    core::ptr::write_volatile(first_word, 1);
    if futex_wait(first_word, 0) != -(Errno::EAGAIN as i64) {
        raw_msg(b"CLONEVM_EXEC_TEST: ERROR first post-exec futex wait did not return EAGAIN\n");
        std::process::exit(1);
    }

    if futex_wake(second_word, 1) != 0 {
        raw_msg(b"CLONEVM_EXEC_TEST: ERROR second post-exec futex wake returned nonzero\n");
        std::process::exit(1);
    }
    core::ptr::write_volatile(second_word, 2);
    if futex_wait(second_word, 1) != -(Errno::EAGAIN as i64) {
        raw_msg(b"CLONEVM_EXEC_TEST: ERROR second post-exec futex wait did not return EAGAIN\n");
        std::process::exit(1);
    }

    raw_msg(b"CLONEVM_EXEC_TEST: post-exec futex keys derived\n");
    raw_msg(b"CLONEVM_EXEC_TEST: PASS\n");
    std::process::exit(0);
}

unsafe fn first_stage() -> ! {
    raw_msg(b"CLONEVM_EXEC_TEST: start\n");

    let stack = map_region(
        CHILD_STACK_SIZE,
        b"CLONEVM_EXEC_TEST: ERROR initial stack mmap failed\n",
    );
    let shared = map_region(
        4096,
        b"CLONEVM_EXEC_TEST: ERROR initial shared mmap failed\n",
    );
    let alive = shared.add(SHARED_ALIVE_OFFSET) as *mut u64;
    let command = shared.add(SHARED_COMMAND_OFFSET) as *mut u64;
    let tid_addr = shared.add(SHARED_TID_OFFSET) as *mut u32;
    core::ptr::write_volatile(alive, 0);
    core::ptr::write_volatile(command, 0);
    core::ptr::write_volatile(tid_addr, u32::MAX);

    if clone_vm_child(stack, phase_one_child, shared, tid_addr) < 0 {
        raw_msg(b"CLONEVM_EXEC_TEST: ERROR initial clone failed\n");
        std::process::exit(1);
    }

    wait_for_nonzero_u64(
        alive,
        b"CLONEVM_EXEC_TEST: ERROR child did not report live\n",
    );
    raw_msg(b"CLONEVM_EXEC_TEST: child live\n");

    prove_child_still_live(alive, tid_addr);
    probe_live_sibling_exec();

    core::ptr::write_volatile(command, 2);
    wait_for_zero_u32(
        tid_addr,
        b"CLONEVM_EXEC_TEST: ERROR child tid was not cleared after release\n",
    );
    raw_msg(b"CLONEVM_EXEC_TEST: child exited\n");

    let argv = second_stage_argv();
    let exec_result = process::execv(PROGRAM, argv.as_ptr());
    drop(exec_result);
    raw_msg(b"CLONEVM_EXEC_TEST: ERROR post-release exec returned\n");
    std::process::exit(1);
}

fn main() {
    unsafe {
        if std::env::args().nth(1).as_deref() == Some("--second-stage") {
            second_stage();
        }
        first_stage();
    }
}
