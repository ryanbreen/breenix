// Standalone EL0 probe for the standard ARM64 QEMU boot. Install as /sbin/init
// on a disposable ext2 seed. Exercises only E6's alignment, auxv and signal ABI.
.text
.global _start
_start:
    adr x9, unaligned_data
    ldur x10, [x9, #1]
    mov x11, #0x1122
    cmp x10, x11
    b.ne fail
    stur x10, [x9, #3]
    ldur x12, [x9, #3]
    cmp x12, x11
    b.ne fail

    // Skip argc, argv (including NUL), then envp.
    ldr x9, [sp]
    add x10, sp, #16
    add x10, x10, x9, lsl #3
1:
    ldr x9, [x10], #8
    cbnz x9, 1b
    mov x13, #0
    mov x14, #0
2:
    ldp x9, x11, [x10], #16
    cbz x9, 5f
    cmp x9, #16
    b.eq 3f
    cmp x9, #26
    b.eq 4f
    cmp x9, #51
    b.ne 2b
    mov x12, #5120
    cmp x11, x12
    b.lo fail
    cmp x11, #8192
    b.hi fail
    mov x14, x11
    b 2b
3:
    and x11, x11, #3
    cmp x11, #3
    b.ne fail
    orr x13, x13, #1
    b 2b
4:
    cbnz x11, fail
    orr x13, x13, #2
    b 2b
5:
    cmp x13, #3
    b.ne fail
    cbz x14, fail

    // Query-derived minimum is sufficient, and one byte below is rejected.
    adr x0, alternate
    adr x9, stack_area
    str x9, [x0]
    sub x9, x14, #1
    str x9, [x0, #16]
    mov x1, #0
    mov x8, #132
    svc #0
    cmn x0, #12
    b.ne fail
    adr x0, alternate
    str x14, [x0, #16]
    mov x1, #0
    mov x8, #132
    svc #0
    cbnz x0, fail

    mov x0, #10
    adr x1, action
    mov x2, #0
    mov x3, #8
    mov x8, #134
    svc #0
    cbnz x0, fail

    mov x8, #172
    svc #0
    mov x1, #10
    movi v0.16b, #0x6c
    mov x8, #129
    svc #0
    cbnz x0, fail
    adr x9, handled
    ldr x10, [x9]
    cbz x10, fail
    umov w10, v0.b[15]
    cmp w10, #0x6c
    b.ne fail

    adr x1, passed
    mov x2, #(passed_end - passed)
    b report
fail:
    adr x1, failed
    mov x2, #(failed_end - failed)
report:
    mov x0, #1
    mov x8, #64
    svc #0
6:
    mov x8, #124 // sched_yield
    svc #0
    b 6b

handler:
    mov x9, sp
    adr x10, stack_area
    cmp x9, x10
    b.lo fail
    adr x11, alternate
    ldr x11, [x11, #16]
    add x10, x10, x11
    cmp x9, x10
    b.hs fail
    adr x9, handled
    mov x10, #1
    str x10, [x9]
    movi v0.16b, #0
    ret

restorer:
    mov x8, #139
    svc #0
    b fail
passed: .ascii "BREENIX_E6_ARM64_OK\n"
passed_end:
failed: .ascii "BREENIX_E6_ARM64_FAILED\n"
failed_end:

.data
unaligned_data: .byte 0
.quad 0x1122
.zero 8
.balign 8
action: .quad handler, 0x0c000000, restorer, 0
alternate: .quad 0, 0, 0
handled: .quad 0

.bss
.balign 16
stack_area: .skip 8192
