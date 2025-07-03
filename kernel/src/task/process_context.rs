//! Process context switching support
//!
//! This module extends the basic context switching to properly handle
//! userspace process contexts, including privilege level transitions.

use super::thread::{Thread, ThreadPrivilege, CpuContext};
use super::context;
use x86_64::VirtAddr;
use x86_64::structures::idt::InterruptStackFrame;

/// Extended context for userspace processes
/// This includes additional state needed for Ring 3 processes
#[derive(Debug, Clone)]
#[repr(C)]
pub struct ProcessContext {
    /// Base CPU context
    pub cpu_context: CpuContext,
    
    /// Kernel stack pointer (RSP0) for syscalls
    pub kernel_rsp: u64,
    
    /// Whether this context is from userspace
    pub from_userspace: bool,
}

impl ProcessContext {
    /// Create a new process context from a Thread
    pub fn from_thread(thread: &Thread) -> Self {
        ProcessContext {
            cpu_context: thread.context.clone(),
            kernel_rsp: thread.stack_top.as_u64(), // Kernel stack for syscalls
            from_userspace: thread.privilege == ThreadPrivilege::User,
        }
    }
    
    /// Create from an interrupt stack frame (for saving userspace state)
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
            kernel_rsp: 0, // Will be set by caller
            from_userspace: (frame.code_segment.0 & 3) == 3, // Check RPL
        }
    }
}

/// Saved general purpose registers
/// This matches the layout pushed in syscall_entry and timer interrupt
/// Order matters! This must match the push order in assembly
/// Stack grows down, so first push ends up at highest address
#[derive(Debug, Clone)]
#[repr(C)]
pub struct SavedRegisters {
    // Memory layout after all pushes (RSP points here)
    // Timer interrupt pushes in this order: rax, rcx, rdx, rbx, rbp, rsi, rdi, r8-r15
    // So in memory (from lowest to highest address):
    pub r15: u64,  // pushed last, so at RSP+0
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
    pub rax: u64,  // pushed first, so at RSP+14*8
}

/// Perform context switch with userspace support
/// 
/// This handles switching between any combination of kernel/user threads
pub unsafe fn switch_with_privilege(
    old_thread: &mut Thread,
    new_thread: &Thread,
) -> Result<(), &'static str> {
    // If switching to a userspace thread for the first time
    if new_thread.privilege == ThreadPrivilege::User && 
       new_thread.entry_point.is_some() &&
       new_thread.context.rip == new_thread.entry_point.unwrap() as *const () as u64 {
        // Initial switch to userspace
        log::debug!("Initial switch to userspace thread {}", new_thread.id);
        
        // Use the userspace switch mechanism
        crate::task::userspace_switch::switch_to_userspace(
            VirtAddr::new(new_thread.context.rip),
            VirtAddr::new(new_thread.context.rsp),
            new_thread.context.cs as u16,
            new_thread.context.ss as u16,
        );
    } else {
        // Regular context switch
        context::perform_context_switch(
            &mut old_thread.context,
            &new_thread.context,
        );
    }
    
    Ok(())
}

/// Save userspace context from interrupt
/// Called from timer interrupt when preempting userspace
pub fn save_userspace_context(
    thread: &mut Thread,
    interrupt_frame: &InterruptStackFrame,
    saved_regs: &SavedRegisters,
) {
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
    
    log::trace!("Saved userspace context for thread {}: RIP={:#x}, RSP={:#x}, RAX={:#x}", 
               thread.id, thread.context.rip, thread.context.rsp, thread.context.rax);
}

/// Restore userspace context to interrupt frame
/// This modifies the interrupt frame so that IRETQ will restore the process
pub fn restore_userspace_context(
    thread: &Thread,
    interrupt_frame: &mut InterruptStackFrame,
    saved_regs: &mut SavedRegisters,
) {
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
            frame.cpu_flags = x86_64::registers::rflags::RFlags::from_bits_truncate(thread.context.rflags);
            // CS and SS are already correct from the saved context
        });
    }
    
    log::trace!("Restored userspace context for thread {}: RIP={:#x}, RSP={:#x}, RAX={:#x}", 
               thread.id, thread.context.rip, thread.context.rsp, thread.context.rax);
}