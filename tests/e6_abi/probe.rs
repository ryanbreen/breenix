#![no_std]
#![feature(c_variadic)]

#[repr(C)]
pub struct Mixed {
    fp: f64,
    integer: u32,
}

#[repr(C)]
pub struct Hfa {
    x: f32,
    y: f32,
    z: f32,
}

unsafe extern "C" {
    fn c_fp(a: f32, b: f64, mixed: Mixed) -> f64;
    fn c_hfa(h: Hfa) -> Hfa;
    fn c_var(tag: i32, ...) -> f64;
}

#[no_mangle]
pub extern "C" fn rust_fp(a: f32, b: f64, mixed: Mixed) -> f64 {
    f64::from(a) + b + mixed.fp + f64::from(mixed.integer)
}

#[no_mangle]
pub extern "C" fn rust_hfa(h: Hfa) -> Hfa {
    Hfa { x: h.z, y: h.x, z: h.y }
}

#[no_mangle]
pub unsafe extern "C" fn rust_var(tag: i32, mut args: ...) -> f64 {
    let mut sum = f64::from(tag);
    for i in 1..=10 {
        sum += f64::from(i) * unsafe { args.arg::<f64>() };
        sum += f64::from(i * unsafe { args.arg::<i32>() });
    }
    sum
}

#[no_mangle]
pub extern "C" fn rust_caller() -> f64 {
    let h = unsafe { c_hfa(Hfa { x: 1.25, y: 2.5, z: 3.75 }) };
    if (h.x, h.y, h.z) != (3.75, 1.25, 2.5) {
        return -1.0;
    }
    unsafe {
        c_fp(1.5, 2.5, Mixed { fp: 4.25, integer: 8 })
            + c_var(7, 1.0f64, 11i32, 2.0f64, 12i32, 3.0f64, 13i32, 4.0f64, 14i32,
                    5.0f64, 15i32, 6.0f64, 16i32, 7.0f64, 17i32, 8.0f64, 18i32,
                    9.0f64, 19i32, 10.0f64, 20i32)
    }
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub unsafe extern "C" fn rust_packed(bytes: *const u8) -> u64 {
    unsafe { bytes.add(1).cast::<u64>().read_unaligned() }
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub unsafe extern "C" fn rust_atomic(address: *mut u64) -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    unsafe { AtomicU64::from_ptr(address) }.load(Ordering::Relaxed)
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub unsafe extern "C" fn rust_device(address: *const u64) -> u64 {
    unsafe { address.read_volatile() }
}
