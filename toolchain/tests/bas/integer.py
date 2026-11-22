"""Integer instruction expectations from the pinned Intel SDM (spec-ledger.md)."""

# The mnemonic headings below name the Vol. 2 opcode tables. Vol. 2A
# Tables 2-2 through 2-7 give ModR/M, SIB, REX and register field encodings.
INTEGER_ENCODINGS = (
    # MOV: C7 /0 id sign-extends to 64 bits. B8+rd io copies an imm64.
    ("mov r9, -2147483648", "49 c7 c1 00 00 00 80"),
    ("mov r15, 2147483647", "49 c7 c7 ff ff ff 7f"),
    ("mov r8, 0xffffffff", "49 b8 ff ff ff ff 00 00 00 00"),
    ("mov rax, 0x8000000000000000", "48 b8 00 00 00 00 00 00 00 80"),
    ("mov qword [r12], -1", "49 c7 04 24 ff ff ff ff"),
    # MOVZX: 0F B6/B7, MOVSX: 0F BE/BF, MOVSXD: REX.W 63.
    ("movzx r10w, byte [r13]", "66 45 0f b6 55 00"),
    ("movzx r9, word [r12]", "4d 0f b7 0c 24"),
    ("movsx eax, ah", "0f be c4"),
    ("movsx eax, spl", "40 0f be c4"),
    ("movsx r10, word [r9]", "4d 0f bf 11"),
    ("movsxd r8, dword [r9]", "4d 63 01"),
    # CBW/CWDE/CDQE: 98; CWD/CDQ/CQO: 99, with operand-size selection.
    ("cbw", "66 98"), ("cwde", "98"), ("cdqe", "48 98"),
    ("cwd", "66 99"), ("cdq", "99"), ("cqo", "48 99"),
    # ADD: 00/02 /r, 80/81/83 /0. REX selects SPL over AH for r/m=100.
    # The RIP displacement in the last ADD is
    # zero because .Lnext follows the immediate, at the end of the instruction.
    ("add ah, 1", "80 c4 01"),
    ("add spl, 1", "40 80 c4 01"),
    ("add [r12], r9b", "45 00 0c 24"),
    ("add r9, [r12]", "4d 03 0c 24"),
    ("add r11w, 128", "66 41 81 c3 80 00"),
    ("add r11w, -128", "66 41 83 c3 80"),
    ("add qword [rip + .Lnext], 128\n.Lnext:", "48 81 05 00 00 00 00 80 00 00 00"),
    # OR: 80 /1; ADC: 11 /r; SBB: 1B /r; AND: 83 /4;
    # SUB: 83 /5; XOR: 31 /r.
    ("or byte [r8], 0x80", "41 80 08 80"),
    ("adc [r13], r10d", "45 11 55 00"),
    ("sbb r10d, [r13]", "45 1b 55 00"),
    ("and rsp, -16", "48 83 e4 f0"),
    ("sub dword [rbp-4], 1", "83 6d fc 01"),
    ("xor r9d, r10d", "45 31 d1"),
    # INC: FE /0; DEC: FF /1; NOT: F6 /2; NEG: F7 /3.
    ("inc byte [r8]", "41 fe 00"),
    ("dec r8w", "66 41 ff c8"),
    ("not bh", "f6 d7"),
    ("neg r9", "49 f7 d9"),
    # SAL/SAR/SHL/SHR and RCL/RCR/ROL/ROR: D0/D1 for 1, D2/D3
    # for CL, C0/C1 for imm8. The ModR/M extension selects the operation.
    ("rol r10b, 1", "41 d0 c2"),
    ("ror word [rsp], cl", "66 d3 0c 24"),
    ("rcl ah, 7", "c0 d4 07"),
    ("rcr r8, 1", "49 d1 d8"),
    ("shl byte [r9], cl", "41 d2 21"),
    ("sal r9d, 0", "41 c1 e1 00"),
    ("shr r9d, 31", "41 c1 e9 1f"),
    ("sar r9, cl", "49 d3 f9"),
    # MUL: F6/F7 /4; IMUL: F6/F7 /5, 0F AF /r, 6B /r ib, 69 /r id;
    # DIV: F6/F7 /6; IDIV: F6/F7 /7.
    ("mul r9b", "41 f6 e1"),
    ("imul word [r12]", "66 41 f7 2c 24"),
    ("imul r9, [r12]", "4d 0f af 0c 24"),
    ("imul r9, [r12], -128", "4d 6b 0c 24 80"),
    ("imul r9, r12, 128", "4d 69 cc 80 00 00 00"),
    ("imul r9d, 3", "45 6b c9 03"),
    ("div qword [rsp]", "48 f7 34 24"),
    ("idiv r11d", "41 f7 fb"),
    # CMP: 39/3B /r, 81 /7 id; TEST: 84/85 /r, F6/F7 /0.
    ("cmp [r13], r9w", "66 45 39 4d 00"),
    ("cmp r9, [r13]", "4d 3b 4d 00"),
    ("cmp r9, 128", "49 81 f9 80 00 00 00"),
    ("test r8b, r9b", "45 84 c8"),
    ("test dword [rbp+8], r9d", "44 85 4d 08"),
    ("test qword [r9], -2147483648", "49 f7 01 00 00 00 80"),
    # SETcc: 0F 90+cc /0; CMOVcc: 0F 40+cc /r; Jcc: 0F 80+cc cd.
    ("setne spl", "40 0f 95 c4"),
    ("setc byte [r12]", "41 0f 92 04 24"),
    ("setg ah", "0f 9f c4"),
    ("cmovnae r9, [r12]", "4d 0f 42 0c 24"),
    ("cmovz r12w, r9w", "66 45 0f 44 e1"),
    ("cmovle r12d, [r13]", "45 0f 4e 65 00"),
    ("jnae -8", "0f 82 f8 ff ff ff"),
    # NOP: 90, so the forward branch has displacement 1.
    ("jge .Ltarget\nnop\n.Ltarget:", "0f 8d 01 00 00 00 90"),
    # PUSH: 50+rd, FF /6, 6A ib, 68 id; POP: 58+rd, 8F /0.
    # Long mode defaults to 64 bits for these forms without REX.W.
    ("push r12", "41 54"),
    ("pop r12", "41 5c"),
    ("push r12w", "66 41 54"),
    ("pop word [r13]", "66 41 8f 45 00"),
    ("push qword [r12]", "41 ff 34 24"),
    ("push -128", "6a 80"),
    ("push 128", "68 80 00 00 00"),
    # LEA: 8D /r. CALL: E8 cd, FF /2; JMP: E9 cd, FF /4.
    ("lea r11, [r13+r12*8+16]", "4f 8d 5c e5 10"),
    ("lea r9d, [r12d+4]", "67 45 8d 4c 24 04"),
    ("call -4", "e8 fc ff ff ff"),
    ("call r11", "41 ff d3"),
    ("call qword [r12]", "41 ff 14 24"),
    ("jmp qword [r13+8]", "41 ff 65 08"),
    # RET: C3/C2 iw, LEAVE: C9, CLD: FC, STD: FD.
    ("ret", "c3"),
    ("ret 0x1234", "c2 34 12"),
    ("leave", "c9"), ("cld", "fc"), ("std", "fd"),
)

# The invalid forms have no corresponding 64-bit encoding in the same tables.
INTEGER_REJECTIONS = (
    ("mov qword [r12], 0xffffffff", "sign-extended imm32"),
    ("cmp r9, 0x80000000", "sign-extended imm32"),
    ("movsx r9, ah", "REX"),
    ("movzx eax, [r9]", "width"),
    ("add r8d, ax", "width"),
    ("shl r9, dl", "cl"),
    ("imul al, [r8]", "width"),
    ("test [r8], [r9]", "register"),
    ("setg r8w", "byte"),
    ("cmovne al, cl", "width"),
    ("pop eax", "width"),
    ("lea r9, rax", "memory"),
    ("call eax", "width"),
    ("ret -1", "unsigned"),
)
