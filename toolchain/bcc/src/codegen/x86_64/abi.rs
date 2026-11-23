//! AMD64 psABI §3.2.3 classification and register allocation.

use crate::pp::{Diagnostic, Position};
use crate::types::{Float, Kind, Record, Type};

pub(super) const INTEGER_REGS: [&str; 6] = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];
const INTEGER_RESULTS: [&str; 2] = ["rax", "rdx"];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Class {
    Empty,
    Sse,
    Integer,
}

#[derive(Clone, Copy)]
pub(super) enum Register {
    Integer(&'static str),
    Sse(usize),
}

pub(super) type RecordRegisters = [Option<Register>; 2];

#[derive(Clone, Copy)]
pub(super) enum RecordAbi {
    Registers([Class; 2]),
    Memory,
}

impl RecordAbi {
    pub(super) fn classify(
        ty: &Type,
        records: &[Record<'_>],
        site: &Position,
    ) -> Result<Self, Diagnostic> {
        let layout = ty.layout().unwrap();
        // After post-merger cleanup, only SSEUP can extend a register
        // aggregate beyond two eightbytes. SSEUP is outside the supported types.
        if layout.size > 16 {
            return Ok(Self::Memory);
        }
        let mut classes = [Class::Empty; 2];
        if classify_fields(ty, 0, records, &mut classes, site)? {
            Ok(Self::Registers(classes))
        } else {
            Ok(Self::Memory)
        }
    }

    pub(super) fn result_registers(self) -> Option<RecordRegisters> {
        match self {
            Self::Registers(classes) => {
                let mut layout = CallLayout::default();
                layout.allocate(classes, &INTEGER_RESULTS, 2)
            }
            Self::Memory => None,
        }
    }
}

fn classify_fields(
    ty: &Type,
    offset: usize,
    records: &[Record<'_>],
    classes: &mut [Class; 2],
    site: &Position,
) -> Result<bool, Diagnostic> {
    let layout = ty.layout().unwrap();
    if !offset.is_multiple_of(layout.align) {
        return Ok(false);
    }
    let class = match &ty.kind {
        Kind::Record(record) => {
            let definition = records[record.id].definition.as_ref().unwrap();
            for member in &definition.members {
                let start = offset + member.offset;
                if let Some(field) = member.bit_field {
                    // A bit-field contributes INTEGER to each eightbyte it
                    // occupies. Its offset is measured in bits, not aligned as
                    // if a separate scalar object lived at its first byte.
                    let first_bit = start * 8 + usize::from(field.shift);
                    let last_bit = first_bit + usize::from(field.width) - 1;
                    classes[first_bit / 64..=last_bit / 64].fill(Class::Integer);
                } else if !classify_fields(&member.ty, start, records, classes, site)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Kind::Array(element, count) => {
            let stride = element.layout().unwrap().size;
            for index in 0..*count {
                if !classify_fields(element, offset + index * stride, records, classes, site)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Kind::Integer(_) | Kind::Pointer(_) => Class::Integer,
        Kind::Float(_) => Class::Sse,
        Kind::LongDouble => {
            return Err(Diagnostic::new(
                site,
                "x87 aggregate ABI is deferred to 25.7b3",
            ));
        }
        Kind::Void | Kind::Function { .. } => unreachable!(),
    };
    // NO_CLASS gives way to either leaf class, and INTEGER dominates SSE
    // when fields share an eightbyte (including overlapping union members).
    classes[offset / 8] = classes[offset / 8].max(class);
    Ok(true)
}

#[derive(Clone, Copy)]
pub(super) enum Place {
    Integer(&'static str),
    Vector(usize, Float),
    Record(RecordRegisters),
    Stack(usize),
}

#[derive(Default)]
pub(super) struct CallLayout {
    pub(super) integers: usize,
    pub(super) vectors: usize,
    pub(super) stack: usize,
}

impl CallLayout {
    pub(super) fn place(
        &mut self,
        ty: &Type,
        records: &[Record<'_>],
        site: &Position,
    ) -> Result<Place, Diagnostic> {
        match ty.kind {
            Kind::Record(_) => {
                if let RecordAbi::Registers(classes) = RecordAbi::classify(ty, records, site)? {
                    if let Some(registers) = self.allocate(classes, &INTEGER_REGS, 8) {
                        return Ok(Place::Record(registers));
                    }
                }
            }
            Kind::Float(float) if self.vectors < 8 => {
                let index = self.vectors;
                self.vectors += 1;
                return Ok(Place::Vector(index, float));
            }
            Kind::Integer(_) | Kind::Pointer(_) if self.integers < INTEGER_REGS.len() => {
                let reg = INTEGER_REGS[self.integers];
                self.integers += 1;
                return Ok(Place::Integer(reg));
            }
            _ => {}
        }
        let layout = ty
            .layout()
            .unwrap_or(crate::types::Layout { size: 8, align: 8 });
        let size = eightbyte_size(layout.size, site)?;
        let offset = self.stack.next_multiple_of(layout.align.max(8));
        self.stack = checked_stack_end(offset, size, site)?;
        Ok(Place::Stack(offset))
    }

    fn allocate(
        &mut self,
        classes: [Class; 2],
        integer_regs: &[&'static str],
        vector_limit: usize,
    ) -> Option<RecordRegisters> {
        let (mut integers, mut vectors) = (self.integers, self.vectors);
        let mut registers = [None; 2];
        for (class, register) in classes.into_iter().zip(&mut registers) {
            *register = match class {
                Class::Empty => None,
                Class::Integer => {
                    let name = *integer_regs.get(integers)?;
                    integers += 1;
                    Some(Register::Integer(name))
                }
                Class::Sse if vectors < vector_limit => {
                    let index = vectors;
                    vectors += 1;
                    Some(Register::Sse(index))
                }
                Class::Sse => return None,
            };
        }
        // Commit both banks together. A failed allocation leaves both free
        // for later arguments and sends this whole record to memory.
        self.integers = integers;
        self.vectors = vectors;
        Some(registers)
    }
}

pub(super) fn eightbyte_size(size: usize, site: &Position) -> Result<usize, Diagnostic> {
    size.checked_next_multiple_of(8)
        .ok_or_else(|| stack_error(site))
}

pub(super) fn checked_stack_end(
    offset: usize,
    size: usize,
    site: &Position,
) -> Result<usize, Diagnostic> {
    offset
        .checked_add(size)
        .filter(|&end| end <= i32::MAX as usize - 15)
        .ok_or_else(|| stack_error(site))
}

pub(super) fn stack_error(site: &Position) -> Diagnostic {
    Diagnostic::new(site, "stack frame exceeds the addressable range")
}
