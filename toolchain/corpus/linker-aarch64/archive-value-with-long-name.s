/* AAPCS64: integer return in w0; AAELF64: symbol and section directives. */
.text
.globl archive_value
.type archive_value,%function
archive_value:
    mov w0, #23
    ret
.size archive_value, .-archive_value

.section .rodata,"a",%progbits
.p2align 2
.globl archive_cell
.type archive_cell,%object
archive_cell:
    .word 19
.size archive_cell, .-archive_cell
.section .note.GNU-stack,"",%progbits
