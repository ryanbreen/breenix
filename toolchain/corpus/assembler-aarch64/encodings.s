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
