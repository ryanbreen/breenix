//! Process initialization for the x86-64 hosted shared library.

use crate::env::environ;
use crate::lifecycle::ArrayEntry;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

static AUXV: AtomicPtr<usize> = AtomicPtr::new(ptr::null_mut());

/// The interpreter owns the initialization arrays for the dynamic load set.
pub(crate) fn fini_array() -> &'static [ArrayEntry] {
    &[]
}

pub(crate) fn auxv_entry(kind: u64) -> Option<u64> {
    let mut entry = AUXV.load(Ordering::Acquire).cast_const();
    while !entry.is_null() {
        // SAFETY: the original stack contains the AT_NULL-terminated vector.
        let tag = unsafe { entry.read() };
        if tag == 0 {
            break;
        }
        if tag == kind as usize {
            // SAFETY: each auxiliary entry has a tag and a value.
            return Some(unsafe { entry.add(1).read() } as u64);
        }
        // SAFETY: the current tag is nonzero, so the next pair exists.
        entry = unsafe { entry.add(2) };
    }
    None
}

/// Called during single-threaded process startup with the kernel's initial stack.
/// The executable can call this again after the interpreter ran constructors.
#[no_mangle]
pub unsafe extern "C" fn __breenix_libc_init(stack: *const usize) {
    if !AUXV.load(Ordering::Acquire).is_null() {
        return;
    }
    // SAFETY: argc is the first word of the initial stack.
    let argc = unsafe { stack.read() };
    // SAFETY: the environment follows argc, argc argv pointers and their NULL.
    let envp = unsafe { stack.add(argc + 2) }.cast::<*const u8>();
    let mut end = envp;
    // SAFETY: the environment ends with a NULL entry on the initial stack.
    while !unsafe { end.read() }.is_null() {
        // SAFETY: a non-null entry is followed by another entry or the NULL.
        end = unsafe { end.add(1) };
    }
    // SAFETY: the kernel's auxv immediately follows that NULL.
    let auxv = unsafe { end.add(1) }.cast::<usize>().cast_mut();
    // SAFETY: single-threaded startup owns the one process environment.
    unsafe { environ = envp.cast_mut().cast() };
    AUXV.store(auxv, Ordering::Release);
    // SAFETY: envp still points to the kernel's initial environment and auxv.
    unsafe { crate::pthread::startup(envp) };
}
