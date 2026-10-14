//! Program termination: atexit, exit, _Exit, at_quick_exit, quick_exit and abort
//! (ISO C17 7.22.4, POSIX.1-2024).
//!
//! exit runs the atexit handlers in the reverse order of their registration (a handler
//! may register more, which then run next), flushes every stdio stream, and ends the
//! process with _Exit.

use core::sync::atomic::{AtomicBool, Ordering};

/// ISO C requires room for at least 32 handlers in each list.
const SLOTS: usize = 64;

struct HandlerData {
    count: usize,
    list: [Option<extern "C" fn()>; SLOTS],
}

struct Handlers {
    busy: AtomicBool,
    data: core::cell::UnsafeCell<HandlerData>,
}

// The atomic lock protects every access to data; callbacks run after unlocking.
unsafe impl Sync for Handlers {}

static AT_EXIT: Handlers = Handlers::new();
static AT_QUICK_EXIT: Handlers = Handlers::new();

impl Handlers {
    const fn new() -> Self {
        Self { busy: AtomicBool::new(false), data: core::cell::UnsafeCell::new(
            HandlerData { count: 0, list: [None; SLOTS] }) }
    }
    fn lock(&self) {
        while self.busy.swap(true, Ordering::Acquire) { core::hint::spin_loop(); }
    }
    fn unlock(&self) { self.busy.store(false, Ordering::Release); }
    fn push(&self, f: extern "C" fn()) -> i32 {
        self.lock();
        let ok = unsafe {
            let data = &mut *self.data.get();
            if data.count == SLOTS { false } else {
                data.list[data.count] = Some(f);
                data.count += 1;
                true
            }
        };
        self.unlock();
        if ok { 0 } else { -1 }
    }
    fn pop(&self) -> Option<extern "C" fn()> {
        self.lock();
        let f = unsafe {
            let data = &mut *self.data.get();
            if data.count == 0 { None } else {
                data.count -= 1;
                data.list[data.count].take()
            }
        };
        self.unlock();
        f
    }
    fn run(&self) {
        while let Some(f) = self.pop() { f(); }
    }
}

#[no_mangle]
pub unsafe extern "C" fn atexit(f: extern "C" fn()) -> i32 {
    AT_EXIT.push(f)
}

#[no_mangle]
pub unsafe extern "C" fn at_quick_exit(f: extern "C" fn()) -> i32 {
    AT_QUICK_EXIT.push(f)
}

#[no_mangle]
pub extern "C" fn exit(status: i32) -> ! {
    AT_EXIT.run();
    unsafe { crate::stdio::flush_all(); }
    _Exit(status)
}

#[no_mangle]
pub extern "C" fn quick_exit(status: i32) -> ! {
    AT_QUICK_EXIT.run();
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicUsize;
    static CALLBACKS: Handlers = Handlers::new();
    static ORDER: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn child() { ORDER.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v * 10 + 2)).unwrap(); }
    extern "C" fn parent() {
        ORDER.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v * 10 + 1)).unwrap();
        assert_eq!(CALLBACKS.push(child), 0);
    }
    #[test]
    fn callback_can_register_another_callback() {
        assert_eq!(CALLBACKS.push(child), 0);
        assert_eq!(CALLBACKS.push(parent), 0);
        CALLBACKS.run();
        assert_eq!(ORDER.load(Ordering::SeqCst), 122);
    }
    #[test]
    fn concurrent_registration_keeps_every_handler() {
        let handlers = std::sync::Arc::new(Handlers::new());
        let threads: std::vec::Vec<_> = (0..8).map(|_| {
            let handlers = handlers.clone();
            std::thread::spawn(move || { for _ in 0..8 { assert_eq!(handlers.push(child), 0); } })
        }).collect();
        for t in threads { t.join().unwrap(); }
        for _ in 0..SLOTS { assert!(handlers.pop().is_some()); }
        assert!(handlers.pop().is_none());
    }
}
