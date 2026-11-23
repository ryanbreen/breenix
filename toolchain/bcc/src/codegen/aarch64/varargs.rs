//! AAPCS64 2025Q1 §§6.8.2 and 14.1–14.4, little-endian LP64.

use super::{
    abi::scalar_slot, value_prefix, ArgumentSlots, CallObject, Diagnostic, Emitter, Expression,
    Layout, Position, Type, ARGUMENT_REGISTERS, FRAME_RECORD,
};
use crate::types::aarch64_va_list::{GR_OFFS, GR_TOP, LAYOUT, STACK, VR_OFFS, VR_TOP};
use std::fmt::Write;

const GP_STRIDE: usize = 8;
const SIMD_STRIDE: usize = 16;
const GP_BYTES: usize = ARGUMENT_REGISTERS * GP_STRIDE;
const SAVE_BYTES: usize = GP_BYTES + ARGUMENT_REGISTERS * SIMD_STRIDE;

pub(super) struct VariadicFrame {
    // The 16-byte-aligned GP area starts here, followed by the SIMD area.
    distance: usize,
    named: ArgumentSlots,
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn save_variadic_arguments(
        &mut self,
        named: ArgumentSlots,
        site: &Position,
    ) -> Result<VariadicFrame, Diagnostic> {
        let distance = self.frame_slot(
            Layout {
                size: SAVE_BYTES,
                align: SIMD_STRIDE,
            },
            site,
        )?;
        self.offset_address("x9", "x29", -(distance as i64));
        for index in 0..ARGUMENT_REGISTERS {
            let gp_offset = index * GP_STRIDE;
            let simd_offset = GP_BYTES + index * SIMD_STRIDE;
            writeln!(
                self.text,
                "    str x{index}, [x9, #{gp_offset}]\n    str q{index}, [x9, #{simd_offset}]"
            )
            .unwrap();
        }
        Ok(VariadicFrame { distance, named })
    }

    pub(super) fn va_start(&mut self, list: &Expression<'src>) -> Result<(), Diagnostic> {
        self.value(list)?;
        let frame = self
            .variadic
            .as_ref()
            .expect("sema restricts va_start to variadic functions");
        let incoming_stack = FRAME_RECORD + frame.named.stack_bytes;
        let gp_top = frame.distance - GP_BYTES;
        let simd_top = frame.distance - SAVE_BYTES;
        let gp_remaining = (ARGUMENT_REGISTERS - frame.named.general) * GP_STRIDE;
        let simd_remaining = (ARGUMENT_REGISTERS - frame.named.simd) * SIMD_STRIDE;

        for (field, from_fp) in [
            (STACK, incoming_stack as i64),
            (GR_TOP, -(gp_top as i64)),
            (VR_TOP, -(simd_top as i64)),
        ] {
            self.offset_address("x9", "x29", from_fp);
            writeln!(self.text, "    str x9, [x0, #{field}]").unwrap();
        }
        for (field, remaining) in [(GR_OFFS, gp_remaining), (VR_OFFS, simd_remaining)] {
            writeln!(
                self.text,
                "    movz w9, #{remaining}\n    sub w9, wzr, w9\n    str w9, [x0, #{field}]"
            )
            .unwrap();
        }
        Ok(())
    }

    pub(super) fn va_arg(
        &mut self,
        list: &Expression<'src>,
        ty: &Type,
        site: &Position,
    ) -> Result<(), Diagnostic> {
        let object = self.call_object(ty, site)?;
        let indirect = object.is_some_and(CallObject::indirect);
        // §6.8.2 B.4 replaces an ordinary large composite with a pointer
        // before assigning a register or stack slot.
        let direct = object.filter(|value| !value.indirect());
        let slot = direct.map_or_else(|| scalar_slot(ty), CallObject::slot);
        let simd_parts = direct.and_then(CallObject::simd_parts);
        let in_simd = simd_parts.is_some() || value_prefix(ty) != 'x';
        let (top_field, offset_field, consumed) = if in_simd {
            let count = simd_parts.map_or(1, |(_, _, count)| count);
            (VR_TOP, VR_OFFS, count * SIMD_STRIDE)
        } else {
            (GR_TOP, GR_OFFS, slot.size)
        };

        self.value(list)?;
        let on_stack = self.label();
        let ready = self.label();
        writeln!(
            self.text,
            "    ldr w10, [x0, #{offset_field}]\n    cmp w10, #0\n    b.ge {on_stack}"
        )
        .unwrap();
        if !in_simd && slot.align > GP_STRIDE {
            self.text
                .push_str("    add w10, w10, #15\n    and w10, w10, #0xfffffff0\n");
        }
        writeln!(
            self.text,
            "    add w11, w10, #{consumed}\n    str w11, [x0, #{offset_field}]\n    cmp w11, #0\n    b.gt {on_stack}\n    ldr x9, [x0, #{top_field}]\n    sxtw x10, w10\n    add x9, x9, x10"
        )
        .unwrap();

        // The members of a homogeneous composite are spread across
        // 16-byte register slots, but adjacent in the C object.
        if let Some((prefix, bytes, count)) = simd_parts.filter(|&(_, _, count)| count > 1) {
            let buffer = self.frame_slot(
                Layout {
                    size: slot.size,
                    align: slot.align.max(bytes),
                },
                site,
            )?;
            self.offset_address("x11", "x29", -(buffer as i64));
            for index in 0..count {
                let register_offset = index * SIMD_STRIDE;
                let member_offset = index * bytes;
                writeln!(
                    self.text,
                    "    ldr {prefix}0, [x9, #{register_offset}]\n    str {prefix}0, [x11, #{member_offset}]"
                )
                .unwrap();
            }
            self.text.push_str("    mov x9, x11\n");
        }

        writeln!(
            self.text,
            "    b {ready}\n{on_stack}:\n    ldr x9, [x0, #{STACK}]"
        )
        .unwrap();
        if slot.align > GP_STRIDE {
            self.text
                .push_str("    add x9, x9, #15\n    and x9, x9, #0xfffffffffffffff0\n");
        }
        self.offset_address("x10", "x9", slot.size as i64);
        writeln!(self.text, "    str x10, [x0, #{STACK}]\n{ready}:").unwrap();
        if indirect {
            self.text.push_str("    ldr x9, [x9]\n");
        }
        self.load(ty, "x9");
        Ok(())
    }

    pub(super) fn va_copy(
        &mut self,
        dest: &Expression<'src>,
        source: &Expression<'src>,
    ) -> Result<(), Diagnostic> {
        self.value(dest)?;
        self.push_value('x');
        self.value(source)?;
        self.text.push_str("    ldr x9, [sp], #16\n");
        self.copy_record(LAYOUT.size);
        Ok(())
    }
}
