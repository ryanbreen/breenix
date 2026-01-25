//! Process context switching support
//!
//! This module extends the basic context switching to properly handle
//! userspace process contexts, including privilege level transitions.
//!
//! Architecture-specific types:
//! - x86_64: Uses InterruptStackFrame and SavedRegisters (RAX-R15)
//! - AArch64: Uses Aarch64ExceptionFrame and SavedRegisters (X0-X30, SP, ELR, SPSR)

#[cfg(target_arch = "x86_64")]
use super::thread::{CpuContext, Thread, ThreadPrivilege};
#[cfg(target_arch = "x86_64")]
use x86_64::structures::idt::InterruptStackFrame;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

// =============================================================================
// x86_64 ProcessContext
// =============================================================================

/// Extended context for userspace processes (x86_64)
/// This includes additional state needed for Ring 3 processes
#[cfg(target_arch = "x86_64")]
#[derive(Debug, Clone)]
#[repr(C)]
#[allow(dead_code)]
pub struct ProcessContext {
    /// Base CPU context
    pub cpu_context: CpuContext,

    /// Kernel stack pointer (RSP0) for syscalls
    pub kernel_rsp: u64,

    /// Whether this context is from userspace
    pub from_userspace: bool,
}

#[cfg(target_arch = "x86_64")]
impl ProcessContext {
    /// Create a new process context from a Thread
    #[allow(dead_code)]
    pub fn from_thread(thread: &Thread) -> Self {
        ProcessContext {
            cpu_context: thread.context.clone(),
            kernel_rsp: thread.stack_top.as_u64(), // Kernel stack for syscalls
            from_userspace: thread.privilege == ThreadPrivilege::User,
        }
    }

    /// Create from an interrupt stack frame (for saving userspace state)
    #[allow(dead_code)]
    pub fn from_interrupt_frame(frame: &InterruptStackFrame, saved_regs: &SavedRegisters) -> Self {
        let context = CpuContext {
            rax: saved_regs.rax,
            rbx: saved_regs.rbx,
            rcx: saved_regs.rcx,
            rdx: saved_regs.rdx,
            rsi: saved_regs.rsi,
            rdi: saved_regs.rdi,
            rbp: saved_regs.rbp,
            rsp: frame.stack_pointer.as_u64(),
            r8: saved_regs.r8,
            r9: saved_regs.r9,
            r10: saved_regs.r10,
            r11: saved_regs.r11,
            r12: saved_regs.r12,
            r13: saved_regs.r13,
            r14: saved_regs.r14,
            r15: saved_regs.r15,
            rip: frame.instruction_pointer.as_u64(),
            rflags: frame.cpu_flags.bits(),
            cs: frame.code_segment.0 as u64,
            ss: frame.stack_segment.0 as u64,
        };

        ProcessContext {
            cpu_context: context,
            kernel_rsp: 0,                                   // Will be set by caller
            from_userspace: (frame.code_segment.0 & 3) == 3, // Check RPL
        }
    }
}

// =============================================================================
// x86_64 SavedRegisters
// =============================================================================

/// Saved general purpose registers (x86_64)
/// This matches the layout pushed in syscall_entry and timer interrupt
/// Order matters! This must match the push order in assembly
/// Stack grows down, so first push ends up at highest address
#[cfg(target_arch = "x86_64")]
#[derive(Debug, Clone)]
#[repr(C)]
pub struct SavedRegisters {
    // Memory layout after all pushes (RSP points here)
    // Timer interrupt pushes in this order: rax, rcx, rdx, rbx, rbp, rsi, rdi, r8-r15
    // So in memory (from lowest to highest address):
    pub r15: u64, // pushed last, so at RSP+0
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64, // pushed first, so at RSP+14*8
}

#[cfg(target_arch = "x86_64")]
impl SavedRegisters {
    /// Create a new SavedRegisters with all registers zeroed
    #[allow(dead_code)]
    pub const fn new() -> Self {
        Self {
            r15: 0, r14: 0, r13: 0, r12: 0, r11: 0, r10: 0, r9: 0, r8: 0,
            rdi: 0, rsi: 0, rbp: 0, rbx: 0, rdx: 0, rcx: 0, rax: 0,
        }
    }

    /// Get syscall number (stored in RAX on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn syscall_number(&self) -> u64 {
        self.rax
    }

    /// Set syscall number
    #[inline]
    #[allow(dead_code)]
    pub fn set_syscall_number(&mut self, num: u64) {
        self.rax = num;
    }

    /// Get return value (stored in RAX on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn return_value(&self) -> u64 {
        self.rax
    }

    /// Set return value
    #[inline]
    #[allow(dead_code)]
    pub fn set_return_value(&mut self, val: u64) {
        self.rax = val;
    }

    /// Get syscall argument 1 (RDI on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg1(&self) -> u64 { self.rdi }

    /// Get syscall argument 2 (RSI on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg2(&self) -> u64 { self.rsi }

    /// Get syscall argument 3 (RDX on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg3(&self) -> u64 { self.rdx }

    /// Get syscall argument 4 (R10 on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg4(&self) -> u64 { self.r10 }

    /// Get syscall argument 5 (R8 on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg5(&self) -> u64 { self.r8 }

    /// Get syscall argument 6 (R9 on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg6(&self) -> u64 { self.r9 }

    /// Set syscall argument 1 (RDI on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn set_arg1(&mut self, val: u64) { self.rdi = val; }

    /// Set syscall argument 2 (RSI on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn set_arg2(&mut self, val: u64) { self.rsi = val; }

    /// Set syscall argument 3 (RDX on x86_64)
    #[inline]
    #[allow(dead_code)]
    pub fn set_arg3(&mut self, val: u64) { self.rdx = val; }
}

// Note: switch_with_privilege function removed as part of spawn mechanism cleanup
// The new architecture doesn't need kernel-to-userspace transitions

/// Save userspace context from interrupt (x86_64)
/// Called from timer interrupt when preempting userspace
#[cfg(target_arch = "x86_64")]
pub fn save_userspace_context(
    thread: &mut Thread,
    interrupt_frame: &InterruptStackFrame,
    saved_regs: &SavedRegisters,
) {
    // NOTE: No logging here per CLAUDE.md - this is called from interrupt context
    // Update thread's context from interrupt frame
    thread.context.rax = saved_regs.rax;
    thread.context.rbx = saved_regs.rbx;
    thread.context.rcx = saved_regs.rcx;
    thread.context.rdx = saved_regs.rdx;
    thread.context.rsi = saved_regs.rsi;
    thread.context.rdi = saved_regs.rdi;
    thread.context.rbp = saved_regs.rbp;
    thread.context.r8 = saved_regs.r8;
    thread.context.r9 = saved_regs.r9;
    thread.context.r10 = saved_regs.r10;
    thread.context.r11 = saved_regs.r11;
    thread.context.r12 = saved_regs.r12;
    thread.context.r13 = saved_regs.r13;
    thread.context.r14 = saved_regs.r14;
    thread.context.r15 = saved_regs.r15;

    // From interrupt frame
    thread.context.rip = interrupt_frame.instruction_pointer.as_u64();
    thread.context.rsp = interrupt_frame.stack_pointer.as_u64();
    thread.context.rflags = interrupt_frame.cpu_flags.bits();
    thread.context.cs = interrupt_frame.code_segment.0 as u64;
    thread.context.ss = interrupt_frame.stack_segment.0 as u64;
    log::trace!(
        "Saved userspace context for thread {}: RIP={:#x}, RSP={:#x}, RAX={:#x}",
        thread.id,
        thread.context.rip,
        thread.context.rsp,
        thread.context.rax
    );
}

/// Restore userspace context to interrupt frame (x86_64)
/// This modifies the interrupt frame so that IRETQ will restore the process
#[cfg(target_arch = "x86_64")]
pub fn restore_userspace_context(
    thread: &Thread,
    interrupt_frame: &mut InterruptStackFrame,
    saved_regs: &mut SavedRegisters,
) {
    // NOTE: No logging here per CLAUDE.md - this is called from interrupt context
    // Restore general purpose registers
    saved_regs.rax = thread.context.rax;
    saved_regs.rbx = thread.context.rbx;
    saved_regs.rcx = thread.context.rcx;
    saved_regs.rdx = thread.context.rdx;
    saved_regs.rsi = thread.context.rsi;
    saved_regs.rdi = thread.context.rdi;
    saved_regs.rbp = thread.context.rbp;
    saved_regs.r8 = thread.context.r8;
    saved_regs.r9 = thread.context.r9;
    saved_regs.r10 = thread.context.r10;
    saved_regs.r11 = thread.context.r11;
    saved_regs.r12 = thread.context.r12;
    saved_regs.r13 = thread.context.r13;
    saved_regs.r14 = thread.context.r14;
    saved_regs.r15 = thread.context.r15;

    // Restore interrupt frame for IRETQ
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            frame.instruction_pointer = VirtAddr::new(thread.context.rip);
            frame.stack_pointer = VirtAddr::new(thread.context.rsp);
            frame.cpu_flags =
                x86_64::registers::rflags::RFlags::from_bits_truncate(thread.context.rflags);

            // CRITICAL: Set CS and SS for userspace
            if thread.privilege == ThreadPrivilege::User {
                // Use the actual selectors from the GDT module
                frame.code_segment = crate::gdt::user_code_selector();
                frame.stack_segment = crate::gdt::user_data_selector();
            } else {
                frame.code_segment = crate::gdt::kernel_code_selector();
                frame.stack_segment = crate::gdt::kernel_data_selector();
            }
        });
    }
    // NOTE: No logging here per CLAUDE.md - this is called from interrupt context
}

// =============================================================================
// AArch64 SavedRegisters
// =============================================================================

/// Saved general purpose registers (AArch64)
///
/// This struct captures all general-purpose registers (X0-X30), the stack pointer,
/// program counter (ELR_EL1), and saved program status (SPSR_EL1) for context
/// switching and signal delivery.
///
/// ARM64 calling convention (AAPCS64):
/// - X0-X7: Arguments/results (syscall args in X0-X5, syscall number in X8)
/// - X8: Indirect result register (used for syscall number on Linux/Breenix)
/// - X9-X15: Temporaries (caller-saved)
/// - X16-X17: Intra-procedure call scratch (IP0, IP1)
/// - X18: Platform register (reserved)
/// - X19-X28: Callee-saved registers
/// - X29: Frame pointer (FP)
/// - X30: Link register (LR) - return address
/// - SP: Stack pointer
/// - PC: Program counter (stored in ELR_EL1 for exceptions)
#[cfg(target_arch = "aarch64")]
#[derive(Debug, Clone)]
#[repr(C)]
pub struct SavedRegisters {
    // General-purpose registers X0-X30 (31 registers)
    pub x0: u64,   // Argument 1 / return value
    pub x1: u64,   // Argument 2
    pub x2: u64,   // Argument 3
    pub x3: u64,   // Argument 4
    pub x4: u64,   // Argument 5
    pub x5: u64,   // Argument 6
    pub x6: u64,   // Caller-saved
    pub x7: u64,   // Caller-saved
    pub x8: u64,   // Syscall number (Linux ABI)
    pub x9: u64,   // Temporary
    pub x10: u64,  // Temporary
    pub x11: u64,  // Temporary
    pub x12: u64,  // Temporary
    pub x13: u64,  // Temporary
    pub x14: u64,  // Temporary
    pub x15: u64,  // Temporary
    pub x16: u64,  // IP0 (intra-procedure-call scratch)
    pub x17: u64,  // IP1 (intra-procedure-call scratch)
    pub x18: u64,  // Platform register (reserved)
    pub x19: u64,  // Callee-saved
    pub x20: u64,  // Callee-saved
    pub x21: u64,  // Callee-saved
    pub x22: u64,  // Callee-saved
    pub x23: u64,  // Callee-saved
    pub x24: u64,  // Callee-saved
    pub x25: u64,  // Callee-saved
    pub x26: u64,  // Callee-saved
    pub x27: u64,  // Callee-saved
    pub x28: u64,  // Callee-saved
    pub x29: u64,  // Frame pointer (FP)
    pub x30: u64,  // Link register (LR)

    /// Stack pointer (SP_EL0 for userspace)
    pub sp: u64,

    /// Exception link register (program counter / return address from exception)
    /// This is ELR_EL1 which holds the address to return to after the exception
    pub elr: u64,

    /// Saved program status register (SPSR_EL1)
    /// Contains the processor state (NZCV flags, exception mask bits, execution state)
    /// to restore when returning from the exception
    pub spsr: u64,
}

#[cfg(target_arch = "aarch64")]
impl SavedRegisters {
    /// Create a new SavedRegisters with all registers zeroed
    #[allow(dead_code)]
    pub const fn new() -> Self {
        Self {
            x0: 0, x1: 0, x2: 0, x3: 0, x4: 0, x5: 0, x6: 0, x7: 0,
            x8: 0, x9: 0, x10: 0, x11: 0, x12: 0, x13: 0, x14: 0, x15: 0,
            x16: 0, x17: 0, x18: 0, x19: 0, x20: 0, x21: 0, x22: 0, x23: 0,
            x24: 0, x25: 0, x26: 0, x27: 0, x28: 0, x29: 0, x30: 0,
            sp: 0, elr: 0, spsr: 0,
        }
    }

    /// Create SavedRegisters from an Aarch64ExceptionFrame
    ///
    /// This captures all register state from an exception frame for later restoration.
    /// Used for signal delivery and context switching.
    #[allow(dead_code)]
    pub fn from_exception_frame(frame: &crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame) -> Self {
        Self {
            x0: frame.x0,
            x1: frame.x1,
            x2: frame.x2,
            x3: frame.x3,
            x4: frame.x4,
            x5: frame.x5,
            x6: frame.x6,
            x7: frame.x7,
            x8: frame.x8,
            x9: frame.x9,
            x10: frame.x10,
            x11: frame.x11,
            x12: frame.x12,
            x13: frame.x13,
            x14: frame.x14,
            x15: frame.x15,
            x16: frame.x16,
            x17: frame.x17,
            x18: frame.x18,
            x19: frame.x19,
            x20: frame.x20,
            x21: frame.x21,
            x22: frame.x22,
            x23: frame.x23,
            x24: frame.x24,
            x25: frame.x25,
            x26: frame.x26,
            x27: frame.x27,
            x28: frame.x28,
            x29: frame.x29,
            x30: frame.x30,
            sp: 0, // SP_EL0 is not in the exception frame, need to read separately
            elr: frame.elr,
            spsr: frame.spsr,
        }
    }

    /// Create SavedRegisters from an exception frame with explicit SP value
    ///
    /// Use this when you have access to SP_EL0 (e.g., from assembly code that
    /// saved it separately from the exception frame).
    #[allow(dead_code)]
    pub fn from_exception_frame_with_sp(
        frame: &crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
        sp: u64,
    ) -> Self {
        let mut regs = Self::from_exception_frame(frame);
        regs.sp = sp;
        regs
    }

    /// Apply saved registers back to an exception frame
    ///
    /// This writes the saved register state back to an exception frame,
    /// which will be restored when returning from the exception via ERET.
    #[allow(dead_code)]
    pub fn apply_to_frame(&self, frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame) {
        frame.x0 = self.x0;
        frame.x1 = self.x1;
        frame.x2 = self.x2;
        frame.x3 = self.x3;
        frame.x4 = self.x4;
        frame.x5 = self.x5;
        frame.x6 = self.x6;
        frame.x7 = self.x7;
        frame.x8 = self.x8;
        frame.x9 = self.x9;
        frame.x10 = self.x10;
        frame.x11 = self.x11;
        frame.x12 = self.x12;
        frame.x13 = self.x13;
        frame.x14 = self.x14;
        frame.x15 = self.x15;
        frame.x16 = self.x16;
        frame.x17 = self.x17;
        frame.x18 = self.x18;
        frame.x19 = self.x19;
        frame.x20 = self.x20;
        frame.x21 = self.x21;
        frame.x22 = self.x22;
        frame.x23 = self.x23;
        frame.x24 = self.x24;
        frame.x25 = self.x25;
        frame.x26 = self.x26;
        frame.x27 = self.x27;
        frame.x28 = self.x28;
        frame.x29 = self.x29;
        frame.x30 = self.x30;
        // Note: SP is in SP_EL0, not the exception frame - must be handled separately
        frame.elr = self.elr;
        frame.spsr = self.spsr;
    }

    // =========================================================================
    // Syscall argument accessors (ARM64 ABI)
    // =========================================================================

    /// Get syscall number (stored in X8 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn syscall_number(&self) -> u64 {
        self.x8
    }

    /// Set syscall number
    #[inline]
    #[allow(dead_code)]
    pub fn set_syscall_number(&mut self, num: u64) {
        self.x8 = num;
    }

    /// Get return value (stored in X0 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn return_value(&self) -> u64 {
        self.x0
    }

    /// Set return value
    #[inline]
    #[allow(dead_code)]
    pub fn set_return_value(&mut self, val: u64) {
        self.x0 = val;
    }

    /// Get syscall argument 1 (X0 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg1(&self) -> u64 { self.x0 }

    /// Get syscall argument 2 (X1 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg2(&self) -> u64 { self.x1 }

    /// Get syscall argument 3 (X2 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg3(&self) -> u64 { self.x2 }

    /// Get syscall argument 4 (X3 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg4(&self) -> u64 { self.x3 }

    /// Get syscall argument 5 (X4 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg5(&self) -> u64 { self.x4 }

    /// Get syscall argument 6 (X5 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn arg6(&self) -> u64 { self.x5 }

    /// Set syscall argument 1 (X0 on ARM64)
    /// Note: This also sets the return value since X0 is used for both
    #[inline]
    #[allow(dead_code)]
    pub fn set_arg1(&mut self, val: u64) { self.x0 = val; }

    /// Set syscall argument 2 (X1 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn set_arg2(&mut self, val: u64) { self.x1 = val; }

    /// Set syscall argument 3 (X2 on ARM64)
    #[inline]
    #[allow(dead_code)]
    pub fn set_arg3(&mut self, val: u64) { self.x2 = val; }

    // =========================================================================
    // Program counter / stack pointer accessors
    // =========================================================================

    /// Get the program counter (instruction pointer)
    #[inline]
    #[allow(dead_code)]
    pub fn instruction_pointer(&self) -> u64 {
        self.elr
    }

    /// Set the program counter (instruction pointer)
    #[inline]
    #[allow(dead_code)]
    pub fn set_instruction_pointer(&mut self, addr: u64) {
        self.elr = addr;
    }

    /// Get the stack pointer
    #[inline]
    #[allow(dead_code)]
    pub fn stack_pointer(&self) -> u64 {
        self.sp
    }

    /// Set the stack pointer
    #[inline]
    #[allow(dead_code)]
    pub fn set_stack_pointer(&mut self, addr: u64) {
        self.sp = addr;
    }

    /// Get the link register (return address for BL instruction)
    #[inline]
    #[allow(dead_code)]
    pub fn link_register(&self) -> u64 {
        self.x30
    }

    /// Set the link register
    #[inline]
    #[allow(dead_code)]
    pub fn set_link_register(&mut self, addr: u64) {
        self.x30 = addr;
    }

    /// Get the frame pointer
    #[inline]
    #[allow(dead_code)]
    pub fn frame_pointer(&self) -> u64 {
        self.x29
    }

    /// Set the frame pointer
    #[inline]
    #[allow(dead_code)]
    pub fn set_frame_pointer(&mut self, addr: u64) {
        self.x29 = addr;
    }

    // =========================================================================
    // SPSR (Saved Program Status Register) accessors
    // =========================================================================

    /// Get the saved program status register
    #[inline]
    #[allow(dead_code)]
    pub fn program_status(&self) -> u64 {
        self.spsr
    }

    /// Set the saved program status register
    #[inline]
    #[allow(dead_code)]
    pub fn set_program_status(&mut self, status: u64) {
        self.spsr = status;
    }

    /// Check if the saved state was from EL0 (userspace)
    #[inline]
    #[allow(dead_code)]
    pub fn is_from_userspace(&self) -> bool {
        // SPSR_EL1.M[3:0] contains the exception level
        // EL0t = 0b0000 (EL0 with SP_EL0)
        (self.spsr & 0xF) == 0
    }

    /// Create SPSR for returning to EL0 (userspace) with interrupts enabled
    #[inline]
    #[allow(dead_code)]
    pub fn spsr_el0_default() -> u64 {
        // EL0t (mode 0) with DAIF clear (interrupts enabled)
        0x0
    }

    /// Create SPSR for returning to EL1h (kernel) with interrupts masked
    #[inline]
    #[allow(dead_code)]
    pub fn spsr_el1_default() -> u64 {
        // EL1h (mode 5) with DAIF masked
        0x3c5
    }
}
