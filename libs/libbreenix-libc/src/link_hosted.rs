//! POSIX.1-2024 dlopen/dlsym/dlclose/dlerror and LSB Core 5.0 §14.17.

#![deny(unsafe_op_in_unsafe_fn)]

use crate::dl_abi::ObjectInfo;
use core::ffi::{c_char, c_int, c_ulonglong, c_void};
use core::mem::size_of;

#[repr(C)]
pub(crate) struct DlPhdrInfo {
    pub dlpi_addr: u64,
    pub dlpi_name: *const c_char,
    pub dlpi_phdr: *const c_void,
    pub dlpi_phnum: u16,
    pub dlpi_adds: c_ulonglong,
    pub dlpi_subs: c_ulonglong,
    pub dlpi_tls_modid: usize,
    pub dlpi_tls_data: *mut c_void,
    // Breenix extension, present when the callback's size covers these fields.
    pub text_base: u64,
    pub got_base: u64,
}

type Callback = unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int;

/// # Safety
///
/// The supplied records remain valid through the callback, which must be
/// callable with data as its context.
pub(crate) unsafe fn visit_objects(
    count: usize,
    mut get: impl FnMut(usize) -> Option<ObjectInfo>,
    callback: Callback,
    data: *mut c_void,
) -> c_int {
    for i in 0..count {
        let Some(record) = get(i) else { continue };
        let object = record.object;
        let Ok(phnum) = u16::try_from(object.phnum) else {
            continue;
        };
        let mut info = DlPhdrInfo {
            dlpi_addr: object.bias,
            dlpi_name: if i == 0 { c"".as_ptr() } else { object.name },
            dlpi_phdr: object.phdr,
            dlpi_phnum: phnum,
            dlpi_adds: count as u64,
            dlpi_subs: 0,
            dlpi_tls_modid: object.tls_module_id,
            dlpi_tls_data: tls_data(object.tls_module_id),
            text_base: record.text_base,
            got_base: record.got_base,
        };
        // SAFETY: info lives through the call and data comes from the caller.
        let result = unsafe { callback(&mut info, size_of::<DlPhdrInfo>(), data) };
        if result != 0 {
            return result;
        }
    }
    0
}

#[cfg(not(test))]
use crate::pthread::hosted_tls::tls_data;

#[cfg(test)]
fn tls_data(_: usize) -> *mut c_void {
    core::ptr::null_mut()
}

#[cfg(not(test))]
mod exports {
    use super::*;
    use crate::dl_abi::{DlInfo, DlRuntime, RTLD_GLOBAL, RTLD_LAZY, RTLD_NOW};
    use crate::pthread::hosted_tls::replace_dl_error;
    use core::ffi::CStr;
    use core::mem::MaybeUninit;
    use core::ptr;
    use core::sync::atomic::{AtomicPtr, Ordering};

    static RUNTIME: AtomicPtr<DlRuntime> = AtomicPtr::new(ptr::null_mut());

    /// The interpreter publishes its private ABI before running constructors.
    #[no_mangle]
    unsafe extern "C" fn __breenix_libc_register_dl(runtime: *const DlRuntime) {
        RUNTIME.store(runtime.cast_mut(), Ordering::Release);
    }

    fn runtime() -> Option<&'static DlRuntime> {
        // SAFETY: The only writer publishes the interpreter's immutable ABI
        // table, and the interpreter stays mapped for the life of the process.
        unsafe { RUNTIME.load(Ordering::Acquire).as_ref() }
    }

    fn error(message: &'static CStr) {
        replace_dl_error(Some(message));
    }

    #[no_mangle]
    unsafe extern "C" fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void {
        if flags & !(RTLD_NOW | RTLD_LAZY | RTLD_GLOBAL) != 0
            || !matches!(flags & (RTLD_NOW | RTLD_LAZY), RTLD_NOW | RTLD_LAZY)
        {
            error(c"dlopen: invalid mode");
            return ptr::null_mut();
        }
        let handle = runtime().map_or(0, |rt| {
            // SAFETY: POSIX supplies a readable string or NULL.
            unsafe { (rt.open)(name, flags & RTLD_GLOBAL != 0) }
        });
        if handle == 0 {
            error(c"dlopen: cannot load or relocate the requested object");
        }
        handle as *mut c_void
    }

    #[no_mangle]
    extern "C" fn dlclose(handle: *mut c_void) -> c_int {
        let status = runtime().map_or(-1, |rt| (rt.close)(handle as usize));
        if status != 0 {
            error(c"dlclose: invalid or closed handle");
        }
        status
    }

    #[no_mangle]
    unsafe extern "C" fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void {
        let mut address = 0;
        if !name.is_null()
            && runtime().is_some_and(|rt| {
                // SAFETY: name is a C string and address is a writable out parameter.
                unsafe { (rt.symbol)(handle as usize, name, &mut address) == 0 }
            })
        {
            return address as *mut c_void;
        }
        error(c"dlsym: symbol or handle not found");
        ptr::null_mut()
    }

    #[no_mangle]
    extern "C" fn dlerror() -> *mut c_char {
        replace_dl_error(None).map_or(ptr::null_mut(), |message| message.as_ptr().cast_mut())
    }

    #[no_mangle]
    unsafe extern "C" fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int {
        if !info.is_null()
            && runtime().is_some_and(|rt| {
                // SAFETY: dladdr's caller provides a writable Dl_info.
                unsafe { (rt.address)(address as u64, info) != 0 }
            })
        {
            return 1;
        }
        error(c"dladdr: address does not belong to a loaded object");
        0
    }

    #[no_mangle]
    unsafe extern "C" fn dl_iterate_phdr(callback: Callback, data: *mut c_void) -> c_int {
        let Some(rt) = runtime() else { return 0 };
        // SAFETY: the loader publishes immutable records before incrementing
        // count and the caller supplies a live callback and context.
        unsafe {
            visit_objects(
                (rt.count)(),
                |index| {
                    let mut info = MaybeUninit::uninit();
                    (rt.object)(index, info.as_mut_ptr()).then(|| info.assume_init())
                },
                callback,
                data,
            )
        }
    }
}
