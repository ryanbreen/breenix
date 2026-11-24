//! DWARF 5 call-frame instructions (§6.4 and Table 7.29).
//! See <https://dwarfstd.org/doc/DWARF5.pdf> and the §6.4.2.4 erratum.
//!
//! Input consists of the instruction streams and parameters from a decoded CIE
//! and FDE for a flat address space. Expression operands borrow their encoded
//! bytecode. Evaluating expressions and recovering machine registers belong to
//! the caller.

/// Target byte order for fixed-width instruction operands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endian {
    Little,
    Big,
}

/// A rule for computing the canonical frame address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CfaRule<'a> {
    RegisterOffset { register: u64, offset: i64 },
    Expression(&'a [u8]),
}

/// A rule for recovering a register from the previous frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterRule<'a> {
    Undefined,
    SameValue,
    Offset(i64),
    ValOffset(i64),
    Register(u64),
    Expression(&'a [u8]),
    ValExpression(&'a [u8]),
}

/// The CFA and register columns of a frame row, indexed by DWARF register number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rules<'a, const N: usize> {
    pub cfa: Option<CfaRule<'a>>,
    pub registers: [RegisterRule<'a>; N],
}

impl<const N: usize> Default for Rules<'_, N> {
    fn default() -> Self {
        Self {
            cfa: None,
            registers: [RegisterRule::Undefined; N],
        }
    }
}

/// The rules for the half-open address interval `start..end`.
pub struct Row<'a, 'rules, const N: usize> {
    pub start: u64,
    pub end: u64,
    pub rules: &'rules Rules<'a, N>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stream {
    Cie,
    Fde,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Truncated,
    IntegerOverflow,
    UnknownOpcode(u8),
    InvalidAddressSize,
    InvalidLocation,
    InvalidCfaRule,
    RegisterOutOfRange(u64),
    StateStackFull,
    StateStackEmpty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub stream: Stream,
    /// Byte offset of the offending instruction in its stream.
    pub offset: usize,
    pub kind: ErrorKind,
}

/// Instruction bytes and parameters extracted from a matching CIE and FDE.
#[derive(Clone, Copy)]
pub struct Program<'a> {
    pub cie: &'a [u8],
    pub fde: &'a [u8],
    pub initial_location: u64,
    pub address_range: u64,
    pub code_alignment: u64,
    pub data_alignment: i64,
    pub address_size: u8,
    pub endian: Endian,
}

impl<'a> Program<'a> {
    /// Emit the table in address order, using `saved` for remembered rule sets.
    ///
    /// Supply `Rules::default()` for the DWARF defaults, or the defaults from
    /// the target ABI. `N` must cover the target's register columns. The length
    /// of `saved` determines the maximum nesting depth of remembered states.
    /// A malformed stream can fail after emitting a prefix of the table.
    pub fn rows<const N: usize>(
        &self,
        defaults: Rules<'a, N>,
        saved: &mut [Rules<'a, N>],
        mut emit: impl FnMut(Row<'a, '_, N>),
    ) -> Result<(), Error> {
        if !(1..=8).contains(&self.address_size) {
            return Err(Error {
                stream: Stream::Cie,
                offset: 0,
                kind: ErrorKind::InvalidAddressSize,
            });
        }
        let end = self
            .initial_location
            .checked_add(self.address_range)
            .ok_or(Error {
                stream: Stream::Fde,
                offset: 0,
                kind: ErrorKind::IntegerOverflow,
            })?;

        let mut location = self.initial_location;
        let mut rules = defaults;
        let mut initial = defaults;
        for (stream, mut input) in [(Stream::Cie, self.cie), (Stream::Fde, self.fde)] {
            let len = input.len();
            let mut depth = 0usize;
            while !input.is_empty() {
                let offset = len - input.len();
                let step = self.decode(&mut input, location).and_then(|instruction| {
                    match instruction {
                        Instruction::Nop => {}
                        Instruction::Location(next) => {
                            if stream == Stream::Cie || next > end {
                                return Err(ErrorKind::InvalidLocation);
                            }
                            if location < next {
                                emit(Row {
                                    start: location,
                                    end: next,
                                    rules: &rules,
                                });
                                location = next;
                            }
                        }
                        Instruction::Register(number, rule) => {
                            rules.registers[register_index::<N>(number)?] = rule;
                        }
                        Instruction::RestoreRegister(number) => {
                            let index = register_index::<N>(number)?;
                            rules.registers[index] = initial.registers[index];
                        }
                        Instruction::Cfa(rule) => rules.cfa = Some(rule),
                        Instruction::CfaRegister(new) => match &mut rules.cfa {
                            Some(CfaRule::RegisterOffset { register, .. }) => *register = new,
                            _ => return Err(ErrorKind::InvalidCfaRule),
                        },
                        Instruction::CfaOffset(new) => match &mut rules.cfa {
                            Some(CfaRule::RegisterOffset { offset, .. }) => *offset = new,
                            _ => return Err(ErrorKind::InvalidCfaRule),
                        },
                        // The §6.4.2.4 erratum includes the CFA in saved state.
                        // The row address is independent of the remembered rules.
                        Instruction::RememberState => {
                            *saved.get_mut(depth).ok_or(ErrorKind::StateStackFull)? = rules;
                            depth += 1;
                        }
                        Instruction::RestoreState => {
                            depth = depth.checked_sub(1).ok_or(ErrorKind::StateStackEmpty)?;
                            rules = saved[depth];
                        }
                    }
                    Ok(())
                });
                step.map_err(|kind| Error {
                    stream,
                    offset,
                    kind,
                })?;
            }
            initial = rules;
        }

        if location < end {
            emit(Row {
                start: location,
                end,
                rules: &rules,
            });
        }
        Ok(())
    }

    fn decode(&self, input: &mut &'a [u8], location: u64) -> Result<Instruction<'a>, ErrorKind> {
        use Instruction::*;
        use RegisterRule as Reg;

        let opcode = byte(input)?;
        let instruction = match opcode {
            // DW_CFA_advance_loc, DW_CFA_offset, and DW_CFA_restore.
            0x40..=0x7f => self.advance(location, u64::from(opcode & 0x3f))?,
            0x80..=0xbf => Register(
                u64::from(opcode & 0x3f),
                Reg::Offset(self.factored(i128::from(read_uleb128(input)?))?),
            ),
            0xc0..=0xff => RestoreRegister(u64::from(opcode & 0x3f)),
            0x00 => Nop,
            // DW_CFA_set_loc, then DW_CFA_advance_loc{1,2,4}.
            0x01 => {
                let next = uint(input, usize::from(self.address_size), self.endian)?;
                if next <= location {
                    return Err(ErrorKind::InvalidLocation);
                }
                Location(next)
            }
            0x02..=0x04 => {
                let width = 1 << (opcode - 0x02);
                self.advance(location, uint(input, width, self.endian)?)?
            }
            0x05 => Register(
                read_uleb128(input)?,
                Reg::Offset(self.factored(i128::from(read_uleb128(input)?))?),
            ),
            0x06 => RestoreRegister(read_uleb128(input)?),
            0x07 => Register(read_uleb128(input)?, Reg::Undefined),
            0x08 => Register(read_uleb128(input)?, Reg::SameValue),
            0x09 => Register(read_uleb128(input)?, Reg::Register(read_uleb128(input)?)),
            0x0a => RememberState,
            0x0b => RestoreState,
            0x0c => Cfa(CfaRule::RegisterOffset {
                register: read_uleb128(input)?,
                offset: unfactored(input)?,
            }),
            0x0d => CfaRegister(read_uleb128(input)?),
            0x0e => CfaOffset(unfactored(input)?),
            0x0f => Cfa(CfaRule::Expression(expression(input)?)),
            0x10 => Register(read_uleb128(input)?, Reg::Expression(expression(input)?)),
            0x11 => Register(
                read_uleb128(input)?,
                Reg::Offset(self.factored(i128::from(read_sleb128(input)?))?),
            ),
            0x12 => Cfa(CfaRule::RegisterOffset {
                register: read_uleb128(input)?,
                offset: self.factored(i128::from(read_sleb128(input)?))?,
            }),
            0x13 => CfaOffset(self.factored(i128::from(read_sleb128(input)?))?),
            0x14 => Register(
                read_uleb128(input)?,
                Reg::ValOffset(self.factored(i128::from(read_uleb128(input)?))?),
            ),
            0x15 => Register(
                read_uleb128(input)?,
                Reg::ValOffset(self.factored(i128::from(read_sleb128(input)?))?),
            ),
            0x16 => Register(read_uleb128(input)?, Reg::ValExpression(expression(input)?)),
            _ => return Err(ErrorKind::UnknownOpcode(opcode)),
        };
        Ok(instruction)
    }

    fn advance(&self, location: u64, delta: u64) -> Result<Instruction<'a>, ErrorKind> {
        let delta = delta
            .checked_mul(self.code_alignment)
            .ok_or(ErrorKind::IntegerOverflow)?;
        let next = location
            .checked_add(delta)
            .ok_or(ErrorKind::IntegerOverflow)?;
        Ok(Instruction::Location(next))
    }

    fn factored(&self, offset: i128) -> Result<i64, ErrorKind> {
        // The operand came from a u64 or i64, so its product with an i64 fits i128.
        i64::try_from(offset * i128::from(self.data_alignment))
            .map_err(|_| ErrorKind::IntegerOverflow)
    }
}

enum Instruction<'a> {
    Nop,
    Location(u64),
    Register(u64, RegisterRule<'a>),
    RestoreRegister(u64),
    Cfa(CfaRule<'a>),
    CfaRegister(u64),
    CfaOffset(i64),
    RememberState,
    RestoreState,
}

fn register_index<const N: usize>(number: u64) -> Result<usize, ErrorKind> {
    match usize::try_from(number) {
        Ok(index) if index < N => Ok(index),
        _ => Err(ErrorKind::RegisterOutOfRange(number)),
    }
}

fn byte(input: &mut &[u8]) -> Result<u8, ErrorKind> {
    let (&byte, rest) = input.split_first().ok_or(ErrorKind::Truncated)?;
    *input = rest;
    Ok(byte)
}

fn take<'a>(input: &mut &'a [u8], len: usize) -> Result<&'a [u8], ErrorKind> {
    let (value, rest) = input.split_at_checked(len).ok_or(ErrorKind::Truncated)?;
    *input = rest;
    Ok(value)
}

fn uint(input: &mut &[u8], width: usize, endian: Endian) -> Result<u64, ErrorKind> {
    let bytes = take(input, width)?;
    let shift_in = |value, &byte| (value << 8) | u64::from(byte);
    Ok(match endian {
        Endian::Little => bytes.iter().rev().fold(0, shift_in),
        Endian::Big => bytes.iter().fold(0, shift_in),
    })
}

fn expression<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], ErrorKind> {
    let len = usize::try_from(read_uleb128(input)?).map_err(|_| ErrorKind::IntegerOverflow)?;
    take(input, len)
}

fn unfactored(input: &mut &[u8]) -> Result<i64, ErrorKind> {
    i64::try_from(read_uleb128(input)?).map_err(|_| ErrorKind::IntegerOverflow)
}

/// Read an unsigned LEB128 value from the front of the slice (DWARF 5 §7.6).
pub fn read_uleb128(input: &mut &[u8]) -> Result<u64, ErrorKind> {
    u64::try_from(leb128(input, false)?).map_err(|_| ErrorKind::IntegerOverflow)
}

/// Read a signed LEB128 value from the front of the slice (DWARF 5 §7.6).
pub fn read_sleb128(input: &mut &[u8]) -> Result<i64, ErrorKind> {
    i64::try_from(leb128(input, true)?).map_err(|_| ErrorKind::IntegerOverflow)
}

fn leb128(input: &mut &[u8], signed: bool) -> Result<i128, ErrorKind> {
    let mut value = 0;
    // Ten groups suffice for a 64-bit value. Accumulate in a wider integer so that
    // the high bits in the final group are checked after sign extension.
    for shift in (0..64).step_by(7) {
        let byte = byte(input)?;
        value |= i128::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            if signed && byte & 0x40 != 0 {
                value |= -1i128 << (shift + 7);
            }
            return Ok(value);
        }
    }
    Err(ErrorKind::IntegerOverflow)
}

#[cfg(test)]
#[test]
fn truncated_instructions_is_error_not_panic() {
    use crate::cfi::{Endian, ErrorKind, Program, Rules, Stream};

    // A complete single instruction in each entry. Test prefixes ending after
    // its opcode, since a cut at an instruction boundary can be valid CFI.
    let instructions: &[&[u8]] = &[
        &[0x01, 4, 0, 0, 0],       // set_loc
        &[0x02, 1],                // advance_loc1
        &[0x03, 1, 0],             // advance_loc2
        &[0x04, 1, 0, 0, 0],       // advance_loc4
        &[0x0c, 1, 0x80, 1],       // def_cfa, multi-byte ULEB128 offset
        &[0x12, 1, 0xbf, 0x7f],    // def_cfa_sf, multi-byte SLEB128 offset
        &[0x81, 2],                // offset
        &[0x0f, 2, 0x31, 0x32],    // def_cfa_expression
        &[0x10, 1, 2, 0x31, 0x22], // expression
        &[0x16, 1, 2, 0x31, 0x22], // val_expression
    ];
    let program = Program {
        cie: &[0x0c, 1, 0],
        fde: &[],
        initial_location: 0,
        address_range: 64,
        code_alignment: 1,
        data_alignment: -8,
        address_size: 4,
        endian: Endian::Little,
    };
    for instruction in instructions {
        Program {
            fde: instruction,
            ..program
        }
        .rows(Rules::<2>::default(), &mut [], |_| {})
        .unwrap();
        for end in 1..instruction.len() {
            let error = Program {
                fde: &instruction[..end],
                ..program
            }
            .rows(Rules::<2>::default(), &mut [], |_| {})
            .unwrap_err();
            assert_eq!(
                (error.stream, error.offset, error.kind),
                (Stream::Fde, 0, ErrorKind::Truncated),
                "{:02x?}",
                &instruction[..end]
            );
        }
    }
    let error = Program {
        cie: &[0, 0x09, 1],
        ..program
    }
    .rows(Rules::<2>::default(), &mut [], |_| {})
    .unwrap_err();
    assert_eq!(
        (error.stream, error.offset, error.kind),
        (Stream::Cie, 1, ErrorKind::Truncated)
    );
}
