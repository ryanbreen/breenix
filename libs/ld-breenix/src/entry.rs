//! AMD64 psABI §3.4 initial stack and §4.4 RELA bootstrap.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

#[unsafe(naked)]
#[no_mangle]
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

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    ld_breenix::runtime::fail(info)
}
