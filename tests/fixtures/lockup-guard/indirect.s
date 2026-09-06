.section .data
.align 3
alloc_slot:
    .quad __rust_alloc
.text
.global dump_lockup_state
dump_lockup_state:
    adrp x8, alloc_slot
    add x8, x8, :lo12:alloc_slot
    ldr x8, [x8]
    blr x8
    ret
.global __rust_alloc
__rust_alloc:
    ret
