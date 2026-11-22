//! Legacy SSE/SSE2 and x87 forms from the Intel SDM Vol. 2 opcode tables.

use super::{Address, Encoder, Operand, Reg};
use crate::Result;

impl<'src> Encoder<'_, 'src, '_> {
    fn vector_operand(&mut self) -> Result<Operand<'src>> {
        const XMM: [&str; 16] = [
            "xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6", "xmm7", "xmm8", "xmm9",
            "xmm10", "xmm11", "xmm12", "xmm13", "xmm14", "xmm15",
        ];
        let start = self.p.column;
        let name = self.p.word();
        if let Some(number) = XMM.iter().position(|&candidate| candidate == name.text) {
            return Ok(Operand {
                column: name.column,
                reg: Reg::new(number as u8, 16),
                memory: Address::default(),
            });
        }
        self.p.column = start;
        self.operand()
    }

    fn match_vector_rm(&self, rm: Operand<'_>, width: usize) -> Result<()> {
        if rm.is_memory() {
            rm.match_width(self.p, width)?;
        } else if rm.reg.width != 16 {
            return Err(self
                .p
                .error(rm.column, "expected an XMM register or memory"));
        }
        Ok(())
    }

    // The high byte of the opcode holds the mandatory legacy prefix.
    fn vector_pair(&mut self, load: u32, store: u32, width: usize, immediate: bool) -> Result<()> {
        let dest = self.vector_operand()?;
        self.p.expect(b',', "expected ',' between SSE operands")?;
        let src = self.vector_operand()?;
        let (opcode, xmm, rm) = if store != 0 && dest.is_memory() {
            (store, src.reg, dest)
        } else {
            (load, dest.reg, src)
        };
        if xmm.width != 16 {
            return Err(self
                .p
                .error(dest.column, "SSE instruction requires an XMM register"));
        }
        self.match_vector_rm(rm, width)?;
        let value = if immediate {
            self.p.expect(b',', "expected ',' before SSE immediate")?;
            if self.p.peek() == b'-' {
                return Err(self
                    .p
                    .error(self.p.column, "expected an unsigned SSE immediate"));
            }
            self.p.integer(1, false)?
        } else {
            0
        };
        let trailing = usize::from(immediate);
        self.modrm(opcode, xmm, rm, trailing)?;
        self.emit_word(value, trailing)
    }

    fn movd_q(&mut self, quad: bool) -> Result<()> {
        let dest = self.vector_operand()?;
        self.p
            .expect(b',', "expected ',' between SSE move operands")?;
        let src = self.vector_operand()?;
        let width = if quad { 8 } else { 4 };
        let load = dest.reg.width == 16;
        let (mut xmm, rm) = if load {
            (dest.reg, src)
        } else {
            (src.reg, dest)
        };
        if xmm.width != 16 {
            return Err(self
                .p
                .error(dest.column, "SSE move requires an XMM register"));
        }
        if quad && (rm.is_memory() || rm.reg.width == 16) {
            // MOVQ xmm,xmm/m64: F3 0F 7E. MOVQ m64,xmm: 66 0F D6.
            self.match_vector_rm(rm, width)?;
            self.modrm(if load { 0xf30f7e } else { 0x660fd6 }, xmm, rm, 0)
        } else {
            rm.match_width(self.p, width)?;
            xmm.width = width;
            self.modrm(if load { 0x660f6e } else { 0x660f7e }, xmm, rm, 0)
        }
    }

    fn scalar_integer(&mut self, opcode: u32, scalar_width: usize, to_integer: bool) -> Result<()> {
        let dest = self.vector_operand()?;
        self.p
            .expect(b',', "expected ',' between conversion operands")?;
        let src = self.vector_operand()?;
        let mut reg = dest.reg;
        if to_integer {
            if !matches!(reg.width, 4 | 8) {
                return Err(self.p.error(
                    dest.column,
                    "conversion requires a 32-bit or 64-bit integer register",
                ));
            }
            self.match_vector_rm(src, scalar_width)?;
        } else {
            if reg.width != 16 {
                return Err(self
                    .p
                    .error(dest.column, "conversion requires an XMM destination"));
            }
            reg.width = src.explicit_width(self.p)?;
            if !matches!(reg.width, 4 | 8) {
                return Err(self.p.error(
                    src.column,
                    "conversion requires a 32-bit or 64-bit integer source",
                ));
            }
        }
        self.modrm(opcode, reg, src, 0)
    }

    pub(super) fn sse(&mut self) -> Result<bool> {
        let name = self.op.text;
        let (opcode, store, width, immediate) = match name {
            "movd" | "movq" => {
                self.movd_q(name == "movq")?;
                return Ok(true);
            }
            "ldmxcsr" | "stmxcsr" => {
                let rm = self.required_memory(4)?;
                let digit = if name == "ldmxcsr" { 2 } else { 3 };
                self.modrm(0x0fae, Reg::new(digit, 0), rm, 0)?;
                return Ok(true);
            }
            "cvtsi2ss" | "cvtsi2sd" | "cvtss2si" | "cvtsd2si" | "cvttss2si" | "cvttsd2si" => {
                let (opcode, width, to_integer) = match name {
                    "cvtsi2ss" => (0xf30f2a, 4, false),
                    "cvtsi2sd" => (0xf20f2a, 8, false),
                    "cvtss2si" => (0xf30f2d, 4, true),
                    "cvtsd2si" => (0xf20f2d, 8, true),
                    "cvttss2si" => (0xf30f2c, 4, true),
                    _ => (0xf20f2c, 8, true),
                };
                self.scalar_integer(opcode, width, to_integer)?;
                return Ok(true);
            }
            "movss" => (0xf30f10, 0xf30f11, 4, false),
            "movsd" => (0xf20f10, 0xf20f11, 8, false),
            "movups" => (0x0f10, 0x0f11, 16, false),
            "movupd" => (0x660f10, 0x660f11, 16, false),
            "movaps" => (0x0f28, 0x0f29, 16, false),
            "movapd" => (0x660f28, 0x660f29, 16, false),
            "movdqa" => (0x660f6f, 0x660f7f, 16, false),
            "movdqu" => (0xf30f6f, 0xf30f7f, 16, false),
            "comiss" => (0x0f2f, 0, 4, false),
            "comisd" => (0x660f2f, 0, 8, false),
            "ucomiss" => (0x0f2e, 0, 4, false),
            "ucomisd" => (0x660f2e, 0, 8, false),
            "cvtss2sd" => (0xf30f5a, 0, 4, false),
            "cvtsd2ss" => (0xf20f5a, 0, 8, false),
            "shufps" => (0x0fc6, 0, 16, true),
            "shufpd" => (0x660fc6, 0, 16, true),
            "pshufd" => (0x660f70, 0, 16, true),
            "punpcklbw" => (0x660f60, 0, 16, false),
            "punpcklwd" => (0x660f61, 0, 16, false),
            "punpckldq" => (0x660f62, 0, 16, false),
            "punpcklqdq" => (0x660f6c, 0, 16, false),
            "unpcklps" => (0x0f14, 0, 16, false),
            "unpcklpd" => (0x660f14, 0, 16, false),
            "pcmpeqb" => (0x660f74, 0, 16, false),
            "pcmpeqw" => (0x660f75, 0, 16, false),
            "pcmpeqd" => (0x660f76, 0, 16, false),
            "paddb" => (0x660ffc, 0, 16, false),
            "paddw" => (0x660ffd, 0, 16, false),
            "paddd" => (0x660ffe, 0, 16, false),
            "paddq" => (0x660fd4, 0, 16, false),
            "psubb" => (0x660ff8, 0, 16, false),
            "psubw" => (0x660ff9, 0, 16, false),
            "psubd" => (0x660ffa, 0, 16, false),
            "psubq" => (0x660ffb, 0, 16, false),
            "pand" => (0x660fdb, 0, 16, false),
            "pandn" => (0x660fdf, 0, 16, false),
            "por" => (0x660feb, 0, 16, false),
            "pxor" => (0x660fef, 0, 16, false),
            _ => {
                let (stem, suffix) = name.split_at(name.len().saturating_sub(2));
                let (opcode, scalar) = match stem {
                    "add" => (0x0f58, true),
                    "sub" => (0x0f5c, true),
                    "mul" => (0x0f59, true),
                    "div" => (0x0f5e, true),
                    "sqrt" => (0x0f51, true),
                    "min" => (0x0f5d, true),
                    "max" => (0x0f5f, true),
                    "cmp" => (0x0fc2, true),
                    "and" => (0x0f54, false),
                    "andn" => (0x0f55, false),
                    "or" => (0x0f56, false),
                    "xor" => (0x0f57, false),
                    _ => return Ok(false),
                };
                let (prefix, width) = match suffix {
                    "ps" => (0, 16),
                    "pd" => (0x660000, 16),
                    "ss" if scalar => (0xf30000, 4),
                    "sd" if scalar => (0xf20000, 8),
                    _ => return Ok(false),
                };
                (prefix | opcode, 0, width, stem == "cmp")
            }
        };
        self.vector_pair(opcode, store, width, immediate)?;
        Ok(true)
    }

    fn take_x87_register(&mut self) -> Result<Option<u32>> {
        let start = self.p.column;
        let name = self.p.word();
        match name.text.as_bytes() {
            b"st" => {
                if !self.p.take(b'(') {
                    return Ok(Some(0));
                }
                let number = self.p.count()?;
                if number > 7 {
                    return Err(self
                        .p
                        .error(name.column, "x87 stack register index exceeds 7"));
                }
                self.p
                    .expect(b')', "expected ')' after x87 register index")?;
                Ok(Some(number as u32))
            }
            [b's', b't', number @ b'0'..=b'7'] => Ok(Some(u32::from(number - b'0'))),
            _ => {
                self.p.column = start;
                Ok(None)
            }
        }
    }

    fn x87_register(&mut self) -> Result<u32> {
        self.take_x87_register()?.ok_or_else(|| {
            self.p
                .error(self.p.column, "expected an x87 stack register")
        })
    }

    // The width selects the opcode or digit without setting 66 or REX.W.
    fn x87_memory(&mut self, rm: Operand<'_>, form: u32) -> Result<()> {
        if form == 0 {
            return Err(self.p.error(rm.column, "unsupported x87 memory width"));
        }
        self.modrm(form >> 4, Reg::new((form & 7) as u8, 0), rm, 0)
    }

    fn x87_load_store(&mut self) -> Result<bool> {
        // Memory rows are m16, m32, m64, m80 (opcode then ModR/M digit).
        let (opcode, forms) = match self.op.text {
            "fld" => (0xd9c0, [0, 0xd90, 0xdd0, 0xdb5]),
            "fst" => (0xddd0, [0, 0xd92, 0xdd2, 0]),
            "fstp" => (0xddd8, [0, 0xd93, 0xdd3, 0xdb7]),
            "fild" => (0, [0xdf0, 0xdb0, 0xdf5, 0]),
            "fist" => (0, [0xdf2, 0xdb2, 0, 0]),
            "fistp" => (0, [0xdf3, 0xdb3, 0xdf7, 0]),
            "fcom" => (0xd8d0, [0, 0xd82, 0xdc2, 0]),
            "fcomp" => (0xd8d8, [0, 0xd83, 0xdc3, 0]),
            "fiadd" => (0, [0xde0, 0xda0, 0, 0]),
            "fimul" => (0, [0xde1, 0xda1, 0, 0]),
            "ficom" => (0, [0xde2, 0xda2, 0, 0]),
            "ficomp" => (0, [0xde3, 0xda3, 0, 0]),
            "fisub" => (0, [0xde4, 0xda4, 0, 0]),
            "fisubr" => (0, [0xde5, 0xda5, 0, 0]),
            "fidiv" => (0, [0xde6, 0xda6, 0, 0]),
            "fidivr" => (0, [0xde7, 0xda7, 0, 0]),
            "fucom" => (0xdde0, [0; 4]),
            "fucomp" => (0xdde8, [0; 4]),
            "fxch" => (0xd9c8, [0; 4]),
            "ffree" => (0xddc0, [0; 4]),
            _ => return Ok(false),
        };
        let mut register = self.take_x87_register()?;
        if register.is_none()
            && self.p.at_end()
            && matches!(self.op.text, "fxch" | "fcom" | "fcomp" | "fucom" | "fucomp")
        {
            register = Some(1);
        }
        if let Some(register) = register {
            if opcode == 0 {
                return Err(self
                    .p
                    .error(self.op.column, "x87 instruction requires a memory operand"));
            }
            self.opcode(opcode + register)?;
        } else {
            let rm = self.required_memory(0)?;
            let form = match rm.explicit_width(self.p)? {
                2 => forms[0],
                4 => forms[1],
                8 => forms[2],
                10 => forms[3],
                _ => 0,
            };
            self.x87_memory(rm, form)?;
        }
        Ok(true)
    }

    fn x87_arithmetic(&mut self) -> Result<bool> {
        let name = self.op.text;
        let stem = name.strip_suffix('p').unwrap_or(name);
        let pop = name != stem;
        let (digit, dest_digit) = match stem {
            "fadd" => (0, 0),
            "fmul" => (1, 1),
            "fsub" => (4, 5),
            "fsubr" => (5, 4),
            "fdiv" => (6, 7),
            "fdivr" => (7, 6),
            _ => return Ok(false),
        };
        let dest_i = 0xc0 + 8 * dest_digit;
        if pop && self.p.at_end() {
            self.opcode(0xde00 + dest_i + 1)?;
            return Ok(true);
        }
        if let Some(dest) = self.take_x87_register()? {
            self.p.expect(b',', "expected ',' between x87 operands")?;
            let src = self.x87_register()?;
            if src == 0 && (dest != 0 || pop) {
                self.opcode(if pop { 0xde00 } else { 0xdc00 } + dest_i + dest)?;
            } else if !pop && dest == 0 {
                self.opcode(0xd8c0 + 8 * digit + src)?;
            } else {
                return Err(self.p.error(
                    self.op.column,
                    "x87 arithmetic requires ST(0) as an operand",
                ));
            }
        } else {
            if pop {
                return Err(self
                    .p
                    .error(self.p.column, "expected an x87 stack destination"));
            }
            let rm = self.required_memory(0)?;
            let form = match rm.explicit_width(self.p)? {
                4 => 0xd80 | digit,
                8 => 0xdc0 | digit,
                _ => 0,
            };
            self.x87_memory(rm, form)?;
        }
        Ok(true)
    }

    pub(super) fn x87(&mut self) -> Result<bool> {
        match self.op.text {
            "fcomi" | "fcomip" | "fucomi" | "fucomip" => {
                let opcode = match self.op.text {
                    "fcomi" => 0xdbf0,
                    "fcomip" => 0xdff0,
                    "fucomi" => 0xdbe8,
                    _ => 0xdfe8,
                };
                if self.x87_register()? != 0 {
                    return Err(self
                        .p
                        .error(self.op.column, "x87 flag comparison requires ST(0) first"));
                }
                self.p.expect(b',', "expected ',' between x87 operands")?;
                let reg = self.x87_register()?;
                self.opcode(opcode + reg)?;
            }
            "fldcw" | "fnstcw" | "fstcw" => {
                let rm = self.required_memory(2)?;
                if self.op.text == "fstcw" {
                    self.emit(&[0x9b])?;
                }
                self.x87_memory(
                    rm,
                    if self.op.text == "fldcw" {
                        0xd95
                    } else {
                        0xd97
                    },
                )?;
            }
            "fnstsw" | "fstsw" => {
                let rm = self.operand()?;
                if self.op.text == "fstsw" {
                    self.emit(&[0x9b])?;
                }
                if rm.reg.width == 2 && rm.reg.number == 0 {
                    self.opcode(0xdfe0)?;
                } else {
                    if !rm.is_memory() {
                        return Err(self
                            .p
                            .error(rm.column, "x87 status destination requires AX or m16"));
                    }
                    rm.match_width(self.p, 2)?;
                    self.x87_memory(rm, 0xdd7)?;
                }
            }
            name => {
                let opcode = match name {
                    "fwait" => 0x9b,
                    "fninit" => 0xdbe3,
                    "finit" => 0x9bdbe3,
                    "fnclex" => 0xdbe2,
                    "fclex" => 0x9bdbe2,
                    "fld1" => 0xd9e8,
                    "fldl2t" => 0xd9e9,
                    "fldl2e" => 0xd9ea,
                    "fldpi" => 0xd9eb,
                    "fldlg2" => 0xd9ec,
                    "fldln2" => 0xd9ed,
                    "fldz" => 0xd9ee,
                    "fchs" => 0xd9e0,
                    "fabs" => 0xd9e1,
                    "ftst" => 0xd9e4,
                    "fxam" => 0xd9e5,
                    "fsqrt" => 0xd9fa,
                    "frndint" => 0xd9fc,
                    "fprem" => 0xd9f8,
                    "fprem1" => 0xd9f5,
                    "fcompp" => 0xded9,
                    "fucompp" => 0xdae9,
                    _ => return Ok(self.x87_load_store()? || self.x87_arithmetic()?),
                };
                self.opcode(opcode)?;
            }
        }
        Ok(true)
    }
}
