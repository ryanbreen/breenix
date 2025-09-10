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

; CRITICAL: Place interrupt entry code in dedicated section that stays mapped
; This ensures the code is accessible after CR3 switches to process page tables
section .text.entry
bits 64

; Define constant for saved register count to avoid magic numbers
%define SAVED_REGS_COUNT 15
%define SAVED_REGS_SIZE (SAVED_REGS_COUNT * 8)

timer_interrupt_entry:
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
    
    ; CRITICAL: Check if we came from userspace and need to swap GS
    ; Get CS from interrupt frame to check privilege level
    ; Frame layout after pushes: [r15...rax][RIP][CS][RFLAGS][RSP][SS]
    ; CS is at RSP + 15*8 + 8 (15 saved regs + RIP)
    mov rax, [rsp + SAVED_REGS_SIZE + 8]  ; Get CS
    and rax, 3                         ; Check privilege level (RPL bits)
    cmp rax, 3                         ; Ring 3?
    jne .skip_swapgs_entry             ; If not from userspace, skip swapgs
    
    ; We came from userspace, swap to kernel GS
    swapgs
    
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
    
    ; CRITICAL DIAGNOSTIC: Verify GDT descriptors before IRETQ
    ; Test if CS and SS selectors are valid for Ring 3
    push rax
    push rdx
    push rcx
    
    ; Test CS selector (0x33) with VERR
    mov ax, 0x33
    verr ax
    jz .cs_verr_ok
    ; CS not readable from Ring 3 - print error
    mov dx, 0x3F8
    mov al, '!'
    out dx, al
    mov al, 'C'
    out dx, al
    mov al, 'S'
    out dx, al
.cs_verr_ok:
    
    ; Test SS selector (0x2b) with VERW
    mov ax, 0x2b
    verw ax
    jz .ss_verw_ok
    ; SS not writable from Ring 3 - print error
    mov dx, 0x3F8
    mov al, '!'
    out dx, al
    mov al, 'S'
    out dx, al
    mov al, 'S'
    out dx, al
.ss_verw_ok:
    
    ; Get access rights with LAR for CS
    mov ax, 0x33
    lar rdx, ax
    jnz .cs_lar_failed
    ; Success - RDX has access rights
    jmp .cs_lar_ok
.cs_lar_failed:
    mov dx, 0x3F8
    mov al, '?'
    out dx, al
    mov al, 'C'
    out dx, al
.cs_lar_ok:
    
    ; Get access rights with LAR for SS
    mov ax, 0x2b
    lar rcx, ax
    jnz .ss_lar_failed
    ; Success - RCX has access rights
    jmp .ss_lar_ok
.ss_lar_failed:
    mov dx, 0x3F8
    mov al, '?'
    out dx, al
    mov al, 'S'
    out dx, al
.ss_lar_ok:
    
    ; REMOVED: Logging CR3/GDTR here - already done before CR3 switch
    ; After swapgs, we can't safely call kernel functions that might
    ; access per-CPU data or other kernel structures
    
    pop rcx
    pop rdx
    pop rax
    
.stack_looks_ok:
    ; No error code to remove
    ; NO EXTRA POPS - registers already restored above!
    
    ; Call trace function to log IRETQ frame with IF bit check
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
    
    ; CRITICAL DEBUG: Output marker to prove we reach IRETQ
    ; If we see this marker, we made it to iretq
    push rax
    push rdx
    mov dx, 0x3F8       ; COM1 port
    mov al, 'Q'         ; 'Q' for iretQ
    out dx, al
    pop rdx
    pop rax
    
    ; Return from interrupt to userspace
    iretq
    
.no_userspace_return:
    ; No error code to remove
    ; NO EXTRA POPS - registers already restored above!
    
    ; Return from interrupt to kernel
    iretq