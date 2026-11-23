#![no_std]

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
extern crate linux_test;

extern "C" {
    fn c_interop(
        tag: i32,
        scale: f64,
        value: Mixed,
        callback: extern "C" fn(Mixed) -> Mixed,
    ) -> Mixed;
}

#[panic_handler]
#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    extern "C" {
        fn abort() -> !;
    }
    // SAFETY: libc's abort has no preconditions and does not return.
    unsafe { abort() }
}

#[repr(C)]
pub struct Mixed {
    pub i: i32,
    pub d: f64,
}

#[no_mangle]
pub extern "C" fn rust_interop_scalar(tag: i32, scale: f64) -> f64 {
    f64::from(tag) + scale
}

#[no_mangle]
pub extern "C" fn rust_interop(value: Mixed, callback: extern "C" fn(Mixed) -> Mixed) -> Mixed {
    if value.i != 11 || value.d != 1.5 {
        return value;
    }
    let result = callback(Mixed { i: 17, d: 2.5 });
    if result.i != 23 || result.d != 4.5 {
        return result;
    }
    Mixed { i: 29, d: 8.5 }
}

extern "C" fn rust_callback(mut value: Mixed) -> Mixed {
    if value.i != 20 || value.d != 3.75 {
        value.i = -1;
        return value;
    }
    Mixed { i: 31, d: 7.5 }
}

#[no_mangle]
pub extern "C" fn rust_call_c() -> i32 {
    // SAFETY: the staged C callee uses this prototype and calls the supplied
    // callback with a valid by-value Mixed record.
    let result = unsafe { c_interop(7, 1.5, Mixed { i: 13, d: 2.25 }, rust_callback) };
    i32::from(result.i != 31 || result.d != 7.5)
}
