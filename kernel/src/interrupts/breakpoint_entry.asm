; Breakpoint exception entry with proper swapgs handling
; 
; This handles INT3 breakpoints from both kernel and userspace
; Critical: Must handle swapgs when coming from Ring 3

global breakpoint_entry
extern rust_breakpoint_handler

; CRITICAL: Place exception entry code in dedicated section that stays mapped
; This ensures the code is accessible after CR3 switches to process page tables
section .text.entry
bits 64

; Define constant for saved register count to avoid magic numbers
%define SAVED_REGS_COUNT 15
%define SAVED_REGS_SIZE (SAVED_REGS_COUNT * 8)

breakpoint_entry:
    ; CRITICAL: Disable interrupts BEFORE saving any registers
    ; This prevents race condition where another interrupt fires during register save
    ; Even though breakpoint is an exception/trap, we ensure atomicity by explicitly
    ; disabling interrupts for the entire register save sequence
    cli

    ; Breakpoint exception doesn't push error code
    ; Push dummy error code for uniform stack frame
    push qword 0

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
    ; Frame layout after pushes: [r15...rax][error_code][RIP][CS][RFLAGS][RSP][SS]
    ; CS is at RSP + 15*8 + 8 + 8 (15 saved regs + error code + RIP)
    mov rax, [rsp + SAVED_REGS_SIZE + 16]  ; Get CS
    and rax, 3                              ; Check privilege level (RPL bits)
    cmp rax, 3                              ; Ring 3?
    jne .skip_swapgs_entry                  ; If not from userspace, skip swapgs
    
    ; We came from userspace, swap to kernel GS
    swapgs
    
.skip_swapgs_entry:
    ; Clear direction flag for string operations
    cld
    
    ; Call the Rust breakpoint handler
    ; Pass pointer to saved registers and frame as argument
    mov rdi, rsp
    call rust_breakpoint_handler
    
    ; Raw serial output: Rust handler returned
    mov dx, 0x3F8
    mov al, 'R'         ; 'R' for Return
    out dx, al
    mov al, 'E'         ; 'E' for rEturn
    out dx, al
    mov al, 'T'         ; 'T' for reTurn
    out dx, al
    
    ; Check if we need to swap GS back before returning
    ; Frame layout is same as above
    mov rax, [rsp + SAVED_REGS_SIZE + 16]  ; Get CS again
    and rax, 3                              ; Check privilege level (RPL bits)
    cmp rax, 3                              ; Ring 3?
    jne .skip_swapgs_exit                   ; If not returning to userspace, skip swapgs
    
    ; Returning to userspace, swap back to user GS
    swapgs

.skip_swapgs_exit:
    ; CRITICAL: Disable interrupts before restoring registers
    ; This prevents race condition where another interrupt fires while registers
    ; are being restored, potentially corrupting them
    cli

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
    
    ; Remove dummy error code
    add rsp, 8
    
    ; Raw serial output: About to IRETQ
    mov dx, 0x3F8
    mov al, 'I'         ; 'I' for IRETQ
    out dx, al
    mov al, 'Q'         ; 'Q' for iretQ
    out dx, al
    
    ; Return from interrupt
    iretq