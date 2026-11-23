//! Floating representation queries and stack allocation for AMD64.

use super::*;
use crate::types::FloatFormat;

impl<'src> Emitter<'_, 'src> {
    fn float_words(&mut self, operand: &Expression<'src>) -> Result<(), Diagnostic> {
        if operand.ty.kind == Kind::LongDouble
            && operand.lvalue()
            && !Self::atomic_needs_storage(operand)
        {
            // Read an extended lvalue's representation without passing a
            // subnormal or signaling NaN through the x87 stack.
            self.address(operand)?;
            self.text
                .push_str("    mov rdx, qword [rax]\n    movzx eax, word [rax + 8]\n");
        } else {
            self.value(operand)?;
            match operand.ty.kind {
                Kind::Float(Float::Single) => self.text.push_str("    movd eax, xmm0\n"),
                Kind::Float(Float::Double) => self.text.push_str("    movq rax, xmm0\n"),
                Kind::LongDouble => {
                    self.adjust_stack("sub", 16);
                    self.text.push_str("    fstp tbyte [rsp]\n    mov rdx, qword [rsp]\n    movzx eax, word [rsp + 8]\n");
                    self.adjust_stack("add", 16);
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    pub(super) fn float_sign(&mut self, operand: &Expression<'src>) -> Result<(), Diagnostic> {
        self.float_words(operand)?;
        let shift = match operand.ty.kind {
            Kind::Float(Float::Single) => 31,
            Kind::Float(Float::Double) => 63,
            Kind::LongDouble => 15,
            _ => unreachable!(),
        };
        writeln!(self.text, "    shr rax, {shift}").unwrap();
        Ok(())
    }

    pub(super) fn float_class(
        &mut self,
        operand: &Expression<'src>,
        [nan, infinite, normal, subnormal, zero]: [i32; 5],
    ) -> Result<(), Diagnostic> {
        self.float_words(operand)?;
        let format = FloatFormat::of(&operand.ty.kind, Target::X86_64);
        if format.explicit_integer {
            self.text.push_str("    btr rdx, 63\n");
        } else {
            writeln!(
                self.text,
                "    mov rdx, rax\n    shl rdx, {}",
                64 - format.fraction
            )
            .unwrap();
            writeln!(self.text, "    shr rax, {}", format.fraction).unwrap();
        }
        let max_exponent = 2 * format.bias + 1;
        let extremes = self.label();
        let small = self.label();
        let done = self.label();
        writeln!(self.text, "    and eax, {max_exponent}\n    jz {small}\n    cmp eax, {max_exponent}\n    je {extremes}\n    mov eax, {normal}\n    jmp {done}\n{small}:\n    mov eax, {zero}\n    mov ecx, {subnormal}\n    test rdx, rdx\n    cmovne eax, ecx\n    jmp {done}\n{extremes}:\n    mov eax, {infinite}\n    mov ecx, {nan}\n    test rdx, rdx\n    cmovne eax, ecx\n{done}:\n    movsxd rax, eax").unwrap();
        Ok(())
    }

    pub(super) fn alloca(&mut self, size: &Expression<'src>) -> Result<(), Diagnostic> {
        self.converted(size, &Type::new(Kind::Integer(Integer::UnsignedLong)))?;
        let base = self.alloca_base.as_ref().unwrap().memory(0);
        let align = self.call_alignment;
        // The permanent allocations sit above the current expression
        // stack. Move active spills and outgoing arguments down together,
        // preserving their offsets and the active call alignment.
        writeln!(self.text, "    add rax, {}\n    and rax, -{align}\n    mov rsi, rsp\n    mov rcx, qword {base}\n    sub rcx, rsp\n    sub rsp, rax\n    sub qword {base}, rax\n    mov rdi, rsp\n    rep movsb\n    mov rax, qword {base}", align - 1).unwrap();
        Ok(())
    }
}
