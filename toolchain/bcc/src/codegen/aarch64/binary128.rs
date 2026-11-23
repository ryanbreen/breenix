//! IEEE 754-2019 §3.4 and AAPCS64 2025Q1 §§5.5, 6.8 and 6.9.
//!
//! C values use Q registers. brt accepts their encodings as AAPCS64
//! unsigned 128-bit integers in consecutive GPRs, low half first.

use super::{flow::symbol_address, Binary, Emitter, Float, Kind, Target};
use std::fmt::Write;

const ONE: u128 = 0x3fff_u128 << 112;

impl Emitter<'_, '_> {
    pub(super) fn binary128_constant(&mut self, bits: u128) {
        let label = self.label();
        writeln!(self.rodata, "    .balign 16\n{label}:").unwrap();
        write_bits(&mut self.rodata, bits);
        symbol_address(&mut self.text, "x9", &label);
        self.text.push_str("    ldr q0, [x9]\n");
    }

    pub(super) fn negate_binary128(&mut self) {
        // IEEE 754-2019 §5.5.1 defines negate as a sign-bit operation.
        self.text.push_str(
            "    fmov x1, v0.d[1]\n    eor x1, x1, #0x8000000000000000\n    fmov v0.d[1], x1\n",
        );
    }

    pub(super) fn binary128_binary(&mut self, op: Binary) {
        use Binary::*;
        self.text
            .push_str("    fmov x2, d0\n    fmov x3, v0.d[1]\n    ldr q0, [sp], #16\n");
        self.binary128_argument();
        let helper = match op {
            Add => "__brt_tf_add",
            Subtract => "__brt_tf_sub",
            Multiply => "__brt_tf_mul",
            Divide => "__brt_tf_div",
            _ => {
                self.binary128_call("__brt_tf_cmp");
                // The low word of brt's result is -1, 0, 1 or 2 (unordered).
                // Ordered comparisons must exclude 2, including >=.
                let (constant, condition) = match op {
                    Equal => (0, "eq"),
                    NotEqual => (0, "ne"),
                    Less => (0, "lt"),
                    LessEqual => (0, "le"),
                    Greater => (1, "eq"),
                    GreaterEqual => (1, "ls"),
                    _ => unreachable!(),
                };
                writeln!(
                    self.text,
                    "    cmp x0, #{constant}\n    cset w0, {condition}"
                )
                .unwrap();
                return;
            }
        };
        self.binary128_call(helper);
        self.binary128_result();
    }

    pub(super) fn binary128_update(&mut self, increment: bool, postfix: bool) {
        self.text.push_str("    str x9, [sp, #-16]!\n");
        if postfix {
            self.push_value('q');
        }
        self.binary128_step(increment);
        let address_offset = if postfix { 16 } else { 0 };
        writeln!(
            self.text,
            "    ldr x9, [sp, #{address_offset}]\n    str q0, [x9]"
        )
        .unwrap();
        if postfix {
            self.text.push_str("    ldr q0, [sp], #16\n");
        }
        self.adjust_stack("add", 16);
    }

    pub(super) fn binary128_step(&mut self, increment: bool) {
        self.push_value('q');
        self.binary128_constant(ONE);
        self.binary128_binary(if increment {
            Binary::Add
        } else {
            Binary::Subtract
        });
    }

    pub(super) fn compare_binary128_zero(&mut self) {
        self.binary128_argument();
        self.text.push_str("    mov x2, xzr\n    mov x3, xzr\n");
        self.binary128_call("__brt_tf_cmp");
        self.text.push_str("    cmp x0, #0\n");
    }

    pub(super) fn widen_binary128(&mut self, scalar: &Kind) {
        let (widen, _) = conversion_helpers(scalar);
        match scalar {
            Kind::Float(Float::Single) => self.text.push_str("    fmov w0, s0\n"),
            Kind::Float(Float::Double) => self.text.push_str("    fmov x0, d0\n"),
            _ => {}
        }
        self.binary128_call(widen);
        self.binary128_result();
    }

    pub(super) fn narrow_binary128(&mut self, scalar: &Kind) {
        let (_, narrow) = conversion_helpers(scalar);
        self.binary128_argument();
        self.binary128_call(narrow);
        match scalar {
            Kind::Float(Float::Single) => self.text.push_str("    fmov s0, w0\n"),
            Kind::Float(Float::Double) => self.text.push_str("    fmov d0, x0\n"),
            _ => {}
        }
    }

    fn binary128_argument(&mut self) {
        self.text
            .push_str("    fmov x0, d0\n    fmov x1, v0.d[1]\n");
    }

    fn binary128_result(&mut self) {
        self.text
            .push_str("    fmov d0, x0\n    fmov v0.d[1], x1\n");
    }

    fn binary128_call(&mut self, helper: &'static str) {
        self.externs.insert(helper);
        writeln!(self.text, "    bl {helper}").unwrap();
    }
}

fn conversion_helpers(scalar: &Kind) -> (&'static str, &'static str) {
    match scalar {
        Kind::Float(Float::Single) => ("__brt_tf_from_f32", "__brt_tf_to_f32"),
        Kind::Float(Float::Double) => ("__brt_tf_from_f64", "__brt_tf_to_f64"),
        Kind::Integer(integer) => {
            match (
                integer.is_signed(Target::Aarch64),
                integer.layout().size > 4,
            ) {
                (true, true) => ("__brt_tf_from_i64", "__brt_tf_to_i64"),
                (true, false) => ("__brt_tf_from_i32", "__brt_tf_to_i32"),
                (false, true) => ("__brt_tf_from_u64", "__brt_tf_to_u64"),
                (false, false) => ("__brt_tf_from_u32", "__brt_tf_to_u32"),
            }
        }
        _ => unreachable!(),
    }
}

fn write_bits(out: &mut String, bits: u128) {
    writeln!(
        out,
        "    .quad 0x{:x}\n    .quad 0x{:x}",
        bits as u64,
        bits >> 64
    )
    .unwrap();
}
