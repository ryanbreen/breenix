#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "breenix")]
mod abi;
#[cfg(any(test, target_os = "breenix"))]
mod arch;
#[cfg(any(test, target_os = "breenix"))]
mod loaded;

pub mod cfi;
pub mod ehframe;
pub mod level1;

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod bunwind;

// Cargo's standalone no_std static library needs a panic handler. The host
// test executable instead uses the harness's panic runtime.
#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    abort()
}

#[cfg(not(test))]
fn abort() -> ! {
    extern "C" {
        #[link_name = "abort"]
        fn platform_abort() -> !;
    }

    // SAFETY: The platform C ABI's abort takes no arguments and terminates.
    unsafe { platform_abort() }
}
