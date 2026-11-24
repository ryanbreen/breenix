// The fixture supplies its own call and landing instructions with no CFI.
// Register numbers come from the AMD64 psABI DWARF mapping and AADWARF64 §4.
use crate::arch::{capture_context, restore_context, RegisterContext};
use core::arch::naked_asm;

#[cfg(target_arch = "x86_64")]
const CARRIED: [(usize, u64); 2] = [(3, 43), (15, 47)];
#[cfg(target_arch = "aarch64")]
const CARRIED: [(usize, u64); 4] = [(19, 43), (28, 47), (72, 51), (79, 53)];

#[cfg(target_arch = "x86_64")]
const CONTROL: (usize, usize) = (7, 16);
#[cfg(target_arch = "aarch64")]
const CONTROL: (usize, usize) = (31, 32);

#[cfg(target_arch = "x86_64")]
const EXCEPTION: (usize, usize) = (0, 1);
#[cfg(target_arch = "aarch64")]
const EXCEPTION: (usize, usize) = (0, 1);

unsafe extern "C" fn enter_landing_pad(
    context: &mut RegisterContext,
    sp: u64,
    return_ip: u64,
    landing_ip: u64,
) -> ! {
    assert_eq!(context.registers[CONTROL.0], sp);
    assert_eq!(context.registers[CONTROL.1], return_ip);
    for (register, value) in CARRIED {
        assert_eq!(context.registers[register], value, "DWARF register {register}");
    }
    context.registers[CONTROL.1] = landing_ip;
    context.registers[EXCEPTION.0] = 19;
    context.registers[EXCEPTION.1] = 23;
    // SAFETY: The assembly fixture's frame is live and its landing label
    // expects exactly this stack pointer, register state and instruction pointer.
    unsafe { restore_context(context) }
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn landing_pad_probe(context: &mut RegisterContext) -> u64 {
    naked_asm!(
        "push rbx",
        "push r12",
        "push r15",
        "mov r12, rdi",
        "mov ebx, 43",
        "mov r15d, 47",
        "call {capture}",
        "2:",
        "xor ebx, ebx",
        "xor r15d, r15d",
        "mov rdi, r12",
        "mov rsi, rsp",
        "lea rdx, [rip + 2b]",
        "lea rcx, [rip + 3f]",
        "call {enter}",
        "ud2",
        "3:",
        "cmp rax, 19",
        "jne 4f",
        "cmp rdx, 23",
        "jne 4f",
        "cmp rbx, 43",
        "jne 4f",
        "cmp r15, 47",
        "jne 4f",
        "mov eax, 42",
        "jmp 5f",
        "4: xor eax, eax",
        "5:",
        "pop r15",
        "pop r12",
        "pop rbx",
        "ret",
        capture = sym capture_context,
        enter = sym enter_landing_pad,
    );
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn landing_pad_probe(context: &mut RegisterContext) -> u64 {
    naked_asm!(
        "stp x19, x20, [sp, #-48]!",
        "stp x28, x30, [sp, #16]",
        "stp d8, d15, [sp, #32]",
        "mov x20, x0",
        "mov x19, #43",
        "mov x28, #47",
        "mov x9, #51",
        "fmov d8, x9",
        "mov x9, #53",
        "fmov d15, x9",
        "bl {capture}",
        "2:",
        "mov x19, xzr",
        "mov x28, xzr",
        "fmov d8, xzr",
        "fmov d15, xzr",
        "mov x0, x20",
        "mov x1, sp",
        "adr x2, 2b",
        "adr x3, 3f",
        "bl {enter}",
        "brk #0",
        "3:",
        "cmp x0, #19",
        "b.ne 4f",
        "cmp x1, #23",
        "b.ne 4f",
        "cmp x19, #43",
        "b.ne 4f",
        "cmp x28, #47",
        "b.ne 4f",
        "fmov x9, d8",
        "cmp x9, #51",
        "b.ne 4f",
        "fmov x9, d15",
        "cmp x9, #53",
        "b.ne 4f",
        "mov x0, #42",
        "b 5f",
        "4: mov x0, xzr",
        "5:",
        "ldp d8, d15, [sp, #32]",
        "ldp x28, x30, [sp, #16]",
        "ldp x19, x20, [sp], #48",
        "ret",
        capture = sym capture_context,
        enter = sym enter_landing_pad,
    );
}

#[test]
fn restore_context_resumes_at_landing_pad() {
    let mut context = RegisterContext::default();
    // SAFETY: The fixture receives writable context storage and saves the
    // C ABI's nonvolatile registers before modifying them.
    assert_eq!(unsafe { landing_pad_probe(&mut context) }, 42);
}
