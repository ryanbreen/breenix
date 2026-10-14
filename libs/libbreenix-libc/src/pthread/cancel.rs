//! POSIX cancellation. Deferred delivery redirects only a PC in our syscall
//! window; async delivery is explicitly requested by the application.
use super::*;
use crate::signal::{CANCEL_MASK, CANCEL_SIGNAL};

const REQUESTED: u32 = 1;
const DISABLED: u32 = 2;
const ASYNC: u32 = 4;
const CANCEL_RETURN: i64 = i64::MIN;

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

#[no_mangle]
pub unsafe extern "C" fn __breenix_cleanup_push(
    frame: *mut Cleanup, routine: unsafe extern "C" fn(*mut u8), arg: *mut u8,
) {
    let p = current();
    *frame = Cleanup { previous: (*p).cleanup, routine: Some(routine), arg };
    (*p).cleanup = frame;
}
#[no_mangle]
pub unsafe extern "C" fn __breenix_cleanup_pop(frame: *mut Cleanup, execute: i32) {
    (*current()).cleanup = (*frame).previous;
    if execute != 0 { ((*frame).routine.unwrap())((*frame).arg); }
}
pub(super) unsafe fn finish(p: *mut Thread) {
    (*p).cancel.store(DISABLED, Release);
    while !(*p).cleanup.is_null() {
        let frame = (*p).cleanup;
        (*p).cleanup = (*frame).previous;
        ((*frame).routine.unwrap())((*frame).arg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cancel(handle: usize) -> i32 {
    REGISTRY.lock();
    let p = find(handle);
    if p.is_null() { REGISTRY.unlock(); return ESRCH; }
    if (*p).clear.load(Acquire) == 0 { REGISTRY.unlock(); return 0; }
    (*p).cancel.fetch_or(REQUESTED, Release);
    let id = (*p).id.load(Relaxed);
    REGISTRY.unlock();
    let r = raw::syscall3(nr::TGKILL, pid() as u64, id as u64, CANCEL_SIGNAL as u64) as i64;
    if r < 0 { -r as i32 } else { 0 }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setcancelstate(state: i32, old: *mut i32) -> i32 {
    if !(0..=1).contains(&state) { return EINVAL; }
    let p = current();
    let prior = if state == 1 {
        (*p).cancel.fetch_or(DISABLED, core::sync::atomic::Ordering::AcqRel)
    } else { (*p).cancel.fetch_and(!DISABLED, core::sync::atomic::Ordering::AcqRel) };
    if !old.is_null() { *old = ((prior & DISABLED) != 0) as i32; }
    // Enabling deferred cancellation does not itself introduce a point.
    if state == 0 && prior & ASYNC != 0 { pthread_testcancel(); }
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setcanceltype(kind: i32, old: *mut i32) -> i32 {
    if !(0..=1).contains(&kind) { return EINVAL; }
    let p = current();
    let prior = if kind == 1 {
        (*p).cancel.fetch_or(ASYNC, core::sync::atomic::Ordering::AcqRel)
    } else { (*p).cancel.fetch_and(!ASYNC, core::sync::atomic::Ordering::AcqRel) };
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
    let start = core::ptr::addr_of!(__breenix_cancel_begin) as usize;
    let end = core::ptr::addr_of!(__breenix_cancel_end) as usize;
    if *pc >= start && *pc < end {
        *pc = core::ptr::addr_of!(__breenix_cancel_return) as usize;
    }
}
pub(super) unsafe fn install() {
    let action = libbreenix::signal::Sigaction {
        handler: handler as *const () as u64,
        flags: libbreenix::signal::SA_SIGINFO | libbreenix::signal::SA_RESTORER,
        restorer: libbreenix::signal::__restore_rt as *const () as u64,
        mask: 0,
    };
    if (raw::syscall4(nr::SIGACTION, CANCEL_SIGNAL as u64, &action as *const _ as u64, 0, 8) as i64) < 0 {
        super::super::exit_group(127);
    }
    raw::syscall4(nr::SIGPROCMASK, 1, &CANCEL_MASK as *const u64 as u64, 0, 8);
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
    if r == CANCEL_RETURN { pthread_testcancel(); }
    r
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
        "nop",
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
        "nop",
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
