//! Floating representation queries for AArch64.

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
}
