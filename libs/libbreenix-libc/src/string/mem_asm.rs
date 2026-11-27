//! Scalar copy and fill loops in assembly, so no optimizer pass can rewrite
//! them into calls to themselves.

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // x0 = dst (returned unchanged), x1 = src, x2 = n; x3 walks dst.
    core::arch::naked_asm!(
        "mov x3, x0",
        "cmp x2, #16",
        "b.lo 2f",
        "1:",
        "ldp x4, x5, [x1], #16",
        "stp x4, x5, [x3], #16",
        "sub x2, x2, #16",
        "cmp x2, #16",
        "b.hs 1b",
        "2:",
        "cbz x2, 4f",
        "3:",
        "ldrb w4, [x1], #1",
        "strb w4, [x3], #1",
        "subs x2, x2, #1",
        "b.ne 3b",
        "4:",
        "ret",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // A forward copy is safe unless dst lies inside (src, src + n); then copy from the end.
    core::arch::naked_asm!(
        "sub x3, x0, x1",
        "cmp x3, x2",
        "b.lo 0f",
        "b {fwd}",
        "0:",
        "add x3, x0, x2",
        "add x1, x1, x2",
        "cmp x2, #16",
        "b.lo 2f",
        "1:",
        "ldp x4, x5, [x1, #-16]!",
        "stp x4, x5, [x3, #-16]!",
        "sub x2, x2, #16",
        "cmp x2, #16",
        "b.hs 1b",
        "2:",
        "cbz x2, 4f",
        "3:",
        "ldrb w4, [x1, #-1]!",
        "strb w4, [x3, #-1]!",
        "subs x2, x2, #1",
        "b.ne 3b",
        "4:",
        "ret",
        fwd = sym memcpy,
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov x3, x0",
        "and x1, x1, #0xff",
        "mov x4, #0x0101010101010101",
        "mul x1, x1, x4",
        "cmp x2, #16",
        "b.lo 2f",
        "1:",
        "stp x1, x1, [x3], #16",
        "sub x2, x2, #16",
        "cmp x2, #16",
        "b.hs 1b",
        "2:",
        "cbz x2, 4f",
        "3:",
        "strb w1, [x3], #1",
        "subs x2, x2, #1",
        "b.ne 3b",
        "4:",
        "ret",
    )
}

#[cfg(target_arch = "x86_64")]
fn enhanced_rep_movsb() -> bool {
    use core::arch::x86_64::{__cpuid, __cpuid_count};
    use core::sync::atomic::{AtomicU8, Ordering::Relaxed};

    static ERMS: AtomicU8 = AtomicU8::new(0);
    match ERMS.load(Relaxed) {
        0 => {
            // Intel Optimization Reference Manual vol. 1, §3.7.6:
            // CPUID.07H.0H:EBX[9] enumerates enhanced REP MOVSB/STOSB.
            let supported =
                unsafe { __cpuid(0).eax >= 7 && __cpuid_count(7, 0).ebx & (1 << 9) != 0 };
            ERMS.store(1 + u8::from(supported), Relaxed);
            supported
        }
        cached => cached == 2,
    }
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // Avoid string-engine startup on short copies. For a bulk copy use the
    // enhanced engine only when CPUID advertises it.
    if n >= 256 && enhanced_rep_movsb() {
        core::arch::asm!(
            "rep movsb",
            inout("rdi") dst => _,
            inout("rsi") src => _,
            inout("rcx") n => _,
            options(nostack, preserves_flags),
        );
        dst
    } else {
        copy_scalar(dst, src, n)
    }
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn copy_scalar(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov rax, rdi",
        "cmp rdx, 32",
        "jb 3f",
        "2:",
        "mov rcx, [rsi]",
        "mov r8, [rsi + 8]",
        "mov r9, [rsi + 16]",
        "mov r10, [rsi + 24]",
        "mov [rdi], rcx",
        "mov [rdi + 8], r8",
        "mov [rdi + 16], r9",
        "mov [rdi + 24], r10",
        "add rsi, 32",
        "add rdi, 32",
        "sub rdx, 32",
        "cmp rdx, 32",
        "jae 2b",
        "3:",
        // For each short size, adjacent or overlapping first/last transfers
        // cover exactly the requested range.
        "cmp rdx, 16",
        "jb 4f",
        "mov rcx, [rsi]",
        "mov r8, [rsi + 8]",
        "mov r9, [rsi + rdx - 16]",
        "mov r10, [rsi + rdx - 8]",
        "mov [rdi], rcx",
        "mov [rdi + 8], r8",
        "mov [rdi + rdx - 16], r9",
        "mov [rdi + rdx - 8], r10",
        "ret",
        "4:",
        "cmp rdx, 8",
        "jb 5f",
        "mov rcx, [rsi]",
        "mov r8, [rsi + rdx - 8]",
        "mov [rdi], rcx",
        "mov [rdi + rdx - 8], r8",
        "ret",
        "5:",
        "cmp rdx, 4",
        "jb 6f",
        "mov ecx, [rsi]",
        "mov r8d, [rsi + rdx - 4]",
        "mov [rdi], ecx",
        "mov [rdi + rdx - 4], r8d",
        "ret",
        "6:",
        "cmp rdx, 2",
        "jb 7f",
        "mov cx, [rsi]",
        "mov r8w, [rsi + rdx - 2]",
        "mov [rdi], cx",
        "mov [rdi + rdx - 2], r8w",
        "ret",
        "7:",
        "test rdx, rdx",
        "jz 8f",
        "mov cl, [rsi]",
        "mov [rdi], cl",
        "8:",
        "ret",
    )
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov rax, rdi",
        "mov rcx, rdx",
        "mov r8, rdi",
        "sub r8, rsi",
        "cmp r8, rdx",
        "jae 2f",
        // dst overlaps the source's tail: copy backwards with DF set, then clear it.
        "lea rsi, [rsi + rdx - 1]",
        "lea rdi, [rdi + rdx - 1]",
        "std",
        "rep movsb",
        "cld",
        "ret",
        "2:",
        "rep movsb",
        "ret",
    )
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov r8, rdi",
        "mov eax, esi",
        "mov rcx, rdx",
        "rep stosb",
        "mov rax, r8",
        "ret",
    )
}
