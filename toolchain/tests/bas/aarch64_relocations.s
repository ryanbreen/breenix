// AAELF64 2025Q1 §§5.7.5–5.7.8. The expected RELA records are in run.py.
.section .text
.global external
bl external+4
b .Ldata
b.ne external-4
cbz w1, external
cbnz x2, .Ldata+4
tbz x3, #37, external
tbnz w4, #6, .Ldata
ldr x5, .Ldata+8
adrp x6, .Llocal+4
add x6, x6, :lo12:.Llocal+4
ldrb w7, [x6, :lo12:.Ldata+1]
strh w8, [x6, :lo12:.Ldata+2]
ldr w9, [x6, :lo12:.Ldata+4]
str x10, [x6, :lo12:.Ldata+8]
ldr q11, [x6, :lo12:.Ldata+16]
adrp x12, :got:external
ldr x12, [x12 :got_lo12:external]
add x13, x13, :tprel_hi12:tls+8
add x13, x13, :tprel_lo12_nc:tls+8
.Llocal: nop

.section .data
.quad 0, 0
.Ldata:
dq external+7
dd .Llocal-3
dd external - .

.section .tdata
tls: .quad 0
