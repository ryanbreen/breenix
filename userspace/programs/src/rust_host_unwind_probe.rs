//! Isolated hosted unwind and floating-point ABI cases.
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

extern "C-unwind" {
    fn rust_host_c_frame(callback: extern "C-unwind" fn());
}
extern "C" {
    fn rust_host_c_double(value: f64, callback: extern "C" fn(f64) -> f64) -> f64;
    fn rust_host_c_float(value: f32, callback: extern "C" fn(f32) -> f32) -> f32;
}

struct Guard<'a>(&'a RefCell<Vec<u8>>, u8);
impl Drop for Guard<'_> {
    fn drop(&mut self) { self.0.borrow_mut().push(self.1); }
}
extern "C-unwind" fn panic_callback() { std::panic::panic_any(42u32); }
extern "C" fn double_callback(value: f64) -> f64 { value }
extern "C" fn float_callback(value: f32) -> f32 { value }

fn nested() {
    let order = RefCell::new(Vec::new());
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _outer = Guard(&order, 2);
        let inner = catch_unwind(AssertUnwindSafe(|| {
            let _inner = Guard(&order, 1);
            std::panic::panic_any(42u32);
        }));
        assert_eq!(*inner.unwrap_err().downcast::<u32>().unwrap(), 42);
        order.borrow_mut().push(3);
        std::panic::panic_any("outer");
    }));
    assert_eq!(*result.unwrap_err().downcast::<&str>().unwrap(), "outer");
    order.borrow_mut().push(4);
    assert_eq!(*order.borrow(), [1, 3, 2, 4]);
}
fn through_c() {
    let result = catch_unwind(|| {
        // SAFETY: the C function calls the supplied callback with the declared ABI.
        unsafe { rust_host_c_frame(panic_callback) };
    });
    assert_eq!(*result.unwrap_err().downcast::<u32>().unwrap(), 42);
}
fn float_ffi() {
    for bits in [0x3ff2_3456_789a_bcde, 0x8000_0000_0000_0000, 0x7ff8_1234_5678_9abc] {
        let value = f64::from_bits(bits);
        // SAFETY: callback and C function use matching scalar C ABIs.
        assert_eq!(unsafe { rust_host_c_double(value, double_callback) }.to_bits(), bits);
    }
    for bits in [0x3f9a_bcde, 0x8000_0000, 0x7fc1_2345] {
        let value = f32::from_bits(bits);
        // SAFETY: callback and C function use matching scalar C ABIs.
        assert_eq!(unsafe { rust_host_c_float(value, float_callback) }.to_bits(), bits);
    }
}
fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("catch-unwind-nested") => nested(),
        Some("unwind-through-c-frame") => through_c(),
        Some("float-ffi-psabi") => float_ffi(),
        _ => panic!("unknown unwind case"),
    }
    println!("ok");
}
