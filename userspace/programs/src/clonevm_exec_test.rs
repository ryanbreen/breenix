//! Runtime proof that exec detaches a former CLONE_VM process cleanly.
//!
//! Both architectures prove that a borrowed non-thread child can exec while
//! its parent lives, and that an owner exec leaves its old sharers alive.
//! After exec they exercise futex keys from the new address-space identity.

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::{io, Fd};
use libbreenix::memory;
use libbreenix::process;
use libbreenix::syscall::{nr, raw};
use libbreenix::time::now_monotonic;
use libbreenix::Timespec;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

const SHARED_ALIVE_OFFSET: usize = 0;
const SHARED_COMMAND_OFFSET: usize = 8;
const SHARED_TID_OFFSET: usize = 16;
const RENDEZVOUS_OFFSET: usize = 24;
const READY_OFFSET: usize = 28;
const SECOND_READY_OFFSET: usize = 32;
const CHILD_STATUS_OFFSET: usize = 36;
const RELEASE_OFFSET: usize = 40;
const NEVER_WOKEN_OFFSET: usize = 44;
const RENDEZVOUS_TID_OFFSET: usize = 48;

const CHILD_STACK_SIZE: usize = 64 * 1024;
const SPIN_LIMIT: u64 = 2_000_000;
const CLONE_FLAGS: u64 = 0x00000100 | 0x00000400 | 0x00200000 | 0x01000000;
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;

// A fresh image resets this word; every old sharer must retain its value.
static IMAGE_GENERATION: AtomicU32 = AtomicU32::new(0);

const PROGRAM: &[u8] = b"/usr/libexec/breenix/clonevm_exec_test\0";
const ARG0: &[u8] = b"clonevm_exec_test\0";
const SECOND_STAGE_ARG: &[u8] = b"--second-stage\0";

extern "C" {
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

unsafe fn raw_msg(msg: &[u8]) {
    write(2, msg.as_ptr(), msg.len());
}

/// Custody of the one clone-VM child this process may have alive at a time.
///
/// A parent that exits an error path while its sibling is still scheduled
/// leaves a process with no producer for the word that sibling spins on: the
/// sibling then burns its whole spin bound before the boot can complete. Every
/// parent error exit therefore goes through `fail_closed`, which releases the
/// child and waits for the kernel to clear its `tid_addr` first.
static CHILD_TID_ADDRESS: AtomicPtr<u32> = AtomicPtr::new(ptr::null_mut());
/// The `u32` release word of the post-exec rendezvous child, written to 1.
static CHILD_RELEASE_WORD: AtomicPtr<u32> = AtomicPtr::new(ptr::null_mut());
/// The `u64` command word of the first-stage child, written to 2.
static CHILD_RELEASE_COMMAND: AtomicPtr<u64> = AtomicPtr::new(ptr::null_mut());

fn take_child_custody(release: *mut u32, command: *mut u64, tid_addr: *mut u32) {
    CHILD_RELEASE_WORD.store(release, Ordering::SeqCst);
    CHILD_RELEASE_COMMAND.store(command, Ordering::SeqCst);
    CHILD_TID_ADDRESS.store(tid_addr, Ordering::SeqCst);
}

/// Called once the child has been reaped on the success path, so a later
/// failure does not wait on a tid that is already clear.
fn drop_child_custody() {
    CHILD_TID_ADDRESS.store(ptr::null_mut(), Ordering::SeqCst);
    CHILD_RELEASE_WORD.store(ptr::null_mut(), Ordering::SeqCst);
    CHILD_RELEASE_COMMAND.store(ptr::null_mut(), Ordering::SeqCst);
}

/// Release the child and wait for the kernel to clear its `tid_addr`. Taking
/// custody with a swap makes this at-most-once even if a later failure path
/// re-enters it.
unsafe fn release_and_reap_child() {
    let tid_addr = CHILD_TID_ADDRESS.swap(ptr::null_mut(), Ordering::SeqCst);
    if tid_addr.is_null() {
        return;
    }

    let release = CHILD_RELEASE_WORD.swap(ptr::null_mut(), Ordering::SeqCst);
    if !release.is_null() {
        core::ptr::write_volatile(release, 1);
    }
    let command = CHILD_RELEASE_COMMAND.swap(ptr::null_mut(), Ordering::SeqCst);
    if !command.is_null() {
        core::ptr::write_volatile(command, 2);
    }

    let mut iteration = 0;
    while iteration < SPIN_LIMIT {
        if core::ptr::read_volatile(tid_addr) == 0 {
            return;
        }
        sys_yield();
        iteration += 1;
    }

    raw_msg(b"CLONEVM_EXEC_TEST: ERROR child was not reaped on the fail-closed exit\n");
}

/// The only parent error exit in this program.
unsafe fn fail_closed(msg: &[u8]) -> ! {
    release_and_reap_child();
    raw_msg(msg);
    std::process::exit(1);
}

/// Yield through the library wrapper, which declares that the kernel writes
/// the syscall return register (#608). The hand-rolled block this replaces
/// named that register as an input only, so the compiler was entitled to hoist
/// the syscall-number load out of every spin below and each later trap ran
/// with the previous call's return value as its syscall number.
unsafe fn sys_yield() {
    raw::syscall0(nr::YIELD);
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

unsafe fn futex_wait_with_timeout(
    word: *mut u32,
    expected: u32,
    timeout: *const Timespec,
) -> i64 {
    raw::syscall6(
        nr::FUTEX,
        word as u64,
        FUTEX_WAIT,
        expected as u64,
        timeout as u64,
        0,
        0,
    ) as i64
}

unsafe fn futex_wake(word: *mut u32, count: u32) -> i64 {
    raw::syscall6(nr::FUTEX, word as u64, FUTEX_WAKE, count as u64, 0, 0, 0) as i64
}

/// Readiness precedes FUTEX_WAIT's atomic queue insertion. Wait for that
/// insertion rather than treating a wake with no waiter as a failed handoff.
/// Success still requires waking exactly one queued waiter.
unsafe fn wake_queued_waiter(word: *mut u32) -> bool {
    let deadline = monotonic_ns(b"CLONEVM_EXEC_TEST: ERROR wake clock failed\n")
        .saturating_add(1_000_000_000);
    loop {
        match futex_wake(word, 1) {
            1 => return true,
            0 => {}
            _ => return false,
        }
        if monotonic_ns(b"CLONEVM_EXEC_TEST: ERROR wake clock failed\n") >= deadline {
            return false;
        }
        sys_yield();
    }
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
            fail_closed(error);
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

    fail_closed(error);
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

    fail_closed(error);
}

unsafe fn wait_for_nonzero_u32(address: *mut u32, error: &[u8]) {
    let mut iteration = 0;
    while iteration < SPIN_LIMIT {
        if core::ptr::read_volatile(address) != 0 {
            return;
        }
        sys_yield();
        iteration += 1;
    }

    fail_closed(error);
}

unsafe fn wait_for_child_status(address: *mut u32, expected: u32, error: &[u8]) {
    let mut iteration = 0;
    while iteration < SPIN_LIMIT {
        let status = core::ptr::read_volatile(address);
        if status == expected {
            return;
        }
        if status == u32::MAX {
            fail_closed(error);
        }
        sys_yield();
        iteration += 1;
    }

    fail_closed(error);
}

unsafe fn monotonic_ns(error: &[u8]) -> u64 {
    let timestamp = match now_monotonic() {
        Ok(timestamp) => timestamp,
        Err(error_value) => {
            drop(error_value);
            fail_closed(error);
        }
    };
    if timestamp.tv_sec < 0 || timestamp.tv_nsec < 0 || timestamp.tv_nsec >= 1_000_000_000 {
        fail_closed(error);
    }
    timestamp.tv_sec as u64 * 1_000_000_000 + timestamp.tv_nsec as u64
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

    fail_closed(b"CLONEVM_EXEC_TEST: ERROR child was not live before exec probe\n");
}

extern "C" fn post_exec_rendezvous_child(arg: *mut u8) -> *mut u8 {
    unsafe {
        let rendezvous = arg.add(RENDEZVOUS_OFFSET) as *mut u32;
        let ready = arg.add(READY_OFFSET) as *mut u32;
        let second_word = arg.add(core::mem::size_of::<u32>()) as *mut u32;
        let second_ready = arg.add(SECOND_READY_OFFSET) as *mut u32;
        let child_status = arg.add(CHILD_STATUS_OFFSET) as *mut u32;
        let release = arg.add(RELEASE_OFFSET) as *mut u32;

        if !spin_until_u32(ready, 1) {
            core::ptr::write_volatile(child_status, u32::MAX);
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR sibling did not observe parent wait readiness\n");
            thread_exit(1);
        }
        if !wake_queued_waiter(rendezvous) {
            core::ptr::write_volatile(child_status, u32::MAX);
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR sibling wake of parent failed\n");
            thread_exit(1);
        }
        core::ptr::write_volatile(child_status, 1);

        core::ptr::write_volatile(second_ready, 1);
        if futex_wait(second_word, 0) != 0 {
            core::ptr::write_volatile(child_status, u32::MAX);
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR sibling wait was not woken by parent\n");
            thread_exit(1);
        }
        core::ptr::write_volatile(child_status, 2);

        if !spin_until_u32(release, 1) {
            core::ptr::write_volatile(child_status, u32::MAX);
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR sibling release timed out\n");
            thread_exit(1);
        }
        thread_exit(0);
    }
}

extern "C" fn phase_one_child(arg: *mut u8) -> *mut u8 {
    unsafe {
        let alive = arg.add(SHARED_ALIVE_OFFSET) as *mut u64;
        let command = arg.add(SHARED_COMMAND_OFFSET) as *mut u64;
        core::ptr::write_volatile(alive, process::getpid().unwrap().raw());

        let mut iteration = 0;
        // Bounded like every other wait here, by yields. sched_yield with no
        // other runnable thread returns at once, so a bound of 10,000 yields
        // ran out in well under a second, before the parent's exec probe was
        // done; it only held while each such yield idled the CPU for a tick.
        while iteration < SPIN_LIMIT {
            if core::ptr::read_volatile(command) == 3 {
                let argv = second_stage_argv();
                let result = process::execv(PROGRAM, argv.as_ptr());
                drop(result);
                raw_msg(b"CLONEVM_EXEC_TEST: ERROR borrowed child exec returned\n");
                thread_exit(1);
            }
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

/// CLONE_FILES shares descriptor custody, so neither old sharer closes the
/// other endpoint. Nonblocking pipes keep a missing producer bounded instead.
unsafe fn receive_handoff(fd: Fd) -> bool {
    let deadline = monotonic_ns(b"CLONEVM_EXEC_TEST: ERROR handoff clock failed\n")
        .saturating_add(5_000_000_000);
    loop {
        let mut byte = [0];
        match io::read(fd, &mut byte) {
            Ok(1) => return byte == [1],
            Err(Error::Os(Errno::EAGAIN)) => {},
            _ => return false,
        }
        if monotonic_ns(b"CLONEVM_EXEC_TEST: ERROR handoff clock failed\n") >= deadline {
            return false;
        }
        sys_yield();
    }
}

/// These two processes retain the original mapping across their owner's exec.
/// Their handshake must still use the same futex key after custody transfers.
extern "C" fn retained_waiter(arg: *mut u8) -> *mut u8 {
    unsafe {
        core::ptr::write_volatile(arg.add(READY_OFFSET) as *mut u32, 1);
        let timeout = Timespec { tv_sec: 5, tv_nsec: 0 };
        if futex_wait_with_timeout(arg.add(RENDEZVOUS_OFFSET) as *mut u32, 0, &timeout) != 0 {
            thread_exit(1);
        }
        core::ptr::write_volatile(arg.add(CHILD_STATUS_OFFSET) as *mut u32, 1);
        thread_exit(0);
    }
}

extern "C" fn retained_coordinator(arg: *mut u8) -> *mut u8 {
    unsafe {
        let trigger = Fd::from_raw(core::ptr::read_volatile(arg.add(56) as *mut u64));
        let report = Fd::from_raw(core::ptr::read_volatile(arg.add(64) as *mut u64));
        let stack = map_region(CHILD_STACK_SIZE,
            b"CLONEVM_EXEC_TEST: ERROR retained waiter stack failed\n");
        let tid = arg.add(RENDEZVOUS_TID_OFFSET) as *mut u32;
        core::ptr::write_volatile(tid, u32::MAX);
        if clone_vm_child(stack, retained_waiter, arg, tid) < 0 {
            thread_exit(1);
        }
        if !spin_until_u32(arg.add(READY_OFFSET) as *mut u32, 1) {
            thread_exit(1);
        }
        core::ptr::write_volatile(arg as *mut u64, process::getpid().unwrap().raw());
        if !receive_handoff(trigger) {
            thread_exit(1);
        }
        // Only the new image writes the trigger. Reaching this point
        // proves the owner has exec'd; both accesses below use the old root.
        if IMAGE_GENERATION.load(Ordering::SeqCst) != 37
            || core::ptr::read_volatile(arg.add(SHARED_COMMAND_OFFSET) as *mut u64) != 7
            || !wake_queued_waiter(arg.add(RENDEZVOUS_OFFSET) as *mut u32) {
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR old mapping or futex identity lost\n");
            thread_exit(1);
        }
        if !spin_until_u32(tid, 0) {
            thread_exit(1);
        }
        if core::ptr::read_volatile(arg.add(CHILD_STATUS_OFFSET) as *mut u32) != 1
            || io::write(report, &[1]).ok() != Some(1) {
            thread_exit(1);
        }
        thread_exit(0);
    }
}

unsafe fn owner_second_stage() -> ! {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR owner handoff arguments\n");
    }
    let trigger = Fd::from_raw(args[2].parse().unwrap());
    let report = Fd::from_raw(args[3].parse().unwrap());
    let child: i32 = args[4].parse().unwrap();
    let mut status = 0;
    if io::write(trigger, &[1]).ok() != Some(1)
        || !receive_handoff(report)
        || process::waitpid(child, &mut status, 0).map(|pid| pid.raw() as i32).ok() != Some(child)
        || !process::wifexited(status) || process::wexitstatus(status) != 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR retained sharers failed after owner exec\n");
    }
    raw_msg(b"CLONEVM_EXEC_TEST: owner exec preserved sharer mapping and futex identity\n");
    second_stage();
}

unsafe fn second_stage() -> ! {
    if IMAGE_GENERATION.load(Ordering::SeqCst) != 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR new image retained old memory\n");
    }
    // These nonblocking calls exercise both futex entry points on the post-exec
    // row at two addresses, so each group key is derived from that row's
    // post-exec identity.
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
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR first post-exec futex wake returned nonzero\n");
    }
    core::ptr::write_volatile(first_word, 1);
    if futex_wait(first_word, 0) != -(Errno::EAGAIN as i64) {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR first post-exec futex wait did not return EAGAIN\n");
    }

    if futex_wake(second_word, 1) != 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR second post-exec futex wake returned nonzero\n");
    }
    core::ptr::write_volatile(second_word, 2);
    if futex_wait(second_word, 1) != -(Errno::EAGAIN as i64) {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR second post-exec futex wait did not return EAGAIN\n");
    }

    let rendezvous = shared.add(RENDEZVOUS_OFFSET) as *mut u32;
    let ready = shared.add(READY_OFFSET) as *mut u32;
    let second_ready = shared.add(SECOND_READY_OFFSET) as *mut u32;
    let child_status = shared.add(CHILD_STATUS_OFFSET) as *mut u32;
    let release = shared.add(RELEASE_OFFSET) as *mut u32;
    let never_woken = shared.add(NEVER_WOKEN_OFFSET) as *mut u32;
    let tid_addr = shared.add(RENDEZVOUS_TID_OFFSET) as *mut u32;
    core::ptr::write_volatile(rendezvous, 0);
    core::ptr::write_volatile(second_word, 0);
    core::ptr::write_volatile(ready, 0);
    core::ptr::write_volatile(second_ready, 0);
    core::ptr::write_volatile(child_status, 0);
    core::ptr::write_volatile(release, 0);
    core::ptr::write_volatile(never_woken, 0);
    core::ptr::write_volatile(tid_addr, u32::MAX);

    let sibling_stack = map_region(
        CHILD_STACK_SIZE,
        b"CLONEVM_EXEC_TEST: ERROR rendezvous stack mmap failed\n",
    );
    if clone_vm_child(sibling_stack, post_exec_rendezvous_child, shared, tid_addr) < 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR rendezvous clone failed\n");
    }
    take_child_custody(release, ptr::null_mut(), tid_addr);

    core::ptr::write_volatile(ready, 1);
    if futex_wait(rendezvous, 0) != 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR parent wait was not woken by sibling\n");
    }
    wait_for_child_status(
        child_status,
        1,
        b"CLONEVM_EXEC_TEST: ERROR sibling did not complete parent wake\n",
    );

    wait_for_nonzero_u32(
        second_ready,
        b"CLONEVM_EXEC_TEST: ERROR sibling did not report second wait readiness\n",
    );
    if !wake_queued_waiter(second_word) {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR parent wake of sibling failed\n");
    }
    wait_for_child_status(
        child_status,
        2,
        b"CLONEVM_EXEC_TEST: ERROR sibling did not complete parent wake handoff\n",
    );

    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 50_000_000,
    };
    let timeout_start = monotonic_ns(
        b"CLONEVM_EXEC_TEST: ERROR monotonic clock failed before futex timeout\n",
    );
    if futex_wait_with_timeout(never_woken, 0, &timeout) != -(Errno::ETIMEDOUT as i64) {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT\n");
    }
    let elapsed = monotonic_ns(
        b"CLONEVM_EXEC_TEST: ERROR monotonic clock failed after futex timeout\n",
    )
    .saturating_sub(timeout_start);
    if elapsed < 50_000_000 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR futex timeout elapsed less than 50ms\n");
    }

    core::ptr::write_volatile(release, 1);
    wait_for_zero_u32(
        tid_addr,
        b"CLONEVM_EXEC_TEST: ERROR rendezvous child tid was not cleared after release\n",
    );
    drop_child_custody();
    if std::env::args().nth(1).as_deref() == Some("--owner-second-stage") {
        raw_msg(b"CLONEVM_EXEC_TEST: post-exec rendezvous complete\n");
    } else {
        raw_msg(b"CLONEVM_EXEC_TEST: borrowed image rendezvous complete\n");
    }

    raw_msg(b"CLONEVM_EXEC_TEST: post-exec futex keys derived\n");
    raw_msg(b"CLONEVM_EXEC_TEST: PASS\n");
    std::process::exit(0);
}

unsafe fn first_stage() -> ! {
    raw_msg(b"CLONEVM_EXEC_TEST: start\n");
    IMAGE_GENERATION.store(37, Ordering::SeqCst);
    let invalid = raw::syscall6(nr::CLONE, 0x100 | 0x10000, 0x1000,
                               phase_one_child as *const () as u64, 0, 0, 0) as i64;
    if invalid != -(Errno::EINVAL as i64) {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR CLONE_THREAD accepted without CLONE_SIGHAND\n");
    }

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

    // A non-thread child must detach while its owning parent is still live.
    // The new image runs the complete post-exec futex test, and must exit zero.
    let child = clone_vm_child(stack, phase_one_child, shared, tid_addr);
    if child < 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR borrowed clone failed\n");
    }
    wait_for_nonzero_u64(alive, b"CLONEVM_EXEC_TEST: ERROR borrowed child not live\n");
    // clone returns a TID in Breenix's function-entry ABI; waitpid selects
    // the PID published by the child, never that scheduler identity.
    let child_pid = core::ptr::read_volatile(alive) as i32;
    core::ptr::write_volatile(command, 3);
    let mut status = 0;
    if process::waitpid(child_pid, &mut status, 0).map(|pid| pid.raw() as i32).ok() != Some(child_pid)
        || !process::wifexited(status) || process::wexitstatus(status) != 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR borrowed child image failed\n");
    }
    if IMAGE_GENERATION.load(Ordering::SeqCst) != 37 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR borrowed exec changed parent memory\n");
    }
    raw_msg(b"CLONEVM_EXEC_TEST: borrowed child exec passed while parent live\n");
    core::ptr::write_volatile(alive, 0);
    core::ptr::write_volatile(command, 0);
    core::ptr::write_volatile(tid_addr, u32::MAX);

    let (trigger_read, trigger_write) = io::pipe2(0x800).unwrap();
    let (report_read, report_write) = io::pipe2(0x800).unwrap();
    core::ptr::write_volatile(shared.add(56) as *mut u64, trigger_read.raw());
    core::ptr::write_volatile(shared.add(64) as *mut u64, report_write.raw());
    core::ptr::write_volatile(command, 7);
    if clone_vm_child(stack, retained_coordinator, shared, tid_addr) < 0 {
        fail_closed(b"CLONEVM_EXEC_TEST: ERROR retained coordinator clone failed\n");
    }
    take_child_custody(ptr::null_mut(), ptr::null_mut(), tid_addr);
    wait_for_nonzero_u64(alive, b"CLONEVM_EXEC_TEST: ERROR retained sharers not live\n");
    prove_child_still_live(alive, tid_addr);
    let child_pid = core::ptr::read_volatile(alive);
    // Pipe descriptors and the child PID survive exec; no old address is
    // passed to the new image. Its futex checks run on a fresh mapping.
    let owner_arg = b"--owner-second-stage\0";
    let trigger_arg = format!("{}\0", trigger_write.raw());
    let report_arg = format!("{}\0", report_read.raw());
    let child_arg = format!("{}\0", child_pid);
    let argv = [ARG0.as_ptr(), owner_arg.as_ptr(), trigger_arg.as_ptr(),
        report_arg.as_ptr(), child_arg.as_ptr(), ptr::null()];
    let result = process::execv(PROGRAM, argv.as_ptr());
    drop(result);
    fail_closed(b"CLONEVM_EXEC_TEST: ERROR owner exec returned\n");
}

fn main() {
    unsafe {
        match std::env::args().nth(1).as_deref() {
            Some("--second-stage") => second_stage(),
            Some("--owner-second-stage") => owner_second_stage(),
            _ => first_stage(),
        }
    }
}
