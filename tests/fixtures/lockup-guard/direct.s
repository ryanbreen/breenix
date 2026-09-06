.global dump_lockup_state
dump_lockup_state:
    bl __rust_alloc
    ret
.global __rust_alloc
__rust_alloc:
    ret
