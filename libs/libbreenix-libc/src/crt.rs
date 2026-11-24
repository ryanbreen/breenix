//! Process entry: `_start` takes argc, argv and envp from the initial stack, sets
//! `environ`, records the auxiliary vector, starts the thread runtime, runs the
//! ELF initialization arrays and calls `main(argc, argv, envp)`.

use crate::env::environ;
use crate::lifecycle::{run_main, ArrayEntry};
use crate::{exit, pthread};
use core::sync::atomic::{AtomicPtr, Ordering};

// Only linker-hosted.ld provides these private aliases. LLD can synthesize
// the conventional names for ordinary links, too. These weak references
// leave ordinary programs' arrays empty even if the ELF has orphan sections.
core::arch::global_asm!(
    ".pushsection .rodata.breenix_array_bounds,\"a\"",
    ".p2align 3",
    ".globl __breenix_array_bounds",
    ".hidden __breenix_array_bounds",
    "__breenix_array_bounds:",
    ".weak __breenix_preinit_array_start",
    ".weak __breenix_preinit_array_end",
    ".weak __breenix_init_array_start",
    ".weak __breenix_init_array_end",
    ".weak __breenix_fini_array_start",
    ".weak __breenix_fini_array_end",
    ".quad __breenix_preinit_array_start, __breenix_preinit_array_end",
    ".quad __breenix_init_array_start, __breenix_init_array_end",
    ".quad __breenix_fini_array_start, __breenix_fini_array_end",
    ".popsection",
);

#[repr(C)]
struct ArrayBounds {
    start: *const ArrayEntry,
    end: *const ArrayEntry,
}

extern "C" {
    static __breenix_array_bounds: [ArrayBounds; 3];
}

impl ArrayBounds {
    fn entries(&self) -> &'static [ArrayEntry] {
        if self.start.is_null() || self.end.addr() <= self.start.addr() {
            return &[];
        }
        let count = (self.end.addr() - self.start.addr()) / core::mem::size_of::<ArrayEntry>();
        // SAFETY: the linker places these symbols around a contiguous and
        // aligned array of function pointers in the loaded image.
        unsafe { core::slice::from_raw_parts(self.start, count) }
    }
}

pub(crate) fn fini_array() -> &'static [ArrayEntry] {
    // SAFETY: the static declaration describes the three pairs in the table.
    unsafe { __breenix_array_bounds[2].entries() }
}

/// The auxiliary vector the kernel placed after the environment's NULL: pairs of
/// (type, value) ending with type 0 (AT_NULL). Null before `_start` has run.
static AUXV: AtomicPtr<u64> = AtomicPtr::new(core::ptr::null_mut());

/// The value of auxiliary vector entry `kind`, if the kernel supplied one.
pub(crate) fn auxv_entry(kind: u64) -> Option<u64> {
    let mut entry = AUXV.load(Ordering::Acquire) as *const u64;
    if entry.is_null() {
        return None;
    }
    // SAFETY: the kernel ends the vector with an AT_NULL pair, and the initial stack
    // that holds it lives for the whole process.
    unsafe {
        while *entry != 0 {
            if *entry == kind {
                return Some(*entry.add(1));
            }
            entry = entry.add(2);
        }
    }
    None
}

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
        let mut end = envp;
        while !(*end).is_null() {
            end = end.add(1);
        }
        AUXV.store(end.add(1) as *mut u64, Ordering::Release);
        pthread::startup(envp);

        extern "C" {
            fn main(argc: i32, argv: *const *const u8, envp: *const *const u8) -> i32;
        }
        let [preinit, init, _] = &__breenix_array_bounds;
        let ret = run_main(preinit.entries(), init.entries(), || {
            main(argc as i32, argv, envp)
        });
        exit(ret);
    }
}
