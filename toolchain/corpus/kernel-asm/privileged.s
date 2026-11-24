; Intel SDM 325462-082US, Vol. 2 instruction reference and addressing tables.
; The 64-bit rows match privileged_64_encodings in bas_kernel.rs.
section .text
bits 64
    swapgs
    iretq
    o64 sysret
    mov rax, cr0
    mov rdx, cr3
    mov r8, cr4
    mov cr0, rax
    mov cr3, rdx
    mov cr4, r8
    wrmsr
    rdmsr
    out dx, al
    hlt
    cli
    cld
    sete al
    sete dil
    sete r8b
    mov [gs:40], rsp
    mov rax, qword [gs:80]

; The legacy address/operand overrides match ap_trampoline_assembles.
bits 16
    mov ax, [ebx]
    mov eax, [bp + si - 2]
    o32 mov eax, ebx
bits 32
    mov ax, [bx + di]
    mov eax, [esp]
