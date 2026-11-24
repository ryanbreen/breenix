.text
.globl hosted_probe_fn
.type hosted_probe_fn, %function
hosted_probe_fn:
    .cfi_startproc
    ret
    .cfi_endproc
.size hosted_probe_fn, . - hosted_probe_fn

# LSB Core 5.0 Table 10-4. The probe checks placement, so the LSDA
# only needs to supply bytes.
.section .gcc_except_table.probe,"a",%progbits
.globl hosted_probe_lsda
hosted_probe_lsda:
    .byte 0

# ELF gABI Table 3.8. Use a bare entry and a priority-suffixed entry
# in each array to exercise both input selectors under --gc-sections.
.irp phase,preinit,init,fini
    .section .\phase\()_array,"aw",%\phase\()_array
    .p2align 3
    .quad hosted_probe_fn
    .section .\phase\()_array.0100,"aw",%\phase\()_array
    .p2align 3
    .quad hosted_probe_fn
.endr

# Referencing the bounds causes PROVIDE_HIDDEN to materialize them. Weak
# references let the same probe link before the hosted script exists.
.section .rodata.hosted_probe_bounds,"a",%progbits
.p2align 3
.globl hosted_probe_bounds
hosted_probe_bounds:
.irp phase,preinit,init,fini
    .weak __\phase\()_array_start
    .weak __\phase\()_array_end
    .quad __\phase\()_array_start
    .quad __\phase\()_array_end
.endr

.section .note.GNU-stack,"",%progbits
