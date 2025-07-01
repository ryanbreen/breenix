; Timer interrupt handler with full context switching support
; This handles timer interrupts from both kernel and userspace

global timer_interrupt_entry
extern timer_interrupt_rust_handler
extern PICS
extern InterruptIndex_Timer

section .text
bits 64

timer_interrupt_entry:
    ; Check if we came from userspace (CS & 3 != 0)
    test qword [rsp + 8], 3  ; CS is at RSP+8 in interrupt frame
    jz .from_kernel
    
    ; We came from userspace, need to swap GS
    swapgs
    
.from_kernel:
    ; Save all general purpose registers
    push rax
    push rcx
    push rdx
    push rbx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    
    ; Set up kernel data segments
    mov ax, 0x10  ; Kernel data segment
    mov ds, ax
    mov es, ax
    
    ; Call the Rust handler
    ; RDI = pointer to saved registers (mutable)
    ; RSI = pointer to interrupt stack frame (mutable)
    mov rdi, rsp              ; Saved registers
    lea rsi, [rsp + 15*8]     ; Interrupt frame (after saved regs)
    call timer_interrupt_rust_handler
    
    ; Restore all general purpose registers
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    pop rdx
    pop rcx
    pop rax
    
    ; Check if we're returning to userspace
    test qword [rsp + 8], 3  ; CS is at RSP+8
    jz .to_kernel
    
    ; Returning to userspace, swap GS back
    swapgs
    
.to_kernel:
    ; Return from interrupt
    iretq