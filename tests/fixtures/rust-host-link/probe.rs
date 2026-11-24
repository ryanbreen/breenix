use std::{arch::global_asm, hint::black_box};

global_asm!(include_str!("probe.s"));

extern "C" {
    fn hosted_probe_fn();
    static hosted_probe_lsda: u8;
    static hosted_probe_bounds: [usize; 6];
}

fn main() {
    let _ = std::panic::catch_unwind(|| {
        if black_box(false) {
            panic!("hosted unwind probe");
        }
    });
    // SAFETY: The fixture defines the function and both static objects.
    unsafe {
        hosted_probe_fn();
        black_box(&hosted_probe_lsda);
        black_box(&hosted_probe_bounds);
    }
}
