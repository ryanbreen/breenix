//! Program termination: atexit, exit, _Exit, at_quick_exit, quick_exit and abort
//! (ISO C17 7.22.4, POSIX.1-2024).
//!
//! exit runs the atexit handlers in the reverse order of their registration (a handler
//! may register more, which then run next), flushes every stdio stream, and ends the
//! process with _Exit.

use core::sync::atomic::{AtomicBool, Ordering};

/// ISO C requires room for at least 32 handlers in each list.
const SLOTS: usize = 64;

struct Handlers {
    busy: AtomicBool,
    count: usize,
    list: [Option<extern "C" fn()>; SLOTS],
}

static mut AT_EXIT: Handlers = Handlers { busy: AtomicBool::new(false), count: 0, list: [None; SLOTS] };
static mut AT_QUICK_EXIT: Handlers = Handlers { busy: AtomicBool::new(false), count: 0, list: [None; SLOTS] };

impl Handlers {
    fn lock(&self) {
        while self.busy.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }
    fn unlock(&self) {
        self.busy.store(false, Ordering::Release);
    }
    fn push(&mut self, f: extern "C" fn()) -> i32 {
        self.lock();
        let ok = self.count < SLOTS;
        if ok {
            self.list[self.count] = Some(f);
            self.count += 1;
        }
        self.unlock();
        if ok { 0 } else { -1 }
    }
    fn pop(&mut self) -> Option<extern "C" fn()> {
        self.lock();
        let f = if self.count > 0 {
            self.count -= 1;
            self.list[self.count]
        } else {
            None
        };
        self.unlock();
        f
    }
    fn run(&mut self) {
        while let Some(f) = self.pop() {
            f();
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn atexit(f: extern "C" fn()) -> i32 {
    (*core::ptr::addr_of_mut!(AT_EXIT)).push(f)
}

#[no_mangle]
pub unsafe extern "C" fn at_quick_exit(f: extern "C" fn()) -> i32 {
    (*core::ptr::addr_of_mut!(AT_QUICK_EXIT)).push(f)
}

#[no_mangle]
pub extern "C" fn exit(status: i32) -> ! {
    unsafe { (*core::ptr::addr_of_mut!(AT_EXIT)).run() };
    _Exit(status)
}

#[no_mangle]
pub extern "C" fn quick_exit(status: i32) -> ! {
    unsafe { (*core::ptr::addr_of_mut!(AT_QUICK_EXIT)).run() };
    _Exit(status)
}

/// Ends the process at once: no handlers, no stream flushing.
#[no_mangle]
pub extern "C" fn _Exit(status: i32) -> ! {
    crate::exit_group(status)
}

const SIGABRT: i32 = 6;
const SIG_UNBLOCK: i32 = 1;
const SIG_DFL: usize = 0;

/// Raises SIGABRT even when it is blocked. If a handler returns, or the signal is
/// ignored, the default action is restored and the signal raised again (POSIX abort).
#[no_mangle]
pub extern "C" fn abort() -> ! {
    let set: u64 = 1 << (SIGABRT - 1);
    unsafe {
        crate::sigprocmask(SIG_UNBLOCK, &set, core::ptr::null_mut());
    }
    crate::raise(SIGABRT);
    crate::signal(SIGABRT, SIG_DFL);
    crate::raise(SIGABRT);
    crate::exit_group(127)
}
