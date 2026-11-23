//! AAPCS64 §6.8.2 scalar argument locations.

use super::ARGUMENT_REGISTERS;
use super::{value_prefix, Argument, Diagnostic, Emitter, Expression, ExpressionKind, Kind, Type};
use std::fmt::Write;

pub(super) enum ArgumentLocation {
    Register { index: usize, scratch_offset: usize },
    Stack(usize),
}

#[derive(Default)]
pub(super) struct ArgumentSlots {
    general: usize,
    float: usize,
    stack_bytes: usize,
}

impl ArgumentSlots {
    pub(super) fn next(&mut self, ty: &Type) -> ArgumentLocation {
        let scratch_offset = (self.general + self.float) * 8;
        let next_register = match ty.kind {
            Kind::Float(_) => &mut self.float,
            _ => &mut self.general,
        };
        // C.1 and C.9 allocate from independent register banks. C.5–6
        // and C.14–17 give these little-endian scalars eight-byte slots.
        if *next_register < ARGUMENT_REGISTERS {
            let index = *next_register;
            *next_register += 1;
            ArgumentLocation::Register {
                index,
                scratch_offset,
            }
        } else {
            let offset = self.stack_bytes;
            self.stack_bytes += 8;
            ArgumentLocation::Stack(offset)
        }
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn call(
        &mut self,
        callee: &Expression<'src>,
        arguments: &[Argument<'src>],
    ) -> Result<(), Diagnostic> {
        let direct = match (&callee.ty.kind, &callee.kind) {
            (Kind::Function { .. }, ExpressionKind::Identifier(name)) => Some(*name),
            _ => None,
        };
        let mut slots = ArgumentSlots::default();
        let locations: Vec<_> = arguments.iter().map(|arg| slots.next(&arg.ty)).collect();
        let callee_slot = arguments.len() * 8;
        let reserve = (callee_slot + usize::from(direct.is_none()) * 8).next_multiple_of(16);
        self.adjust_stack("sub", reserve);
        if let Some(name) = direct {
            self.use_symbol(name, callee.position())?;
        } else {
            self.value(callee)?;
            self.transfer_call_slot("str", 'x', 0, callee_slot);
        }

        // Sema supplies each argument's converted type, including default
        // promotions for anonymous arguments. Stage the register values
        // above the outgoing stack area while nested calls can clobber them.
        for (arg, location) in arguments.iter().zip(&locations) {
            self.converted(&arg.expr, &arg.ty)?;
            let offset = match location {
                ArgumentLocation::Register { scratch_offset, .. } => {
                    slots.stack_bytes + scratch_offset
                }
                ArgumentLocation::Stack(offset) => *offset,
            };
            self.transfer_call_slot("str", value_prefix(&arg.ty), 0, offset);
        }
        for (arg, location) in arguments.iter().zip(locations) {
            if let ArgumentLocation::Register {
                index,
                scratch_offset,
            } = location
            {
                self.transfer_call_slot(
                    "ldr",
                    value_prefix(&arg.ty),
                    index,
                    slots.stack_bytes + scratch_offset,
                );
            }
        }
        if let Some(name) = direct {
            writeln!(self.text, "    bl {name}").unwrap();
        } else {
            self.transfer_call_slot("ldr", 'x', 16, callee_slot);
            self.text.push_str("    blr x16\n");
        }
        self.adjust_stack("add", reserve);
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
