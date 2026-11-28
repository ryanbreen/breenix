//! Process initialization for the hosted shared library.

use crate::env::environ;
use crate::lifecycle::ArrayEntry;
use crate::startup_auxv;
pub(crate) use crate::startup_auxv::entry as auxv_entry;
use core::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// The interpreter owns the initialization arrays for the dynamic load set.
pub(crate) fn fini_array() -> &'static [ArrayEntry] {
    &[]
}

/// Called during single-threaded process startup with the kernel's initial stack.
/// The executable can call this again after the interpreter ran constructors.
#[no_mangle]
pub unsafe extern "C" fn __breenix_libc_init(stack: *const usize) {
    if INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: the original stack outlives startup and this single-threaded
    // call owns the process environment.
    unsafe {
        let envp = startup_auxv::environment(stack);
        environ = envp.cast_mut().cast();
        startup_auxv::initialize(envp);
        crate::pthread::startup(envp);
    }
}
