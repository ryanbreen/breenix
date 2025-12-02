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
;   - Interrupts should be disabled by interrupt gate, but we ensure it explicitly
;   - We're in Ring 0
syscall_entry:
    ; CRITICAL: Disable interrupts BEFORE saving any registers
    ; This prevents race condition where timer interrupt fires during register save
    ; at 1000 Hz, causing register corruption (RDI corruption bug)
    ; Even though INT 0x80 is an interrupt gate (IF cleared by CPU), we ensure
    ; atomicity by explicitly disabling interrupts for the entire register save sequence
    cli

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

    ; Always switch to kernel GS FIRST for INT 0x80 entry
    ; We need kernel GS to read kernel_cr3 from per-CPU data
    ; INT 0x80 is only used from userspace, so we always need swapgs
    swapgs

    ; CRITICAL: Save the process CR3 BEFORE switching to kernel CR3
    ; This allows us to restore it on exit if no context switch happens
    ; Save process CR3 to per-CPU data at gs:[80] (SAVED_PROCESS_CR3_OFFSET)
    mov rax, cr3                       ; Read current (process) CR3
    mov qword [gs:80], rax             ; Save to per-CPU saved_process_cr3

    ; NOTE: We intentionally do NOT switch CR3 on syscall entry anymore.
    ; Process page tables have all kernel mappings copied from the master PML4,
    ; so kernel code can run with the process's page table active.
    ; This allows copy_from_user/copy_to_user to access userspace memory directly.
    ;
    ; The old CR3-switch code is kept for reference but disabled:
    ; mov rax, qword [gs:72]             ; Read kernel CR3 from per-CPU data
    ; test rax, rax                      ; Check if kernel_cr3 is set
    ; jz .skip_cr3_switch                ; If not set, skip (early boot fallback)
    ; mov cr3, rax                       ; Switch to kernel page table
    ; .skip_cr3_switch:

    ; Call the Rust syscall handler
    ; Pass pointer to saved registers as argument
    mov rdi, rsp
    call rust_syscall_handler

    ; CRITICAL FIX: Update RAX in SavedRegisters struct on stack
    ; rust_syscall_handler returns the syscall result in RAX, but the SavedRegisters
    ; struct on the stack still has the OLD RAX value (syscall number from entry).
    ; If check_need_resched_and_switch causes a context switch, it will save the
    ; wrong RAX value. We must update the stack copy NOW before any potential switch.
    ; RAX was pushed first (line 33), so it's at [rsp + 0]
    mov [rsp], rax

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

    ; CRITICAL: Disable interrupts before restoring registers
    ; This prevents race condition where timer interrupt fires while registers
    ; are being restored, potentially corrupting them
    cli

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

    ; CRITICAL: Disable interrupts NOW to prevent race condition
    ; A timer interrupt during trace_iretq_to_ring3() could switch CR3
    ; before we finish, causing page faults when kernel code runs on
    ; process page tables. Must be atomic from here to IRETQ.
    cli

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

    ; CRITICAL: Check if we need to switch CR3 before IRETQ (syscall return)
    ; The context switcher stores target CR3 in GS:64 (NEXT_CR3_OFFSET)
    ; If non-zero, switch to it and clear the flag
    push rax
    push rdx

    ; CRITICAL: Swap back to kernel GS to read next_cr3
    ; We're currently in user GS mode, but next_cr3 is in kernel GS
    swapgs

    ; Read next_cr3 from per-CPU data (GS:64)
    mov rax, qword [gs:64]

    ; Check if CR3 switch is needed (non-zero)
    test rax, rax
    jz .no_cr3_switch_syscall_back_to_user

    ; Interrupts already disabled (CLI before trace function)
    ; Safe to switch CR3 now

    ; CRITICAL FIX: Clear next_cr3 BEFORE switching CR3!
    ; We must do this while kernel page tables are still active,
    ; because after CR3 switch the process page tables may not
    ; have the kernel per-CPU region mapped. Accessing [gs:64]
    ; after CR3 switch would cause a page fault -> triple fault.
    push rdx
    xor rdx, rdx
    mov qword [gs:64], rdx
    pop rdx

    ; Debug: Output marker for CR3 switch
    mov dx, 0x3F8
    push rax
    mov al, '$'
    out dx, al
    mov al, 'S'
    out dx, al
    mov al, 'Y'
    out dx, al
    mov al, 'S'
    out dx, al
    pop rax

    ; NOW safe to switch CR3 to process page table
    ; Kernel per-CPU data already cleared while kernel PT was active
    mov cr3, rax

    ; Swap back to user GS for IRETQ
    swapgs

    jmp .after_cr3_check_syscall

.no_cr3_switch_syscall_back_to_user:
    ; No context switch, but we still need to restore the ORIGINAL process CR3!
    ; We saved it on entry at gs:[80] (SAVED_PROCESS_CR3_OFFSET)
    mov rax, qword [gs:80]             ; Read saved process CR3
    test rax, rax                      ; Check if it was saved (non-zero)
    jz .no_saved_cr3_syscall           ; If 0, skip (shouldn't happen from userspace)

    ; Debug: Output marker for saved CR3 restore
    push rdx
    mov dx, 0x3F8
    push rax
    mov al, '!'                        ; '!' for saved CR3 restore
    out dx, al
    mov al, 'S'
    out dx, al
    mov al, 'Y'
    out dx, al
    mov al, 'S'
    out dx, al
    pop rax
    pop rdx

    ; Switch back to original process CR3
    mov cr3, rax

.no_saved_cr3_syscall:
    ; Swap back to user GS for IRETQ
    swapgs

.after_cr3_check_syscall:
    pop rdx
    pop rax

    ; Return to userspace with IRETQ
    ; This will restore RIP, CS, RFLAGS, RSP, SS from stack
    ; IRETQ will re-enable interrupts from the saved RFLAGS
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

    ; CRITICAL: Check if we need to switch CR3 before IRETQ (first userspace entry)
    ; The context switcher stores target CR3 in GS:64 (NEXT_CR3_OFFSET)
    ; If non-zero, switch to it and clear the flag
    push rax
    push rdx

    ; CRITICAL: Swap back to kernel GS to read next_cr3
    ; We're currently in user GS mode, but next_cr3 is in kernel GS
    swapgs

    ; Read next_cr3 from per-CPU data (GS:64)
    mov rax, qword [gs:64]

    ; Check if CR3 switch is needed (non-zero)
    test rax, rax
    jz .no_cr3_switch_first_entry_back_to_user

    ; Interrupts already disabled (CLI at function start line 260)
    ; Safe to switch CR3 now

    ; CRITICAL FIX: Clear next_cr3 BEFORE switching CR3!
    ; We must do this while kernel page tables are still active,
    ; because after CR3 switch the process page tables may not
    ; have the kernel per-CPU region mapped. Accessing [gs:64]
    ; after CR3 switch would cause a page fault -> triple fault.
    push rdx
    xor rdx, rdx
    mov qword [gs:64], rdx
    pop rdx

    ; Debug: Output marker for CR3 switch
    mov dx, 0x3F8
    push rax
    mov al, '$'
    out dx, al
    mov al, 'F'
    out dx, al
    mov al, 'I'
    out dx, al
    mov al, 'R'
    out dx, al
    mov al, 'S'
    out dx, al
    mov al, 'T'
    out dx, al
    pop rax

    ; NOW safe to switch CR3 to process page table
    ; Kernel per-CPU data already cleared while kernel PT was active
    mov cr3, rax

    ; Swap back to user GS for IRETQ
    swapgs

    jmp .after_cr3_check_first_entry

.no_cr3_switch_first_entry_back_to_user:
    ; No context switch, but we still need to restore the ORIGINAL process CR3!
    ; We saved it on entry at gs:[80] (SAVED_PROCESS_CR3_OFFSET)
    mov rax, qword [gs:80]             ; Read saved process CR3
    test rax, rax                      ; Check if it was saved (non-zero)
    jz .no_saved_cr3_first_entry       ; If 0, skip (shouldn't happen from userspace)

    ; Debug: Output marker for saved CR3 restore
    push rdx
    mov dx, 0x3F8
    push rax
    mov al, '!'                        ; '!' for saved CR3 restore
    out dx, al
    mov al, 'F'
    out dx, al
    mov al, 'I'
    out dx, al
    mov al, 'R'
    out dx, al
    mov al, 'S'
    out dx, al
    mov al, 'T'
    out dx, al
    pop rax
    pop rdx

    ; Switch back to original process CR3
    mov cr3, rax

.no_saved_cr3_first_entry:
    ; Swap back to user GS for IRETQ
    swapgs

.after_cr3_check_first_entry:
    pop rdx
    pop rax

    ; Jump to userspace
    ; IRETQ will re-enable interrupts from the saved RFLAGS
    iretq
    
    ; Should never reach here - add marker for triple fault debugging
    mov dx, 0x3F8
    mov al, 0xCC   ; Crash marker
    out dx, al
    hlt