; AMD64 psABI §§3.4.1, A.2.1. This executor path uses the same x86-64
; write and exit syscall numbers as Breenix. No guest startup or TLS.
section .text
global _start
extern main
_start:
    xor ebp, ebp
    call main
    mov edi, eax
    mov eax, 60
    syscall

global linux_test_write
linux_test_write:
    mov eax, 1
    syscall
    ret
