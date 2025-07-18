//! Context switching implementation
//!
//! This module provides the low-level context switching functionality
//! for preemptive multitasking.

use super::thread::CpuContext;
use core::arch::global_asm;

// Assembly implementation of context switch
global_asm!(r#"
.global switch_context
.global switch_to_thread

// switch_context(old_context: *mut CpuContext, new_context: *const CpuContext)
// RDI = old_context pointer
// RSI = new_context pointer
switch_context:
    // Save current context to old_context
    mov [rdi + 0x00], rax    // Save RAX
    mov [rdi + 0x08], rbx    // Save RBX
    mov [rdi + 0x10], rcx    // Save RCX
    mov [rdi + 0x18], rdx    // Save RDX
    mov [rdi + 0x20], rsi    // Save RSI
    mov [rdi + 0x28], rdi    // Save RDI
    mov [rdi + 0x30], rbp    // Save RBP
    mov [rdi + 0x38], rsp    // Save RSP
    mov [rdi + 0x40], r8     // Save R8
    mov [rdi + 0x48], r9     // Save R9
    mov [rdi + 0x50], r10    // Save R10
    mov [rdi + 0x58], r11    // Save R11
    mov [rdi + 0x60], r12    // Save R12
    mov [rdi + 0x68], r13    // Save R13
    mov [rdi + 0x70], r14    // Save R14
    mov [rdi + 0x78], r15    // Save R15
    
    // Save RIP (return address)
    mov rax, [rsp]
    mov [rdi + 0x80], rax
    
    // Save RFLAGS
    pushfq
    pop rax
    mov [rdi + 0x88], rax
    
    // We don't save CS/SS here as they don't change in kernel mode
    
    // Load new context from new_context
    mov rax, [rsi + 0x88]    // Load new RFLAGS
    push rax
    popfq                    // Restore RFLAGS
    
    mov rax, [rsi + 0x00]    // Load RAX
    mov rbx, [rsi + 0x08]    // Load RBX
    mov rcx, [rsi + 0x10]    // Load RCX
    mov rdx, [rsi + 0x18]    // Load RDX
    // Skip RSI/RDI for now, we need them
    mov rbp, [rsi + 0x30]    // Load RBP
    // Skip RSP for now
    mov r8,  [rsi + 0x40]    // Load R8
    mov r9,  [rsi + 0x48]    // Load R9
    mov r10, [rsi + 0x50]    // Load R10
    mov r11, [rsi + 0x58]    // Load R11
    mov r12, [rsi + 0x60]    // Load R12
    mov r13, [rsi + 0x68]    // Load R13
    mov r14, [rsi + 0x70]    // Load R14
    mov r15, [rsi + 0x78]    // Load R15
    
    // Prepare for stack switch and return
    mov rax, [rsi + 0x80]    // Load new RIP into RAX
    mov rsp, [rsi + 0x38]    // Switch to new stack
    
    // Now load RSI/RDI
    mov rdi, [rsi + 0x28]    // Load RDI
    mov rsi, [rsi + 0x20]    // Load RSI (do this last!)
    
    // Jump to new context
    jmp rax

// switch_to_thread(new_context: *const CpuContext) -> !
// This is used for the initial switch to a thread
// RDI = new_context pointer
switch_to_thread:
    // Load RFLAGS
    mov rax, [rdi + 0x88]
    push rax
    popfq
    
    // Load all registers
    mov rax, [rdi + 0x00]
    mov rbx, [rdi + 0x08]
    mov rcx, [rdi + 0x10]
    mov rdx, [rdi + 0x18]
    mov rsi, [rdi + 0x20]
    // Skip RDI for now
    mov rbp, [rdi + 0x30]
    mov rsp, [rdi + 0x38]
    mov r8,  [rdi + 0x40]
    mov r9,  [rdi + 0x48]
    mov r10, [rdi + 0x50]
    mov r11, [rdi + 0x58]
    mov r12, [rdi + 0x60]
    mov r13, [rdi + 0x68]
    mov r14, [rdi + 0x70]
    mov r15, [rdi + 0x78]
    
    // Get RIP and RDI
    mov rax, [rdi + 0x80]    // RIP
    mov rdi, [rdi + 0x28]    // RDI (do this last!)
    
    // Jump to thread entry point
    jmp rax
"#);

extern "C" {
    /// Switch from old_context to new_context
    pub fn switch_context(old_context: *mut CpuContext, new_context: *const CpuContext);
    
    /// Switch to a thread for the first time (doesn't save current context)
    pub fn switch_to_thread(new_context: *const CpuContext) -> !;
}

/// Perform a context switch between two threads
/// 
/// # Safety
/// Both context pointers must be valid and properly aligned
pub unsafe fn perform_context_switch(
    old_context: &mut CpuContext,
    new_context: &CpuContext,
) {
    switch_context(
        old_context as *mut CpuContext,
        new_context as *const CpuContext,
    );
}

/// Switch to a thread for the first time
/// 
/// # Safety
/// The context must be valid and properly initialized
pub unsafe fn perform_initial_switch(new_context: &CpuContext) -> ! {
    switch_to_thread(new_context as *const CpuContext);
}