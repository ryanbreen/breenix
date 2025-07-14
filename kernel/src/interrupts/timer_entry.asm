; Timer interrupt entry with deferred context switching
; 
; This follows the correct OS design pattern:
; 1. Timer handler does minimal work
; 2. Context switching happens on interrupt return path
; 3. Clear separation of concerns

global timer_interrupt_entry
extern timer_interrupt_handler
extern check_need_resched_and_switch
extern get_thread_cr3

section .data
global _dbg_cr3
global _dbg_rip
global _debug_seen_cr3
global _debug_current_rsp
_dbg_cr3: dq 0
_dbg_rip: dq 0
_pending_cr3: dq 0
_debug_seen_cr3: dq 0
_debug_current_rsp: dq 0

section .text
bits 64

timer_interrupt_entry:
    ; --- Entry: RSP points at hardware frame (RIP,CS,RFLAGS,RSP,SS)
    
    ; IMMEDIATE: Output breadcrumb as first instruction - if this doesn't appear, interrupt not delivered
    mov al, 0xDE          ; ENTRY breadcrumb (different from 0xD0)
    out 0x80, al
    
    ; CRITICAL: Output to serial IMMEDIATELY without using ANY registers
    push rax
    push rdx
    mov dx, 0x3F8           ; COM1 data port
    mov al, '!'             ; Immediate marker
    out dx, al
    pop rdx
    pop rax
    
    ; DEBUG: Output different markers for Ring 0 vs Ring 3 entry
    push rax
    mov eax, [rsp + 12]     ; CS (account for pushed RAX)
    and eax, 3
    cmp eax, 3
    jne .from_ring0
    
    ; From Ring 3
    push rdx
    mov dx, 0x3F8
    mov al, '3'
    out dx, al
    pop rdx
    jmp .continue_entry
    
.from_ring0:
    ; From Ring 0
    push rdx
    mov dx, 0x3F8
    mov al, '0'
    out dx, al
    pop rdx
    
.continue_entry:
    pop rax
    
    mov eax, 0xB0          ; first breadcrumb – proves we got here
    out 0x80, al

    ; Test CPL of interrupted code *before touching the stack*
    ; CS is at [rsp+8]
    mov eax, [rsp + 8]
    and eax, 3
    cmp eax, 3
    jne .after_swapgs      ; came from Ring 0  → no swapgs needed

    swapgs                 ; came from Ring 3

.after_swapgs:
    mov eax, 0xB1          ; breadcrumb after the swapgs
    out 0x80, al
    
    ; DEBUG: Write a marker to serial port to prove we're in timer handler
    push rax
    push rdx
    mov dx, 0x3F8           ; COM1 data port
    mov al, 'T'             ; Timer marker
    out dx, al
    pop rdx
    pop rax
    
    ; Breadcrumb 0xA0 - entering interrupt handler
    push rax
    mov al, 0xA0
    out 0x80, al
    pop rax
    
    ; DEBUG: Output 0xBF right before register saves
    push rax
    mov al, 0xBF
    out 0x80, al
    pop rax
    
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
    
    ; DEBUG: Output 0xC0 after register save
    push rax
    mov al, 0xC0
    out 0x80, al
    pop rax
    
    ; Call the timer handler
    ; This ONLY updates ticks, quantum, and sets need_resched flag
    call timer_interrupt_handler
    
    ; DEBUG: Output 0xC1 after timer handler
    push rax
    mov al, 0xC1
    out 0x80, al
    pop rax
    
    ; Breadcrumb 0xA1 - finished timer handler, about to check reschedule
    push rax
    mov al, 0xA1
    out 0x80, al
    pop rax
    
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
    push rdx                    ; Save rdx - will be used for CR3 value
    
    ; Check if we're returning to ring 3 (userspace)
    mov rax, [rsp + 24 + 8]    ; Get CS from interrupt frame (3 pushes + RIP)
    and rax, 3                 ; Check privilege level
    cmp rax, 3                 ; Ring 3?
    jne .no_userspace_return
    
    ; We're returning to userspace, check if we need to switch page tables
    call get_thread_cr3
    test rax, rax              ; Is there a page table to switch to?
    jz .skip_page_table_switch
    
    ; DEBUG: Mark that we're about to switch CR3
    push rax
    mov [rel _dbg_cr3], rax    ; Store the new CR3 value
    
    ; IMPORTANT: Keep CR3 value in RDX register instead of memory
    ; This avoids the race condition with _pending_cr3 being cleared
    mov rdx, rax               ; Save CR3 in RDX for later
    mov [rel _debug_seen_cr3], rdx     ; Debug: immediately save what we set
    pop rax
    ; CRITICAL: Ensure TLB is fully flushed after page table switch
    ; TEMPORARILY DISABLED - may be causing hang
    ; mov cr3 should be sufficient to flush TLB on modern CPUs
    
    ; push rax                     ; Save rax (contains page table frame)
    ; mov rax, cr4
    ; mov rcx, rax
    ; and rcx, 0xFFFFFFFFFFFFFF7F  ; Clear PGE bit (bit 7)
    ; mov cr4, rcx                  ; Disable global pages (flushes TLB)
    ; mov cr4, rax                  ; Re-enable global pages
    ; pop rax                      ; Restore rax
    ; mfence
    
.skip_page_table_switch:
    ; No CR3 switch needed
    
    ; CRITICAL: Perform CR3 switch right before iretq (RDX has the value)
    test rdx, rdx
    jz .no_cr3_switch
    
    ; DEBUG: Log current RSP before CR3 switch
    push rax
    push rdx
    push rsi
    push rdi
    
    ; Log RSP value via serial
    mov rsi, rsp
    add rsi, 32              ; Adjust for our 4 pushes
    mov [rel _debug_current_rsp], rsi  ; Store for analysis
    mov rdx, 0x3F8
    mov al, 'S'
    out dx, al
    mov al, '='
    out dx, al
    
    ; Output RSP high nibbles (abbreviated for debugging)
    mov rdi, rsi
    shr rdi, 32              ; Get high 32 bits
    mov al, dil              ; Low byte of high dword
    out dx, al
    
    pop rdi
    pop rsi
    pop rdx
    pop rax
    
    ; CRITICAL DEBUG: Serial marker 'A' - Frame ready, still on old CR3
    push rax
    push rdx
    mov dx, 0x3F8
    mov al, 'A'               ; Before CR3 switch
    out dx, al
    pop rdx
    pop rax
    
    ; Breadcrumb 0xA2 - CR3 switch  
    push rax
    mov al, 0xA2
    out 0x80, al
    pop rax
    
    mov cr3, rdx               ; Switch page table using RDX
    
    ; CRITICAL DEBUG: Serial marker 'B' - CR3 switched, testing if stack still accessible
    push rax
    push rdx
    mov dx, 0x3F8
    mov al, 'B'               ; After CR3 switch
    out dx, al
    pop rdx
    pop rax
    
    ; Breadcrumb 0xA3 - Successfully past CR3 switch!
    push rax
    mov al, 0xA3
    out 0x80, al
    pop rax
    
.no_cr3_switch:
    
    ; Now restore the saved registers (skipping RDX which we used for CR3)
    add rsp, 8                 ; Skip saved rdx value 
    pop rcx                    ; Restore rcx
    pop rax                    ; Restore rax
    
    ; DEBUG: Dump IRET stack before returning to userspace
    ; TEMPORARILY DISABLED to debug Arc::inner fault
    ; push rax
    ; push rcx
    ; push rdx
    ; push rsi
    ; push rdi
    
    ; ; Print IRET stack dump message
    ; extern serial_print_iret_stack
    ; mov rdi, rsp
    ; add rdi, 40              ; Skip our 5 pushes to point to IRET frame
    ; call serial_print_iret_stack
    
    ; pop rdi
    ; pop rsi
    ; pop rdx
    ; pop rcx
    ; pop rax
    
    ; Log CR3 and RIP before iretq
    push rax
    push rcx
    mov rax, cr3
    mov [rel _dbg_cr3], rax
    mov rcx, [rsp + 16]          ; Get RIP from interrupt frame (2 pushes + RIP)
    mov [rel _dbg_rip], rcx
    pop rcx
    pop rax
    
    ; Breadcrumb 0xA3 - just before iretq
    push rax
    mov al, 0xA3
    out 0x80, al
    pop rax
    
    ; 3-C: Verify IF flag before iretq
    push rax
    pushfq                     ; Push RFLAGS onto stack
    pop rax                    ; Pop RFLAGS into RAX
    out 0x80, al              ; Output low byte (bit 9 = IF should be 0x200 pattern)
    pop rax                    ; Restore RAX
    
    ; TF flag is already set in Rust code - no need to set here again
    
    ; DEBUG: Output marker right before IRETQ
    push rax
    mov al, 0xBB
    out 0x80, al
    pop rax
    
    ; ----- timer ISR epilogue -----
    ; Determine privilege level we are returning to
    mov     eax, [rsp + 8]      ; CS in the IRET frame
    and     eax, 3
    cmp     eax, 3
    jne     .skip_exit_swapgs   ; returning to Ring 0 → leave GS as kernel

    swapgs                       ; returning to Ring 3 → restore user GS base
    
    ; Emit 0xB2 just after the conditional swapgs
    push rax
    mov al, 0xB2
    out 0x80, al
    pop rax

.skip_exit_swapgs:
    ; Emit 0xB3 immediately before iretq
    push rax
    mov al, 0xB3
    out 0x80, al
    pop rax
    
    ; CRITICAL DEBUG: Breadcrumb 0xB9 - IRET frame is complete and ready
    ; This proves we successfully built/modified the frame and are about to IRETQ
    push rax
    mov al, 0xB9          ; "Frame done" - frame is ready for IRETQ
    out 0x80, al
    pop rax
    
    ; SERIAL DEBUG: Output 'F' to show frame is ready
    push rax
    push rdx
    mov dx, 0x3F8
    mov al, 'F'           ; Frame ready marker
    out dx, al
    pop rdx
    pop rax
    
    ; TRIPLE FAULT DEBUG: Add breadcrumb right before IRETQ
    push rax
    mov al, 0xBE          ; "Before IRETQ" - should be the last thing we see
    out 0x80, al
    pop rax
    
    ; SERIAL DEBUG: Output 'I' right before IRETQ
    push rax
    push rdx
    mov dx, 0x3F8
    mov al, 'I'           ; IRETQ marker
    out dx, al
    pop rdx
    pop rax
    
    ; DEBUG: Dump key IRET frame values via serial before IRETQ
    push rax
    push rdx
    push rcx
    
    ; Output CS selector value
    mov cx, [rsp + 24 + 8]    ; CS is at offset 8 in IRET frame (3 pushes + RIP)
    mov dx, 0x3F8
    mov al, '='
    out dx, al
    mov al, ch               ; High byte of CS
    out dx, al
    mov al, cl               ; Low byte of CS
    out dx, al
    
    ; Output SS selector value
    mov cx, [rsp + 24 + 32]   ; SS is at offset 32 in IRET frame
    mov al, '/'
    out dx, al
    mov al, ch               ; High byte of SS
    out dx, al
    mov al, cl               ; Low byte of SS
    out dx, al
    
    pop rcx
    pop rdx
    pop rax
    
    ; Return to interrupt to userspace
    iretq
    
    ; TRIPLE FAULT DEBUG: Should never reach here
    push rax
    mov al, 0xAF          ; "After IRETQ" - if we see this, IRETQ worked
    out 0x80, al
    pop rax
    
    ; DEBUG: This should never execute - if we see 0xA4, iretq failed
    push rax
    mov al, 0xA4
    out 0x80, al
    pop rax
    
.no_userspace_return:
    pop rdx                    ; Restore rdx
    pop rcx                    ; Restore rcx
    pop rax                    ; Restore rax
    
    ; Return from interrupt to kernel
    iretq