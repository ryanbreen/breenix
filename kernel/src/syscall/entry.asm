; Syscall entry and exit routines for x86_64
; Uses NASM syntax

section .text

global syscall_entry
global syscall_return_to_userspace

; External Rust functions
extern rust_syscall_handler
extern check_need_resched_and_switch

; Syscall entry point from INT 0x80
; This is called when userspace executes INT 0x80
; On entry:
;   - CPU has already switched to kernel stack (TSS.RSP0)
;   - CPU has pushed: SS, RSP, RFLAGS, CS, RIP
;   - Interrupts are disabled
;   - We're in Ring 0
syscall_entry:
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

    ; Return to userspace with IRETQ
    ; This will restore RIP, CS, RFLAGS, RSP, SS from stack
    iretq

; This function switches from kernel to userspace
; Used when starting a new userspace thread
; Arguments:
;   rdi - user RIP (entry point)
;   rsi - user RSP (stack pointer)
;   rdx - user RFLAGS
syscall_return_to_userspace:
    ; Disable interrupts during the switch
    cli

    ; Switch to user GS for userspace
    swapgs

    ; Build IRETQ frame on stack
    ; We need: SS, RSP, RFLAGS, CS, RIP

    ; User data segment selector (SS)
    mov rax, 0x2b  ; User data selector with RPL=3
    push rax

    ; User stack pointer
    push rsi

    ; RFLAGS (with interrupts enabled)
    push rdx

    ; User code segment selector (CS)
    mov rax, 0x33  ; User code selector with RPL=3
    push rax

    ; User instruction pointer
    push rdi

    ; Clear all registers to prevent information leaks
    xor rax, rax
    xor rbx, rbx
    xor rcx, rcx
    xor rdx, rdx
    xor rsi, rsi
    xor rdi, rdi
    xor rbp, rbp
    xor r8, r8
    xor r9, r9
    xor r10, r10
    xor r11, r11
    xor r12, r12
    xor r13, r13
    xor r14, r14
    xor r15, r15

    ; Jump to userspace
    iretq