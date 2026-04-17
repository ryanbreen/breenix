//! Raw-write diagnostic with deliberately padded executable text.

use libbreenix::{io, process, Fd};

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text.hello_raw_padded_pad,\"ax\"",
    ".global hello_raw_padded_pad",
    "hello_raw_padded_pad:",
    ".rept 8192",
    "nop",
    ".endr",
    "ret",
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".section .text.hello_raw_padded_pad,\"ax\"",
    ".global hello_raw_padded_pad",
    "hello_raw_padded_pad:",
    ".rept 8192",
    "nop",
    ".endr",
    "ret",
);

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
extern "C" {
    fn hello_raw_padded_pad();
}

fn run_padding() {
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    unsafe {
        hello_raw_padded_pad();
    }
}

fn main() {
    let _ = io::write(Fd::STDOUT, b"[hello_raw_padded] raw-before\n");
    run_padding();
    let _ = io::write(Fd::STDOUT, b"[hello_raw_padded] raw-after\n");
    process::exit(42);
}
