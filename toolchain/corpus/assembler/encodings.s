; Intel SDM Vol. 2, named instruction tables, and Vol. 2A Tables 2-2–2-5.
movss xmm11, dword [r13 + r10*4 - 8]
movsd qword [r12 + 16], xmm9
movsd xmm1, xmm2
movdqu xmm8, xmmword [eax]
movapd xmmword [rbp], xmm9
movd xmm9, r11d
movq r11, xmm9
movq xmm8, qword [r12]
movq qword [r12], xmm8
addss xmm3, xmm4
subsd xmm9, qword [r12]
mulps xmm4, xmm5
divpd xmm0, xmm15
ucomisd xmm9, xmm12
cvtsi2sd xmm10, r11
cvttss2si r10d, dword [r8]
cvtsd2ss xmm15, xmm2
cmpsd xmm2, xmm9, 3
pxor xmm9, xmm9
paddq xmm15, xmmword [r12]
shufpd xmm10, xmm9, 2
ldmxcsr dword [r13]
fld tbyte [r13 + 16]
fstp tbyte [r12]
fld st(3)
fsub st(3), st(0)
fdivp st(2), st(0)
fild qword [r8]
fistp word [rsp + 8]
fucomip st(0), st(4)
fnstcw word [rbp]
fnstsw ax
lock add word [r12], 1
lock cmpxchg dword [r8], r9d
lock xadd qword [r9], rax
lock btr qword [r10], 5
lock cmpxchg16b xmmword [r11]
xchg byte [rsi], dl
lfence
mfence
sfence
rep movsb
rep stosq
repe cmpsw
repne scasb
lodsd
movsd
