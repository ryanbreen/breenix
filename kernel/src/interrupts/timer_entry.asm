; Timer interrupt entry with deferred context switching
; 
; This follows the correct OS design pattern:
; 1. Timer handler does minimal work
; 2. Context switching happens on interrupt return path
; 3. Clear separation of concerns

global timer_interrupt_entry
extern timer_interrupt_handler
extern check_need_resched_and_switch
extern log_timer_frame_from_userspace
extern trace_iretq_to_ring3
extern send_timer_eoi

; CRITICAL: Place interrupt entry code in dedicated section that stays mapped
; This ensures the code is accessible after CR3 switches to process page tables
section .text.entry
bits 64

; Define constant for saved register count to avoid magic numbers
%define SAVED_REGS_COUNT 15
%define SAVED_REGS_SIZE (SAVED_REGS_COUNT * 8)

timer_interrupt_entry:
    ; CRITICAL: Disable interrupts BEFORE saving any registers
    ; This prevents race condition where another interrupt fires during register save
    ; Even though timer interrupt is an interrupt gate (IF cleared by CPU), we ensure
    ; atomicity by explicitly disabling interrupts for the entire register save sequence
    cli

    ; TEMPORARILY REMOVED: Push dummy error code for uniform stack frame (IRQs don't push error codes)
    ; push qword 0

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
    
    ; CRITICAL: Check if we came from userspace and need to swap GS/CR3
    ; Get CS from interrupt frame to check privilege level
    ; Frame layout after pushes: [r15...rax][RIP][CS][RFLAGS][RSP][SS]
    ; CS is at RSP + 15*8 + 8 (15 saved regs + RIP)
    mov rax, [rsp + SAVED_REGS_SIZE + 8]  ; Get CS
    and rax, 3                         ; Check privilege level (RPL bits)
    cmp rax, 3                         ; Ring 3?
    jne .skip_swapgs_entry             ; If not from userspace, skip swapgs

    ; We came from userspace, swap to kernel GS FIRST
    ; We need kernel GS to read kernel_cr3 from per-CPU data
    swapgs

    ; CRITICAL: Save the process CR3 BEFORE switching to kernel CR3
    ; This allows us to restore it on exit if no context switch happens
    ; Save process CR3 to per-CPU data at gs:[80] (SAVED_PROCESS_CR3_OFFSET)
    mov rax, cr3                       ; Read current (process) CR3
    mov qword [gs:80], rax             ; Save to per-CPU saved_process_cr3

    ; NOTE: We intentionally do NOT switch CR3 on timer interrupt entry anymore.
    ; Process page tables have all kernel mappings copied from the master PML4,
    ; so kernel code can run with the process's page table active.
    ; This keeps userspace memory accessible during kernel execution.
    ;
    ; The old CR3-switch code is kept for reference but disabled:
    ; mov rax, qword [gs:72]             ; Read kernel CR3 from per-CPU data
    ; test rax, rax                      ; Check if kernel_cr3 is set
    ; jz .skip_cr3_switch_entry          ; If not set, skip (early boot fallback)
    ; mov cr3, rax                       ; Switch to kernel page table
    ; .skip_cr3_switch_entry:
    
    ; Log full frame details for first few userspace interrupts
    ; Pass frame pointer to logging function
    ; Align stack to 16 bytes before function call (we have 16 pushes = even)
    push rdi
    push rsi
    lea rdi, [rsp + 16 + SAVED_REGS_SIZE]  ; Pass frame pointer (adjust for pushes)
    call log_timer_frame_from_userspace
    pop rsi
    pop rdi
    
.skip_swapgs_entry:
    ; Prepare parameter for timer handler: from_userspace flag
    ; rdi = 1 if from userspace, 0 if from kernel
    xor rdi, rdi                       ; Clear rdi
    mov rax, [rsp + SAVED_REGS_SIZE + 8]  ; Get CS
    and rax, 3                         ; Check privilege level
    cmp rax, 3                         ; Ring 3?
    sete dil                           ; Set dil (low byte of rdi) to 1 if equal (from userspace)
    
    ; Stack is aligned (16 pushes = 128 bytes = 16-byte aligned)
    ; Call the timer handler with from_userspace parameter
    ; This ONLY updates ticks, quantum, and sets need_resched flag
    call timer_interrupt_handler
    
    ; Now check if we need to reschedule
    ; Defer scheduling decision to Rust can_schedule() (userspace or idle kernel)
    mov rax, [rsp + SAVED_REGS_SIZE + 8]  ; Get CS
    and rax, 3                ; Check privilege level (RPL)
    cmp rax, 3                ; Ring 3 (userspace)?
    ; jne .skip_resched         ; removed: always invoke checker
    
    ; This is the CORRECT place for context switching logic (userspace only)
    mov rdi, rsp              ; Pass pointer to saved registers
    lea rsi, [rsp + 15*8]     ; Pass pointer to interrupt frame
    call check_need_resched_and_switch
    
    ; SENTINEL: Output marker to see if we return from check_need_resched_and_switch
    ; If context switch does a non-local return, we'll never see this
    push rax
    push rdx
    mov dx, 0x3F8       ; COM1 port
    mov al, '@'         ; Sentinel marker after call
    out dx, al
    mov al, '@'         ; Double for visibility
    out dx, al
    pop rdx
    pop rax

    ; CRITICAL: Disable interrupts before restoring registers
    ; This prevents race condition where another interrupt fires while registers
    ; are being restored, potentially corrupting them
    cli

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
    
    ; Check if we're returning to ring 3 (userspace)
    ; Frame is now: [RIP][CS][RFLAGS][RSP][SS] at RSP
    mov rcx, [rsp + 8]         ; Get CS from interrupt frame (use RCX instead of RAX)
    and rcx, 3                 ; Check privilege level
    cmp rcx, 3                 ; Ring 3?
    jne .no_userspace_return
    
    ; FIXED: CR3 switching now happens in the scheduler during context switch
    ; This follows Linux/FreeBSD pattern where page tables are switched when
    ; the scheduler selects a new process, not on interrupt return.
    ; The kernel runs on the process's CR3 after context switch.
    
.skip_page_table_switch:
    ; DISABLED: Log iretq - might touch per-CPU data with process page table
    ; push rdi
    ; mov rdi, rsp
    ; add rdi, 8  ; Adjust for the push
    ; extern log_iretq_frame
    ; call log_iretq_frame
    ; pop rdi
    
    ; DEBUG: Dump IRET frame before swapgs (while GS still points to kernel)
    ; The stack should have [RIP][CS][RFLAGS][RSP][SS]
    push rax
    push rbx
    push rcx
    push rdx
    push rdi
    
    ; Call diagnostic function to print frame
    mov rdi, rsp
    add rdi, 40         ; Adjust for the 5 pushes (5*8=40) to point at IRET frame
    extern dump_iret_frame_to_serial
    call dump_iret_frame_to_serial
    
    pop rdi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    
    ; CRITICAL: Swap to userspace GS when returning to Ring 3
    ; We already know we're returning to userspace (checked above)
    ; so we need to ensure GS is set for userspace
    swapgs
    
    ; DEBUG: Output after swapgs to confirm we survived
    mov dx, 0x3F8       ; COM1 port
    mov al, 'Z'         ; After swapgs marker
    out dx, al

    ; GDT verification temporarily disabled for debugging
    
.stack_looks_ok:
    ; No error code to remove
    ; NO EXTRA POPS - registers already restored above!
    
    ; Call trace function to log IRETQ frame with IF bit check
    ; CRITICAL: We need kernel GS for the trace function to work

    ; CRITICAL: Disable interrupts NOW to prevent race condition
    ; A timer interrupt during trace_iretq_to_ring3() could switch CR3
    ; before we finish, causing page faults when kernel code runs on
    ; process page tables. Must be atomic from here to IRETQ.
    cli

    ; Swap back to kernel GS temporarily
    swapgs

    ; Save registers we need
    push rdi
    push rsi
    push rdx
    push rcx
    push r8
    push r9
    push r10
    push r11

    ; Pass pointer to IRETQ frame (RIP is at RSP+64 after our pushes)
    mov rdi, rsp
    add rdi, 64         ; Skip 8 pushed registers to point to RIP
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

    ; Debug: Output marker after register restore
    push rax
    push rdx
    mov dx, 0x3F8
    mov al, '1'
    out dx, al
    pop rdx
    pop rax

    ; EOI is now sent just before IRETQ to minimize the window for PIC to queue
    ; another interrupt. See .after_cr3_check and .no_userspace_return

    ; Swap back to user GS before IRETQ
    swapgs

    ; Debug: Output marker after swapgs
    push rax
    push rdx
    mov dx, 0x3F8
    mov al, '2'
    out dx, al
    pop rdx
    pop rax

    ; CRITICAL DEBUG: Output marker to prove we reach IRETQ
    ; If we see this marker, we made it to iretq
    push rax
    push rdx
    mov dx, 0x3F8       ; COM1 port
    mov al, 'Q'         ; 'Q' for iretQ
    out dx, al
    pop rdx
    pop rax

    ; CRITICAL: Check if we need to switch CR3 before IRETQ
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
    jz .no_cr3_switch_back_to_user

    ; Interrupts already disabled (CLI before)
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
    mov al, 'C'
    out dx, al
    mov al, 'R'
    out dx, al
    mov al, '3'
    out dx, al
    pop rax

    ; NOW safe to switch CR3 to process page table
    ; Kernel per-CPU data already cleared while kernel PT was active
    mov cr3, rax

    ; Swap back to user GS for IRETQ
    swapgs

    jmp .after_cr3_check

.no_cr3_switch_back_to_user:
    ; No context switch, but we still need to restore the ORIGINAL process CR3!
    ; We saved it on entry at gs:[80] (SAVED_PROCESS_CR3_OFFSET)
    mov rax, qword [gs:80]             ; Read saved process CR3
    test rax, rax                      ; Check if it was saved (non-zero)
    jz .no_saved_cr3                   ; If 0, skip (shouldn't happen from userspace)

    ; Debug: Output marker for saved CR3 restore
    push rdx
    mov dx, 0x3F8
    push rax
    mov al, '!'                        ; '!' for saved CR3 restore
    out dx, al
    mov al, 'C'
    out dx, al
    mov al, 'R'
    out dx, al
    mov al, '3'
    out dx, al
    pop rax
    pop rdx

    ; Switch back to original process CR3
    mov cr3, rax

.no_saved_cr3:
    ; Swap back to user GS for IRETQ
    swapgs

.after_cr3_check:
    pop rdx
    pop rax

    ; CRITICAL FIX: Send EOI just before IRETQ to minimize window for spurious interrupts
    ; We're on user GS here, but send_timer_eoi needs kernel GS to access PICS lock
    ; Sequence: swapgs to kernel, call EOI, swapgs to user, iretq
    ;
    ; CRITICAL FIX #2: Save/restore ALL caller-saved registers around the call!
    ; send_timer_eoi is a Rust function following System V AMD64 ABI.
    ; Per the ABI, caller-saved registers (RAX, RCX, RDX, RSI, RDI, R8-R11) can be clobbered.
    ; Without saving them, userspace returns with corrupted registers!
    ; This was the root cause of the RDI corruption bug.
    push rax
    push rcx
    push rdx
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11

    swapgs                  ; Switch to kernel GS
    call send_timer_eoi     ; Send EOI (requires kernel GS for PICS access)
    swapgs                  ; Switch back to user GS for iretq

    ; Restore all caller-saved registers
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
    ; IRETQ will re-enable interrupts from the saved RFLAGS
    iretq
    
.no_userspace_return:
    ; No error code to remove
    ; NO EXTRA POPS - registers already restored above!

    ; CRITICAL FIX: Send EOI for kernel return path too
    ; We're still on kernel GS (we never swapped since we came from kernel mode)
    ;
    ; CRITICAL FIX #2: Save/restore caller-saved registers around the call!
    ; Same fix as userspace path - send_timer_eoi can clobber registers.
    push rax
    push rcx
    push rdx
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11

    call send_timer_eoi

    ; Restore all caller-saved registers
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rax

    ; Return from interrupt to kernel
    iretq