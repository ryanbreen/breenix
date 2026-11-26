//! The executable owns std and bunwind, which the DSO imports.

use std::panic::{catch_unwind, AssertUnwindSafe};

extern "C-unwind" {
    fn kd_throw_panic(cleanups: &mut u32);
}

fn main() {
    let mut cleanups = 0;
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The fixture DSO implements this C-unwind declaration.
        unsafe { kd_throw_panic(&mut cleanups) };
    }));
    let caught = result.err().and_then(|payload| payload.downcast::<u32>().ok());
    if caught.as_deref() != Some(&42) || cleanups != 1 {
        std::process::exit(1);
    }
}
