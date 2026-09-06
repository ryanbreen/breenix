.global dump_lockup_state
dump_lockup_state:
    bl format_the_banner
    ret
.global format_the_banner
format_the_banner:
    bl widen_the_row
    ret
.global widen_the_row
widen_the_row:
    bl __rust_alloc
    ret
.global __rust_alloc
__rust_alloc:
    ret
