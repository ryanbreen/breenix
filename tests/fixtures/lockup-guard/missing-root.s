.global some_other_function
some_other_function:
    bl __rust_alloc
    ret
.global __rust_alloc
__rust_alloc:
    ret
