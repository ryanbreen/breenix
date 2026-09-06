.global dump_lockup_state
dump_lockup_state:
    b write_the_tail
.global write_the_tail
write_the_tail:
    bl __rust_realloc
    ret
.global __rust_realloc
__rust_realloc:
    ret
