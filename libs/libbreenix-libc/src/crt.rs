//! Process entry: `_start` takes argc, argv and envp from the initial stack, sets
//! `environ`, starts the thread runtime and calls `main(argc, argv, envp)`.

use crate::{env::environ, exit, pthread};

/// Pristine x86_64 GPR values at process entry.
///
/// The index order is rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp, and r8 through
/// r15. `_start` captures these values before executing any runtime code.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub static mut __breenix_entry_gprs: [u64; 16] = [0; 16];

/// The _start entry point for C programs and Rust programs using std.
///
/// Uses a naked function to properly extract argc/argv from the stack
/// as set up by the kernel.
///
/// Stack layout at entry:
/// ```text
/// [rsp]     = argc
/// [rsp+8]   = argv[0]
/// [rsp+16]  = argv[1]
/// ...
/// ```
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // Preserve evidence of the kernel's process-entry register state before
        // the runtime can repurpose any register.
        "mov qword ptr [rip + {entry_gprs} + 0*8], rax",
        "mov qword ptr [rip + {entry_gprs} + 1*8], rbx",
        "mov qword ptr [rip + {entry_gprs} + 2*8], rcx",
        "mov qword ptr [rip + {entry_gprs} + 3*8], rdx",
        "mov qword ptr [rip + {entry_gprs} + 4*8], rsi",
        "mov qword ptr [rip + {entry_gprs} + 5*8], rdi",
        "mov qword ptr [rip + {entry_gprs} + 6*8], rbp",
        "mov qword ptr [rip + {entry_gprs} + 7*8], rsp",
        "mov qword ptr [rip + {entry_gprs} + 8*8], r8",
        "mov qword ptr [rip + {entry_gprs} + 9*8], r9",
        "mov qword ptr [rip + {entry_gprs} + 10*8], r10",
        "mov qword ptr [rip + {entry_gprs} + 11*8], r11",
        "mov qword ptr [rip + {entry_gprs} + 12*8], r12",
        "mov qword ptr [rip + {entry_gprs} + 13*8], r13",
        "mov qword ptr [rip + {entry_gprs} + 14*8], r14",
        "mov qword ptr [rip + {entry_gprs} + 15*8], r15",
        "mov rdi, rsp",  // Pass stack pointer as first argument
        "and rsp, -16",  // Align stack to 16 bytes
        "call {entry}",
        entry_gprs = sym __breenix_entry_gprs,
        entry = sym _start_rust,
    )
}

/// ARM64 _start entry point
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov x0, sp",    // Pass stack pointer as first argument
        "and sp, x0, #-16",  // Align stack to 16 bytes
        "bl {entry}",
        entry = sym _start_rust,
    )
}

/// Rust entry point called from _start with the stack pointer.
///
/// Extracts argc, argv, and envp from the stack and calls `main(argc, argv, envp)`;
/// what main returns is the status passed to `exit`.
/// Stack layout: [argc] [argv[0]..argv[argc-1]] [NULL] [envp[0]..envp[n]] [NULL] [auxv...]
extern "C" fn _start_rust(sp: *const u64) -> ! {
    unsafe {
        let argc = *sp as usize;
        let argv = sp.add(1) as *const *const u8;

        // envp starts after argv NULL terminator: sp + 1 (argc) + argc + 1 (NULL)
        let envp = sp.add(1 + argc + 1) as *const *const u8;
        environ = envp as *mut *mut u8;
        pthread::startup(envp);

        extern "C" {
            fn main(argc: i32, argv: *const *const u8, envp: *const *const u8) -> i32;
        }
        let ret = main(argc as i32, argv, envp);
        exit(ret);
    }
}
