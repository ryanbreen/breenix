//! The shared object contributes a Rust frame with a cleanup landing pad.

struct Cleanup<'a>(&'a mut u32);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        *self.0 += 1;
    }
}

#[no_mangle]
pub extern "C-unwind" fn kd_throw_panic(cleanups: &mut u32) {
    let _guard = Cleanup(cleanups);
    std::panic::resume_unwind(Box::new(42u32));
}
