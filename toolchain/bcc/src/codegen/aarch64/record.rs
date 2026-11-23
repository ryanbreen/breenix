//! AAPCS64 §§5.10 and 10.1.8 record objects and little-endian bit-fields.
//! A64 byte transfers and bit-field instructions follow DDI 0487 M.b,
//! §§C6.2.39, C6.2.219, C6.2.354, C6.2.416 and C6.2.486.

use super::{constant, Emitter, Kind, Target, Type};
use crate::types::{BitField, BitFieldContainer};
use std::fmt::Write;

impl Emitter<'_, '_> {
    // Record expressions carry an address in x0. Copy the representation to
    // x9 and return that address, including for an assignment used as a value.
    pub(super) fn copy_record(&mut self, size: usize) {
        if size != 0 {
            let copy = self.label();
            constant(&mut self.text, "x10", size as u64);
            writeln!(
                self.text,
                "    mov x13, x9\n{copy}:\n    ldrb w11, [x0], #1\n    strb w11, [x13], #1\n    subs x10, x10, #1\n    b.ne {copy}"
            )
            .unwrap();
        }
        self.text.push_str("    mov x0, x9\n");
    }

    pub(super) fn load_value(&mut self, ty: &Type, field: Option<BitField>, address: &str) {
        if let Some(field) = field {
            if address != "x9" {
                writeln!(self.text, "    mov x9, {address}").unwrap();
            }
            self.load_bit_field(ty, field);
        } else {
            self.load(ty, address);
        }
    }

    pub(super) fn store_value(&mut self, ty: &Type, field: Option<BitField>) {
        if let Some(field) = field {
            self.store_bit_field(ty, field);
        } else if let Kind::Record(record) = ty.kind {
            self.copy_record(record.layout.unwrap().size);
        } else {
            self.store_object(ty, 0, "x9");
        }
    }

    fn load_bit_field(&mut self, ty: &Type, field: BitField) {
        if let Some(container) = volatile_container(ty, field) {
            let (load, _, register) = container_transfer(container.bytes);
            self.offset_address("x13", "x9", -i64::from(container.back));
            writeln!(self.text, "    {load} {register}11, [x13]").unwrap();
            let shift = container.back * 8 + field.shift;
            self.extract_field(ty, "x11", shift, field.width);
            return;
        }

        self.text.push_str("    mov x0, xzr\n");
        for (offset, (shift, used, mask)) in field.bytes().enumerate() {
            let width = mask.count_ones();
            writeln!(
                self.text,
                "    ldrb w11, [x9, #{offset}]\n    ubfx x11, x11, #{shift}, #{width}\n    orr x0, x0, x11, lsl #{used}"
            )
            .unwrap();
        }
        self.extract_field(ty, "x0", 0, field.width);
    }

    fn store_bit_field(&mut self, ty: &Type, field: BitField) {
        if let Some(container) = volatile_container(ty, field) {
            let (load, store, register) = container_transfer(container.bytes);
            let rotate = (64 - (container.back * 8 + field.shift)) % 64;
            let last = field.width - 1;
            self.offset_address("x13", "x9", -i64::from(container.back));
            writeln!(
                self.text,
                "    {load} {register}11, [x13]\n    bfm x11, x0, #{rotate}, #{last}\n    {store} {register}11, [x13]"
            )
            .unwrap();
        } else {
            // In particular a packed field can occupy pieces of nine bytes.
            // Modify the occupied bits without touching adjacent objects.
            for (offset, (shift, used, mask)) in field.bytes().enumerate() {
                let rotate = (32 - shift) % 32;
                let last = mask.count_ones() - 1;
                writeln!(
                    self.text,
                    "    ldrb w11, [x9, #{offset}]\n    lsr x13, x0, #{used}\n    bfm w11, w13, #{rotate}, #{last}\n    strb w11, [x9, #{offset}]"
                )
                .unwrap();
            }
        }
        // The assignment or prefix update yields the value that fit the field.
        self.extract_field(ty, "x0", 0, field.width);
    }

    fn extract_field(&mut self, ty: &Type, register: &str, shift: u8, width: u8) {
        let Kind::Integer(integer) = ty.kind else {
            unreachable!("sema must supply an integer bit-field type")
        };
        if shift != 0 || width != 64 {
            let op = if integer.is_signed(Target::Aarch64) {
                "sbfx"
            } else {
                "ubfx"
            };
            writeln!(self.text, "    {op} x0, {register}, #{shift}, #{width}").unwrap();
        } else if register != "x0" {
            writeln!(self.text, "    mov x0, {register}").unwrap();
        }
    }
}

fn volatile_container(ty: &Type, field: BitField) -> Option<BitFieldContainer> {
    field.container.filter(|_| ty.qualifiers.volatile)
}

fn container_transfer(bytes: u8) -> (&'static str, &'static str, char) {
    match bytes {
        1 => ("ldrb", "strb", 'w'),
        2 => ("ldrh", "strh", 'w'),
        4 => ("ldr", "str", 'w'),
        8 => ("ldr", "str", 'x'),
        _ => unreachable!("bit-field container must have an integer size"),
    }
}
