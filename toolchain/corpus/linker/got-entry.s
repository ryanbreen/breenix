/* AMD64 psABI §§3.2.2, 4.4: call alignment and GOTPCREL. */
.text
.globl archive_entry
.type archive_entry,@function
archive_entry:
    sub $8, %rsp
    call archive_value
    add $8, %rsp
    mov archive_cell@GOTPCREL(%rip), %rdx
    add (%rdx), %eax
    ret
.size archive_entry, .-archive_entry
.section .note.GNU-stack,"",@progbits
