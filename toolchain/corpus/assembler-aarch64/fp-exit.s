// Arm ARM DDI 0487 M.b, binary64 load and conversion; libbreenix exit syscall.
.section .text
.global _start
_start:
    adrp x1, result
    ldr d0, [x1, :lo12:result]
    fcvtzs x0, d0
    mov x8, #93
    svc #0
.section .rodata
.align 8
result:
    .quad 0x4045000000000000
