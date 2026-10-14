//! setjmp, longjmp, sigsetjmp and siglongjmp (ISO C 7.13, POSIX.1-2024).
//!
//! `jmp_buf` (include/setjmp.h) is 22 machine words of saved state, then a word saying
//! whether sigsetjmp saved the signal mask, then that mask:
//!
//! - aarch64: x19-x28, x29 (frame pointer), x30 (return address), sp, d8-d15
//!   (AAPCS64's callee-saved registers), at words 0-20.
//! - x86_64: rbx, rbp, r12-r15, the caller's rsp and the return address (the SysV
//!   callee-saved registers), at words 0-7.
//!
//! longjmp restores them and returns from the matching setjmp with its value, or 1
//! when the value is 0. The floating-point environment is not part of the saved state
//! (ISO C 7.13.2.1).

const MASK_SAVED: usize = 22 * 8;
const MASK: usize = 23 * 8;

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn setjmp(_env: *mut u64) -> i32 {
    core::arch::naked_asm!(
        "stp x19, x20, [x0, #0]",
        "stp x21, x22, [x0, #16]",
        "stp x23, x24, [x0, #32]",
        "stp x25, x26, [x0, #48]",
        "stp x27, x28, [x0, #64]",
        "stp x29, x30, [x0, #80]",
        "mov x2, sp",
        "str x2, [x0, #96]",
        "stp d8, d9, [x0, #104]",
        "stp d10, d11, [x0, #120]",
        "stp d12, d13, [x0, #136]",
        "stp d14, d15, [x0, #152]",
        "str xzr, [x0, #176]",
        "mov w0, #0",
        "ret",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn longjmp(_env: *mut u64, _val: i32) -> ! {
    core::arch::naked_asm!(
        "ldp x19, x20, [x0, #0]",
        "ldp x21, x22, [x0, #16]",
        "ldp x23, x24, [x0, #32]",
        "ldp x25, x26, [x0, #48]",
        "ldp x27, x28, [x0, #64]",
        "ldp x29, x30, [x0, #80]",
        "ldr x2, [x0, #96]",
        "mov sp, x2",
        "ldp d8, d9, [x0, #104]",
        "ldp d10, d11, [x0, #120]",
        "ldp d12, d13, [x0, #136]",
        "ldp d14, d15, [x0, #152]",
        "cmp w1, #0",
        "csinc w0, w1, wzr, ne",
        "ret",
    )
}

/// sigsetjmp: setjmp, then record the signal mask when `savemask` is nonzero. x19 is
/// already saved in the buffer, so it carries `env` across the call that records the
/// mask; it and the return address are reloaded from the buffer afterwards.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn sigsetjmp(_env: *mut u64, _savemask: i32) -> i32 {
    core::arch::naked_asm!(
        "stp x19, x20, [x0, #0]",
        "stp x21, x22, [x0, #16]",
        "stp x23, x24, [x0, #32]",
        "stp x25, x26, [x0, #48]",
        "stp x27, x28, [x0, #64]",
        "stp x29, x30, [x0, #80]",
        "mov x2, sp",
        "str x2, [x0, #96]",
        "stp d8, d9, [x0, #104]",
        "stp d10, d11, [x0, #120]",
        "stp d12, d13, [x0, #136]",
        "stp d14, d15, [x0, #152]",
        "mov x19, x0",
        "bl {record}",
        "mov x0, x19",
        "ldr x19, [x0, #0]",
        "ldr x30, [x0, #88]",
        "mov w0, #0",
        "ret",
        record = sym record_mask,
    )
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn setjmp(_env: *mut u64) -> i32 {
    core::arch::naked_asm!(
        "mov [rdi], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",
        "lea rdx, [rsp + 8]",
        "mov [rdi + 48], rdx",
        "mov rdx, [rsp]",
        "mov [rdi + 56], rdx",
        "mov qword ptr [rdi + 176], 0",
        "xor eax, eax",
        "ret",
    )
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn longjmp(_env: *mut u64, _val: i32) -> ! {
    core::arch::naked_asm!(
        "mov rbx, [rdi]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",
        "mov rsp, [rdi + 48]",
        "mov eax, esi",
        "test eax, eax",
        "jnz 2f",
        "mov eax, 1",
        "2:",
        "jmp qword ptr [rdi + 56]",
    )
}

/// sigsetjmp: setjmp, then record the signal mask when `savemask` is nonzero. rbx is
/// already saved in the buffer, so it carries `env` across the call.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn sigsetjmp(_env: *mut u64, _savemask: i32) -> i32 {
    core::arch::naked_asm!(
        "mov [rdi], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",
        "lea rdx, [rsp + 8]",
        "mov [rdi + 48], rdx",
        "mov rdx, [rsp]",
        "mov [rdi + 56], rdx",
        "mov rbx, rdi",
        "sub rsp, 8",
        "call {record}",
        "add rsp, 8",
        "mov rdi, rbx",
        "mov rbx, [rdi]",
        "xor eax, eax",
        "ret",
        record = sym record_mask,
    )
}

/// Record the calling thread's signal mask in `env` when `savemask` is nonzero.
extern "C" fn record_mask(env: *mut u8, savemask: i32) {
    unsafe {
        let saved = env.add(MASK_SAVED) as *mut u64;
        if savemask == 0 {
            *saved = 0;
            return;
        }
        let mask = env.add(MASK) as *mut u64;
        *saved = if crate::sigprocmask(0, core::ptr::null(), mask) == 0 { 1 } else { 0 };
    }
}

/// siglongjmp: restore the signal mask sigsetjmp saved, if it saved one, then longjmp.
#[no_mangle]
pub unsafe extern "C" fn siglongjmp(env: *mut u64, val: i32) -> ! {
    let bytes = env as *mut u8;
    if *(bytes.add(MASK_SAVED) as *const u64) != 0 {
        // SIG_SETMASK is 2.
        crate::sigprocmask(2, bytes.add(MASK) as *const u64, core::ptr::null_mut());
    }
    longjmp(env, val)
}

/// _setjmp: setjmp, which never saves the signal mask. A branch, not a call, so the
/// frame setjmp records is the caller's.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _setjmp(_env: *mut u64) -> i32 {
    core::arch::naked_asm!("b {setjmp}", setjmp = sym setjmp)
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _setjmp(_env: *mut u64) -> i32 {
    core::arch::naked_asm!("jmp {setjmp}", setjmp = sym setjmp)
}

/// _longjmp: longjmp, which never restores a signal mask.
#[no_mangle]
pub unsafe extern "C" fn _longjmp(env: *mut u64, val: i32) -> ! {
    longjmp(env, val)
}
