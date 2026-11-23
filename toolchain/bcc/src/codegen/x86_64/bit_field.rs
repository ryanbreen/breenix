//! Bit-field lvalues address the first byte that holds part of the field.

use super::*;

impl<'src> Emitter<'_, 'src> {
    pub(super) fn load_lvalue(&mut self, expr: &Expression<'src>, thread: Option<&ThreadPlace>) {
        let Some(field) = expr.bit_field() else {
            match thread {
                Some(thread) => self.load(&expr.ty, &thread.memory(0)),
                None => self.load(&expr.ty, "[rax]"),
            }
            return;
        };
        if thread.is_none() {
            self.text.push_str("    mov r10, rax\n");
        }
        self.text.push_str("    xor eax, eax\n");
        for (byte, (shift, used, mask)) in field.bytes().enumerate() {
            let memory =
                thread.map_or_else(|| format!("[r10 + {byte}]"), |thread| thread.memory(byte));
            writeln!(self.text, "    movzx edx, byte {memory}").unwrap();
            if shift != 0 {
                writeln!(self.text, "    shr edx, {shift}").unwrap();
            }
            if mask != u8::MAX >> shift {
                writeln!(self.text, "    and edx, {mask}").unwrap();
            }
            if used != 0 {
                writeln!(self.text, "    shl rdx, {used}").unwrap();
            }
            self.text.push_str("    or rax, rdx\n");
        }
        self.normalize_bit_field(field, &expr.ty);
    }

    pub(super) fn store_lvalue(&mut self, expr: &Expression<'src>, thread: Option<&ThreadPlace>) {
        if let Some(field) = expr.bit_field() {
            self.store_bit_field(field, &expr.ty, thread);
        } else if let Some(thread) = thread {
            self.store_object(&expr.ty, &thread.memory(0));
        } else {
            self.store_object(&expr.ty, "[r11]");
        }
    }

    // R11 addresses the destination and RAX carries the converted RHS.
    // Touch only bytes within the field's extent, preserving neighboring
    // fields and regular members that share its first or last byte.
    pub(super) fn store_bit_field(
        &mut self,
        field: BitField,
        ty: &Type,
        thread: Option<&ThreadPlace>,
    ) {
        for (byte, (shift, used, mask)) in field.bytes().enumerate() {
            self.text.push_str("    mov rdx, rax\n");
            if used != 0 {
                writeln!(self.text, "    shr rdx, {used}").unwrap();
            }
            if mask != u8::MAX {
                writeln!(self.text, "    and edx, {mask}").unwrap();
            }
            if shift != 0 {
                writeln!(self.text, "    shl edx, {shift}").unwrap();
            }
            let keep = !(mask << shift);
            let memory =
                thread.map_or_else(|| format!("[r11 + {byte}]"), |thread| thread.memory(byte));
            if keep != 0 {
                writeln!(
                    self.text,
                    "    movzx ecx, byte {memory}\n    and ecx, {keep}\n    or edx, ecx"
                )
                .unwrap();
            }
            writeln!(self.text, "    mov byte {memory}, dl").unwrap();
        }
        // An assignment or prefix update evaluates to the stored value.
        self.normalize_bit_field(field, ty);
    }

    fn normalize_bit_field(&mut self, field: BitField, ty: &Type) {
        let Kind::Integer(integer) = ty.kind else {
            unreachable!("bit-fields have integer types");
        };
        let shift = 64 - field.width;
        if shift != 0 {
            let right = if integer.is_signed(Target::X86_64) {
                "sar"
            } else {
                "shr"
            };
            writeln!(self.text, "    shl rax, {shift}\n    {right} rax, {shift}").unwrap();
        }
    }
}
