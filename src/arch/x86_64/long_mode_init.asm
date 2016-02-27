global long_mode_start

section .text
bits 64

global _irq_default_handler
extern default_irq_handler

; IRQ handlers must be implemented in raw ASM to be able
; to use the iretq instruction to return from the handler
; Rust compiler uses "retq" instead, which is not suitable

_irq_default_handler:
    call default_irq_handler
    mov al, 0x20
    out 0x20, al
    iretq

%macro  ASM_IRQ_HANDLER 1

global _asm_handler_%1

    _asm_irq_handler_%1:
        push word %1
        call default_irq_handler
        add rsp, 2
        mov al, 0x20
        out 0x20, al
        iretq

%endmacro

%assign i 0
%rep    256
    ASM_IRQ_HANDLER i
%assign i i+1
%endrep

%unmacro ASM_IRQ_HANDLER 1
%macro   ASM_IRQ_HANDLER 1
    dq _asm_irq_handler_%1
%endmacro

global _asm_irq_handler_array

_asm_irq_handler_array:
%assign i 0
%rep    256
    ASM_IRQ_HANDLER i
%assign i i+1
%endrep

long_mode_start:
    ; call the rust main
    extern rust_main
    call setup_SSE
    call rust_main

    .os_returned:
        ; rust main returned, print `OS returned!`
        mov rax, 0x4f724f204f534f4f
        mov [0xb8000], rax
        mov rax, 0x4f724f754f744f65
        mov [0xb8008], rax
        mov rax, 0x4f214f644f654f6e
        mov [0xb8010], rax
        hlt

; Prints `ERROR: ` and the given error code to screen and hangs.
; parameter: error code (in ascii) in al
error:
    mov rbx, 0x4f4f4f524f524f45
    mov [0xb8000], rbx
    mov rbx, 0x4f204f204f3a4f52
    mov [0xb8008], rbx
    mov byte [0xb800e], al
    hlt
    jmp error

; Check for SSE and enable it. If it's not supported throw error "a".
setup_SSE:
    ; check for SSE
    mov rax, 0x1
    cpuid
    test edx, 1<<25
    jz .no_SSE

    ; enable SSE
    mov rax, cr0
    and ax, 0xFFFB      ; clear coprocessor emulation CR0.EM
    or ax, 0x2          ; set coprocessor monitoring  CR0.MP
    mov cr0, rax
    mov rax, cr4
    or ax, 3 << 9       ; set CR4.OSFXSR and CR4.OSXMMEXCPT at the same time
    mov cr4, rax

    ret
.no_SSE:
    mov al, "a"
    jmp error