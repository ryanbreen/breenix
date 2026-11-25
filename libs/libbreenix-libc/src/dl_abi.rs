//! Private LP64 ABI between the Breenix interpreter and hosted libc.

use core::ffi::{c_char, c_int, c_void};

/// The u3 loaded-object record. Pointers borrow resident storage.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoadedObject {
    pub name: *const c_char,
    pub bias: u64,
    pub phdr: *const c_void,
    pub phnum: usize,
    pub dynamic: *const c_void,
    pub tls_module_id: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjectInfo {
    pub object: LoadedObject,
    pub text_base: u64,
    pub got_base: u64,
}

// The constants used by the Breenix dlfcn bindings (LSB Core 5.0 §14.16).
pub const RTLD_LAZY: c_int = 1;
pub const RTLD_NOW: c_int = 2;
pub const RTLD_GLOBAL: c_int = 0x100;

#[repr(C)]
pub struct DlInfo {
    pub dli_fname: *const c_char,
    pub dli_fbase: *mut c_void,
    pub dli_sname: *const c_char,
    pub dli_saddr: *mut c_void,
}

#[repr(C)]
pub struct DlRuntime {
    pub open: unsafe extern "C" fn(*const c_char, bool) -> usize,
    pub close: extern "C" fn(usize) -> c_int,
    // The status distinguishes a valid null-valued symbol from a miss.
    pub symbol: unsafe extern "C" fn(usize, *const c_char, *mut u64) -> c_int,
    pub address: unsafe extern "C" fn(u64, *mut DlInfo) -> c_int,
    pub count: extern "C" fn() -> usize,
    pub object: unsafe extern "C" fn(usize, *mut ObjectInfo) -> bool,
}
