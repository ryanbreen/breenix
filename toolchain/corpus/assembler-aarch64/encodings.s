.section .text
.global _start
_start:
    movz x0, #42
    movz x8, #93
    svc #0
.Lback:
    nop
    b .Lforward
    bl .Lback
    b.ne .Lforward
    cbz w3, .Lback
    cbnz x5, .Lforward
    tbz w7, #31, .Lback
    tbnz x9, #63, .Lforward
    br x16
    blr x17
    ret x19
.Lforward:
    ret
    movz w11, #0x1234, lsl #16
    movz xzr, #0xffff, lsl #48
    add w1, wsp, #4095, lsl #12
    add w1, w2, w3, asr #7
    add w1, wsp, w3, sxtb #4
    adds w1, wsp, #4095, lsl #12
    adds w1, w2, w3, asr #7
    adds w1, wsp, w3, sxtb #4
    sub w1, wsp, #4095, lsl #12
    sub w1, w2, w3, asr #7
    sub w1, wsp, w3, sxtb #4
    subs w1, wsp, #4095, lsl #12
    subs w1, w2, w3, asr #7
    subs w1, wsp, w3, sxtb #4
    cmp wsp, #4095, lsl #12
    cmp w2, w3, asr #7
    cmp wsp, w3, sxtb #4
    cmn wsp, #4095, lsl #12
    cmn w2, w3, asr #7
    cmn wsp, w3, sxtb #4
    add wsp, w2, w3, lsl #4
    sub wsp, wsp, w3
    add w1, w2, #17
    adds wzr, wsp, w3, lsl #1
    and w1, w2, w3, ror #11
    orr w1, w2, w3, ror #11
    eor w1, w2, w3, ror #11
    ands w1, w2, w3, ror #11
    bic w1, w2, w3, ror #11
    bics w1, w2, w3, ror #11
    orn w1, w2, w3, ror #11
    eon w1, w2, w3, ror #11
    tst w2, w3, ror #11
    mvn w1, w3, ror #11
    and w1, w2, #0xff00ff00
    orr w1, w2, #0xff00ff00
    eor w1, w2, #0xff00ff00
    ands w1, w2, #0xff00ff00
    tst w2, #0xff00ff00
    and wsp, w2, #0x55555555
    orr w1, w2, #0x81818181
    eor w1, wzr, #0x80000001
    movz w11, #0x2345, lsl #16
    movn w11, #0x2345, lsl #16
    movk w11, #0x2345, lsl #16
    mov w1, w2
    mov w1, wsp
    mov wsp, w1
    mov w11, #0x23450000
    mov w11, #-2
    mov w1, #0x55555555
    lsl w1, w2, #7
    lsl w1, w2, w3
    lsr w1, w2, #7
    lsr w1, w2, w3
    asr w1, w2, #7
    asr w1, w2, w3
    ror w1, w2, #7
    ror w1, w2, w3
    ubfm w1, w2, #8, #15
    sbfm w1, w2, #12, #19
    bfm w1, w2, #28, #7
    extr w1, w2, w3, #13
    ubfx w1, w2, #25, #7
    sbfx w1, w2, #25, #7
    sxtb w1, w2
    sxth w1, w2
    uxtb w1, w2
    uxth w1, w2
    madd w1, w2, w3, w4
    msub w1, w2, w3, w4
    mul w1, w2, w3
    mneg w1, w2, w3
    sdiv w1, w2, w3
    udiv w1, w2, w3
    csel w1, w2, w3, eq
    csinc w1, w2, w3, ne
    csinv w1, w2, w3, hs
    csneg w1, w2, w3, lo
    cset w1, lt
    csetm w1, ge
    cinc w1, w2, hi
    cneg w1, w2, ls
    ccmp w2, w3, #9, pl
    ccmp w2, #31, #15, mi
    ccmn w2, w3, #9, pl
    ccmn w2, #31, #15, mi
    clz w1, w2
    rbit w1, w2
    rev w1, w2
    add x1, sp, #4095, lsl #12
    add x1, x2, x3, asr #7
    add x1, sp, w3, sxtb #4
    adds x1, sp, #4095, lsl #12
    adds x1, x2, x3, asr #7
    adds x1, sp, w3, sxtb #4
    sub x1, sp, #4095, lsl #12
    sub x1, x2, x3, asr #7
    sub x1, sp, w3, sxtb #4
    subs x1, sp, #4095, lsl #12
    subs x1, x2, x3, asr #7
    subs x1, sp, w3, sxtb #4
    cmp sp, #4095, lsl #12
    cmp x2, x3, asr #7
    cmp sp, w3, sxtb #4
    cmn sp, #4095, lsl #12
    cmn x2, x3, asr #7
    cmn sp, w3, sxtb #4
    add sp, x2, x3, lsl #4
    sub sp, sp, x3
    add x1, x2, #17
    adds xzr, sp, x3, lsl #1
    and x1, x2, x3, ror #11
    orr x1, x2, x3, ror #11
    eor x1, x2, x3, ror #11
    ands x1, x2, x3, ror #11
    bic x1, x2, x3, ror #11
    bics x1, x2, x3, ror #11
    orn x1, x2, x3, ror #11
    eon x1, x2, x3, ror #11
    tst x2, x3, ror #11
    mvn x1, x3, ror #11
    and x1, x2, #0xff00ff00ff00ff00
    orr x1, x2, #0xff00ff00ff00ff00
    eor x1, x2, #0xff00ff00ff00ff00
    ands x1, x2, #0xff00ff00ff00ff00
    tst x2, #0xff00ff00ff00ff00
    and sp, x2, #0x5555555555555555
    orr x1, x2, #0x8181818181818181
    eor x1, xzr, #0x8000000000000001
    movz x11, #0x2345, lsl #48
    movn x11, #0x2345, lsl #48
    movk x11, #0x2345, lsl #48
    mov x1, x2
    mov x1, sp
    mov sp, x1
    mov x11, #0x23450000
    mov x11, #-2
    mov x1, #0x5555555555555555
    lsl x1, x2, #7
    lsl x1, x2, x3
    lsr x1, x2, #7
    lsr x1, x2, x3
    asr x1, x2, #7
    asr x1, x2, x3
    ror x1, x2, #7
    ror x1, x2, x3
    ubfm x1, x2, #8, #15
    sbfm x1, x2, #12, #19
    bfm x1, x2, #60, #7
    extr x1, x2, x3, #13
    ubfx x1, x2, #25, #7
    sbfx x1, x2, #25, #7
    sxtb x1, w2
    sxth x1, w2
    sxtw x1, w2
    madd x1, x2, x3, x4
    msub x1, x2, x3, x4
    mul x1, x2, x3
    mneg x1, x2, x3
    sdiv x1, x2, x3
    udiv x1, x2, x3
    csel x1, x2, x3, eq
    csinc x1, x2, x3, ne
    csinv x1, x2, x3, hs
    csneg x1, x2, x3, lo
    cset x1, lt
    csetm x1, ge
    cinc x1, x2, hi
    cneg x1, x2, ls
    ccmp x2, x3, #9, pl
    ccmp x2, #31, #15, mi
    ccmn x2, x3, #9, pl
    ccmn x2, #31, #15, mi
    clz x1, x2
    rbit x1, x2
    rev x1, x2
    smull x1, w2, w3
    umull x1, w2, w3
    smulh x1, x2, x3
    umulh x1, x2, x3
.Ladr_back: adr x4, .Ladr_forward+1
.Ladr_forward: adr x5, .Ladr_back
    uxtb x1, w2
    uxth x1, w2
    uxtw x1, w2
    mov sp, #0xff

// Memory, barriers and scalar floating point
    ldr w1, [x2, #16380]
    ldr w1, [sp, #-256]!
    ldr w1, [x2], #255
    ldur w1, [x2, #-7]
    ldr w1, [x2, x3, lsl #2]
    ldr x1, [x2, #296]
    ldr x1, [sp, #-256]!
    ldr x1, [x2], #255
    ldur x1, [x2, #-7]
    ldr x1, [x2, w3, uxtw #3]
    str w1, [x2, #148]
    str w1, [sp, #-256]!
    str w1, [x2], #255
    stur w1, [x2, #-7]
    str w1, [x2, w3, sxtw #2]
    str x1, [x2, #296]
    str x1, [sp, #-256]!
    str x1, [x2], #255
    stur x1, [x2, #-7]
    str x1, [x2, x3, sxtx #3]
    ldrb w1, [x2, #37]
    ldrb w1, [sp, #-256]!
    ldrb w1, [x2], #255
    ldurb w1, [x2, #-7]
    ldrb w1, [x2, x3, lsl #0]
    strb w1, [x2, #37]
    strb w1, [sp, #-256]!
    strb w1, [x2], #255
    sturb w1, [x2, #-7]
    strb w1, [x2, w3, uxtw #0]
    ldrh w1, [x2, #74]
    ldrh w1, [sp, #-256]!
    ldrh w1, [x2], #255
    ldurh w1, [x2, #-7]
    ldrh w1, [x2, w3, sxtw #1]
    strh w1, [x2, #74]
    strh w1, [sp, #-256]!
    strh w1, [x2], #255
    sturh w1, [x2, #-7]
    strh w1, [x2, x3, sxtx #1]
    ldrsb w1, [x2, #37]
    ldrsb w1, [sp, #-256]!
    ldrsb w1, [x2], #255
    ldursb w1, [x2, #-7]
    ldrsb w1, [x2, x3, lsl #0]
    ldrsb x1, [x2, #37]
    ldrsb x1, [sp, #-256]!
    ldrsb x1, [x2], #255
    ldursb x1, [x2, #-7]
    ldrsb x1, [x2, w3, uxtw #0]
    ldrsh w1, [x2, #74]
    ldrsh w1, [sp, #-256]!
    ldrsh w1, [x2], #255
    ldursh w1, [x2, #-7]
    ldrsh w1, [x2, w3, sxtw #1]
    ldrsh x1, [x2, #74]
    ldrsh x1, [sp, #-256]!
    ldrsh x1, [x2], #255
    ldursh x1, [x2, #-7]
    ldrsh x1, [x2, x3, sxtx #1]
    ldrsw x1, [x2, #148]
    ldrsw x1, [sp, #-256]!
    ldrsw x1, [x2], #255
    ldursw x1, [x2, #-7]
    ldrsw x1, [x2, x3, lsl #2]
    ldr s1, [x2, #148]
    ldr s1, [sp, #-256]!
    ldr s1, [x2], #255
    ldur s1, [x2, #-7]
    ldr s1, [x2, w3, uxtw #2]
    ldr d1, [x2, #296]
    ldr d1, [sp, #-256]!
    ldr d1, [x2], #255
    ldur d1, [x2, #-7]
    ldr d1, [x2, w3, sxtw #3]
    ldr q1, [x2, #592]
    ldr q1, [sp, #-256]!
    ldr q1, [x2], #255
    ldur q1, [x2, #-7]
    ldr q1, [x2, x3, sxtx #4]
    str s1, [x2, #148]
    str s1, [sp, #-256]!
    str s1, [x2], #255
    stur s1, [x2, #-7]
    str s1, [x2, x3, lsl #2]
    str d1, [x2, #296]
    str d1, [sp, #-256]!
    str d1, [x2], #255
    stur d1, [x2, #-7]
    str d1, [x2, w3, uxtw #3]
    str q1, [x2, #592]
    str q1, [sp, #-256]!
    str q1, [x2], #255
    stur q1, [x2, #-7]
    str q1, [x2, w3, sxtw #4]
    ldr xzr, [sp]
    ldr w1, [x2, x3]
    ldrsb w1, [x2, w3, sxtw]
    ldrb w1, [x2, x3]
    .Lmemory_back: ldr w1, .Lmemory_forward
    .Lmemory_forward: ldr x2, .Lmemory_back
    .Lmemory_simd: ldr q3, .Lmemory_simd
    stp w1, w2, [x3, #-256]
    stp w1, w2, [sp, #-12]!
    stp w1, w2, [x3], #252
    stp x1, x2, [x3, #-512]
    stp x1, x2, [sp, #-24]!
    stp x1, x2, [x3], #504
    stp s1, s2, [x3, #-256]
    stp s1, s2, [sp, #-12]!
    stp s1, s2, [x3], #252
    stp d1, d2, [x3, #-512]
    stp d1, d2, [sp, #-24]!
    stp d1, d2, [x3], #504
    stp q1, q2, [x3, #-1024]
    stp q1, q2, [sp, #-48]!
    stp q1, q2, [x3], #1008
    ldp w1, w2, [x3, #-256]
    ldp w1, w2, [sp, #-12]!
    ldp w1, w2, [x3], #252
    ldp x1, x2, [x3, #-512]
    ldp x1, x2, [sp, #-24]!
    ldp x1, x2, [x3], #504
    ldp s1, s2, [x3, #-256]
    ldp s1, s2, [sp, #-12]!
    ldp s1, s2, [x3], #252
    ldp d1, d2, [x3, #-512]
    ldp d1, d2, [sp, #-24]!
    ldp d1, d2, [x3], #504
    ldp q1, q2, [x3, #-1024]
    ldp q1, q2, [sp, #-48]!
    ldp q1, q2, [x3], #1008
    ldpsw x1, x2, [x3, #-4]
    ldpsw x1, x2, [x3, #-4]!
    ldpsw x1, x2, [x3], #-4
    ldp x1, xzr, [x1]
    ldxrb w1, [x2]
    ldxrh w1, [sp, #0]
    ldxr w1, [x2]
    ldxr x1, [x2]
    stxrb w4, w1, [x2]
    stxrh w4, w1, [sp, #0]
    stxr w4, w1, [x2]
    stxr w4, x1, [x2]
    ldaxrb w1, [x2]
    ldaxrh w1, [sp, #0]
    ldaxr w1, [x2]
    ldaxr x1, [x2]
    stlxrb w4, w1, [x2]
    stlxrh w4, w1, [sp, #0]
    stlxr w4, w1, [x2]
    stlxr w4, x1, [x2]
    ldarb w1, [x2]
    ldarh w1, [sp, #0]
    ldar w1, [x2]
    ldar x1, [x2]
    stlrb w1, [x2]
    stlrh w1, [sp, #0]
    stlr w1, [x2]
    stlr x1, [x2]
    dmb oshld
    dmb oshst
    dmb osh
    dmb nshld
    dmb nshst
    dmb nsh
    dmb ishld
    dmb ishst
    dmb ish
    dmb ld
    dmb st
    dmb sy
    dmb #12
    dsb oshld
    dsb oshst
    dsb osh
    dsb nshld
    dsb nshst
    dsb nsh
    dsb ishld
    dsb ishst
    dsb ish
    dsb ld
    dsb st
    dsb sy
    dsb #12
    dsb oshnxs
    dsb nshnxs
    dsb ishnxs
    dsb synxs
    isb
    isb sy
    isb #7
    fmov s1, s2
    fabs s1, s2
    fneg s1, s2
    fsqrt s1, s2
    fmul s1, s2, s3
    fdiv s1, s2, s3
    fadd s1, s2, s3
    fsub s1, s2, s3
    fmadd s1, s2, s3, s4
    fcsel s1, s2, s3, lo
    fcmp s2, s3
    fcmp s2, #0.0
    fcmpe s2, s3
    fcmpe s2, #0.0
    fcvt d1, s2
    fmov s1, w2
    fmov w1, s2
    fmov s1, #0.1328125
    fmov s1, #-31.0
    fmov s1, #1.5e+0
    fcvtzs w1, s2
    fcvtzu w1, s2
    scvtf s1, w2
    ucvtf s1, w2
    fcvtzs x1, s2
    fcvtzu x1, s2
    scvtf s1, x2
    ucvtf s1, x2
    fmov d1, d2
    fabs d1, d2
    fneg d1, d2
    fsqrt d1, d2
    fmul d1, d2, d3
    fdiv d1, d2, d3
    fadd d1, d2, d3
    fsub d1, d2, d3
    fmadd d1, d2, d3, d4
    fcsel d1, d2, d3, lo
    fcmp d2, d3
    fcmp d2, #0.0
    fcmpe d2, d3
    fcmpe d2, #0.0
    fcvt s1, d2
    fmov d1, x2
    fmov x1, d2
    fmov d1, #0.1328125
    fmov d1, #-31.0
    fmov d1, #1.5e+0
    fcvtzs w1, d2
    fcvtzu w1, d2
    scvtf d1, w2
    ucvtf d1, w2
    fcvtzs x1, d2
    fcvtzu x1, d2
    scvtf d1, x2
    ucvtf d1, x2
