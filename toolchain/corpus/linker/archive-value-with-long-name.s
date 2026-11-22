/* AMD64 psABI §3.2.3: integer return value. */
.text
.globl archive_value
.type archive_value,@function
archive_value:
    mov $23, %eax
    ret
.size archive_value, .-archive_value

.section .rodata,"a",@progbits
.p2align 2
.globl archive_cell
.type archive_cell,@object
archive_cell:
    .long 19
.size archive_cell, .-archive_cell
.section .note.GNU-stack,"",@progbits
