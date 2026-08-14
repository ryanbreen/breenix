//! Register initialization test program (std version).
//!
//! On x86_64, the kernel zeroes every GPR at userspace entry, matching Linux's
//! `ELF_PLAT_INIT` behavior so no kernel register state leaks into a fresh
//! process. The libc entry point snapshots that pristine state before the Rust
//! runtime executes; this test asserts every captured GPR except `rsp` is zero.

use std::process;

#[cfg(target_arch = "x86_64")]
fn main() {
    extern "C" {
        static __breenix_entry_gprs: [u64; 16];
    }

    const REGISTER_NAMES: [&str; 16] = [
        "RAX", "RBX", "RCX", "RDX", "RSI", "RDI", "RBP", "RSP", "R8", "R9", "R10", "R11", "R12",
        "R13", "R14", "R15",
    ];
    const RSP_INDEX: usize = 7;

    let registers = unsafe { core::ptr::read(core::ptr::addr_of!(__breenix_entry_gprs)) };
    let mut passed = true;

    for (index, value) in registers.iter().enumerate() {
        if index != RSP_INDEX && *value != 0 {
            print!(
                "FAIL: {} = 0x{:016x} at process entry (expected 0)\n",
                REGISTER_NAMES[index], value
            );
            passed = false;
        }
    }

    if passed {
        print!("PASS: All x86_64 process-entry GPRs except RSP are zero\n");
    }

    process::exit(if passed { 0 } else { 1 });
}

#[cfg(not(target_arch = "x86_64"))]
fn main() {
    print!("SKIP: register_init_test is x86_64-only\n");
    process::exit(0);
}
