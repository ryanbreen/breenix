// AAPCS64 2025Q1 §§6.4.5, 6.8.2 and 6.9. Eight registers in each bank,
// followed by four 8-byte slots in source order: i8, f8, i9, f9.
// The variadic float arguments are promoted to double (N1570 §6.5.2.2).
.section .text
.global probe
.type probe, @function
.extern receive
probe:
    mov x9, sp
    tst x9, #15
    b.ne .Lbad_arg
    cmp w0, #11
    b.ne .Lbad_arg
    cmp w1, #12
    b.ne .Lbad_arg
    cmp w2, #13
    b.ne .Lbad_arg
    cmp w3, #14
    b.ne .Lbad_arg
    cmp w4, #15
    b.ne .Lbad_arg
    cmp w5, #16
    b.ne .Lbad_arg
    cmp w6, #17
    b.ne .Lbad_arg
    cmp w7, #18
    b.ne .Lbad_arg
    ldr w9, [sp]
    cmp w9, #19
    b.ne .Lbad_arg
    ldr w9, [sp, #16]
    cmp w9, #20
    b.ne .Lbad_arg
    fmov s16, #1.5
    fcmp s0, s16
    b.ne .Lbad_arg
    fmov d16, #2.5
    fcmp d1, d16
    b.ne .Lbad_arg
    fmov d16, #3.5
    fcmp d2, d16
    b.ne .Lbad_arg
    fmov d16, #4.5
    fcmp d3, d16
    b.ne .Lbad_arg
    fmov d16, #5.5
    fcmp d4, d16
    b.ne .Lbad_arg
    fmov d16, #6.5
    fcmp d5, d16
    b.ne .Lbad_arg
    fmov d16, #7.5
    fcmp d6, d16
    b.ne .Lbad_arg
    fmov d16, #8.5
    fcmp d7, d16
    b.ne .Lbad_arg
    ldr d17, [sp, #8]
    fmov d16, #9.5
    fcmp d17, d16
    b.ne .Lbad_arg
    ldr d18, [sp, #24]
    fmov d16, #10.5
    fcmp d18, d16
    b.ne .Lbad_arg

    // Forward the registers and stack values to receive, narrowing its
    // named f8 to single precision.
    sub sp, sp, #48
    stp x29, x30, [sp, #32]
    add x29, sp, #32
    str w9, [sp, #16]
    mov w9, #19
    str w9, [sp]
    fcvt s17, d17
    str s17, [sp, #8]
    str d18, [sp, #24]
    bl receive
    ldp x29, x30, [sp, #32]
    add sp, sp, #48
    // 42.5 is outside the range of the fmov immediate encoding.
    movz x9, #0x4045, lsl #48
    movk x9, #0x4000, lsl #32
    fmov d16, x9
    fcmp d0, d16
    b.ne .Lbad_result
    mov w0, #42
    ret
.Lbad_result:
    mov w0, #2
    ret
.Lbad_arg:
    mov w0, #1
    ret
.size probe, . - probe

.global quiet_nan
.type quiet_nan, @function
quiet_nan:
    movz x9, #0x7ff8, lsl #48
    fmov d0, x9
    ret
.size quiet_nan, . - quiet_nan
