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
    let value = p.count()?;
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

fn system_fields(op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    (op1 << 16) | (crn << 12) | (crm << 8) | (op2 << 5)
}

fn system_register(p: &Parser<'_, '_>, name: Token<'_>, write: bool) -> Result<u32> {
    // DDI 0487 M.b C6.2.286 and C6.2.288: op0=3 for these registers.
    // The remaining fields are op1, CRn, CRm, op2, followed by MSR availability.
    let (op1, crn, crm, op2, writable) = match name.text.to_ascii_lowercase().as_str() {
        "sctlr_el1" => (0, 1, 0, 0, true),         // D24.2.174
        "vbar_el1" => (0, 12, 0, 0, true),         // D24.2.213
        "ttbr0_el1" => (0, 2, 0, 0, true),         // D24.2.208
        "ttbr1_el1" => (0, 2, 0, 1, true),         // D24.2.211
        "tcr_el1" => (0, 2, 0, 2, true),           // D24.2.193
        "mair_el1" => (0, 10, 2, 0, true),         // D24.2.126
        "spsr_el1" => (0, 4, 0, 0, true),          // C5.2.20
        "elr_el1" => (0, 4, 0, 1, true),           // C5.2.5
        "spsr_el2" => (4, 4, 0, 0, true),          // C5.2.21
        "elr_el2" => (4, 4, 0, 1, true),           // C5.2.6
        "hcr_el2" => (4, 1, 1, 0, true),           // D24.2.61
        "cpacr_el1" => (0, 1, 0, 2, true),         // D24.2.35
        "tpidr_el1" => (0, 13, 0, 4, true),        // D24.2.204
        "esr_el1" => (0, 5, 2, 0, true),           // D24.2.44
        "far_el1" => (0, 6, 0, 0, true),           // D24.2.47
        "sp_el0" => (0, 4, 1, 0, true),            // C5.2.14
        "currentel" => (0, 4, 2, 2, false),        // C5.2.2
        "mpidr_el1" => (0, 0, 0, 5, false),        // D24.2.137
        "ctr_el0" => (3, 0, 0, 1, false),          // D24.2.41
        "id_aa64mmfr1_el1" => (0, 0, 7, 1, false), // D24.2.88
        "tpidr_el0" if !write => (3, 13, 0, 2, true), // D24.2.203
        "fpcr" if !write => (3, 4, 4, 0, true),       // C5.2.8
        "fpsr" if !write => (3, 4, 4, 1, true),       // C5.2.10
        _ if write => return Err(p.error(name.column, "unsupported MSR system register")),
        _ => return Err(p.error(name.column, "unsupported MRS system register")),
    };
    if write && !writable {
        return Err(p.error(name.column, "cannot write a read-only system register"));
    }
    Ok(0xd518_0000 | (u32::from(!write) << 21) | system_fields(op1, crn, crm, op2))
}

fn msr(p: &mut Parser<'_, '_>) -> Result<u32> {
    let name = p.word();
    p.expect(b',', "expected ',' after MSR destination")?;
    let op2 = if name.text.eq_ignore_ascii_case("daifset") {
        6
    } else if name.text.eq_ignore_ascii_case("daifclr") {
        7
    } else {
        let encoding = system_register(p, name, true)?;
        return Ok(encoding | register(p, true)?.number);
    };
    // C6.2.287: op1=3, CRn=4, CRm=imm4, Rt=31.
    Ok(0xd500_001f | system_fields(3, 4, immediate(p, 15)?, op2))
}

fn maintenance(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<u32> {
    let name = p.word();
    let (crn, crm, op2, has_operand) = match (op.text, name.text.to_ascii_lowercase().as_str()) {
        ("tlbi", "vmalle1") => (8, 7, 0, false),   // C5.5.77
        ("tlbi", "vmalle1is") => (8, 3, 0, false), // C5.5.78
        ("dc", "ivac") => (7, 6, 1, true),         // C5.3.37
        ("ic", "iallu") => (7, 5, 0, false),       // C5.3.39
        _ => return Err(p.error(name.column, "unsupported AArch64 maintenance operation")),
    };
    let rt = if has_operand {
        p.expect(b',', "expected ',' before DC register")?;
        register(p, true)?.number
    } else {
        if p.take(b',') {
            let rt = register(p, true)?;
            if rt.number != 31 {
                return Err(
                    p.error(rt.column, "maintenance operation requires XZR or no register")
                );
            }
        }
        31
    };
    // C6.2.474: SYS, with op0=1 and op1=0.
    Ok(0xd508_0000 | system_fields(0, crn, crm, op2) | rt)
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
            system_register(p, system, false)? | rt.number
        }
        "msr" => msr(p)?,
        "tlbi" | "dc" | "ic" => maintenance(p, op)?,
        "eret" => 0xd69f_03e0, // C6.2.157
        "wfi" => 0xd503_207f, // C6.2.503
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
