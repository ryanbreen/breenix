//! AADWARF64 §4, mapping from DWARF register numbers to AArch64 registers,
//! and AAPCS64 §§6.1.1–6.1.2. Exception data is carried in X0 and X1.

use core::arch::naked_asm;

// X0–X30, SP, PC and the low 64 bits of V8–V15 (the preserved portion).
pub(crate) const REGISTER_COUNT: usize = 80;
#[cfg(target_os = "breenix")]
pub(crate) const SP: usize = 31;
#[cfg(target_os = "breenix")]
pub(crate) const IP: usize = 32;
#[cfg(target_os = "breenix")]
pub(crate) const NONVOLATILE: &[usize] = &[
    19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 72, 73, 74, 75, 76, 77, 78, 79,
];

#[cfg(target_os = "breenix")]
pub(crate) const fn is_captured_register(index: usize) -> bool {
    matches!(index, 0..=17 | 19..=32 | 72..=79)
}

#[repr(C)]
pub(crate) struct RegisterContext {
    pub(crate) registers: [u64; REGISTER_COUNT],
}

#[cfg(test)]
impl Default for RegisterContext {
    fn default() -> Self {
        Self {
            registers: [0; REGISTER_COUNT],
        }
    }
}

/// Snapshot the ABI call boundary. BL leaves the return PC in X30 and does
/// not move SP. The platform register X18 is reserved.
///
/// # Safety
/// The caller supplies writable context storage.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn capture_context(_context: *mut RegisterContext) {
    naked_asm!(
        "stp x0, x1, [x0, #0]",
        "stp x2, x3, [x0, #16]",
        "stp x4, x5, [x0, #32]",
        "stp x6, x7, [x0, #48]",
        "stp x8, x9, [x0, #64]",
        "stp x10, x11, [x0, #80]",
        "stp x12, x13, [x0, #96]",
        "stp x14, x15, [x0, #112]",
        "stp x16, x17, [x0, #128]",
        "stp xzr, x19, [x0, #144]",
        "stp x20, x21, [x0, #160]",
        "stp x22, x23, [x0, #176]",
        "stp x24, x25, [x0, #192]",
        "stp x26, x27, [x0, #208]",
        "stp x28, x29, [x0, #224]",
        "mov x9, sp",
        "stp x30, x9, [x0, #240]",
        "str x30, [x0, #256]",
        // Initialize the unrepresented DWARF columns between PC and V8.
        "add x9, x0, #264",
        "add x10, x0, #576",
        "2:",
        "str xzr, [x9], #8",
        "cmp x9, x10",
        "b.ne 2b",
        "stp d8, d9, [x9, #0]",
        "stp d10, d11, [x9, #16]",
        "stp d12, d13, [x9, #32]",
        "stp d14, d15, [x9, #48]",
        "ret",
    );
}

/// Install the context and branch without writing LR. X16 is volatile and
/// holds the branch target. X18 stays untouched.
///
/// # Safety
/// The saved SP and nonvolatile registers must belong to a live frame, and
/// the saved PC must be a landing pad compatible with that register state.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn restore_context(_context: &RegisterContext) -> ! {
    naked_asm!(
        "mov x17, x0",
        "ldp x0, x1, [x17, #0]",
        "ldp x2, x3, [x17, #16]",
        "ldp x4, x5, [x17, #32]",
        "ldp x6, x7, [x17, #48]",
        "ldp x8, x9, [x17, #64]",
        "ldp x10, x11, [x17, #80]",
        "ldp x12, x13, [x17, #96]",
        "ldp x14, x15, [x17, #112]",
        "ldr x19, [x17, #152]",
        "ldp x20, x21, [x17, #160]",
        "ldp x22, x23, [x17, #176]",
        "ldp x24, x25, [x17, #192]",
        "ldp x26, x27, [x17, #208]",
        "ldp x28, x29, [x17, #224]",
        "add x16, x17, #576",
        "ldp d8, d9, [x16, #0]",
        "ldp d10, d11, [x16, #16]",
        "ldp d12, d13, [x16, #32]",
        "ldp d14, d15, [x16, #48]",
        "ldr x16, [x17, #248]",
        "mov sp, x16",
        "ldr x30, [x17, #240]",
        "ldr x16, [x17, #256]",
        "ldr x17, [x17, #136]",
        "br x16",
    );
}

// The public assembly entry tail-branches here with the worker in X16.
// Restore the original call boundary in the snapshot after the capture call.
#[cfg(target_os = "breenix")]
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn capture_unwind_caller() {
    naked_asm!(
        ".cfi_startproc",
        "sub sp, sp, #688",
        ".cfi_def_cfa_offset 688",
        "str x0, [sp, #640]",
        "str x1, [sp, #648]",
        "str x2, [sp, #656]",
        "str x30, [sp, #664]",
        ".cfi_offset 30, -24",
        "str x16, [sp, #672]",
        "mov x0, sp",
        "bl {capture}",
        "mov x0, sp",
        "ldr x1, [sp, #640]",
        "ldr x2, [sp, #648]",
        "ldr x3, [sp, #656]",
        "str x1, [sp]",
        "ldr x9, [sp, #664]",
        "str x9, [sp, #240]",
        "add x10, sp, #688",
        "str x10, [sp, #248]",
        "str x9, [sp, #256]",
        "ldr x16, [sp, #672]",
        "blr x16",
        "ldr x30, [sp, #664]",
        ".cfi_restore 30",
        "add sp, sp, #688",
        ".cfi_def_cfa_offset 0",
        "ret",
        ".cfi_endproc",
        capture = sym capture_context,
    );
}

#[cfg(target_os = "breenix")]
macro_rules! entry_asm {
    ($worker:path) => {
        core::arch::naked_asm!(
            "adrp x16, {worker}",
            "add x16, x16, :lo12:{worker}",
            "b {capture}",
            worker = sym $worker,
            capture = sym crate::arch::capture_unwind_caller,
        )
    };
}

#[cfg(target_os = "breenix")]
pub(crate) use entry_asm;
