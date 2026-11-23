//! AMD64 long double operations. The Intel SDM Vol. 1 §8.2 and Vol. 2
//! specify the extended memory format and x87 stack instructions.

use super::*;

impl Emitter<'_, '_> {
    pub(super) fn long_double_constant(&mut self, bits: u128) {
        let label = self.label();
        let significand = bits as u64;
        let sign_exponent = (bits >> 64) as u16;
        writeln!(
            self.rodata,
            "    .align 16\n{label}:\n    .quad 0x{significand:x}\n    .word 0x{sign_exponent:x}"
        )
        .unwrap();
        writeln!(self.text, "    fld tbyte [rip + {label}]").unwrap();
    }

    pub(super) fn x87_binary(&mut self, op: Binary) {
        // The restored left operand is ST(0), the right operand ST(1).
        // Reverse subtract/divide write left OP right into ST(1) and pop.
        let instruction = match op {
            Binary::Add => "faddp",
            Binary::Subtract => "fsubrp",
            Binary::Multiply => "fmulp",
            Binary::Divide => "fdivrp",
            _ => {
                self.text
                    .push_str("    fucomip st(0), st(1)\n    fstp st(0)\n");
                self.compare_result(op, false, true);
                return;
            }
        };
        writeln!(self.text, "    {instruction} st(1), st(0)").unwrap();
    }

    pub(super) fn x87_boolean(&mut self) {
        self.text.push_str(
            "    fldz\n    fucomip st(0), st(1)\n    fstp st(0)\n    setne al\n    setp dl\n    or al, dl\n",
        );
    }

    pub(super) fn integer_to_x87(&mut self, integer: Integer) {
        self.adjust_stack("sub", 16);
        self.text.push_str("    mov qword [rsp], rax\n");
        if is_unsigned_64(integer) {
            let signed = self.label();
            let done = self.label();
            // With bit 63 set, the unsigned input already is the complete
            // significand: the unbiased exponent is 63.
            writeln!(
                self.text,
                "    test rax, rax\n    jns {signed}\n    mov word [rsp + 8], 0x403e\n    fld tbyte [rsp]\n    jmp {done}\n{signed}:\n    fild qword [rsp]\n{done}:"
            )
            .unwrap();
        } else {
            self.text.push_str("    fild qword [rsp]\n");
        }
        self.adjust_stack("add", 16);
    }

    pub(super) fn x87_to_integer(&mut self, integer: Integer, dest: &Type) {
        // C removes the fractional part regardless of the rounding mode.
        // Use FISTP with a temporary control word so conversion needs only
        // the baseline x87 ISA. Restore the caller's control word afterwards.
        self.adjust_stack("sub", 16);
        self.text.push_str(
            "    fnstcw word [rsp + 8]\n    mov ax, word [rsp + 8]\n    or ax, 0xf00\n    mov word [rsp + 10], ax\n    fldcw word [rsp + 10]\n",
        );
        if is_unsigned_64(integer) {
            let small = self.label();
            let restore = self.label();
            // FISTP has no unsigned form. Above 2^63, subtract it exactly
            // at 64-bit precision and set the high bit of the converted part.
            self.long_double_constant(0x403e_8000_0000_0000_0000);
            writeln!(
                self.text,
                "    fucomi st(0), st(1)\n    ja {small}\n    fsubp st(1), st(0)\n    fistp qword [rsp]\n    mov rax, qword [rsp]\n    bts rax, 63\n    jmp {restore}\n{small}:\n    fstp st(0)\n    fistp qword [rsp]\n    mov rax, qword [rsp]\n{restore}:"
            )
            .unwrap();
        } else {
            self.text
                .push_str("    fistp qword [rsp]\n    mov rax, qword [rsp]\n");
        }
        self.text.push_str("    fldcw word [rsp + 8]\n");
        self.adjust_stack("add", 16);
        self.normalize_integer(dest);
    }

    pub(super) fn sse_to_x87(&mut self, float: Float) {
        self.adjust_stack("sub", 16);
        let mov = float_move(float);
        let width = sse_memory_width(float);
        writeln!(self.text, "    {mov} [rsp], xmm0\n    fld {width} [rsp]").unwrap();
        self.adjust_stack("add", 16);
    }

    pub(super) fn x87_to_sse(&mut self, float: Float) {
        self.adjust_stack("sub", 16);
        let mov = float_move(float);
        let width = sse_memory_width(float);
        writeln!(self.text, "    fstp {width} [rsp]\n    {mov} xmm0, [rsp]").unwrap();
        self.adjust_stack("add", 16);
    }
}

fn is_unsigned_64(integer: Integer) -> bool {
    integer.layout().size == 8 && !integer.is_signed(Target::X86_64)
}

fn sse_memory_width(float: Float) -> &'static str {
    match float {
        Float::Single => "dword",
        Float::Double => "qword",
    }
}
