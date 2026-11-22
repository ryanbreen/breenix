section .text
global entry
.type entry, @function
entry:
    mov r9, qword [r12 + r13*4 - 8]
    mov word fs:[rbp], r10w
    mov spl, [rsp]
    mov ah, [rbx + rcx*2 + 0x1234]
    mov r11d, [r12*8 + 0x78]
    mov rax, gs:[0x10]
    mov r8d, [rip + .Lpayload + 1]
    mov r15w, r8w
    mov eax, [r13d + ecx]
    call .Ldone
    jmp .Ldone
.Lback: jne .Ldone
    .zero 128
.Ldone: jne .Lback
    ret
.size entry, . - entry
.type .Lpayload, @object
.Lpayload: db 1, 2, 3
.size .Lpayload, 3

section .data
.type payload, @object
payload:
    .byte -1, 0x80
    .word 0x1234
    .long -2
    .quad 0xffffffffffffffff
    .asciz "a;b\n"
    .ascii "C"
    align 32, 0xa5
    .byte 7
.size payload, .-payload
global payload

section .bss
    .zero 3
    align 16
scratch: .zero 5
.type scratch, @object
.size scratch, 5
