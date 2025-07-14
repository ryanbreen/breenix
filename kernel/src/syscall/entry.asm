; Syscall entry and exit routines for x86_64
; Uses NASM syntax

section .text

global syscall_entry

; External Rust functions
extern rust_syscall_handler
extern check_need_resched_and_switch
extern get_next_page_table

; Syscall entry point from INT 0x80
; This is called when userspace executes INT 0x80
; On entry:
;   - CPU has already switched to kernel stack (TSS.RSP0)
;   - CPU has pushed: SS, RSP, RFLAGS, CS, RIP
;   - Interrupts are disabled
;   - We're in Ring 0
syscall_entry:
    ; 3-B: INT 0x80 arrival breadcrumb
    push rax
    mov al, 0x80
    out 0x80, al
    pop rax
    
    ; Save all general purpose registers
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rdi
    push rsi
    push rbp
    push rbx
    push rdx
    push rcx
    push rax    ; syscall number

    ; Clear direction flag for string operations
    cld

    ; Switch to kernel GS (for TLS)
    swapgs

    ; Call the Rust syscall handler
    ; Pass pointer to saved registers as argument
    mov rdi, rsp
    call rust_syscall_handler

    ; Return value is in RAX, which will be restored to userspace

    ; Switch back to user GS
    swapgs

    ; Check if we need to reschedule before returning to userspace
    ; This is critical for sys_exit to work correctly
    push rax                  ; Save syscall return value
    mov rdi, rsp              ; Pass pointer to saved registers (after push)
    add rdi, 8                ; Adjust for the pushed rax
    lea rsi, [rsp + 16*8]     ; Pass pointer to interrupt frame
    call check_need_resched_and_switch
    pop rax                   ; Restore syscall return value

    ; Restore all general purpose registers
    pop rax     ; This gets the syscall return value set by handler
    pop rcx
    pop rdx
    pop rbx
    pop rbp
    pop rsi
    pop rdi
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15

    ; Check if we need to switch page tables before returning to userspace
    ; We know we're returning to userspace since this is a syscall
    push rax                    ; Save syscall return value
    push rcx                    ; Save rcx
    push rdx                    ; Save rdx
    
    ; Get the page table to switch to
    call get_next_page_table
    test rax, rax              ; Is there a page table to switch to?
    jz .no_page_table_switch
    
    ; Switch to the process page table
    mov cr3, rax
    ; CRITICAL: Ensure TLB is fully flushed after page table switch
    ; On some systems, mov cr3 might not flush all TLB entries completely
    ; Add explicit full TLB flush for absolute certainty  
    push rax                     ; Save rax
    mov rax, cr4
    mov rcx, rax
    and rcx, 0xFFFFFFFFFFFFFF7F  ; Clear PGE bit (bit 7)
    mov cr4, rcx                  ; Disable global pages (flushes TLB)
    mov cr4, rax                  ; Re-enable global pages
    pop rax                      ; Restore rax
    mfence
    
.no_page_table_switch:
    pop rdx                    ; Restore rdx
    pop rcx                    ; Restore rcx
    pop rax                    ; Restore syscall return value

    ; Return to userspace with IRETQ
    ; This will restore RIP, CS, RFLAGS, RSP, SS from stack
    iretq

