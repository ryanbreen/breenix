/* AAPCS64: preserve the link register and stack alignment; AAELF64: GOT relocations. */
.text
.globl archive_entry
.type archive_entry,%function
archive_entry:
    stp x29, x30, [sp, #-16]!
    bl archive_value
    adrp x1, :got:archive_cell
    ldr x1, [x1, :got_lo12:archive_cell]
    ldr w1, [x1]
    add w0, w0, w1
    ldp x29, x30, [sp], #16
    ret
.size archive_entry, .-archive_entry
.section .note.GNU-stack,"",%progbits
