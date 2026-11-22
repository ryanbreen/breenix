section .text
global set_status
set_status:
    mov r8d, 0x12345678
    mov edi, 42
    ret

section .data
global _start
section .text
_start: call -17    ; Next instruction is 17 bytes after set_status.
    mov eax, 60     ; Breenix x86-64 exit syscall.
    syscall
