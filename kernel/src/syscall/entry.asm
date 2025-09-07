; Syscall entry and exit routines for x86_64
; Uses NASM syntax

; CRITICAL: Place syscall entry code in dedicated section that stays mapped
; This ensures the code is accessible after CR3 switches to process page tables
section .text.entry

global syscall_entry
global syscall_return_to_userspace

; External Rust functions
extern rust_syscall_handler
extern check_need_resched_and_switch
extern trace_iretq_to_ring3

; Syscall entry point from INT 0x80
; This is called when userspace executes INT 0x80
; On entry:
;   - CPU has already switched to kernel stack (TSS.RSP0)
;   - CPU has pushed: SS, RSP, RFLAGS, CS, RIP
;   - Interrupts are disabled
;   - We're in Ring 0
syscall_entry:
    ; Save all general purpose registers in SavedRegisters order
    ; Must match timer interrupt order: rax first, r15 last
    push rax    ; syscall number (pushed first, at RSP+14*8)
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
    push r15    ; pushed last, at RSP+0

    ; Clear direction flag for string operations
    cld

    ; Always switch to kernel GS for INT 0x80 entry
    ; INT 0x80 is only used from userspace, so we always need swapgs
    swapgs

    ; Call the Rust syscall handler
    ; Pass pointer to saved registers as argument
    mov rdi, rsp
    call rust_syscall_handler

    ; Return value is in RAX, which will be restored to userspace
    ; NOTE: We stay in kernel GS mode until just before iretq
    ; All kernel functions (scheduling, page table, tracing) need kernel GS

    ; Check if we need to reschedule before returning to userspace
    ; This is critical for sys_exit to work correctly
    push rax                  ; Save syscall return value
    mov rdi, rsp              ; Pass pointer to saved registers (after push)
    add rdi, 8                ; Adjust for the pushed rax
    lea rsi, [rsp + 16*8]     ; Pass pointer to interrupt frame
    call check_need_resched_and_switch
    pop rax                   ; Restore syscall return value

    ; Restore all general purpose registers in reverse push order
    pop r15    ; Last pushed, first popped
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
    pop rax     ; This gets the syscall return value set by handler

    ; Check if we need to switch page tables before returning to userspace
    ; FIXED: CR3 switching now happens in the scheduler during context switch
    ; No need to switch page tables here - we're already running on the
    ; process's page table since the last context switch
    
    ; We know we're returning to userspace since this is a syscall

    ; Trace that we're about to return to Ring 3 with full frame info
    ; Save all registers that might be clobbered by the call
    push rax                   ; Save syscall return value (CRITICAL!)
    push rcx
    push rdx
    push rdi
    push rsi
    push r8
    push r9
    push r10
    push r11
    
    ; Pass pointer to IRETQ frame (RIP, CS, RFLAGS, RSP, SS)
    mov rdi, rsp
    add rdi, 72                ; Skip 9 pushed registers (9 * 8 = 72) to point to RIP
    call trace_iretq_to_ring3
    
    ; Restore all registers in reverse order
    pop r11
    pop r10
    pop r9
    pop r8
    pop rsi
    pop rdi
    pop rdx
    pop rcx
    pop rax                    ; Restore syscall return value (CRITICAL!)

    ; Switch back to user GS right before returning to userspace
    ; All kernel work is now done, safe to switch GS
    swapgs

    ; Direct serial output marker - about to execute IRETQ
    ; Write 0xEE to serial port to indicate we reached IRETQ
    push rax
    push rdx
    mov dx, 0x3F8  ; COM1 port
    mov al, 0xEE   ; Marker byte
    out dx, al
    pop rdx
    pop rax

    ; Return to userspace with IRETQ
    ; This will restore RIP, CS, RFLAGS, RSP, SS from stack
    iretq
    
    ; Should never reach here - add marker for triple fault debugging
    mov dx, 0x3F8
    mov al, 0xDD   ; Dead marker
    out dx, al
    hlt

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

    ; Trace that we're about to jump to Ring 3 with full frame info
    ; Save registers that might be clobbered
    push rdi
    push rsi
    push rdx
    push rcx
    push r8
    push r9
    push r10
    push r11
    
    ; Pass pointer to IRETQ frame
    mov rdi, rsp
    add rdi, 64                ; Skip 8 pushed registers to point to RIP
    call trace_iretq_to_ring3
    
    ; Restore registers
    pop r11
    pop r10
    pop r9
    pop r8
    pop rcx
    pop rdx
    pop rsi
    pop rdi

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

    ; Direct serial output marker - about to execute IRETQ for first userspace entry
    ; Write 0xFF to serial port to indicate we reached IRETQ
    push rax
    push rdx
    mov dx, 0x3F8  ; COM1 port
    mov al, 0xFF   ; First entry marker
    out dx, al
    pop rdx
    pop rax

    ; Jump to userspace
    iretq
    
    ; Should never reach here - add marker for triple fault debugging
    mov dx, 0x3F8
    mov al, 0xCC   ; Crash marker
    out dx, al
    hlt