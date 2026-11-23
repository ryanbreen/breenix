//! Load/store addressing and synchronization encodings from DDI 0487 M.b.

use super::fp::{FpRegister, Precision};
use super::{branch, immediate, modifier, parse_register, Register, Relocation};
use crate::parser::{Parser, Token};
use crate::Result;

#[derive(Clone, Copy)]
enum Transfer {
    General(Register),
    Fp(FpRegister),
}

impl Transfer {
    fn parse(p: &mut Parser<'_, '_>) -> Result<Self> {
        let token = p.word();
        if Precision::of(token).is_some() {
            FpRegister::parse(p, token).map(Self::Fp)
        } else {
            general(p, token, None).map(Self::General)
        }
    }

    fn number(self) -> u32 {
        match self {
            Self::General(reg) => reg.number,
            Self::Fp(reg) => reg.number,
        }
    }

    fn column(self) -> usize {
        match self {
            Self::General(reg) => reg.column,
            Self::Fp(reg) => reg.column,
        }
    }

    fn overlaps_base(self, base: Register) -> bool {
        !base.stack && matches!(self, Self::General(reg) if reg.number == base.number)
    }
}

fn general(p: &Parser<'_, '_>, token: Token<'_>, wide: Option<bool>) -> Result<Register> {
    if !token.text.starts_with(['w', 'x']) {
        return Err(p.error(token.column, "expected a general-purpose register"));
    }
    let reg = parse_register(p, token)?;
    if reg.stack {
        return Err(p.error(reg.column, "stack pointer is not allowed"));
    }
    if wide.is_some_and(|wide| reg.wide != wide) {
        return Err(p.error(reg.column, "register width mismatch"));
    }
    Ok(reg)
}

fn next_general(p: &mut Parser<'_, '_>, wide: Option<bool>) -> Result<Register> {
    let token = p.word();
    general(p, token, wide)
}

fn comma(p: &mut Parser<'_, '_>) -> Result<()> {
    p.expect(b',', "expected ',' before next operand")
}

fn base_register(p: &mut Parser<'_, '_>) -> Result<Register> {
    p.expect(b'[', "expected '[' before address")?;
    let token = p.word();
    if !token.text.starts_with('x') && token.text != "sp" {
        return Err(p.error(token.column, "expected X register or SP"));
    }
    let base = parse_register(p, token)?;
    if base.number == 31 && !base.stack {
        return Err(p.error(token.column, "expected X register or SP"));
    }
    Ok(base)
}

#[derive(Clone, Copy)]
struct Offset {
    bytes: i64,
    column: usize,
}

impl Offset {
    fn parse(p: &mut Parser<'_, '_>) -> Result<Self> {
        p.expect(b'#', "expected '#' before offset")?;
        p.peek();
        let column = p.column;
        let bytes = p.integer(8, true)? as i64;
        Ok(Self { bytes, column })
    }

    fn field(self, p: &Parser<'_, '_>, scale: u32, min: i64, max: i64) -> Result<u32> {
        let unit = 1 << scale;
        if self.bytes % unit != 0 {
            return Err(p.error(self.column, "offset must be aligned to access size"));
        }
        let scaled = self.bytes / unit;
        if !(min..=max).contains(&scaled) {
            return Err(p.error(self.column, "offset out of range"));
        }
        Ok(scaled as u32)
    }
}

enum AddressMode {
    Offset(Offset),
    PreIndex(Offset),
    PostIndex(Offset),
    Register(u32),
}

impl AddressMode {
    fn parse(p: &mut Parser<'_, '_>, base: Register, scale: Option<u32>) -> Result<Self> {
        let mut offset = Offset {
            bytes: 0,
            column: base.column,
        };
        let has_offset = p.take(b',');
        if has_offset {
            if p.peek() == b'#' {
                offset = Offset::parse(p)?;
            } else {
                return register_offset(p, scale).map(Self::Register);
            }
        }
        p.expect(b']', "expected ']' after address")?;
        if p.take(b'!') {
            if !has_offset {
                return Err(p.error(p.column - 1, "pre-index requires an immediate offset"));
            }
            Ok(Self::PreIndex(offset))
        } else if !has_offset && p.take(b',') {
            Ok(Self::PostIndex(Offset::parse(p)?))
        } else {
            Ok(Self::Offset(offset))
        }
    }

    fn writes_back(&self) -> bool {
        matches!(self, Self::PreIndex(_) | Self::PostIndex(_))
    }
}

// C6.2.217 and C7.2.232: Rm[20:16], option[15:13], S[12].
fn register_offset(p: &mut Parser<'_, '_>, scale: Option<u32>) -> Result<u32> {
    p.peek();
    let scale = scale.ok_or_else(|| p.error(p.column, "register offset is not allowed"))?;
    let index = next_general(p, None)?;
    let has_extend = p.take(b',');
    let option = if has_extend {
        let extend = p.word();
        match extend.text {
            "uxtw" => 2,
            "lsl" => 3,
            "sxtw" => 6,
            "sxtx" => 7,
            _ => return Err(p.error(extend.column, "invalid register offset extension")),
        }
    } else {
        3
    };
    if index.wide != (option & 1 != 0) {
        return Err(p.error(index.column, "register width mismatch"));
    }
    let scaled = if has_extend && (option == 3 || p.peek() == b'#') {
        p.expect(b'#', "expected '#' before register offset shift")?;
        p.peek();
        let column = p.column;
        let shift = p.magnitude()?;
        if shift != 0 && shift != u64::from(scale) {
            return Err(p.error(column, "invalid register offset shift"));
        }
        // With byte accesses, explicit #0 is distinguished from omission.
        shift == u64::from(scale)
    } else {
        false
    };
    p.expect(b']', "expected ']' after register offset")?;
    Ok((index.number << 16) | (option << 13) | (u32::from(scaled) << 12))
}

pub(super) fn single(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<()> {
    let (load, unscaled, signed, fixed_scale) = match op.text {
        "ldr" => (true, false, false, None),
        "str" => (false, false, false, None),
        "ldur" => (true, true, false, None),
        "stur" => (false, true, false, None),
        "ldrb" => (true, false, false, Some(0)),
        "strb" => (false, false, false, Some(0)),
        "ldurb" => (true, true, false, Some(0)),
        "sturb" => (false, true, false, Some(0)),
        "ldrh" => (true, false, false, Some(1)),
        "strh" => (false, false, false, Some(1)),
        "ldurh" => (true, true, false, Some(1)),
        "sturh" => (false, true, false, Some(1)),
        "ldrsb" => (true, false, true, Some(0)),
        "ldursb" => (true, true, true, Some(0)),
        "ldrsh" => (true, false, true, Some(1)),
        "ldursh" => (true, true, true, Some(1)),
        "ldrsw" => (true, false, true, Some(2)),
        "ldursw" => (true, true, true, Some(2)),
        _ => unreachable!("single load/store dispatch"),
    };
    let rt = if let Some(scale) = fixed_scale {
        let wide = if signed && scale < 2 {
            None
        } else {
            Some(signed)
        };
        Transfer::General(next_general(p, wide)?)
    } else {
        Transfer::parse(p)?
    };
    comma(p)?;

    if op.text == "ldr" && p.peek() != b'[' {
        // C6.2.216 / C7.2.231: opc:011:V:00:imm19:Rt.
        let (opc, v) = match rt {
            Transfer::General(reg) => (u32::from(reg.wide), 0),
            Transfer::Fp(reg) => (reg.precision.pair_opc(), 1),
        };
        return branch(
            p,
            Relocation::Literal,
            0x1800_0000 | (opc << 30) | (v << 26) | rt.number(),
        );
    }

    // C6.2.215–229, C6.2.258–263, C6.2.414–419, C6.2.445–447
    // and C7.2.230/232/235/387/388/391.
    let (scale, size, v, opc) = match rt {
        Transfer::General(reg) => {
            let scale = fixed_scale.unwrap_or(2 + u32::from(reg.wide));
            let opc = if signed {
                2 + u32::from(!reg.wide)
            } else {
                u32::from(load)
            };
            (scale, scale, 0, opc)
        }
        Transfer::Fp(reg) => {
            let scale = reg.precision.pair_opc() + 2;
            let opc = u32::from(load) | (u32::from(scale == 4) << 1);
            (scale, scale & 3, 1, opc)
        }
    };
    let base = base_register(p)?;
    let word =
        (size << 30) | 0x3800_0000 | (v << 26) | (opc << 22) | (base.number << 5) | rt.number();
    let offset_start = p.column;
    let has_comma = p.take(b',');
    if p.peek() == b':' {
        let column = p.column;
        let name = modifier(p)?;
        if unscaled {
            return Err(p.error(column, "relocation requires an unsigned scaled offset"));
        }
        let kind = match name.text {
            "lo12" if has_comma => match scale {
                0 => Relocation::Ldst8AbsLo12Nc,
                1 => Relocation::Ldst16AbsLo12Nc,
                2 => Relocation::Ldst32AbsLo12Nc,
                3 => Relocation::Ldst64AbsLo12Nc,
                4 => Relocation::Ldst128AbsLo12Nc,
                _ => unreachable!("single load/store access size"),
            },
            // AAELF64 §5.7.9 writes the GOT offset without a comma.
            "got_lo12" => {
                if op.text != "ldr" || !matches!(rt, Transfer::General(reg) if reg.wide) {
                    return Err(p.error(name.column, "GOT offset requires a 64-bit LDR"));
                }
                Relocation::Ld64GotLo12Nc
            }
            _ => return Err(p.error(column, "unsupported load/store relocation modifier")),
        };
        let label = p.word();
        p.emit_aarch64_reference(label, kind, word | (1 << 24))?;
        return p.expect(b']', "expected ']' after address");
    }
    p.column = offset_start;
    let mode = AddressMode::parse(p, base, (!unscaled).then_some(scale))?;
    if unscaled && mode.writes_back() {
        return Err(p.error(base.column, "unscaled load/store does not write back"));
    }
    if mode.writes_back() && rt.overlaps_base(base) {
        return Err(p.error(base.column, "unpredictable writeback overlap"));
    }
    let address = match mode {
        AddressMode::Offset(offset) if !unscaled => {
            1 << 24 | offset.field(p, scale, 0, 4095)? << 10
        }
        AddressMode::Offset(offset) => (offset.field(p, 0, -256, 255)? & 0x1ff) << 12,
        AddressMode::PreIndex(offset) => (offset.field(p, 0, -256, 255)? & 0x1ff) << 12 | 3 << 10,
        AddressMode::PostIndex(offset) => (offset.field(p, 0, -256, 255)? & 0x1ff) << 12 | 1 << 10,
        AddressMode::Register(index) => 1 << 21 | index | 2 << 10,
    };
    p.emit_aarch64(op.column, word | address)
}

pub(super) fn pair(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<u32> {
    let signed = op.text == "ldpsw";
    let load = op.text != "stp";
    let rt = Transfer::parse(p)?;
    comma(p)?;
    let rt2 = Transfer::parse(p)?;
    // C6.2.213/214/413 and C7.2.229/386: opc[31:30], V[26], L[22].
    let (scale, opc, v) = match (rt, rt2) {
        (Transfer::General(a), Transfer::General(b)) if signed && a.wide && b.wide => (2, 1, 0),
        (Transfer::General(a), Transfer::General(b)) if !signed && a.wide == b.wide => {
            (2 + u32::from(a.wide), 2 * u32::from(a.wide), 0)
        }
        (Transfer::Fp(a), Transfer::Fp(b)) if !signed && a.precision == b.precision => {
            let opc = a.precision.pair_opc();
            (opc + 2, opc, 1)
        }
        (Transfer::General(a), _) if signed && !a.wide => {
            return Err(p.error(a.column, "register width mismatch"));
        }
        (Transfer::Fp(a), _) if signed => {
            return Err(p.error(a.column, "expected a general-purpose register"));
        }
        (Transfer::General(_), Transfer::General(_)) | (Transfer::Fp(_), Transfer::Fp(_)) => {
            return Err(p.error(rt2.column(), "register width mismatch"));
        }
        _ => return Err(p.error(rt2.column(), "register class mismatch")),
    };
    comma(p)?;
    let base = base_register(p)?;
    let mode = AddressMode::parse(p, base, None)?;
    if mode.writes_back() && (rt.overlaps_base(base) || rt2.overlaps_base(base)) {
        return Err(p.error(base.column, "unpredictable writeback overlap"));
    }
    if load && rt.number() == rt2.number() {
        return Err(p.error(rt2.column(), "unpredictable load pair overlap"));
    }
    let (mode, offset) = match mode {
        AddressMode::Offset(offset) => (2, offset),
        AddressMode::PreIndex(offset) => (3, offset),
        AddressMode::PostIndex(offset) => (1, offset),
        AddressMode::Register(_) => unreachable!("no register offset in a pair"),
    };
    let imm7 = offset.field(p, scale, -64, 63)? & 127;
    Ok((opc << 30)
        | 0x2800_0000
        | (v << 26)
        | (mode << 23)
        | (u32::from(load) << 22)
        | (imm7 << 15)
        | (rt2.number() << 10)
        | (base.number << 5)
        | rt.number())
}

pub(super) fn atomic(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<u32> {
    let (name, fixed_size) = match op.text.as_bytes().last() {
        Some(b'b') => (&op.text[..op.text.len() - 1], Some(0)),
        Some(b'h') => (&op.text[..op.text.len() - 1], Some(1)),
        _ => (op.text, None),
    };
    // C6.2.191–193/196–198/265–267/401–403/409–411/449–451.
    let (ordered, load, acqrel) = match name {
        "ldxr" => (false, true, false),
        "stxr" => (false, false, false),
        "ldaxr" => (false, true, true),
        "stlxr" => (false, false, true),
        "ldar" => (true, true, true),
        "stlr" => (true, false, true),
        _ => unreachable!("exclusive/ordered load/store dispatch"),
    };
    let status = if !load && !ordered {
        let status = next_general(p, Some(false))?;
        comma(p)?;
        Some(status)
    } else {
        None
    };
    let rt = next_general(p, fixed_size.map(|_| false))?;
    comma(p)?;
    let base = base_register(p)?;
    if p.take(b',') {
        Offset::parse(p)?.field(p, 0, 0, 0)?;
    }
    p.expect(b']', "expected ']' after address")?;
    if let Some(status) = status {
        if status.number == rt.number || (!base.stack && status.number == base.number) {
            return Err(p.error(status.column, "unpredictable exclusive status overlap"));
        }
    }
    let size = fixed_size.unwrap_or(2 + u32::from(rt.wide));
    let rs = status.map_or(31, |reg| reg.number);
    Ok((size << 30)
        | 0x0800_7c00
        | (u32::from(ordered) << 23)
        | (u32::from(load) << 22)
        | (rs << 16)
        | (u32::from(acqrel) << 15)
        | (base.number << 5)
        | rt.number)
}

pub(super) fn barrier(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<u32> {
    // C6.2.150/152/177: CRm[11:8] and opc[7:5].
    let opc = match op.text {
        "dsb" => 4,
        "dmb" => 5,
        "isb" => 6,
        _ => unreachable!("barrier dispatch"),
    };
    let crm = if p.peek() == b'#' {
        immediate(p, 15)?
    } else if op.text == "isb" && p.at_end() {
        15
    } else {
        let option = p.word();
        let name = option.text.to_ascii_lowercase();
        if let ("dsb", Some(scope)) = (op.text, name.strip_suffix("nxs")) {
            // DSB's FEAT_XS encoding uses imm2[11:10] and op2=001.
            let imm2 = match scope {
                "osh" => 0,
                "nsh" => 1,
                "ish" => 2,
                "sy" => 3,
                _ => return Err(p.error(option.column, "invalid barrier option")),
            };
            return Ok(0xd503_323f | (imm2 << 10));
        }
        match name.as_str() {
            "sy" => 15,
            _ if op.text == "isb" => return Err(p.error(option.column, "invalid barrier option")),
            "oshld" => 1,
            "oshst" => 2,
            "osh" => 3,
            "nshld" => 5,
            "nshst" => 6,
            "nsh" => 7,
            "ishld" => 9,
            "ishst" => 10,
            "ish" => 11,
            "ld" => 13,
            "st" => 14,
            _ => return Err(p.error(option.column, "invalid barrier option")),
        }
    };
    Ok(0xd503_301f | (crm << 8) | (opc << 5))
}
