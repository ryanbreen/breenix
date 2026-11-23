// AAPCS64: BL calls a global function; libbreenix uses x8=93 for exit.
.section .text
.global _start
_start:
    bl answer
    mov x8, #93
    svc #0
.section .text.answer
.global answer
answer:
    mov x0, #42
    mov x1, #1
loop:
    cbz x1, loop
    ret
