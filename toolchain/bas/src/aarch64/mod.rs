//! A64 instructions from Arm ARM DDI 0487 M.b, Chapters C6 and C7.

mod fp;
mod integer;
mod memory;

use bobj::elf::aarch64::{
    R_AARCH64_CALL26, R_AARCH64_CONDBR19, R_AARCH64_JUMP26, R_AARCH64_TSTBR14,
};

use crate::parser::{Parser, Token};
use crate::{Location, Result};

#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum BranchKind {
    Adr = 0,     // Local fixup only.
    Literal = 1, // Local fixup only.
    Jump26 = R_AARCH64_JUMP26,
    Call26 = R_AARCH64_CALL26,
    Cond19 = R_AARCH64_CONDBR19,
    Test14 = R_AARCH64_TSTBR14,
}

impl BranchKind {
    pub(crate) fn patch(self, word: u32, displacement: i128, at: Location) -> Result<u32> {
        // AAELF64 §5.7.6: signed S + A - P, scaled by four and inserted
        // into the instruction's immediate field.
        let (width, shift) = match self {
            Self::Jump26 | Self::Call26 => (26, 0),
            Self::Cond19 | Self::Literal => (19, 5),
            Self::Test14 => (14, 5),
            Self::Adr => {
                // C6.2.12: signed byte displacement in immhi:immlo.
                if !(-(1 << 20)..(1 << 20)).contains(&displacement) {
                    return Err(at.error("ADR target out of range"));
                }
                let imm = displacement as u32 & 0x1f_ffff;
                return Ok(word | ((imm & 3) << 29) | ((imm >> 2) << 5));
            }
        };
        let (alignment_error, range_error) = if matches!(self, Self::Literal) {
            // C6.2.216 and C7.2.231: signed imm19 scaled by four.
            (
                "literal target must be four-byte aligned",
                "literal target out of range",
            )
        } else {
            (
                "branch target must be four-byte aligned",
                "branch target out of range",
            )
        };
        if displacement % 4 != 0 {
            return Err(at.error(alignment_error));
        }
        let scaled = displacement / 4;
        let limit = 1i128 << (width - 1);
        if !(-limit..limit).contains(&scaled) {
            return Err(at.error(range_error));
        }
        let mask = (1u32 << width) - 1;
        Ok((word & !(mask << shift)) | ((scaled as u32 & mask) << shift))
    }
}

#[derive(Clone, Copy)]
struct Register {
    number: u32,
    wide: bool,
    stack: bool,
    column: usize,
}

impl Register {
    fn bits(&self) -> u32 {
        if self.wide {
            64
        } else {
            32
        }
    }

    fn sf(&self) -> u32 {
        u32::from(self.wide) << 31
    }
}

fn register(p: &mut Parser<'_, '_>, wide_only: bool) -> Result<Register> {
    let token = p.word();
    if matches!(token.text, "sp" | "wsp") || (wide_only && token.text.starts_with('w')) {
        return Err(p.error(
            token.column,
            "expected an AArch64 register of the required width",
        ));
    }
    parse_register(p, token)
}

fn parse_register(p: &Parser<'_, '_>, token: Token<'_>) -> Result<Register> {
    if matches!(token.text, "sp" | "wsp") {
        return Ok(Register {
            number: 31,
            wide: token.text == "sp",
            stack: true,
            column: token.column,
        });
    }
    let (name, wide) = if let Some(name) = token.text.strip_prefix('x') {
        (name, true)
    } else if let Some(name) = token.text.strip_prefix('w') {
        (name, false)
    } else {
        return Err(p.error(
            token.column,
            "expected an AArch64 register of the required width",
        ));
    };
    let number = if name == "zr" {
        31
    } else {
        name.parse::<u32>()
            .ok()
            .filter(|&number| number < 31)
            .ok_or_else(|| p.error(token.column, "invalid AArch64 register"))?
    };
    Ok(Register {
        number,
        wide,
        stack: false,
        column: token.column,
    })
}

fn immediate(p: &mut Parser<'_, '_>, max: u32) -> Result<u32> {
    p.expect(b'#', "expected '#' before immediate")?;
    p.peek();
    let column = p.column;
    let value = p.magnitude()?;
    if value > u64::from(max) {
        return Err(p.error(column, "immediate out of range"));
    }
    Ok(value as u32)
}

fn condition(name: &str) -> Option<u32> {
    // C6.2.34: the cond field in encoding order.
    match name {
        "hs" => Some(2), // CS
        "lo" => Some(3), // CC
        _ => [
            "eq", "ne", "cs", "cc", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le",
            "al", "nv",
        ]
        .iter()
        .position(|&condition| condition == name)
        .map(|index| index as u32),
    }
}

fn branch(p: &mut Parser<'_, '_>, kind: BranchKind, word: u32) -> Result<()> {
    let label = p.word();
    p.branch_aarch64(label, kind, word)
}

pub(crate) fn instruction(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<()> {
    let word = match op.text {
        "adr" => return integer::adr(p),
        "ldr" | "str" | "ldur" | "stur" | "ldrb" | "strb" | "ldurb" | "sturb" | "ldrh" | "strh"
        | "ldurh" | "sturh" | "ldrsb" | "ldursb" | "ldrsh" | "ldursh" | "ldrsw" | "ldursw" => {
            return memory::single(p, op)
        }
        "ldp" | "stp" | "ldpsw" => memory::pair(p, op)?,
        "ldxr" | "ldxrb" | "ldxrh" | "stxr" | "stxrb" | "stxrh" | "ldaxr" | "ldaxrb" | "ldaxrh"
        | "stlxr" | "stlxrb" | "stlxrh" | "ldar" | "ldarb" | "ldarh" | "stlr" | "stlrb"
        | "stlrh" => memory::atomic(p, op)?,
        "dmb" | "dsb" | "isb" => memory::barrier(p, op)?,
        "fmov" | "fadd" | "fsub" | "fmul" | "fdiv" | "fneg" | "fabs" | "fsqrt" | "fmadd"
        | "fcmp" | "fcmpe" | "fcvt" | "fcvtzs" | "fcvtzu" | "scvtf" | "ucvtf" | "fcsel" => {
            fp::encode(p, op)?
        }
        "svc" => 0xd400_0001 | (immediate(p, 0xffff)? << 5), // C6.2.465
        "nop" => 0xd503_201f,                                // C6.2.298
        "br" => 0xd61f_0000 | (register(p, true)?.number << 5), // C6.2.46
        "blr" => 0xd63f_0000 | (register(p, true)?.number << 5), // C6.2.44
        "ret" => {
            // C6.2.337: Rn defaults to X30.
            let rn = if p.at_end() {
                30
            } else {
                register(p, true)?.number
            };
            0xd65f_0000 | (rn << 5)
        }
        "b" => return branch(p, BranchKind::Jump26, 0x1400_0000), // C6.2.35
        "bl" => return branch(p, BranchKind::Call26, 0x9400_0000), // C6.2.43
        "cbz" | "cbnz" => {
            // C6.2.78 / C6.2.77: sf, op[24], imm19[23:5], Rt[4:0].
            let rt = register(p, false)?;
            p.expect(b',', "expected ',' before branch target")?;
            let nonzero = u32::from(op.text == "cbnz") << 24;
            return branch(
                p,
                BranchKind::Cond19,
                0x3400_0000 | rt.sf() | nonzero | rt.number,
            );
        }
        "tbz" | "tbnz" => {
            // C6.2.478 / C6.2.477: b5[31], op[24], b40[23:19], imm14[18:5], Rt[4:0].
            let rt = register(p, false)?;
            p.expect(b',', "expected ',' before bit number")?;
            let bit = immediate(p, rt.bits() - 1)?;
            p.expect(b',', "expected ',' before branch target")?;
            let nonzero = u32::from(op.text == "tbnz") << 24;
            let bit_field = ((bit >> 5) << 31) | ((bit & 31) << 19);
            return branch(
                p,
                BranchKind::Test14,
                0x3600_0000 | bit_field | nonzero | rt.number,
            );
        }
        _ => {
            if let Some(suffix) = op.text.strip_prefix("b.") {
                let cond = condition(suffix)
                    .ok_or_else(|| p.error(op.column, "unknown AArch64 branch condition"))?;
                return branch(p, BranchKind::Cond19, 0x5400_0000 | cond);
            }
            integer::encode(p, op)?
        }
    };
    p.emit_aarch64(op.column, word)
}
