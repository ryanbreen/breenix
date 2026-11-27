//! Bootstrap from the kernel's initial stack before using relocated data.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

#[unsafe(naked)]
#[no_mangle]
#[cfg(target_arch = "x86_64")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // Find AT_BASE on the kernel's stack using registers alone.
        "mov r12, rsp",
        "mov rcx, [rsp]",
        "lea rdi, [rsp + rcx*8 + 16]",
        "2:",
        "mov rax, [rdi]",
        "add rdi, 8",
        "test rax, rax",
        "jnz 2b",
        "3:",
        "mov rax, [rdi]",
        "test rax, rax",
        "jz 9f",
        "cmp rax, 7",
        "je 4f",
        "add rdi, 16",
        "jmp 3b",
        "4:",
        "mov rbx, [rdi + 8]",
        // gABI §8.3 gives DT_RELA (7) and DT_RELASZ (8). RIP-relative
        // addressing reaches _DYNAMIC without a relocated GOT.
        "lea rdi, [rip + _DYNAMIC]",
        "xor esi, esi",
        "xor ecx, ecx",
        "5:",
        "mov rax, [rdi]",
        "test rax, rax",
        "jz 6f",
        "cmp rax, 7",
        "cmove rsi, [rdi + 8]",
        "cmp rax, 8",
        "cmove rcx, [rdi + 8]",
        "add rdi, 16",
        "jmp 5b",
        "6:",
        "test rcx, rcx",
        "jz 8f",
        "test rsi, rsi",
        "jz 9f",
        "add rsi, rbx",
        "7:",
        "cmp rcx, 24",
        "jb 9f",
        "cmp qword ptr [rsi + 8], 8",
        "jne 9f",
        "mov rdx, [rsi]",
        "mov rax, [rsi + 16]",
        "add rax, rbx",
        "mov [rbx + rdx], rax",
        "add rsi, 24",
        "sub rcx, 24",
        "jnz 7b",
        "8:",
        "mov rdi, r12",
        "and rsp, -16",
        "call {start}",
        "9:",
        "mov eax, {exit}",
        "mov edi, 127",
        "syscall",
        "ud2",
        exit = const libbreenix::syscall::nr::EXIT_GROUP,
        start = sym ld_breenix::runtime::start,
    );
}

#[unsafe(naked)]
#[no_mangle]
#[cfg(target_arch = "aarch64")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // Keep the original, 16-byte-aligned process stack in x0.
        "mov x0, sp",
        "ldr x1, [x0]",
        "add x1, x0, x1, lsl #3",
        "add x1, x1, #16",
        "2:",
        "ldr x2, [x1], #8",
        "cbnz x2, 2b",
        // Skip auxiliary vector pairs until AT_BASE (7) or AT_NULL (0).
        "3:",
        "ldp x2, x3, [x1], #16",
        "cbz x2, 9f",
        "cmp x2, #7",
        "b.ne 3b",
        "cbz x3, 9f",
        // PC-relative addressing reaches _DYNAMIC without touching the GOT.
        // gABI §8.3: DT_RELA (7), DT_RELASZ (8) and DT_RELAENT (9).
        "adrp x1, _DYNAMIC",
        "add x1, x1, :lo12:_DYNAMIC",
        "mov x4, xzr",
        "mov x5, xzr",
        "mov x6, xzr",
        "4:",
        "ldp x2, x7, [x1], #16",
        "cbz x2, 5f",
        "cmp x2, #7",
        "csel x4, x7, x4, eq",
        "cmp x2, #8",
        "csel x5, x7, x5, eq",
        "cmp x2, #9",
        "csel x6, x7, x6, eq",
        "b 4b",
        "5:",
        "cbz x5, 8f",
        "cbz x4, 9f",
        "cmp x6, #24",
        "b.ne 9f",
        "add x4, x4, x3",
        // AAELF64 §5.7.15: null-symbol RELATIVE writes AT_BASE + r_addend.
        "6:",
        "cmp x5, #24",
        "b.lo 9f",
        "ldp x8, x9, [x4]",
        "cmp x9, #1027",
        "b.ne 9f",
        "add x8, x8, x3",
        "tst x8, #7",
        "b.ne 9f",
        "ldr x10, [x4, #16]",
        "add x10, x10, x3",
        "str x10, [x8]",
        "add x4, x4, #24",
        "subs x5, x5, #24",
        "b.ne 6b",
        "8:",
        "bl {start}",
        "9:",
        "mov x8, #{exit}",
        "mov x0, #127",
        "svc #0",
        "brk #0",
        exit = const libbreenix::syscall::nr::EXIT_GROUP,
        start = sym ld_breenix::runtime::start,
    );
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    ld_breenix::runtime::fail(info)
}
