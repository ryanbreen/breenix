section .text
global _start

_start:
    ; rax = 400, rdi = 0xdead_beef
    mov     eax, 400
    mov     edi, 0xdeadbeef
    int     0x80

    ; rax = 401; result returned in rax
    mov     eax, 401
    int     0x80

    mov     rdx, 0xdeadbeef
    cmp     rax, rdx
    jne     fail

success:
    mov     eax, 9          ; SYS_EXIT  
    xor     edi, edi        ; status = 0
    int     0x80

fail:
    mov     eax, 9          ; SYS_EXIT
    mov     edi, 1          ; status = 1
    int     0x80