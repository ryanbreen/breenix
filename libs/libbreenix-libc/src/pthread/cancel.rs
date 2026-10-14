//! POSIX cancellation. Deferred delivery redirects only a PC in our syscall
//! window; async delivery is explicitly requested by the application.
//!
//! The cancel signal (signal 32, reserved below SIGRTMIN) is installed with
//! SA_RESTART by the first pthread_cancel that sends it, so a program that
//! never cancels a thread keeps the SIG_DFL action exec gave it. It is blocked
//! in the signal mask while a thread has cancellation disabled, so a request
//! sent then stays pending (POSIX.1-2024 2.9.5.2) instead of interrupting a
//! wait with EINTR.
use super::*;
use crate::signal::{CANCEL_MASK, CANCEL_SIGNAL};

const REQUESTED: u32 = 1;
const DISABLED: u32 = 2;
const ASYNC: u32 = 4;
/// A new thread inherited its creator's mask with the cancel signal blocked
/// (the creator had cancellation disabled); `thread_start` unblocks it.
const INHERITED_BLOCK: u32 = 8;
const CANCEL_RETURN: i64 = i64::MIN;
/// Linux UAPI sigprocmask `how` values (also in include/signal.h).
const SIG_BLOCK: u64 = 0;
const SIG_UNBLOCK: u64 = 1;

#[repr(C)]
pub(super) struct Cleanup {
    previous: *mut Cleanup,
    routine: Option<unsafe extern "C" fn(*mut u8)>,
    arg: *mut u8,
}
impl Cleanup {
    pub(super) fn new() -> Self {
        Self { previous: null_mut(), routine: None, arg: null_mut() }
    }
}

/// The calling thread's descriptor; a thread without one cannot keep any
/// thread state, which is fatal like the other entry points.
unsafe fn this_thread() -> *mut Thread {
    let p = current();
    if p.is_null() { super::super::exit_group(127); }
    p
}

#[no_mangle]
pub unsafe extern "C" fn __breenix_cleanup_push(
    frame: *mut Cleanup, routine: unsafe extern "C" fn(*mut u8), arg: *mut u8,
) {
    let p = this_thread();
    *frame = Cleanup { previous: (*p).cleanup, routine: Some(routine), arg };
    (*p).cleanup = frame;
}
#[no_mangle]
pub unsafe extern "C" fn __breenix_cleanup_pop(frame: *mut Cleanup, execute: i32) {
    (*this_thread()).cleanup = (*frame).previous;
    if execute != 0 { ((*frame).routine.unwrap())((*frame).arg); }
}
pub(super) unsafe fn finish(p: *mut Thread) {
    (*p).cancel.fetch_or(DISABLED, AcqRel);
    while !(*p).cleanup.is_null() {
        let frame = (*p).cleanup;
        (*p).cleanup = (*frame).previous;
        ((*frame).routine.unwrap())((*frame).arg);
    }
}

unsafe fn mask_cancel(how: u64) {
    raw::syscall4(nr::SIGPROCMASK, how, &CANCEL_MASK as *const u64 as u64, 0, 8);
}
/// The cancel signal's bit when the calling thread has cancellation disabled,
/// so a mask the application installs keeps that signal blocked meanwhile.
pub(crate) unsafe fn reserved_block() -> u64 {
    if !RUNTIME_READY.load(Acquire) { return 0; }
    let p = descriptor_from_tp();
    if !p.is_null() && (*p).cancel.load(Relaxed) & DISABLED != 0 { CANCEL_MASK } else { 0 }
}
/// pthread_create: a creator with cancellation disabled hands its blocked
/// cancel signal to the new thread, which starts enabled.
pub(super) unsafe fn creating(child: *mut Thread) {
    if reserved_block() != 0 { (*child).cancel.store(INHERITED_BLOCK, Relaxed); }
}
pub(super) unsafe fn thread_start(p: *mut Thread) {
    if (*p).cancel.fetch_and(!INHERITED_BLOCK, Relaxed) & INHERITED_BLOCK != 0 {
        mask_cancel(SIG_UNBLOCK);
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cancel(handle: usize) -> i32 {
    REGISTRY.lock();
    let p = find(handle);
    if p.is_null() { REGISTRY.unlock(); return ESRCH; }
    if (*p).clear.load(Acquire) == 0 { REGISTRY.unlock(); return 0; }
    let prior = (*p).cancel.fetch_or(REQUESTED, AcqRel);
    let id = (*p).id.load(Relaxed);
    REGISTRY.unlock();
    // The signal only wakes a thread blocked at a cancellation point. A second
    // request needs none, and a thread with cancellation disabled sees the
    // request when it next enters the syscall window (its enable is ordered
    // after this fetch_or on the same word), so neither queues a signal.
    if prior & (REQUESTED | DISABLED) != 0 { return 0; }
    let installed = install_handler();
    if installed < 0 { return -installed as i32; }
    let r = raw::syscall3(nr::TGKILL, pid() as u64, id as u64, CANCEL_SIGNAL as u64) as i64;
    // ESRCH: the thread ended after the registry check, and a request to an
    // ended thread has nothing left to do.
    if r < 0 && r != -(ESRCH as i64) { -r as i32 } else { 0 }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setcancelstate(state: i32, old: *mut i32) -> i32 {
    if !(0..=1).contains(&state) { return EINVAL; }
    let p = this_thread();
    let was = (*p).cancel.load(Relaxed) & DISABLED != 0;
    if state == 1 && !was {
        // Block first: a cancel signal sent from here on stays pending.
        mask_cancel(SIG_BLOCK);
        (*p).cancel.fetch_or(DISABLED, AcqRel);
    } else if state == 0 && was {
        let prior = (*p).cancel.fetch_and(!DISABLED, AcqRel);
        mask_cancel(SIG_UNBLOCK);
        // Enabling deferred cancellation does not itself introduce a point.
        if prior & ASYNC != 0 { pthread_testcancel(); }
    }
    if !old.is_null() { *old = was as i32; }
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setcanceltype(kind: i32, old: *mut i32) -> i32 {
    if !(0..=1).contains(&kind) { return EINVAL; }
    let p = this_thread();
    let prior = if kind == 1 {
        (*p).cancel.fetch_or(ASYNC, AcqRel)
    } else { (*p).cancel.fetch_and(!ASYNC, AcqRel) };
    if !old.is_null() { *old = ((prior & ASYNC) != 0) as i32; }
    if kind == 1 { pthread_testcancel(); }
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_testcancel() {
    if !RUNTIME_READY.load(Acquire) { return; }
    let p = descriptor_from_tp();
    if !p.is_null() && (*p).cancel.load(Acquire) & (REQUESTED | DISABLED) == REQUESTED {
        pthread_exit(usize::MAX as *mut u8);
    }
}

extern "C" {
    #[link_name = "__breenix_cancel_entry"]
    fn syscall_window(nr: u64, args: *const u64, state: *const AtomicU32) -> i64;
    static __breenix_cancel_begin: u8;
    static __breenix_cancel_end: u8;
    static __breenix_cancel_return: u8;
}

/// The PC fields are from kernel/src/signal/types.rs (Linux UAPI layout).
unsafe extern "C" fn handler(_sig: i32, _info: *mut u8, context: *mut u8) {
    let p = descriptor_from_tp();
    if p.is_null() { return; }
    let state = (*p).cancel.load(Acquire);
    if state & (REQUESTED | DISABLED) != REQUESTED { return; }
    if state & ASYNC != 0 { pthread_exit(usize::MAX as *mut u8); }
    #[cfg(target_arch = "aarch64")]
    let pc = context.add(176 + 264) as *mut usize;
    #[cfg(target_arch = "x86_64")]
    let pc = context.add(40 + 128) as *mut usize;
    // The window ends at the syscall instruction. A PC on it means the call
    // has not run, or a restartable wait was interrupted and the kernel wound
    // the PC back to run it again; a PC at the end label means it completed,
    // and its result is returned (an EINTR result is acted on by
    // cancel_syscall).
    let start = core::ptr::addr_of!(__breenix_cancel_begin) as usize;
    let end = core::ptr::addr_of!(__breenix_cancel_end) as usize;
    if *pc >= start && *pc < end {
        *pc = core::ptr::addr_of!(__breenix_cancel_return) as usize;
    }
}
/// The runtime starts with the cancel signal unblocked. An image exec'd by a
/// thread that had cancellation disabled inherits it blocked, and possibly
/// pending: a pthread_cancel that ran between that thread's block and its
/// DISABLED store queued the signal. Exec reset the handler to SIG_DFL, which
/// would end this image, so a pending cancel signal is discarded first: setting
/// SIG_IGN discards it (POSIX.1-2024 2.4.3), and the previous action is put
/// back.
pub(super) unsafe fn runtime_start() {
    let mut blocked: u64 = 0;
    raw::syscall4(nr::SIGPROCMASK, SIG_BLOCK, 0, &mut blocked as *mut u64 as u64, 8);
    if blocked & CANCEL_MASK == 0 {
        return;
    }
    let ignore = libbreenix::signal::Sigaction {
        handler: libbreenix::signal::SIG_IGN, flags: 0, restorer: 0, mask: 0,
    };
    let mut previous = libbreenix::signal::Sigaction {
        handler: libbreenix::signal::SIG_DFL, flags: 0, restorer: 0, mask: 0,
    };
    raw::syscall4(nr::SIGACTION, CANCEL_SIGNAL as u64, &ignore as *const _ as u64,
        &mut previous as *mut _ as u64, 8);
    raw::syscall4(nr::SIGACTION, CANCEL_SIGNAL as u64, &previous as *const _ as u64, 0, 8);
    mask_cancel(SIG_UNBLOCK);
}

static HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Install the cancel signal's handler before the first signal is sent. A
/// fork child inherits the handler and this flag together; exec resets both.
/// Returns the raw negative-errno kernel result.
unsafe fn install_handler() -> i64 {
    if HANDLER_INSTALLED.load(Acquire) {
        return 0;
    }
    let action = libbreenix::signal::Sigaction {
        handler: handler as *const () as u64,
        flags: libbreenix::signal::SA_SIGINFO | libbreenix::signal::SA_RESTORER
            | libbreenix::signal::SA_RESTART,
        restorer: libbreenix::signal::__restore_rt as *const () as u64,
        mask: 0,
    };
    let r = raw::syscall4(nr::SIGACTION, CANCEL_SIGNAL as u64, &action as *const _ as u64, 0, 8) as i64;
    if r == 0 {
        HANDLER_INSTALLED.store(true, Release);
    }
    r
}

/// Returns the raw negative-errno kernel result. Assembly closes the race
/// between observing the pending request and entering the blocking syscall.
pub(crate) unsafe fn cancel_syscall(nr: u64, a: [u64; 6]) -> i64 {
    if !RUNTIME_READY.load(Acquire) {
        return raw::syscall6(nr, a[0], a[1], a[2], a[3], a[4], a[5]) as i64;
    }
    let p = descriptor_from_tp();
    if p.is_null() {
        return raw::syscall6(nr, a[0], a[1], a[2], a[3], a[4], a[5]) as i64;
    }
    let r = syscall_window(nr, a.as_ptr(), &(*p).cancel);
    // A wait the kernel does not restart (nanosleep, poll, futex) returns
    // EINTR when the cancel signal interrupts it; acting then has the side
    // effects of that EINTR return, as POSIX.1-2024 2.9.5.2 requires.
    if r == CANCEL_RETURN || r == -(EINTR as i64) { pthread_testcancel(); }
    if r == CANCEL_RETURN { -(EINTR as i64) } else { r }
}

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
        ".text",
        ".balign 16",
        ".global __breenix_cancel_entry",
        ".hidden __breenix_cancel_entry",
        ".type __breenix_cancel_entry, %function",
        "__breenix_cancel_entry:",
        ".global __breenix_cancel_begin",
        ".hidden __breenix_cancel_begin",
        "__breenix_cancel_begin:",
        "ldar w9, [x2]",
        "and w9, w9, #3",
        "cmp w9, #1",
        "b.eq __breenix_cancel_return",
        "mov x8, x0",
        "mov x9, x1",
        "ldp x0, x1, [x9]",
        "ldp x2, x3, [x9, #16]",
        "ldp x4, x5, [x9, #32]",
        "svc #0",
        ".global __breenix_cancel_end",
        ".hidden __breenix_cancel_end",
        "__breenix_cancel_end:",
        "ret",
        ".global __breenix_cancel_return",
        ".hidden __breenix_cancel_return",
        "__breenix_cancel_return:",
        "mov x0, #0x8000000000000000",
        "ret",
        ".size __breenix_cancel_entry, . - __breenix_cancel_entry",
);
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
        ".text",
        ".balign 16",
        ".global __breenix_cancel_entry",
        ".hidden __breenix_cancel_entry",
        ".type __breenix_cancel_entry, %function",
        "__breenix_cancel_entry:",
        ".global __breenix_cancel_begin",
        ".hidden __breenix_cancel_begin",
        "__breenix_cancel_begin:",
        "mov eax, [rdx]",
        "and eax, 3",
        "cmp eax, 1",
        "je __breenix_cancel_return",
        "mov rax, rdi",
        "mov r11, rsi",
        "mov rdi, [r11]",
        "mov rsi, [r11 + 8]",
        "mov rdx, [r11 + 16]",
        "mov r10, [r11 + 24]",
        "mov r8, [r11 + 32]",
        "mov r9, [r11 + 40]",
        "syscall",
        ".global __breenix_cancel_end",
        ".hidden __breenix_cancel_end",
        "__breenix_cancel_end:",
        "ret",
        ".global __breenix_cancel_return",
        ".hidden __breenix_cancel_return",
        "__breenix_cancel_return:",
        "mov rax, 0x8000000000000000",
        "ret",
        ".size __breenix_cancel_entry, . - __breenix_cancel_entry",
);
