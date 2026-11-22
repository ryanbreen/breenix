section .text
    call external
    call external@plt
    mov rax, [rip + payload + 3]
    mov r9, [rip + external@gotpcrel]
    mov eax, [rbx + payload - 2]
    mov eax, fs:[tls@tpoff]
    cmp dword [rip + external + 8], 0x1234
    ret
extern external
section .data
payload:
    dq external + 7
    dd payload
    dd external@32s - 3
section .tdata
    .type tls, @tls
tls:
    dd 42
