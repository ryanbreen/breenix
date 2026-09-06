.global dump_lockup_state
dump_lockup_state:
    bl print_the_banner
    ret
.global print_the_banner
print_the_banner:
    bl write_one_byte
    ret
.global write_one_byte
write_one_byte:
    ret
