; Minimal fault handlers to detect IRETQ validation failures
; NASM syntax

section .text

global gp_stub
global ss_stub
global np_stub
global df_stub

; Macro for quick fault handlers
%macro QUICK_FAULT 1
    mov al, %1
    out 0x80, al
    hlt
    jmp $-2  ; Jump back to hlt (infinite loop)
%endmacro

; #GP handler - outputs 0xE3 (distinguishes #GP during IRETQ)
gp_stub:
    QUICK_FAULT 0xE3

; #SS handler - outputs 0xE4 (distinguishes #SS during IRETQ)
ss_stub:
    QUICK_FAULT 0xE4

; #NP handler - outputs 0xE5 (distinguishes #NP during IRETQ)
np_stub:
    QUICK_FAULT 0xE5

; Double fault handler - outputs 0xDF and halts
df_stub:
    QUICK_FAULT 0xDF