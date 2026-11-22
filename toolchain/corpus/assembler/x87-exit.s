; Intel SDM Vol. 2 FILD, FSUBP, FISTP, LOCK ADD, REP MOVSB and MFENCE.
section .text
global _start
_start:
    fild dword [rip + initial]
    fld1
    fsubp st(1), st(0)
    fistp dword [rip + counter]
    lock add dword [rip + counter], 1
    lea rsi, [rip + counter]
    lea rdi, [rip + copied]
    mov ecx, 4
    cld
    rep movsb
    mfence
    mov edi, [rip + copied]
    mov eax, 60
    syscall

section .rodata
align 4
initial:
    dd 73
section .data
align 4
counter:
    dd 0
section .bss
align 4
copied:
    .zero 4
