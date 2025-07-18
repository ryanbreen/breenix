.global _start
_start:
    /* rax = 400, rdi = 0xdead_beef */
    mov     $400, %eax
    mov     $0xdeadbeef, %rdi
    int     $0x80

    /* rax = 401; result returned in rax */
    mov     $401, %eax
    int     $0x80

    cmp     $0xdeadbeef, %rax
    jne     fail

success:
    mov     $9, %eax        # SYS_EXIT
    xor     %rdi, %rdi      # status = 0
    int     $0x80

fail:
    mov     $9, %eax
    mov     $1, %rdi        # status = 1
    int     $0x80