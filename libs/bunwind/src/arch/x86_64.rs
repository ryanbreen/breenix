//! System V AMD64 psABI §3.2.1 and §3.6.2, DWARF Register Number Mapping.
//! Exception data reaches the landing pad in RAX (0) and RDX (1).

use core::arch::naked_asm;

pub(crate) const REGISTER_COUNT: usize = 17;
#[cfg(target_os = "breenix")]
pub(crate) const SP: usize = 7;
#[cfg(target_os = "breenix")]
pub(crate) const IP: usize = 16;
#[cfg(target_os = "breenix")]
pub(crate) const NONVOLATILE: &[usize] = &[3, 6, 12, 13, 14, 15];

#[cfg(target_os = "breenix")]
pub(crate) const fn is_captured_register(index: usize) -> bool {
    index < REGISTER_COUNT
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct RegisterContext {
    pub(crate) registers: [u64; REGISTER_COUNT],
}

/// Snapshot the ABI call boundary. The return PC comes from the stack slot
/// pushed by CALL, and the saved SP excludes that slot.
///
/// # Safety
/// The caller supplies writable context storage.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn capture_context(_context: *mut RegisterContext) {
    naked_asm!(
        "mov [rdi], rax",
        "mov [rdi + 8], rdx",
        "mov [rdi + 16], rcx",
        "mov [rdi + 24], rbx",
        "mov [rdi + 32], rsi",
        "mov [rdi + 40], rdi",
        "mov [rdi + 48], rbp",
        "mov [rdi + 64], r8",
        "mov [rdi + 72], r9",
        "mov [rdi + 80], r10",
        "mov [rdi + 88], r11",
        "mov [rdi + 96], r12",
        "mov [rdi + 104], r13",
        "mov [rdi + 112], r14",
        "mov [rdi + 120], r15",
        "lea r11, [rsp + 8]",
        "mov [rdi + 56], r11",
        "mov r11, [rsp]",
        "mov [rdi + 128], r11",
        "ret",
    );
}

/// Install the context and transfer control without creating a return slot.
/// R11 is volatile and holds the branch target.
///
/// # Safety
/// The saved SP and nonvolatile registers must belong to a live frame, and
/// the saved PC must be a landing pad compatible with that register state.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn restore_context(_context: &RegisterContext) -> ! {
    naked_asm!(
        "mov rax, [rdi]",
        "mov rdx, [rdi + 8]",
        "mov rcx, [rdi + 16]",
        "mov rbx, [rdi + 24]",
        "mov rsi, [rdi + 32]",
        "mov rbp, [rdi + 48]",
        "mov r8, [rdi + 64]",
        "mov r9, [rdi + 72]",
        "mov r10, [rdi + 80]",
        "mov r12, [rdi + 96]",
        "mov r13, [rdi + 104]",
        "mov r14, [rdi + 112]",
        "mov r15, [rdi + 120]",
        "mov r11, [rdi + 128]",
        "mov rsp, [rdi + 56]",
        "mov rdi, [rdi + 40]",
        "jmp r11",
    );
}

// The public assembly entry has no compiler prologue. Its tail jump passes
// the worker in R10. Snapshot the ABI caller before running any Rust code.
#[cfg(target_os = "breenix")]
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn capture_unwind_caller() {
    naked_asm!(
        ".cfi_startproc",
        "sub rsp, 168",
        ".cfi_def_cfa_offset 176",
        "mov [rsp + 136], rdi",
        "mov [rsp + 144], rsi",
        "mov [rsp + 152], rdx",
        "mov [rsp + 160], r10",
        "mov rdi, rsp",
        "call {capture}",
        "mov rdi, rsp",
        "mov rsi, [rsp + 136]",
        "mov rdx, [rsp + 144]",
        "mov rcx, [rsp + 152]",
        "mov [rsp + 40], rsi",
        "lea r11, [rsp + 176]",
        "mov [rsp + 56], r11",
        "mov r11, [rsp + 168]",
        "mov [rsp + 128], r11",
        "call qword ptr [rsp + 160]",
        "add rsp, 168",
        ".cfi_def_cfa_offset 8",
        "ret",
        ".cfi_endproc",
        capture = sym capture_context,
    );
}

#[cfg(target_os = "breenix")]
macro_rules! entry_asm {
    ($worker:path) => {
        core::arch::naked_asm!(
            "lea r10, [rip + {worker}]",
            "jmp {capture}",
            worker = sym $worker,
            capture = sym crate::arch::capture_unwind_caller,
        )
    };
}

#[cfg(target_os = "breenix")]
pub(crate) use entry_asm;
