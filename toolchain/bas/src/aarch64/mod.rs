//! A64 instructions from Arm ARM DDI 0487 M.b, Chapters C6 and C7.

mod fp;
mod integer;
mod memory;
mod relocation;

pub(crate) use relocation::Relocation;

use crate::parser::{Parser, Token};
use crate::Result;

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

fn modifier<'src>(p: &mut Parser<'src, '_>) -> Result<Token<'src>> {
    p.expect(b':', "expected ':' before relocation modifier")?;
    let name = p.word();
    p.expect(b':', "expected ':' after relocation modifier")?;
    Ok(name)
}

fn branch(p: &mut Parser<'_, '_>, kind: Relocation, word: u32) -> Result<()> {
    let label = p.word();
    p.emit_aarch64_reference(label, kind, word)
}

pub(crate) fn instruction(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<()> {
    let word = match op.text {
        "adr" | "adrp" => return integer::adr(p, op),
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
        "mrs" => {
            let rt = register(p, true)?;
            p.expect(b',', "expected ',' before system register")?;
            let system = p.word();
            if !system.text.eq_ignore_ascii_case("tpidr_el0") {
                return Err(p.error(system.column, "unsupported MRS system register"));
            }
            // C6.2.286 and D24.2.203: op0=3, op1=3, CRn=13, CRm=0, op2=2.
            0xd53b_d040 | rt.number
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
        "b" => return branch(p, Relocation::Jump26, 0x1400_0000), // C6.2.35
        "bl" => return branch(p, Relocation::Call26, 0x9400_0000), // C6.2.43
        "cbz" | "cbnz" => {
            // C6.2.78 / C6.2.77: sf, op[24], imm19[23:5], Rt[4:0].
            let rt = register(p, false)?;
            p.expect(b',', "expected ',' before branch target")?;
            let nonzero = u32::from(op.text == "cbnz") << 24;
            return branch(
                p,
                Relocation::Cond19,
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
                Relocation::Test14,
                0x3600_0000 | bit_field | nonzero | rt.number,
            );
        }
        _ => {
            if let Some(suffix) = op.text.strip_prefix("b.") {
                let cond = condition(suffix)
                    .ok_or_else(|| p.error(op.column, "unknown AArch64 branch condition"))?;
                return branch(p, Relocation::Cond19, 0x5400_0000 | cond);
            }
            return integer::encode(p, op);
        }
    };
    p.emit_aarch64(op.column, word)
}
