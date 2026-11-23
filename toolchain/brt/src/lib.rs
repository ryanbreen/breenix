//! Runtime arithmetic for bcc-generated code.

#![cfg_attr(target_os = "breenix", no_std)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod f128;

#[cfg(feature = "exports")]
mod exports;

#[cfg(target_os = "breenix")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // N1570 §7.22.4.1.
    extern "C" {
        fn abort() -> !;
    }

    // SAFETY: abort takes no arguments and terminates the process.
    unsafe { abort() }
}

#[cfg(test)]
mod tests;
