//! AAPCS64 §§6.8.2 and 6.9 scalar and non-HFA argument/result locations.

use super::{
    require_value, value_prefix, Argument, Diagnostic, Emitter, Expression, ExpressionKind, Kind,
    Layout, Position, Type, ARGUMENT_REGISTERS, FRAME_RECORD,
};
use crate::types::RecordKind;
use std::fmt::Write;

pub(super) enum ArgumentLocation {
    Register(usize),
    Stack(usize),
}

#[derive(Default)]
pub(super) struct ArgumentSlots {
    general: usize,
    float: usize,
    stack_bytes: usize,
}

impl ArgumentSlots {
    pub(super) fn next(&mut self, ty: &Type, record: Option<Layout>) -> ArgumentLocation {
        // B.4 replaces large non-HFAs with a pointer. B.5 rounds the
        // remaining composites to whole doublewords.
        let direct = record.filter(|&layout| !indirect_record(layout));
        let bytes = direct.map_or(8, |layout| layout.size.next_multiple_of(8));
        let align = direct.map_or(8, |layout| layout.align.max(8));
        let next_register = match ty.kind {
            Kind::Float(_) => &mut self.float,
            _ => &mut self.general,
        };
        // C.10, C.12–15: align register pairs as needed and spill a
        // composite as a whole if the register bank cannot hold it.
        let index = next_register.next_multiple_of(align / 8);
        let end = index + bytes / 8;
        if end <= ARGUMENT_REGISTERS {
            *next_register = end;
            ArgumentLocation::Register(index)
        } else {
            *next_register = ARGUMENT_REGISTERS;
            let offset = self.stack_bytes.next_multiple_of(align);
            self.stack_bytes = offset + bytes;
            ArgumentLocation::Stack(offset)
        }
    }
}

pub(super) fn indirect_record(layout: Layout) -> bool {
    layout.size > 16
}

pub(super) fn register_record_slot(layout: Layout) -> Layout {
    Layout {
        size: layout.size.next_multiple_of(8),
        align: layout.align.max(8),
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn call_record(
        &self,
        ty: &Type,
        site: &Position,
    ) -> Result<Option<Layout>, Diagnostic> {
        if ty.kind != Kind::Void {
            require_value(ty, site)?;
        }
        if !matches!(ty.kind, Kind::Record(_)) {
            return Ok(None);
        }
        let layout = ty.layout().unwrap();
        if layout.size == 0 {
            return Err(Diagnostic::new(site, "cannot pass an empty AArch64 record"));
        }
        if self
            .homogeneous_float(ty)
            .is_some_and(|(element, size)| size <= 4 * element)
        {
            return Err(Diagnostic::new(
                site,
                "AArch64 homogeneous floating-point aggregate calls are deferred to A4",
            ));
        }
        Ok(Some(layout))
    }

    fn homogeneous_float(&self, ty: &Type) -> Option<(usize, usize)> {
        let size = ty.layout()?.size;
        match &ty.kind {
            Kind::Float(_) | Kind::LongDouble => Some((size, size)),
            Kind::Array(element, _) => Some((self.homogeneous_float(element)?.0, size)),
            Kind::Record(tag) => {
                let record = &self.records[tag.id];
                let members = &record.definition.as_ref()?.members;
                let element = self.homogeneous_float(&members.first()?.ty)?.0;
                let mut span = 0;
                for member in members {
                    let (base, bytes) = self.homogeneous_float(&member.ty)?;
                    let offset = if record.kind == RecordKind::Struct { span } else { 0 };
                    if base != element || member.offset != offset {
                        return None;
                    }
                    span = span.max(offset + bytes);
                }
                (span == size).then_some((element, size))
            }
            _ => None,
        }
    }

    pub(super) fn receive_parameter(
        &mut self,
        local: usize,
        ty: &Type,
        record: Option<Layout>,
        location: ArgumentLocation,
    ) {
        if let Some(layout) = record {
            match (location, indirect_record(layout)) {
                (ArgumentLocation::Register(index), false) => {
                    self.local_address(local, 0, "x9");
                    self.save_record_registers(index, layout.size, "x9");
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
            self.copy_record(layout.size);
        } else {
            self.local_address(local, 0, "x10");
            let register = match location {
                ArgumentLocation::Register(index) => index,
                ArgumentLocation::Stack(offset) => {
                    self.offset_address("x9", "x29", (FRAME_RECORD + offset) as i64);
                    self.load(ty, "x9");
                    0
                }
            };
            self.store_object(ty, register, "x10");
        }
    }

    pub(super) fn return_record(&mut self, layout: Layout, indirect: Option<usize>) {
        if let Some(distance) = indirect {
            self.offset_address("x9", "x29", -(distance as i64));
            self.text.push_str("    ldr x9, [x9]\n");
            self.copy_record(layout.size);
        } else {
            self.text.push_str("    mov x9, x0\n");
            for (index, offset) in (0..layout.size).step_by(8).enumerate() {
                let bytes = (layout.size - offset).min(8);
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

    fn save_record_registers(&mut self, first: usize, size: usize, address: &str) {
        for (index, offset) in (0..size).step_by(8).enumerate() {
            writeln!(
                self.text,
                "    str x{}, [{address}, #{offset}]",
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
        let record_result = self.call_record(result, site)?;
        let result_slot = record_result
            .map(|layout| {
                let slot = if indirect_record(layout) {
                    layout
                } else {
                    register_record_slot(layout)
                };
                self.frame_slot(slot, site)
            })
            .transpose()?;

        let mut slots = ArgumentSlots::default();
        let mut staged = Vec::with_capacity(arguments.len());
        for arg in arguments {
            let record = self.call_record(&arg.ty, arg.expr.position())?;
            staged.push((slots.next(&arg.ty, record), record, 0));
        }
        let mut cursor = slots.stack_bytes;
        for (location, record, offset) in &mut staged {
            *offset = match location {
                ArgumentLocation::Stack(outgoing) if !record.is_some_and(indirect_record) => {
                    *outgoing
                }
                _ => {
                    let layout = record.map_or(Layout { size: 8, align: 8 }, |layout| {
                        if indirect_record(layout) {
                            layout
                        } else {
                            register_record_slot(layout)
                        }
                    });
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
        for (arg, (location, record, offset)) in arguments.iter().zip(&staged) {
            self.converted(&arg.expr, &arg.ty)?;
            if let Some(layout) = record {
                self.offset_address("x9", "sp", *offset as i64);
                self.copy_record(layout.size);
                if let ArgumentLocation::Stack(outgoing) = location {
                    if indirect_record(*layout) {
                        self.transfer_call_slot("str", 'x', 0, *outgoing);
                    }
                }
            } else {
                self.transfer_call_slot("str", value_prefix(&arg.ty), 0, *offset);
            }
        }
        if let Some(distance) = result_slot.filter(|_| record_result.is_some_and(indirect_record)) {
            self.offset_address("x8", "x29", -(distance as i64));
        }
        for (arg, (location, record, offset)) in arguments.iter().zip(staged) {
            let ArgumentLocation::Register(index) = location else {
                continue;
            };
            if let Some(layout) = record {
                if indirect_record(layout) {
                    self.offset_address(&format!("x{index}"), "sp", offset as i64);
                } else {
                    for (part, byte) in (0..layout.size).step_by(8).enumerate() {
                        self.transfer_call_slot("ldr", 'x', index + part, offset + byte);
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
        if let Some((layout, distance)) = record_result.zip(result_slot) {
            self.offset_address("x9", "x29", -(distance as i64));
            if !indirect_record(layout) {
                self.save_record_registers(0, layout.size, "x9");
            }
            self.text.push_str("    mov x0, x9\n");
        }
        Ok(())
    }

    fn transfer_call_slot(&mut self, op: &str, width: char, register: usize, offset: usize) {
        let scale = if width == 's' { 4 } else { 8 };
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
