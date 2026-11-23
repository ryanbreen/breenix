//! Scalar floating-point encodings from DDI 0487 M.b, Chapter C7.

use super::{condition, parse_register, Register};
use crate::parser::{Parser, Token};
use crate::Result;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Precision {
    Single,
    Double,
    Quad,
}

impl Precision {
    pub(super) fn of(token: Token<'_>) -> Option<Self> {
        match token.text.as_bytes().first()? {
            b's' => Some(Self::Single),
            b'd' => Some(Self::Double),
            b'q' => Some(Self::Quad),
            _ => None,
        }
    }

    pub(super) fn pair_opc(self) -> u32 {
        match self {
            Self::Single => 0,
            Self::Double => 1,
            Self::Quad => 2,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct FpRegister {
    pub(super) number: u32,
    pub(super) precision: Precision,
    pub(super) column: usize,
}

impl FpRegister {
    pub(super) fn parse(p: &Parser<'_, '_>, token: Token<'_>) -> Result<Self> {
        let precision = Precision::of(token)
            .ok_or_else(|| p.error(token.column, "expected S, D or Q register"))?;
        let number = token.text[1..]
            .parse::<u32>()
            .ok()
            .filter(|&number| number < 32)
            .ok_or_else(|| p.error(token.column, "invalid SIMD/FP register"))?;
        Ok(Self {
            number,
            precision,
            column: token.column,
        })
    }

    fn scalar(p: &Parser<'_, '_>, token: Token<'_>) -> Result<Self> {
        if !matches!(
            Precision::of(token),
            Some(Precision::Single | Precision::Double)
        ) {
            return Err(p.error(token.column, "expected S or D register"));
        }
        Self::parse(p, token)
    }

    fn next(p: &mut Parser<'_, '_>) -> Result<Self> {
        let token = p.word();
        Self::scalar(p, token)
    }

    fn same(self, p: &mut Parser<'_, '_>) -> Result<u32> {
        let other = Self::next(p)?;
        if self.precision != other.precision {
            return Err(p.error(other.column, "register width mismatch"));
        }
        Ok(other.number)
    }

    fn ftype(self) -> u32 {
        self.precision.pair_opc() << 22
    }
}

fn general(p: &Parser<'_, '_>, token: Token<'_>) -> Result<Register> {
    let reg = parse_register(p, token)?;
    if reg.stack {
        return Err(p.error(reg.column, "stack pointer is not allowed"));
    }
    Ok(reg)
}

fn comma(p: &mut Parser<'_, '_>) -> Result<()> {
    p.expect(b',', "expected ',' before next operand")
}

const DECIMAL_UNIT: u32 = 10_000_000;

// Parse an exact magnitude in units of 10^-7. The constants in
// Table C2-2 have denominators that divide 128.
fn decimal_units(mantissa: &str, exponent: i32) -> Option<u32> {
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }

    let digits = format!("{whole}{fraction}");
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    let coefficient = significant.trim_end_matches('0');
    let trailing = i32::try_from(significant.len() - coefficient.len()).ok()?;
    let power = exponent
        .checked_sub(i32::try_from(fraction.len()).ok()?)?
        .checked_add(trailing)?
        .checked_add(7)?;
    let unit = 10u32.checked_pow(u32::try_from(power).ok()?)?;
    coefficient.parse::<u32>().ok()?.checked_mul(unit)
}

fn fp_immediate(p: &mut Parser<'_, '_>, message: &'static str) -> Result<(u32, bool, usize)> {
    p.expect(b'#', "expected '#' before FP immediate")?;
    p.peek();
    let column = p.column;
    let negative = p.take(b'-');
    if !negative {
        p.take(b'+');
    }
    let value = p.word();
    let (mantissa, exponent) = if let Some((mantissa, exponent)) = value.text.split_once(['e', 'E'])
    {
        let exponent = if exponent.is_empty() {
            let sign = if p.take(b'-') {
                -1
            } else if p.take(b'+') {
                1
            } else {
                0
            };
            let digits = p.word();
            if sign == 0 || !digits.text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(p.error(column, message));
            }
            digits
                .text
                .parse::<i32>()
                .ok()
                .and_then(|n| n.checked_mul(sign))
        } else {
            exponent.parse::<i32>().ok()
        };
        (mantissa, exponent.ok_or_else(|| p.error(column, message))?)
    } else {
        (value.text, 0)
    };
    let units = decimal_units(mantissa, exponent).ok_or_else(|| p.error(column, message))?;
    Ok((units, negative, column))
}

fn move_immediate(p: &mut Parser<'_, '_>, dest: FpRegister) -> Result<u32> {
    const ERROR: &str = "unencodable FMOV immediate";
    let (units, negative, column) = fp_immediate(p, ERROR)?;
    // C2.2.3 Table C2-1: exponent = UInt(NOT(b):c:d)-3, mantissa
    // = (16+efgh)/16. Multiplying by DECIMAL_UNIT keeps the entire
    // range exact even on a soft-float assembler host.
    let imm7 = (0..128)
        .find(|&imm| {
            let biased_exponent = (imm >> 4) ^ 4;
            let mantissa = 16 + (imm & 15);
            units == mantissa * (DECIMAL_UNIT / 128) * (1 << biased_exponent)
        })
        .ok_or_else(|| p.error(column, ERROR))?;
    let imm8 = (u32::from(negative) << 7) | imm7;
    Ok(0x1e20_1000 | dest.ftype() | (imm8 << 13) | dest.number)
}

fn move_register(p: &mut Parser<'_, '_>) -> Result<u32> {
    let dest = p.word();
    if dest.text.starts_with(['w', 'x']) {
        let gp = general(p, dest)?;
        comma(p)?;
        let fp = FpRegister::next(p)?;
        if gp.wide != (fp.precision == Precision::Double) {
            return Err(p.error(fp.column, "register width mismatch"));
        }
        // C7.2.157: sf, ftype, rmode=00, opcode=110 (FP to GPR).
        return Ok(0x1e26_0000 | gp.sf() | fp.ftype() | (fp.number << 5) | gp.number);
    }

    let fp = FpRegister::scalar(p, dest)?;
    comma(p)?;
    if p.peek() == b'#' {
        return move_immediate(p, fp);
    }
    let source = p.word();
    if source.text.starts_with(['w', 'x']) {
        let gp = general(p, source)?;
        if gp.wide != (fp.precision == Precision::Double) {
            return Err(p.error(gp.column, "register width mismatch"));
        }
        // C7.2.157: opcode=111 (GPR to FP).
        Ok(0x1e27_0000 | gp.sf() | fp.ftype() | (gp.number << 5) | fp.number)
    } else {
        // C7.2.156: same-precision register move.
        let other = FpRegister::scalar(p, source)?;
        if fp.precision != other.precision {
            return Err(p.error(other.column, "register width mismatch"));
        }
        Ok(0x1e20_4000 | fp.ftype() | (other.number << 5) | fp.number)
    }
}

fn convert(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<u32> {
    if op.text == "fcvt" {
        // C7.2.73: ftype is the source precision and opc the destination.
        let dest = FpRegister::next(p)?;
        comma(p)?;
        let src = FpRegister::next(p)?;
        if src.precision == dest.precision {
            return Err(p.error(src.column, "FCVT requires different precisions"));
        }
        return Ok(0x1e22_4000
            | src.ftype()
            | (dest.precision.pair_opc() << 15)
            | (src.number << 5)
            | dest.number);
    }
    // C7.2.106/111/279/413: sf and ftype select widths independently.
    let dest = p.word();
    comma(p)?;
    let src = p.word();
    let (gp, fp, base, rd, rn) = match op.text {
        "fcvtzs" | "fcvtzu" => {
            let gp = general(p, dest)?;
            let fp = FpRegister::scalar(p, src)?;
            (gp, fp, 0x1e38_0000, gp.number, fp.number)
        }
        _ => {
            let fp = FpRegister::scalar(p, dest)?;
            let gp = general(p, src)?;
            (gp, fp, 0x1e22_0000, fp.number, gp.number)
        }
    };
    let unsigned = u32::from(matches!(op.text, "fcvtzu" | "ucvtf")) << 16;
    Ok(base | gp.sf() | fp.ftype() | unsigned | (rn << 5) | rd)
}

fn compare(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<u32> {
    let rn = FpRegister::next(p)?;
    comma(p)?;
    let operand = if p.peek() == b'#' {
        const ERROR: &str = "expected FP zero immediate";
        let (units, _, column) = fp_immediate(p, ERROR)?;
        if units != 0 {
            return Err(p.error(column, ERROR));
        }
        1 << 3
    } else {
        rn.same(p)? << 16
    };
    // C7.2.70/71: opcode2[4] signals and opcode2[3] selects zero.
    let signaling = u32::from(op.text == "fcmpe") << 4;
    Ok(0x1e20_2000 | rn.ftype() | operand | signaling | (rn.number << 5))
}

pub(super) fn encode(p: &mut Parser<'_, '_>, op: Token<'_>) -> Result<u32> {
    match op.text {
        "fmov" => return move_register(p),
        "fcvt" | "fcvtzs" | "fcvtzu" | "scvtf" | "ucvtf" => return convert(p, op),
        "fcmp" | "fcmpe" => return compare(p, op),
        _ => {}
    }

    let rd = FpRegister::next(p)?;
    comma(p)?;
    let rn = rd.same(p)?;
    let registers = rd.ftype() | (rn << 5) | rd.number;
    // One-source operations: C7.2.48/166/199.
    let unary = match op.text {
        "fabs" => Some(0x1e20_c000),
        "fneg" => Some(0x1e21_4000),
        "fsqrt" => Some(0x1e21_c000),
        _ => None,
    };
    if let Some(base) = unary {
        return Ok(base | registers);
    }
    comma(p)?;
    let rm = rd.same(p)?;
    let registers = registers | (rm << 16);
    let base = match op.text {
        // Two-source operations: C7.2.52/114/162/201.
        "fmul" => 0x1e20_0800,
        "fdiv" => 0x1e20_1800,
        "fadd" => 0x1e20_2800,
        "fsub" => 0x1e20_3800,
        "fmadd" => {
            comma(p)?;
            0x1f00_0000 | (rd.same(p)? << 10) // C7.2.120: Ra[14:10].
        }
        "fcsel" => {
            comma(p)?;
            let cond = p.word();
            let field = condition(cond.text)
                .ok_or_else(|| p.error(cond.column, "unknown AArch64 condition"))?;
            0x1e20_0c00 | (field << 12) // C7.2.72: cond[15:12].
        }
        _ => unreachable!("scalar FP dispatch"),
    };
    Ok(base | registers)
}
