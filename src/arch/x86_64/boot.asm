global start
;extern long_mode_start

section .text
bits 32
start:
    ; Initialize stack pointer
    mov esp, stack_top
    mov edi, ebx       ; Move Multiboot info pointer to edi

    call test_multiboot
    call test_cpuid
    call test_long_mode

    call setup_page_tables
    call enable_paging

    ; load the 64-bit GDT
    lgdt [gdtr]

    jmp gdt.kernel_code:long_mode_start

; Prints `ERR: ` and the given error code to screen and hangs.
; parameter: error code (in ascii) in al
error:
    mov dword [0xb8000], 0x4f524f45
    mov dword [0xb8004], 0x4f3a4f52
    mov dword [0xb8008], 0x4f204f20
    mov byte  [0xb800a], al
    hlt

test_multiboot:
    cmp eax, 0x36d76289
    jne .no_multiboot
    ret
.no_multiboot:
    mov al, "0"
    jmp error

test_cpuid:
    pushfd               ; Store the FLAGS-register.
    pop eax              ; Restore the A-register.
    mov ecx, eax         ; Set the C-register to the A-register.
    xor eax, 1 << 21     ; Flip the ID-bit, which is bit 21.
    push eax             ; Store the A-register.
    popfd                ; Restore the FLAGS-register.
    pushfd               ; Store the FLAGS-register.
    pop eax              ; Restore the A-register.
    push ecx             ; Store the C-register.
    popfd                ; Restore the FLAGS-register.
    xor eax, ecx         ; Do a XOR-operation on the A-register and the C-register.
    jz .no_cpuid         ; The zero flag is set, no CPUID.
    ret                  ; CPUID is available for use.
.no_cpuid:
    mov al, "1"
    jmp error

test_long_mode:
    mov eax, 0x80000000    ; Set the A-register to 0x80000000.
    cpuid                  ; CPU identification.
    cmp eax, 0x80000001    ; Compare the A-register with 0x80000001.
    jb .no_long_mode       ; It is less, there is no long mode.
    mov eax, 0x80000001    ; Set the A-register to 0x80000001.
    cpuid                  ; CPU identification.
    test edx, 1 << 29      ; Test if the LM-bit, which is bit 29, is set in the D-register.
    jz .no_long_mode       ; They aren't, there is no long mode.
    ret
.no_long_mode:
    mov al, "2"
    jmp error

setup_page_tables:
    ; setup recursive p4
    mov eax, p4_table
    or eax, 0b11 ; present + writable
    mov [p4_table + 511 * 8], eax

    ; map first P4 entry to P3 table
    mov eax, p3_table
    or eax, 0b11 ; present + writable
    mov [p4_table], eax

    ; map first P3 entry to P2 table
    mov eax, p2_table
    or eax, 0b11 ; present + writable
    mov [p3_table], eax

    ; map each P2 entry to a huge 2MiB page
    mov ecx, 0         ; counter variable

.map_p2_table:
    ; map ecx-th P2 entry to a huge page that starts at address 2MiB*ecx
    mov eax, 0x200000  ; 2MiB
    mul ecx            ; start address of ecx-th page
    or eax, 0b10000011 ; present + writable + huge
    mov [p2_table + ecx * 8], eax ; map ecx-th entry

    inc ecx            ; increase counter
    cmp ecx, 512       ; if counter == 512, the whole P2 table is mapped
    jne .map_p2_table  ; else map the next entry

    ret

enable_paging:
    ; load P4 to cr3 register (cpu uses this to access the P4 table)
    mov eax, p4_table
    mov cr3, eax

    ; enable PAE-flag in cr4 (Physical Address Extension)
    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax

    ; set the long mode bit in the EFER MSR (model specific register)
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

    ; enable paging in the cr0 register
    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax

    ret

section .bss
align 4096
p4_table:
    resb 4096
p3_table:
    resb 4096
p2_table:
    resb 4096
stack_bottom:
    resb 4096*8192
stack_top:


section .text
bits 64

%include "src/arch/x86_64/descriptor_flags.inc"
%include "src/arch/x86_64/gdt_entry.inc"

long_mode_start:
    ; load the IDT
    ;lidt [idtr]

    ; update selectors
    mov ax, gdt.kernel_data
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; call the rust main
    extern rust_main
    call setup_SSE
    call rust_main

    .os_returned:
        ; rust main returned, print `OS returned!`
        mov rax, 0x4f724f204f534f4f
        mov [0xb8000], rax
        mov rax, 0x4f724f754f744f65
        mov [0xb8008], rax
        mov rax, 0x4f214f644f654f6e
        mov [0xb8010], rax
        hlt

; Prints `ERROR: ` and the given error code to screen and hangs.
; parameter: error code (in ascii) in al
error_msg:
    mov rbx, 0x4f4f4f524f524f45
    mov [0xb8000], rbx
    mov rbx, 0x4f204f204f3a4f52
    mov [0xb8008], rbx
    mov byte [0xb800e], al
    hlt
    jmp error_msg

; Check for SSE and enable it. If it's not supported throw error "a".
setup_SSE:
    ; check for SSE
    mov rax, 0x1
    cpuid
    test edx, 1<<25
    jz .no_SSE

    ; enable SSE
    mov rax, cr0
    and ax, 0xFFFB      ; clear coprocessor emulation CR0.EM
    or ax, 0x2          ; set coprocessor monitoring  CR0.MP
    mov cr0, rax
    mov rax, cr4
    or ax, 3 << 9       ; set CR4.OSFXSR and CR4.OSXMMEXCPT at the same time
    mov cr4, rax

    ret
.no_SSE:
    mov al, "a"
    jmp error_msg


gdtr:
    dw gdt.end + 1  ; size
    dq gdt          ; offset

gdt:
    .null equ $ - gdt
    dq 0

    .kernel_code equ $ - gdt
    istruc GDTEntry
        at GDTEntry.limitl, dw 0
        at GDTEntry.basel, dw 0
        at GDTEntry.basem, db 0
        at GDTEntry.attribute, db attrib.present | attrib.user | attrib.code
        at GDTEntry.flags__limith, db flags.long_mode
        at GDTEntry.baseh, db 0
    iend

    .kernel_data equ $ - gdt
    istruc GDTEntry
        at GDTEntry.limitl, dw 0
        at GDTEntry.basel, dw 0
        at GDTEntry.basem, db 0
    ; AMD System Programming Manual states that the writeable bit is ignored in long mode, but ss can not be set to this descriptor without it
        at GDTEntry.attribute, db attrib.present | attrib.user | attrib.writable
        at GDTEntry.flags__limith, db 0
        at GDTEntry.baseh, db 0
    iend

    .user_code equ $ - gdt
    istruc GDTEntry
        at GDTEntry.limitl, dw 0
        at GDTEntry.basel, dw 0
        at GDTEntry.basem, db 0
        at GDTEntry.attribute, db attrib.present | attrib.ring3 | attrib.user | attrib.code
        at GDTEntry.flags__limith, db flags.long_mode
        at GDTEntry.baseh, db 0
    iend

    .user_data equ $ - gdt
    istruc GDTEntry
        at GDTEntry.limitl, dw 0
        at GDTEntry.basel, dw 0
        at GDTEntry.basem, db 0
    ; AMD System Programming Manual states that the writeable bit is ignored in long mode, but ss can not be set to this descriptor without it
        at GDTEntry.attribute, db attrib.present | attrib.ring3 | attrib.user | attrib.writable
        at GDTEntry.flags__limith, db 0
        at GDTEntry.baseh, db 0
    iend

    .tss equ $ - gdt
    istruc GDTEntry
        at GDTEntry.limitl, dw (tss.end - tss) & 0xFFFF
        at GDTEntry.basel, dw (tss-$$+0x7C00) & 0xFFFF
        at GDTEntry.basem, db ((tss-$$+0x7C00) >> 16) & 0xFF
        at GDTEntry.attribute, db attrib.present | attrib.ring3 | attrib.tssAvailabe64
        at GDTEntry.flags__limith, db ((tss.end - tss) >> 16) & 0xF
        at GDTEntry.baseh, db ((tss-$$+0x7C00) >> 24) & 0xFF
    iend
    dq 0 ;tss descriptors are extended to 16 Bytes

    .end equ $ - gdt

struc TSS
    .reserved1 resd 1    ;The previous TSS - if we used hardware task switching this would form a linked list.
    .rsp0 resq 1        ;The stack pointer to load when we change to kernel mode.
    .rsp1 resq 1        ;everything below here is unusued now..
    .rsp2 resq 1
    .reserved2 resd 1
    .reserved3 resd 1
    .ist1 resq 1
    .ist2 resq 1
    .ist3 resq 1
    .ist4 resq 1
    .ist5 resq 1
    .ist6 resq 1
    .ist7 resq 1
    .reserved4 resd 1
    .reserved5 resd 1
    .reserved6 resw 1
    .iomap_base resw 1
endstruc

tss:
    istruc TSS
        at TSS.rsp0, dd 0x200000 - 128
        at TSS.iomap_base, dw 0xFFFF
    iend
.end: