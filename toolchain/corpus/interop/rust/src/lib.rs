#![no_std]

use core::panic::PanicInfo;

extern "C" {
    fn abort() -> !;
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    // SAFETY: libc's abort has no preconditions and does not return.
    unsafe { abort() }
}

#[repr(C)]
pub struct Mixed {
    pub i: i32,
    pub d: f64,
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
