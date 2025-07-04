; Timer interrupt entry with deferred context switching
; 
; This follows the correct OS design pattern:
; 1. Timer handler does minimal work
; 2. Context switching happens on interrupt return path
; 3. Clear separation of concerns

global timer_interrupt_entry
extern timer_interrupt_handler
extern check_need_resched_and_switch

section .text
bits 64

timer_interrupt_entry:
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
    
    ; Call the timer handler
    ; This ONLY updates ticks, quantum, and sets need_resched flag
    call timer_interrupt_handler
    
    ; Now check if we need to reschedule
    ; This is the CORRECT place for context switching logic
    mov rdi, rsp              ; Pass pointer to saved registers
    lea rsi, [rsp + 15*8]     ; Pass pointer to interrupt frame
    call check_need_resched_and_switch
    
    ; Restore all general purpose registers
    ; Note: If we switched contexts, these will be different registers!
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
    
    ; Return from interrupt
    iretq