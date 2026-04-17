//! Minimal std println post-exec diagnostic for ARM64.

fn main() {
    println!("[hello_println] start");
    std::process::exit(42);
}

