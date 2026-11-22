; AMD64 psABI §A.2 and Breenix's x86-64 EXIT syscall (libbreenix).
section .text
global _start
_start:
    movsd xmm8, [rip + result]
    cvttsd2si edi, xmm8
    mov eax, 60
    syscall

section .rodata
align 8
result:
    dq 0x4045000000000000 ; IEEE binary64 for 42
