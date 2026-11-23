//! Scalar IEEE binary32 and binary64 lowering with A64 FP instructions.

use std::fmt::Write;

use super::{constant, Binary, Emitter, Float, Kind, Type};

pub(super) fn prefix(precision: Float) -> char {
    match precision {
        Float::Single => 's',
        Float::Double => 'd',
    }
}

impl Emitter<'_, '_> {
    pub(super) fn float_constant(&mut self, ty: &Type, bits: u64) {
        let Kind::Float(precision) = ty.kind else {
            unreachable!()
        };
        // Sema already rounded the literal to the target representation.
        let integer = match precision {
            Float::Single => "w0",
            Float::Double => "x0",
        };
        constant(&mut self.text, integer, bits);
        writeln!(self.text, "    fmov {}0, {integer}", prefix(precision)).unwrap();
    }

    pub(super) fn float_binary(&mut self, op: Binary, precision: Float) {
        use Binary::*;
        let fp = prefix(precision);
        writeln!(self.text, "    ldr {fp}1, [sp], #16").unwrap();
        let instruction = match op {
            Add => "fadd",
            Subtract => "fsub",
            Multiply => "fmul",
            Divide => "fdiv",
            _ => {
                // FCMP sets NZCV to 0011 for unordered operands (Arm DDI
                // 0487 M.b §C7.2.70). Exclude that case for ordered predicates.
                let condition = match op {
                    Equal => "eq",
                    NotEqual => "ne",
                    Less => "mi",
                    LessEqual => "ls",
                    Greater => "gt",
                    GreaterEqual => "ge",
                    _ => unreachable!(),
                };
                writeln!(self.text, "    fcmp {fp}1, {fp}0\n    cset w0, {condition}").unwrap();
                return;
            }
        };
        writeln!(self.text, "    {instruction} {fp}0, {fp}1, {fp}0").unwrap();
    }

    pub(super) fn float_update(&mut self, precision: Float, increment: bool, postfix: bool) {
        let fp = prefix(precision);
        if postfix {
            writeln!(self.text, "    fmov {fp}2, {fp}0").unwrap();
        }
        let op = if increment { "fadd" } else { "fsub" };
        writeln!(
            self.text,
            "    fmov {fp}1, #1.0\n    {op} {fp}0, {fp}0, {fp}1\n    str {fp}0, [x9]"
        )
        .unwrap();
        if postfix {
            writeln!(self.text, "    fmov {fp}0, {fp}2").unwrap();
        }
    }
}
