.section .text
.global _start
_start:
    movz x0, #42
    movz x8, #93
    svc #0
.Lback:
    nop
    b .Lforward
    bl .Lback
    b.ne .Lforward
    cbz w3, .Lback
    cbnz x5, .Lforward
    tbz w7, #31, .Lback
    tbnz x9, #63, .Lforward
    br x16
    blr x17
    ret x19
.Lforward:
    ret
    movz w11, #0x1234, lsl #16
    movz xzr, #0xffff, lsl #48
