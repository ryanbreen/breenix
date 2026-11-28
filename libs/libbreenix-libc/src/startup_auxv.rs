//! The original exec stack provides one auxiliary vector for the process.

use core::sync::atomic::{AtomicPtr, Ordering};

static AUXV: AtomicPtr<usize> = AtomicPtr::new(core::ptr::null_mut());

pub(crate) fn entry(kind: u64) -> Option<u64> {
    let mut entry = AUXV.load(Ordering::Acquire).cast_const();
    if entry.is_null() {
        return None;
    }
    // SAFETY: the original exec stack holds AT_NULL-terminated pairs for the
    // lifetime of the process.
    unsafe {
        while entry.read() != 0 {
            if entry.read() as u64 == kind {
                return Some(entry.add(1).read() as u64);
            }
            entry = entry.add(2);
        }
    }
    None
}

pub(crate) unsafe fn environment(stack: *const usize) -> *const *const u8 {
    // SAFETY: argc, argc argument pointers, and their NULL precede envp.
    unsafe { stack.add(stack.read() + 2).cast() }
}

pub(crate) unsafe fn initialize(envp: *const *const u8) {
    // SAFETY: envp is the original environment on the exec stack, terminated
    // by a NULL pointer, immediately followed by the auxiliary vector.
    let auxv = unsafe {
        let mut end = envp;
        while !end.read().is_null() {
            end = end.add(1);
        }
        end.add(1).cast::<usize>().cast_mut()
    };
    let _ = AUXV.compare_exchange(
        core::ptr::null_mut(),
        auxv,
        Ordering::Release,
        Ordering::Relaxed,
    );
    #[cfg(feature = "e6")]
    crate::string::initialize();
}

/// Enable the auxv queries and dispatch used by a program with a custom entry.
///
/// # Safety
///
/// `stack` must be the original, unmodified exec stack, which must remain
/// readable for the rest of the process.
#[cfg(feature = "e6")]
#[no_mangle]
pub unsafe extern "C" fn __breenix_init_auxv(stack: *const usize) {
    unsafe { initialize(environment(stack)) };
}
