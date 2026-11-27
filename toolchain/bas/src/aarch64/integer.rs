//! A64 integer instruction encodings from DDI 0487 M.b C6.2.

use super::{condition, immediate, modifier, parse_register, Register, Relocation};
use crate::parser::{Parser, Token};
use crate::Result;

pub(super) fn encode(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<()> {
    let word = match op.text {
        "add" => return add_sub(p, op.column, 0, false),
        "adds" => return add_sub(p, op.column, 1, false),
        "sub" => return add_sub(p, op.column, 2, false),
        "subs" => return add_sub(p, op.column, 3, false),
        "cmp" => return add_sub(p, op.column, 3, true),
        "cmn" => return add_sub(p, op.column, 1, true),
        "and" => logical(p, 0, false, op.text),
        "orr" => logical(p, 1, false, op.text),
        "eor" => logical(p, 2, false, op.text),
        "ands" | "tst" => logical(p, 3, false, op.text),
        "bic" => logical(p, 0, true, op.text),
        "orn" | "mvn" => logical(p, 1, true, op.text),
        "eon" => logical(p, 2, true, op.text),
        "bics" => logical(p, 3, true, op.text),
        "movn" => move_wide(p, 0x1280_0000),
        "movz" => move_wide(p, 0x5280_0000),
        "movk" => move_wide(p, 0x7280_0000),
        "mov" => mov(p),
        "lsl" | "lslv" => shift(p, 0, op.text == "lsl"),
        "lsr" | "lsrv" => shift(p, 1, op.text == "lsr"),
        "asr" | "asrv" => shift(p, 2, op.text == "asr"),
        "ror" | "rorv" => shift(p, 3, op.text == "ror"),
        "sbfm" => bitfield(p, 0x1300_0000, false),
        "bfm" => bitfield(p, 0x3300_0000, false),
        "ubfm" => bitfield(p, 0x5300_0000, false),
        "sbfx" => bitfield(p, 0x1300_0000, true),
        "ubfx" => bitfield(p, 0x5300_0000, true),
        "extr" => extract(p),
        "sxtb" => extend(p, 7, true),
        "sxth" => extend(p, 15, true),
        "sxtw" => extend(p, 31, true),
        "uxtb" => extend(p, 7, false),
        "uxth" => extend(p, 15, false),
        "uxtw" => extend(p, 31, false),
        "madd" => multiply(p, 0x1b00_0000, false),
        "msub" => multiply(p, 0x1b00_8000, false),
        "mul" => multiply(p, 0x1b00_0000, true),
        "mneg" => multiply(p, 0x1b00_8000, true),
        "smull" => fixed_multiply(p, 0x9b20_7c00, false),
        "umull" => fixed_multiply(p, 0x9ba0_7c00, false),
        "smulh" => fixed_multiply(p, 0x9b40_7c00, true),
        "umulh" => fixed_multiply(p, 0x9bc0_7c00, true),
        "udiv" => divide(p, 0x1ac0_0800),
        "sdiv" => divide(p, 0x1ac0_0c00),
        "csel" => select(p, 0x1a80_0000, op.text),
        "csinc" | "cset" | "cinc" => select(p, 0x1a80_0400, op.text),
        "csinv" | "csetm" => select(p, 0x5a80_0000, op.text),
        "csneg" | "cneg" => select(p, 0x5a80_0400, op.text),
        "ccmn" => compare(p, 0x3a40_0000),
        "ccmp" => compare(p, 0x7a40_0000),
        "rbit" | "rev" | "clz" => unary(p, op.text),
        _ => Err(p.error(op.column, "unknown AArch64 instruction")),
    }?;
    p.emit_aarch64(op.column, word)
}

#[derive(Clone, Copy)]
enum Register31 {
    Zero,
    Stack,
}

use Register31::{Stack, Zero};

impl Register {
    fn check_width(self, p: &Parser<'_, '_>, wide: bool) -> Result<()> {
        if self.wide != wide {
            return Err(p.error(self.column, "AArch64 register width mismatch"));
        }
        Ok(())
    }

    fn check_31(self, p: &Parser<'_, '_>, meaning: Register31) -> Result<()> {
        match (meaning, self.stack, self.number) {
            (Zero, true, _) => Err(p.error(self.column, "stack pointer is not allowed here")),
            (Stack, false, 31) => {
                Err(p.error(self.column, "expected stack pointer or general register"))
            }
            _ => Ok(()),
        }
    }

    fn check(self, p: &Parser<'_, '_>, wide: bool, meaning: Register31) -> Result<u32> {
        self.check_width(p, wide)?;
        self.check_31(p, meaning)?;
        Ok(self.number)
    }
}

fn reg(p: &mut Parser<'_, '_>) -> Result<Register> {
    let token = p.word();
    parse_register(p, token)
}

fn general(p: &mut Parser<'_, '_>) -> Result<Register> {
    let register = reg(p)?;
    register.check_31(p, Zero)?;
    Ok(register)
}

fn comma(p: &mut Parser<'_, '_>) -> Result<()> {
    p.expect(b',', "expected ',' before operand")
}

fn next_reg(p: &mut Parser<'_, '_>, wide: bool) -> Result<u32> {
    comma(p)?;
    reg(p)?.check(p, wide, Zero)
}

fn rd_rn(p: &mut Parser<'_, '_>) -> Result<(Register, u32)> {
    let rd = general(p)?;
    let rn = next_reg(p, rd.wide)?;
    Ok((rd, rn))
}

// C6.2.6 and C6.2.15: shifted-register forms share shift and imm6.
fn shift_fields(p: &mut Parser<'_, '_>, shift: Token<'_>, max: u32, rotate: bool) -> Result<u32> {
    let kind = match shift.text {
        "lsl" => 0,
        "lsr" => 1,
        "asr" => 2,
        "ror" if rotate => 3,
        _ if rotate => return Err(p.error(shift.column, "expected LSL, LSR, ASR or ROR")),
        _ => return Err(p.error(shift.column, "shift must be LSL, LSR or ASR")),
    };
    Ok((kind << 22) | (immediate(p, max)? << 10))
}

fn extension(name: &str) -> Option<u32> {
    [
        "uxtb", "uxth", "uxtw", "uxtx", "sxtb", "sxth", "sxtw", "sxtx",
    ]
    .iter()
    .position(|&extend| extend == name)
    .map(|option| option as u32)
}

fn add_sub(p: &mut Parser<'_, '_>, column: usize, op: u32, alias: bool) -> Result<()> {
    // C6.2.4–6, .9–11, .455–457, .462–464 (CMN/CMP fix Rd to ZR).
    let first = reg(p)?;
    let (rd, rn) = if alias {
        (None, first)
    } else {
        comma(p)?;
        (Some(first), reg(p)?)
    };
    rn.check_width(p, first.wide)?;
    comma(p)?;
    if op == 0 && p.peek() == b':' {
        rn.check_31(p, Stack)?;
        first.check_31(p, Stack)?;
        let name = modifier(p)?;
        let (kind, shift) = match name.text {
            "lo12" => (Relocation::AddAbsLo12Nc, 0),
            "tprel_hi12" => (Relocation::TlsleAddTprelHi12, 1),
            "tprel_lo12_nc" => (Relocation::TlsleAddTprelLo12Nc, 0),
            _ => return Err(p.error(name.column, "unsupported ADD relocation modifier")),
        };
        let word = first.sf() | 0x1100_0000 | (shift << 22) | (rn.number << 5) | first.number;
        let label = p.word();
        return p.emit_aarch64_reference(label, kind, word);
    }
    let flags = op & 1 != 0;
    let has_sp = rn.stack || rd.is_some_and(|rd| rd.stack);
    let (enc, rn31, rd31) = if p.peek() == b'#' {
        let imm12 = immediate(p, 4095)?;
        let mut sh = 0;
        if p.take(b',') {
            let shift = p.word();
            if shift.text != "lsl" {
                return Err(p.error(shift.column, "expected 'lsl'"));
            }
            p.peek();
            let column = p.column;
            sh = match immediate(p, 12)? {
                0 => 0,
                12 => 1,
                _ => return Err(p.error(column, "immediate shift must be 0 or 12")),
            };
        }
        (
            0x1100_0000 | (sh << 22) | (imm12 << 10),
            Stack,
            if flags { Zero } else { Stack },
        )
    } else {
        let rm = reg(p)?;
        let modifier = p.take(b',').then(|| p.word());
        let option = modifier
            .and_then(|token| extension(token.text))
            .or_else(|| {
                // With SP, LSL (or no modifier) selects the native-width extension.
                (has_sp && modifier.is_none_or(|token| token.text == "lsl"))
                    .then_some(2 + u32::from(first.wide))
            });
        if let Some(option) = option {
            let rm_wide = first.wide && matches!(option, 3 | 7);
            let rm = rm.check(p, rm_wide, Zero)?;
            let amount = if modifier.is_some_and(|token| token.text == "lsl" || !p.at_end()) {
                immediate(p, 4)?
            } else {
                0
            };
            let bits = 0x0b20_0000 | (rm << 16) | (option << 13) | (amount << 10);
            (bits, Stack, if flags { Zero } else { Stack })
        } else {
            let rm = rm.check(p, first.wide, Zero)?;
            let bits = if let Some(shift) = modifier {
                shift_fields(p, shift, first.bits() - 1, false)?
            } else {
                0
            };
            (0x0b00_0000 | (rm << 16) | bits, Zero, Zero)
        }
    };
    rn.check_31(p, rn31)?;
    let rd = if let Some(rd) = rd {
        rd.check_31(p, rd31)?;
        rd.number
    } else {
        31
    };
    let word = first.sf() | (op << 29) | enc | (rn.number << 5) | rd;
    p.emit_aarch64(column, word)
}

fn literal(p: &mut Parser<'_, '_>, width: u32) -> Result<(u64, usize)> {
    p.expect(b'#', "expected '#' before immediate")?;
    p.peek();
    let column = p.column;
    let mask = u64::MAX >> (64 - width);
    let value = p.integer((width / 8) as usize, false)?;
    Ok((value & mask, column))
}

fn logical_mask(value: u64, width: u32) -> Option<u32> {
    // Invert DecodeBitMasks (J1.2.3.19). Halve the pattern as long as its
    // halves repeat, then rotate the remaining element into low-order ones.
    let mut size = width;
    let mut element = value;
    while size > 2 {
        let half_mask = (1u64 << (size / 2)) - 1;
        let low = element & half_mask;
        if element >> (size / 2) != low {
            break;
        }
        element = low;
        size /= 2;
    }
    let ones = element.count_ones();
    if ones == 0 || ones == size {
        return None;
    }
    let mask = u64::MAX >> (64 - size);
    let run = (1u64 << ones) - 1;
    let rotation = (0..size).find(|&r| {
        // Left rotation within the element reverses the decoder's ROR.
        let rotated = (element << r) | (element >> ((size - r) % size));
        rotated & mask == run
    })?;
    // HighestSetBit(N:NOT(imms)) selects log2(size). S occupies the low
    // log2(size) bits of imms. Use the least nonnegative rotation.
    let n = u32::from(size == 64);
    let imms = ((!(size - 1) << 1) & 63) | (ones - 1);
    Some((n << 22) | (rotation << 16) | (imms << 10))
}

fn logical(p: &mut Parser<'_, '_>, opc: u32, invert: bool, op: &str) -> Result<u32> {
    // C6.2.14–17, .41–42, .154–156, .293, .299–301, .483–484.
    let first = reg(p)?;
    comma(p)?;
    let (rd, rn) = match op {
        "tst" => (None, Some(first)),
        "mvn" => (Some(first), None),
        _ => {
            let rn = reg(p)?;
            rn.check_width(p, first.wide)?;
            comma(p)?;
            (Some(first), Some(rn))
        }
    };
    if let Some(rn) = rn {
        rn.check_31(p, Zero)?;
    }
    let rn = rn.map_or(31, |reg| reg.number);
    let bits = first.sf() | (opc << 29) | (rn << 5) | rd.map_or(31, |reg| reg.number);
    if p.peek() == b'#' && (!invert || op == "bic") {
        if let Some(rd) = rd {
            rd.check_31(p, if opc == 3 { Zero } else { Stack })?;
        }
        let (value, column) = literal(p, first.bits())?;
        // The immediate BIC spelling clears bits using AND (immediate),
        // C6.2.14, with the mask complemented at the operand width.
        let value = if invert {
            !value & (u64::MAX >> (64 - first.bits()))
        } else {
            value
        };
        let mask = logical_mask(value, first.bits())
            .ok_or_else(|| p.error(column, "unencodable logical immediate"))?;
        return Ok(bits | 0x1200_0000 | mask);
    }
    if let Some(rd) = rd {
        rd.check_31(p, Zero)?;
    }
    let rm = reg(p)?.check(p, first.wide, Zero)?;
    let shift = if p.take(b',') {
        let shift = p.word();
        shift_fields(p, shift, first.bits() - 1, true)?
    } else {
        0
    };
    Ok(bits | 0x0a00_0000 | (u32::from(invert) << 21) | (rm << 16) | shift)
}

fn move_wide(p: &mut Parser<'_, '_>, base: u32) -> Result<u32> {
    // C6.2.282–284: hw selects a halfword and imm16 holds its value.
    let rd = general(p)?;
    comma(p)?;
    let value = immediate(p, 0xffff)?;
    let mut halfword = 0;
    if p.take(b',') {
        let shift = p.word();
        if shift.text != "lsl" {
            return Err(p.error(shift.column, "expected 'lsl'"));
        }
        let amount = immediate(p, rd.bits() - 16)?;
        if amount % 16 != 0 {
            let message = if base == 0x5280_0000 {
                "MOVZ shift must be a multiple of 16"
            } else {
                "shift must be a multiple of 16"
            };
            return Err(p.error(shift.column, message));
        }
        halfword = amount / 16;
    }
    Ok(base | rd.sf() | (halfword << 21) | (value << 5) | rd.number)
}

fn wide_immediate(value: u64) -> Option<u32> {
    let halfword = if value == 0 {
        0
    } else {
        value.trailing_zeros() / 16
    };
    let shift = halfword * 16;
    let imm16 = value >> shift;
    (imm16 <= 0xffff).then_some((halfword << 21) | ((imm16 as u32) << 5))
}

fn mov(p: &mut Parser<'_, '_>) -> Result<u32> {
    // C6.2.277–281: ADD #0 for SP, ORR with ZR for registers or masks,
    // and MOVZ/MOVN for wide immediates.
    let rd = reg(p)?;
    comma(p)?;
    if p.peek() != b'#' {
        let rn = reg(p)?;
        let stack = rd.stack || rn.stack;
        let meaning = if stack { Stack } else { Zero };
        rd.check_31(p, meaning)?;
        let rn = rn.check(p, rd.wide, meaning)?;
        let bits = if stack {
            0x1100_0000 | (rn << 5)
        } else {
            0x2a00_03e0 | (rn << 16)
        };
        return Ok(rd.sf() | bits | rd.number);
    }
    let (value, column) = literal(p, rd.bits())?;
    let out = rd.sf() | rd.number;
    if !rd.stack {
        if let Some(bits) = wide_immediate(value) {
            return Ok(out | 0x5280_0000 | bits);
        }
        let inverse = !value & (u64::MAX >> (64 - rd.bits()));
        if let Some(bits) = wide_immediate(inverse) {
            return Ok(out | 0x1280_0000 | bits);
        }
    }
    rd.check_31(p, Stack)?;
    let bits = logical_mask(value, rd.bits())
        .ok_or_else(|| p.error(column, "unencodable MOV immediate"))?;
    Ok(out | 0x3200_03e0 | bits)
}

fn bitfields(rd: Register, rn: u32, base: u32, immr: u32, imms: u32) -> u32 {
    base | rd.sf() | (rd.sf() >> 9) | (immr << 16) | (imms << 10) | (rn << 5) | rd.number
}

fn bitfield(p: &mut Parser<'_, '_>, base: u32, extract: bool) -> Result<u32> {
    // C6.2.39, .354–355, .486–487. Extract aliases specify lsb and width.
    let (rd, rn) = rd_rn(p)?;
    comma(p)?;
    let immr = immediate(p, rd.bits() - 1)?;
    comma(p)?;
    p.peek();
    let column = p.column;
    let imms = if extract {
        let width = immediate(p, rd.bits() - immr)?;
        if width == 0 {
            return Err(p.error(column, "bitfield width must be positive"));
        }
        immr + width - 1
    } else {
        immediate(p, rd.bits() - 1)?
    };
    Ok(bitfields(rd, rn, base, immr, imms))
}

fn extend(p: &mut Parser<'_, '_>, imms: u32, signed: bool) -> Result<u32> {
    // C6.2.471–473, .486, .499–500. Extension takes a W source.
    let mut rd = general(p)?;
    if imms == 31 {
        rd.check_width(p, true)?;
    }
    let rn = next_reg(p, false)?;
    if !signed && imms < 31 {
        // Keep the W encoding for the X-destination spelling of UXTB/UXTH.
        rd.wide = false;
    }
    let base = if signed { 0x1300_0000 } else { 0x5300_0000 };
    Ok(bitfields(rd, rn, base, 0, imms))
}

fn extract(p: &mut Parser<'_, '_>) -> Result<u32> {
    // C6.2.160: EXTR puts Rm where the bitfield class puts immr.
    let (rd, rn) = rd_rn(p)?;
    let rm = next_reg(p, rd.wide)?;
    comma(p)?;
    let lsb = immediate(p, rd.bits() - 1)?;
    Ok(bitfields(rd, rn, 0x1380_0000, rm, lsb))
}

fn shift(p: &mut Parser<'_, '_>, op: u32, accepts_immediate: bool) -> Result<u32> {
    // C6.2.19–21, .268–273, .346–348.
    let (rd, rn) = rd_rn(p)?;
    comma(p)?;
    if accepts_immediate && p.peek() == b'#' {
        let max = rd.bits() - 1;
        let amount = immediate(p, max)?;
        let (base, r, s) = match op {
            0 => (0x5300_0000, (rd.bits() - amount) & max, max - amount),
            1 => (0x5300_0000, amount, max),
            2 => (0x1300_0000, amount, max),
            _ => (0x1380_0000, rn, amount),
        };
        return Ok(bitfields(rd, rn, base, r, s));
    }
    let rm = reg(p)?.check(p, rd.wide, Zero)?;
    Ok(0x1ac0_2000 | rd.sf() | (rm << 16) | (op << 10) | (rn << 5) | rd.number)
}

fn multiply(p: &mut Parser<'_, '_>, base: u32, alias: bool) -> Result<u32> {
    // C6.2.274, .276, .290, .292: MUL/MNEG fix Ra to ZR.
    let (rd, rn) = rd_rn(p)?;
    let rm = next_reg(p, rd.wide)?;
    let ra = if alias { 31 } else { next_reg(p, rd.wide)? };
    Ok(base | rd.sf() | (rm << 16) | (ra << 10) | (rn << 5) | rd.number)
}

fn fixed_multiply(p: &mut Parser<'_, '_>, base: u32, wide_sources: bool) -> Result<u32> {
    // C6.2.378–379, .497–498: the high and widening forms both write Xd.
    let rd = general(p)?;
    rd.check_width(p, true)?;
    let rn = next_reg(p, wide_sources)?;
    let rm = next_reg(p, wide_sources)?;
    Ok(base | (rm << 16) | (rn << 5) | rd.number)
}

fn divide(p: &mut Parser<'_, '_>, base: u32) -> Result<u32> {
    // C6.2.356, .489.
    let (rd, rn) = rd_rn(p)?;
    let rm = next_reg(p, rd.wide)?;
    Ok(base | rd.sf() | (rm << 16) | (rn << 5) | rd.number)
}

fn cond(p: &mut Parser<'_, '_>, invert: bool) -> Result<u32> {
    let token = p.word();
    let cond =
        condition(token.text).ok_or_else(|| p.error(token.column, "unknown AArch64 condition"))?;
    if invert && cond >= 14 {
        return Err(p.error(token.column, "condition cannot be inverted"));
    }
    Ok(cond ^ u32::from(invert))
}

fn select(p: &mut Parser<'_, '_>, base: u32, op: &str) -> Result<u32> {
    // C6.2.86, .99, .138–143. Aliases invert the condition and either
    // repeat the source register or substitute ZR for both inputs.
    let rd = general(p)?;
    let (rn, rm, invert) = match op {
        "cset" | "csetm" => (31, 31, true),
        "cinc" | "cneg" => {
            let rn = next_reg(p, rd.wide)?;
            (rn, rn, true)
        }
        _ => (next_reg(p, rd.wide)?, next_reg(p, rd.wide)?, false),
    };
    comma(p)?;
    let cond = cond(p, invert)?;
    Ok(base | rd.sf() | (rm << 16) | (cond << 12) | (rn << 5) | rd.number)
}

fn compare(p: &mut Parser<'_, '_>, base: u32) -> Result<u32> {
    // C6.2.79–82: bit 11 selects an immediate second operand.
    let rn = general(p)?;
    comma(p)?;
    let (operand, form) = if p.peek() == b'#' {
        (immediate(p, 31)?, 1 << 11)
    } else {
        (reg(p)?.check(p, rn.wide, Zero)?, 0)
    };
    comma(p)?;
    let nzcv = immediate(p, 15)?;
    comma(p)?;
    let cond = cond(p, false)?;
    Ok(base | rn.sf() | (operand << 16) | (cond << 12) | form | (rn.number << 5) | nzcv)
}

fn unary(p: &mut Parser<'_, '_>, op: &str) -> Result<u32> {
    // C6.2.91, .320, .341.
    let (rd, rn) = rd_rn(p)?;
    let opcode = match op {
        "rbit" => 0,
        "rev" => 2 + u32::from(rd.wide),
        _ => 4,
    };
    Ok(0x5ac0_0000 | rd.sf() | (opcode << 10) | (rn << 5) | rd.number)
}

pub(super) fn adr(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<()> {
    let rd = general(p)?;
    rd.check_width(p, true)?;
    comma(p)?;
    let (kind, opcode) = if op.text == "adr" {
        (Relocation::Adr, 0x1000_0000)
    } else {
        let kind = if p.peek() == b':' {
            let name = modifier(p)?;
            if name.text != "got" {
                return Err(p.error(name.column, "unsupported ADRP relocation modifier"));
            }
            Relocation::AdrGotPage
        } else {
            Relocation::AdrPrelPgHi21
        };
        (kind, 0x9000_0000)
    };
    let label = p.word();
    p.emit_aarch64_reference(label, kind, opcode | rd.number)
}
