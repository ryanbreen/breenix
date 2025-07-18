; Timer interrupt entry with deferred context switching
; 
; This follows the correct OS design pattern:
; 1. Timer handler does minimal work
; 2. Context switching happens on interrupt return path
; 3. Clear separation of concerns

global timer_interrupt_entry
extern timer_interrupt_handler
extern check_need_resched_and_switch
extern get_next_page_table
extern log_iret_to_userspace

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
    
    ; CRITICAL: Check if we came from userspace and need to swap GS
    ; Get CS from interrupt frame to check privilege level
    mov rax, [rsp + 15*8 + 8]   ; Skip saved regs + RIP to get CS
    and rax, 3                   ; Check privilege level
    cmp rax, 3                   ; Ring 3?
    jne .skip_swapgs_entry       ; If not from userspace, skip swapgs
    
    ; We came from userspace, swap to kernel GS
    swapgs
    
.skip_swapgs_entry:
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
    
    ; Check if we need to switch page tables before returning to userspace
    ; This is critical - we must do this right before iretq
    push rax                    ; Save rax
    push rcx                    ; Save rcx
    push rdx                    ; Save rdx
    
    ; Check if we're returning to ring 3 (userspace)
    mov rax, [rsp + 24 + 8]    ; Get CS from interrupt frame (3 pushes + RIP)
    and rax, 3                 ; Check privilege level
    cmp rax, 3                 ; Ring 3?
    jne .no_userspace_return
    
    ; We're returning to userspace, check if we need to switch page tables
    call get_next_page_table
    test rax, rax              ; Is there a page table to switch to?
    jz .skip_page_table_switch
    
    ; Switch to the process page table
    mov cr3, rax
    ; CRITICAL: Ensure TLB is fully flushed after page table switch
    ; On some systems, mov cr3 might not flush all TLB entries completely
    ; Add explicit full TLB flush for absolute certainty
    push rax                     ; Save rax (contains page table frame)
    mov rax, cr4
    mov rcx, rax
    and rcx, 0xFFFFFFFFFFFFFF7F  ; Clear PGE bit (bit 7)
    mov cr4, rcx                  ; Disable global pages (flushes TLB)
    mov cr4, rax                  ; Re-enable global pages
    pop rax                      ; Restore rax
    mfence
    
.skip_page_table_switch:
    ; CRITICAL: Swap back to userspace GS before returning to ring 3
    swapgs
    
    pop rdx                    ; Restore rdx
    pop rcx                    ; Restore rcx
    pop rax                    ; Restore rax
    
    ; Log IRET details for debugging
    push rax
    push rcx
    push rdx
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    
    ; Get interrupt frame values from stack
    mov rdi, [rsp + 9*8]         ; RIP (9 pushes above)
    mov rsi, [rsp + 9*8 + 24]    ; RSP (RIP + CS + RFLAGS)
    mov rdx, [rsp + 9*8 + 8]     ; CS
    mov rcx, [rsp + 9*8 + 32]    ; SS
    call log_iret_to_userspace
    
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rax
    
    ; Return from interrupt to userspace
    iretq
    
.no_userspace_return:
    pop rdx                    ; Restore rdx
    pop rcx                    ; Restore rcx
    pop rax                    ; Restore rax
    
    ; Return from interrupt to kernel
    iretq