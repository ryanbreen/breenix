//! Raw-before-println diagnostic for ARM64 std stdout narrowing.

use libbreenix::{io, Fd};

fn main() {
    let _ = io::write(Fd::STDOUT, b"[hello_raw_then_println] raw-before\n");
    println!("[hello_raw_then_println] println");
    let _ = io::write(Fd::STDOUT, b"[hello_raw_then_println] raw-after\n");
    std::process::exit(42);
}

