//! AAPCS64 2025Q1 §§5.5, 5.10.5, 6.8.2 and 6.9.

use super::{
    require_value, value_prefix, Argument, Diagnostic, Emitter, Expression, ExpressionKind, Float,
    Integer, Kind, Layout, Position, Type, ARGUMENT_REGISTERS, FRAME_RECORD,
};
use crate::types::RecordKind;
use std::fmt::Write;

#[derive(Clone, Copy)]
pub(super) enum ArgumentLocation {
    Register(usize),
    Stack(usize),
}

#[derive(Default)]
pub(super) struct ArgumentSlots {
    pub(super) general: usize,
    pub(super) simd: usize,
    pub(super) stack_bytes: usize,
}

impl ArgumentSlots {
    pub(super) fn next(&mut self, ty: &Type, object: Option<CallObject>) -> ArgumentLocation {
        // B.3–B.6: retain homogeneous objects, replace large ordinary
        // composites with a pointer and round ordinary direct values to words.
        let direct = object.filter(|value| !value.indirect());
        let layout = direct.map_or_else(|| scalar_slot(ty), CallObject::slot);
        let (bank, count, register_align) = match direct {
            Some(CallObject {
                layout,
                class: ObjectClass::Simd(base),
            }) => (&mut self.simd, layout.size / base.bytes(), 1),
            _ if value_prefix(ty) != 'x' => (&mut self.simd, 1, 1),
            _ => (&mut self.general, layout.size / 8, layout.align / 8),
        };
        // C.1–C.6 and C.9–C.17: allocate the whole value in one bank
        // or exhaust that bank and put the whole value on the stack.
        let index = bank.next_multiple_of(register_align);
        let end = index + count;
        if end <= ARGUMENT_REGISTERS {
            *bank = end;
            ArgumentLocation::Register(index)
        } else {
            *bank = ARGUMENT_REGISTERS;
            let offset = self.stack_bytes.next_multiple_of(layout.align);
            self.stack_bytes = offset + layout.size;
            ArgumentLocation::Stack(offset)
        }
    }
}

pub(super) fn scalar_slot(ty: &Type) -> Layout {
    if ty.kind == Kind::LongDouble {
        ty.layout().unwrap()
    } else {
        Layout { size: 8, align: 8 }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CallObject {
    layout: Layout,
    class: ObjectClass,
}

#[derive(Clone, Copy)]
enum ObjectClass {
    General,
    Simd(SimdBase),
    Indirect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SimdBase {
    Float(Float),
    Binary128,
    Vector(usize),
}

impl SimdBase {
    fn bytes(self) -> usize {
        match self {
            Self::Float(float) => float.layout().size,
            Self::Binary128 => 16,
            Self::Vector(bytes) => bytes,
        }
    }

    fn prefix(self) -> char {
        match self {
            Self::Float(float) => super::float::prefix(float),
            Self::Vector(8) => 'd',
            Self::Vector(_) | Self::Binary128 => 'q',
        }
    }
}

impl CallObject {
    pub(super) fn indirect(self) -> bool {
        matches!(self.class, ObjectClass::Indirect)
    }

    pub(super) fn slot(self) -> Layout {
        Layout {
            size: self.layout.size.next_multiple_of(8),
            align: self.layout.align.max(8),
        }
    }

    pub(super) fn simd_parts(self) -> Option<(char, usize, usize)> {
        match self.class {
            ObjectClass::Simd(base) => {
                let bytes = base.bytes();
                Some((base.prefix(), bytes, self.layout.size / bytes))
            }
            _ => None,
        }
    }

    fn register_parts(self) -> (char, usize) {
        match self.class {
            ObjectClass::Simd(base) => (base.prefix(), base.bytes()),
            ObjectClass::General => ('x', 8),
            ObjectClass::Indirect => unreachable!(),
        }
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn call_object(
        &self,
        ty: &Type,
        site: &Position,
    ) -> Result<Option<CallObject>, Diagnostic> {
        if ty.kind != Kind::Void {
            require_value(ty, site)?;
        }
        if !matches!(ty.kind, Kind::Record(_) | Kind::ShortVector { .. }) {
            return Ok(None);
        }
        let layout = ty.layout().unwrap();
        if layout.size == 0 {
            return Err(Diagnostic::new(site, "cannot pass an empty AArch64 record"));
        }
        let class = match self
            .homogeneous_base(ty)
            .filter(|base| layout.size <= 4 * base.bytes())
        {
            Some(base) => ObjectClass::Simd(base),
            None if layout.size > 16 => ObjectClass::Indirect,
            None => ObjectClass::General,
        };
        Ok(Some(CallObject { layout, class }))
    }

    fn homogeneous_base(&self, ty: &Type) -> Option<SimdBase> {
        match &ty.kind {
            Kind::Float(float) => Some(SimdBase::Float(*float)),
            Kind::LongDouble => Some(SimdBase::Binary128),
            // §5.10.5 treats the vector width as the fundamental type.
            Kind::ShortVector { bytes, .. } => Some(SimdBase::Vector(*bytes)),
            Kind::Array(element, _) => self.homogeneous_base(element),
            Kind::Record(tag) => {
                let record = &self.records[tag.id];
                let members = &record.definition.as_ref()?.members;
                let element = self.homogeneous_base(&members.first()?.ty)?;
                let mut span = 0;
                for member in members {
                    let offset = if record.kind == RecordKind::Struct {
                        span
                    } else {
                        0
                    };
                    if member.offset != offset || self.homogeneous_base(&member.ty)? != element {
                        return None;
                    }
                    span = span.max(offset + member.ty.layout()?.size);
                }
                (span == ty.layout()?.size).then_some(element)
            }
            _ => None,
        }
    }

    pub(super) fn receive_parameter(
        &mut self,
        local: usize,
        ty: &Type,
        incoming: &Type,
        object: Option<CallObject>,
        location: ArgumentLocation,
    ) {
        if let Some(object) = object {
            match (location, object.indirect()) {
                (ArgumentLocation::Register(index), false) => {
                    self.local_address(local, 0, "x9");
                    self.save_object_registers(index, object);
                    return;
                }
                (ArgumentLocation::Register(index), true) => {
                    writeln!(self.text, "    mov x0, x{index}").unwrap();
                }
                (ArgumentLocation::Stack(offset), indirect) => {
                    self.offset_address("x0", "x29", (FRAME_RECORD + offset) as i64);
                    if indirect {
                        self.text.push_str("    ldr x0, [x0]\n");
                    }
                }
            }
            self.local_address(local, 0, "x9");
            self.copy_record(object.layout.size);
        } else {
            self.local_address(local, 0, "x10");
            let register = match location {
                ArgumentLocation::Register(index) => index,
                ArgumentLocation::Stack(offset) => {
                    self.offset_address("x9", "x29", (FRAME_RECORD + offset) as i64);
                    self.load(incoming, "x9");
                    0
                }
            };
            // N1570 §§6.5.2.2p6 and 6.9.1p10: assign promoted
            // identifier-list arguments to the declared parameter type.
            match (&incoming.kind, &ty.kind) {
                (Kind::Float(Float::Double), Kind::Float(Float::Single)) => {
                    writeln!(self.text, "    fcvt s{register}, d{register}").unwrap();
                }
                (Kind::Integer(Integer::Int), Kind::Integer(Integer::Bool)) => {
                    writeln!(self.text, "    cmp w{register}, #0\n    cset w{register}, ne").unwrap();
                }
                _ => {}
            }
            self.store_object(ty, register, "x10");
        }
    }

    pub(super) fn return_object(&mut self, object: CallObject, indirect: Option<usize>) {
        if let Some(distance) = indirect {
            self.offset_address("x9", "x29", -(distance as i64));
            self.text.push_str("    ldr x9, [x9]\n");
            self.copy_record(object.layout.size);
        } else if let ObjectClass::Simd(base) = object.class {
            let prefix = base.prefix();
            for (index, offset) in (0..object.layout.size).step_by(base.bytes()).enumerate() {
                writeln!(self.text, "    ldr {prefix}{index}, [x0, #{offset}]").unwrap();
            }
        } else {
            self.text.push_str("    mov x9, x0\n");
            for (index, offset) in (0..object.layout.size).step_by(8).enumerate() {
                let bytes = (object.layout.size - offset).min(8);
                if bytes == 8 {
                    writeln!(self.text, "    ldr x{index}, [x9, #{offset}]").unwrap();
                } else {
                    // C.12 leaves the upper bits unspecified. Load only the
                    // bytes belonging to the record, which may be packed.
                    writeln!(self.text, "    mov x{index}, xzr").unwrap();
                    for byte in 0..bytes {
                        writeln!(
                            self.text,
                            "    ldrb w10, [x9, #{}]\n    orr x{index}, x{index}, x10, lsl #{}",
                            offset + byte,
                            byte * 8
                        )
                        .unwrap();
                    }
                }
            }
        }
    }

    // x9 is the destination. Integer record slots include space for the
    // unused bytes of the last doubleword.
    fn save_object_registers(&mut self, first: usize, object: CallObject) {
        let (prefix, bytes) = object.register_parts();
        for (index, offset) in (0..object.layout.size).step_by(bytes).enumerate() {
            writeln!(
                self.text,
                "    str {prefix}{}, [x9, #{offset}]",
                first + index
            )
            .unwrap();
        }
    }

    pub(super) fn call(
        &mut self,
        callee: &Expression<'src>,
        arguments: &[Argument<'src>],
        result: &Type,
    ) -> Result<(), Diagnostic> {
        let direct = match (&callee.ty.kind, &callee.kind) {
            (Kind::Function { .. }, ExpressionKind::Identifier(name)) => Some(*name),
            _ => None,
        };
        let site = callee.position();
        let object_result = self.call_object(result, site)?;
        let result_slot = object_result
            .map(|object| {
                let slot = if object.indirect() {
                    object.layout
                } else {
                    object.slot()
                };
                self.frame_slot(slot, site)
            })
            .transpose()?;

        let mut slots = ArgumentSlots::default();
        let mut staged = Vec::with_capacity(arguments.len());
        for arg in arguments {
            let object = self.call_object(&arg.ty, arg.expr.position())?;
            staged.push((slots.next(&arg.ty, object), object, 0));
        }
        let mut cursor = slots.stack_bytes;
        for (arg, (location, object, offset)) in arguments.iter().zip(&mut staged) {
            *offset = match location {
                ArgumentLocation::Stack(outgoing) if !object.is_some_and(CallObject::indirect) => {
                    *outgoing
                }
                _ => {
                    let layout = object.map_or_else(
                        || scalar_slot(&arg.ty),
                        |object| {
                            if object.indirect() {
                                object.layout
                            } else {
                                object.slot()
                            }
                        },
                    );
                    let start = cursor.next_multiple_of(layout.align.max(8));
                    cursor = checked_end(start, layout.size, site)?;
                    start
                }
            };
        }
        let callee_slot = cursor.next_multiple_of(8);
        let reserve =
            checked_end(callee_slot, usize::from(direct.is_none()) * 8, site)?.next_multiple_of(16);
        self.adjust_stack("sub", reserve);
        if let Some(name) = direct {
            self.use_symbol(name, site)?;
        } else {
            self.value(callee)?;
            self.transfer_call_slot("str", 'x', 0, callee_slot);
        }

        // Stage values above SP while nested calls can clobber registers.
        for (arg, (location, object, offset)) in arguments.iter().zip(&staged) {
            self.converted(&arg.expr, &arg.ty)?;
            if let Some(object) = object {
                self.offset_address("x9", "sp", *offset as i64);
                self.copy_record(object.layout.size);
                if let ArgumentLocation::Stack(outgoing) = location {
                    if object.indirect() {
                        self.transfer_call_slot("str", 'x', 0, *outgoing);
                    }
                }
            } else {
                self.transfer_call_slot("str", value_prefix(&arg.ty), 0, *offset);
            }
        }
        if let Some(distance) =
            result_slot.filter(|_| object_result.is_some_and(CallObject::indirect))
        {
            self.offset_address("x8", "x29", -(distance as i64));
        }
        for (arg, (location, object, offset)) in arguments.iter().zip(staged) {
            let ArgumentLocation::Register(index) = location else {
                continue;
            };
            if let Some(object) = object {
                if object.indirect() {
                    self.offset_address(&format!("x{index}"), "sp", offset as i64);
                } else {
                    let (prefix, bytes) = object.register_parts();
                    for (part, byte) in (0..object.layout.size).step_by(bytes).enumerate() {
                        self.transfer_call_slot("ldr", prefix, index + part, offset + byte);
                    }
                }
            } else {
                self.transfer_call_slot("ldr", value_prefix(&arg.ty), index, offset);
            }
        }
        if let Some(name) = direct {
            writeln!(self.text, "    bl {name}").unwrap();
        } else {
            self.transfer_call_slot("ldr", 'x', 16, callee_slot);
            self.text.push_str("    blr x16\n");
        }
        self.adjust_stack("add", reserve);
        if let Some((object, distance)) = object_result.zip(result_slot) {
            self.offset_address("x9", "x29", -(distance as i64));
            if !object.indirect() {
                self.save_object_registers(0, object);
            }
            self.text.push_str("    mov x0, x9\n");
        }
        Ok(())
    }

    fn transfer_call_slot(&mut self, op: &str, width: char, register: usize, offset: usize) {
        let scale = match width {
            's' => 4,
            'q' => 16,
            _ => 8,
        };
        if offset <= 4095 * scale {
            writeln!(self.text, "    {op} {width}{register}, [sp, #{offset}]").unwrap();
        } else {
            self.offset_address("x9", "sp", offset as i64);
            writeln!(self.text, "    {op} {width}{register}, [x9]").unwrap();
        }
    }
}

fn checked_end(start: usize, size: usize, site: &Position) -> Result<usize, Diagnostic> {
    start
        .checked_add(size)
        .filter(|&end| end <= i32::MAX as usize - 15)
        .ok_or_else(|| Diagnostic::new(site, "call stack area exceeds the addressable range"))
}
