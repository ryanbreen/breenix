//! Floating representation queries and stack allocation for AArch64.

use super::*;
use crate::types::FloatFormat;

impl<'src> Emitter<'_, 'src> {
    // Place the high word (containing sign and exponent) in X0.
    fn float_words(&mut self, operand: &Expression<'src>) -> Result<(), Diagnostic> {
        self.value(operand)?;
        self.text.push_str(match operand.ty.kind {
            Kind::Float(Float::Single) => "    fmov w0, s0\n",
            Kind::Float(Float::Double) => "    fmov x0, d0\n",
            Kind::LongDouble => "    fmov x1, d0\n    fmov x0, v0.d[1]\n",
            _ => unreachable!(),
        });
        Ok(())
    }

    pub(super) fn float_sign(&mut self, operand: &Expression<'src>) -> Result<(), Diagnostic> {
        self.float_words(operand)?;
        let shift = if operand.ty.kind == Kind::Float(Float::Single) {
            31
        } else {
            63
        };
        writeln!(self.text, "    lsr x0, x0, #{shift}").unwrap();
        Ok(())
    }

    pub(super) fn float_class(
        &mut self,
        operand: &Expression<'src>,
        [nan, infinite, normal, subnormal, zero]: [i32; 5],
    ) -> Result<(), Diagnostic> {
        self.float_words(operand)?;
        let format = FloatFormat::of(&operand.ty.kind, Target::Aarch64);
        let shift = format.fraction % 64;
        writeln!(self.text, "    lsl x2, x0, #{}", 64 - shift).unwrap();
        if operand.ty.kind == Kind::LongDouble {
            self.text.push_str("    orr x2, x2, x1\n");
        }
        let max_exponent = 2 * format.bias + 1;
        let extremes = self.label();
        let small = self.label();
        let done = self.label();
        writeln!(
            self.text,
            "    lsr x0, x0, #{shift}\n    and x0, x0, #{max_exponent}\n    cbz x0, {small}"
        )
        .unwrap();
        constant(&mut self.text, "x1", max_exponent as u64);
        writeln!(self.text, "    cmp x0, x1\n    b.eq {extremes}").unwrap();
        constant(&mut self.text, "x0", normal as i64 as u64);
        writeln!(self.text, "    b {done}\n{small}:").unwrap();
        self.select_class(zero, subnormal);
        writeln!(self.text, "    b {done}\n{extremes}:").unwrap();
        self.select_class(infinite, nan);
        writeln!(self.text, "{done}:").unwrap();
        Ok(())
    }

    fn select_class(&mut self, without_fraction: i32, with_fraction: i32) {
        constant(&mut self.text, "x0", without_fraction as i64 as u64);
        constant(&mut self.text, "x1", with_fraction as i64 as u64);
        self.text
            .push_str("    cmp x2, #0\n    csel x0, x0, x1, eq\n");
    }

    pub(super) fn alloca(&mut self, size: &Expression<'src>) -> Result<(), Diagnostic> {
        self.converted(size, &Type::new(Kind::Integer(Integer::UnsignedLong)))?;
        let distance = self.alloca_base.unwrap();
        self.offset_address("x9", "x29", -(distance as i64));
        let copy = self.label();
        let done = self.label();
        // AAPCS64 §6.4.5: move SP before writing to the new stack area,
        // and keep it 16-byte aligned. Copy the live spills and outgoing
        // arguments forward to handle overlap. Existing allocations stay
        // above the old boundary, with the new one immediately below it.
        let align_mask = !((STACK_ALIGNMENT - 1) as u64);
        writeln!(
            self.text,
            "    add x0, x0, #{}\n    and x0, x0, #{align_mask:#x}\n    mov x10, sp\n    sub sp, sp, x0\n    mov x11, sp\n    ldr x12, [x9]\n    cmp x10, x12\n    b.eq {done}\n{copy}:\n    ldp x13, x14, [x10], #16\n    stp x13, x14, [x11], #16\n    cmp x10, x12\n    b.ne {copy}\n{done}:\n    sub x0, x12, x0\n    str x0, [x9]",
            STACK_ALIGNMENT - 1,
        )
        .unwrap();
        Ok(())
    }
}
