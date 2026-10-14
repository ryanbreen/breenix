//! memcpy, memmove and memset in assembly, so no optimizer pass can rewrite their loops
//! into calls to themselves.

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
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov rax, rdi",
        "mov rcx, rdx",
        "rep movsb",
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
