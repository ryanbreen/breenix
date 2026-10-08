; Application processor startup trampoline.
;
; A startup IPI starts a processor in real mode at CS:IP = (page << 8):0, so
; this code runs from a copy the kernel places at a page below 1 MiB
; (`arch_impl::x86_64::ap_start`). It is never executed where it is linked.
; Every address it uses is an offset from its own start, added to the page
; base it computes from CS, and the fields the kernel fills in per processor
; sit in the data block at the end.
;
; Path: real mode -> 32-bit protected mode (flat segments) -> long mode on the
; trampoline page table, which identity-maps the first 2 MiB and shares every
; other top-level entry with the master kernel page table -> the kernel entry
; at its linked address, on the processor's own stack, with its logical CPU
; number in RDI. The kernel entry loads the master page table itself.

section .rodata
bits 16

global ap_trampoline_start
global ap_trampoline_end
global ap_trampoline_gdt
global ap_trampoline_gdt_desc
global ap_trampoline_pm32
global ap_trampoline_pm32_ptr
global ap_trampoline_lm64
global ap_trampoline_lm64_ptr
global ap_trampoline_pml4
global ap_trampoline_efer
global ap_trampoline_stack
global ap_trampoline_cpu
global ap_trampoline_entry

%define OFF(label) ((label) - ap_trampoline_start)

ap_trampoline_start:
    cli
    cld
    mov ax, cs
    mov ds, ax
    xor ebx, ebx
    mov bx, ax
    shl ebx, 4                      ; EBX = physical base of this page

    o32 lgdt [OFF(ap_trampoline_gdt_desc)]
    mov eax, cr0
    or eax, 1                       ; PE
    mov cr0, eax
    jmp dword far [OFF(ap_trampoline_pm32_ptr)]

bits 32
ap_trampoline_pm32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    mov eax, cr4
    or eax, 1 << 5                  ; PAE
    mov cr4, eax
    mov eax, [ebx + OFF(ap_trampoline_pml4)]
    mov cr3, eax

    mov ecx, 0xC0000080             ; IA32_EFER
    mov eax, [ebx + OFF(ap_trampoline_efer)]
    xor edx, edx
    wrmsr

    mov eax, cr0
    or eax, 0x80000001              ; PG | PE
    mov cr0, eax
    jmp far [ebx + OFF(ap_trampoline_lm64_ptr)]

bits 64
ap_trampoline_lm64:
    mov ebx, ebx                    ; zero the upper half of RBX
    xor eax, eax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    mov rsp, [rbx + OFF(ap_trampoline_stack)]
    and rsp, -16
    push 0                          ; no return address: RSP % 16 == 8 at entry
    mov rdi, [rbx + OFF(ap_trampoline_cpu)]
    mov rax, [rbx + OFF(ap_trampoline_entry)]
    jmp rax

align 8
ap_trampoline_gdt:
    dq 0
    dq 0x00CF9A000000FFFF           ; 0x08: 32-bit code, flat
    dq 0x00CF92000000FFFF           ; 0x10: data, flat
    dq 0x00AF9A000000FFFF           ; 0x18: 64-bit code
ap_trampoline_gdt_end:

ap_trampoline_gdt_desc:
    dw ap_trampoline_gdt_end - ap_trampoline_gdt - 1
    dd 0                            ; base: page + OFF(ap_trampoline_gdt)

ap_trampoline_pm32_ptr:
    dd 0                            ; page + OFF(ap_trampoline_pm32)
    dw 0x08
ap_trampoline_lm64_ptr:
    dd 0                            ; page + OFF(ap_trampoline_lm64)
    dw 0x18

align 8
ap_trampoline_pml4:
    dd 0                            ; physical address of the trampoline PML4
ap_trampoline_efer:
    dd 0                            ; EFER to load: LME and the boot CPU's NXE/SCE
ap_trampoline_stack:
    dq 0                            ; top of this processor's stack
ap_trampoline_cpu:
    dq 0                            ; logical CPU number
ap_trampoline_entry:
    dq 0                            ; kernel entry, `ap_start::ap_entry`
ap_trampoline_end:
