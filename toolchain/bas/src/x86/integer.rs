//! Integer, branch and atomic forms from the Intel SDM Vol. 2 opcode tables.

use super::{operand_width, segment, starts_number, Encoder, Operand, Reg};
use crate::Result;

fn fits_sign_extended(value: u64, from: usize, to: usize) -> bool {
    // Compare the bit patterns at the destination width, including hex literals.
    let shift = (8 - from) * 8;
    let extended = (((value << shift) as i64) >> shift) as u64;
    ((value ^ extended) << ((8 - to) * 8)) == 0
}

impl Encoder<'_, '_, '_> {
    // MOV: B0/B8+rd, C6/C7 /0, 88–8B /r.
    fn mov_immediate(&mut self, dest: Operand<'_>) -> Result<()> {
        let width = dest.integer_width(self.p)?;
        let value = self.p.integer(width, false)?;
        let imm32 = width == 8 && fits_sign_extended(value, 4, 8);
        let immediate = if imm32 { 4 } else { width };
        if !dest.is_memory() && !imm32 {
            let rex = u8::from(dest.reg.number >= 8);
            self.prefixes(dest.reg, dest, rex, 0)?;
            let opcode = if width == 1 { 0xb0 } else { 0xb8 };
            self.emit(&[opcode | (dest.reg.number & 7)])?;
        } else {
            if immediate == 8 {
                return Err(self.p.error(
                    dest.column,
                    "qword memory move requires a sign-extended imm32",
                ));
            }
            let opcode = if width == 1 { 0xc6 } else { 0xc7 };
            self.modrm(opcode, Reg::new(0, width), dest, immediate)?;
        }
        self.emit_word(value, immediate)
    }

    fn mov(&mut self) -> Result<()> {
        let (first, delimiter, _) = self.p.lookahead();
        if first.text.starts_with("cr") {
            let control = self.p.word();
            self.p.expect(b',', "expected ',' between mov operands")?;
            let reg = self.operand()?;
            return self.control_move(control, reg, false);
        }
        if let Some(segment) = segment_register(first.text).filter(|_| delimiter != b':') {
            self.p.word();
            if segment == 1 {
                return Err(self.p.error(first.column, "mov cannot load cs"));
            }
            self.p.expect(b',', "expected ',' between mov operands")?;
            let src = self.operand()?;
            let width = if src.width() == 8 && self.bits() == 64 {
                8
            } else {
                src.match_width(self.p, 2)?;
                0
            };
            return self.modrm(0x8e, Reg::new(segment, width), src, 0);
        }
        let dest = self.operand()?;
        self.p.expect(b',', "expected ',' between mov operands")?;
        if starts_number(self.p.peek()) {
            return self.mov_immediate(dest);
        }
        let (next, delimiter, _) = self.p.lookahead();
        if next.text.starts_with("cr") {
            let control = self.p.word();
            return self.control_move(control, dest, true);
        }
        if let Some(segment) = segment_register(next.text).filter(|_| delimiter != b':') {
            self.p.word();
            let width = if dest.is_memory() {
                dest.match_width(self.p, 2)?;
                0
            } else if dest.reg.width >= 2 {
                dest.reg.width
            } else {
                return Err(self
                    .p
                    .error(dest.column, "segment selector requires at least 16 bits"));
            };
            return self.modrm(0x8c, Reg::new(segment, width), dest, 0);
        }
        let src = self.operand()?;
        let (opcode, reg, rm) = if src.is_memory() {
            (0x8a, dest.reg, src)
        } else {
            (0x88, src.reg, dest)
        };
        if reg.width == 0 {
            return Err(self.p.error(src.column, "mov requires a register operand"));
        }
        rm.match_width(self.p, reg.width)?;
        self.modrm(opcode + u32::from(reg.width != 1), reg, rm, 0)
    }

    // SDM Vol. 2B MOV to/from control registers: 0F 20/22 /r.
    fn control_move(
        &mut self,
        control: crate::parser::Token<'_>,
        reg: Operand<'_>,
        read: bool,
    ) -> Result<()> {
        let number = match control.text {
            "cr0" => 0,
            "cr3" => 3,
            "cr4" => 4,
            _ => return Err(self.p.error(control.column, "unsupported control register")),
        };
        let long = self.bits() == 64;
        if reg.reg.width != if long { 8 } else { 4 } {
            return Err(self.p.error(
                reg.column,
                if long {
                    "control register move requires a 64-bit register"
                } else {
                    "control register move requires a 32-bit register"
                },
            ));
        }
        if reg.reg.number >= 8 {
            self.emit(&[0x41])?;
        }
        self.emit(&[
            0x0f,
            if read { 0x20 } else { 0x22 },
            0xc0 | (number << 3) | (reg.reg.number & 7),
        ])
    }

    // MOVZX/MOVSX: 0F B6/B7 and 0F BE/BF. MOVSXD: REX.W + 63.
    fn extend(&mut self) -> Result<()> {
        let dest = self.operand()?;
        self.p
            .expect(b',', "expected ',' before extension source")?;
        let src = self.operand()?;
        let width = src.integer_width(self.p)?;
        let opcode = if self.op.text == "movsxd" {
            if dest.reg.width != 8 || width != 4 {
                return Err(self.p.error(
                    self.op.column,
                    "movsxd requires a 64-bit register and 32-bit source",
                ));
            }
            0x63
        } else {
            if dest.reg.width <= width || width > 2 {
                return Err(self.p.error(
                    self.op.column,
                    "extension requires a byte or word source and a wider register",
                ));
            }
            let opcode = if self.op.text == "movzx" {
                0x0fb6
            } else {
                0x0fbe
            };
            opcode + u32::from(width == 2)
        };
        self.modrm(opcode, dest.reg, src, 0)
    }

    fn full_immediate_width(&self, column: usize, value: u64, width: usize) -> Result<usize> {
        if width != 8 {
            return Ok(width);
        }
        if !fits_sign_extended(value, 4, 8) {
            return Err(self
                .p
                .error(column, "64-bit operand requires a sign-extended imm32"));
        }
        Ok(4)
    }

    // ADD/OR/ADC/SBB/AND/SUB/XOR/CMP: 00–3B /r and 80/81/83 /digit.
    // TEST: 84/85 /r and F6/F7 /0.
    fn binary(&mut self, group: u8, test: bool) -> Result<()> {
        let dest = self.operand()?;
        self.p.expect(b',', "expected ',' between operands")?;
        self.check_lock(dest)?;
        let lock = self.lock_prefix();
        if starts_number(self.p.peek()) {
            let width = dest.integer_width(self.p)?;
            let column = self.p.column;
            let value = self.p.integer(width, false)?;
            let mut immediate = self.full_immediate_width(column, value, width)?;
            let mut opcode = if test { 0xf6 } else { 0x80 } + u32::from(width != 1);
            if !test && width != 1 && fits_sign_extended(value, 1, width) {
                opcode = 0x83;
                immediate = 1;
            }
            // The accumulator form saves a ModR/M byte when using a full
            // immediate (SDM Vol. 2A/B, arithmetic opcode tables).
            if self.bits() != 64
                && !self.locked
                && !test
                && opcode != 0x83
                && !dest.is_memory()
                && dest.reg.number == 0
            {
                self.operand_override(width)?;
                self.emit(&[group * 8 + 4 + u8::from(width != 1)])?;
                return self.emit_word(value, immediate);
            }
            self.modrm(lock | opcode, Reg::new(group, width), dest, immediate)?;
            return self.emit_word(value, immediate);
        }

        let src = self.operand()?;
        let load = src.is_memory() && !test;
        let (reg, rm) = if load {
            (dest.reg, src)
        } else {
            (src.reg, dest)
        };
        if reg.width == 0 {
            return Err(self.p.error(src.column, "expected a register operand"));
        }
        rm.match_width(self.p, reg.width)?;
        let opcode = if test {
            0x84
        } else {
            u32::from(group) * 8 + u32::from(load) * 2
        };
        self.modrm(lock | (opcode + u32::from(reg.width != 1)), reg, rm, 0)
    }

    fn unary(&mut self, opcode: u32, digit: u8, rm: Operand<'_>) -> Result<()> {
        let width = rm.integer_width(self.p)?;
        self.check_lock(rm)?;
        self.modrm(
            self.lock_prefix() | (opcode + u32::from(width != 1)),
            Reg::new(digit, width),
            rm,
            0,
        )
    }

    // SAL/SAR/SHL/SHR and RCL/RCR/ROL/ROR: C0/C1, D0–D3 /digit.
    fn shift(&mut self, digit: u8) -> Result<()> {
        let dest = self.operand()?;
        let width = dest.integer_width(self.p)?;
        self.p.expect(b',', "expected a shift count after ','")?;
        let (opcode, immediate, value) = if starts_number(self.p.peek()) {
            let column = self.p.column;
            let value = self.p.count()?;
            if value > u64::from(u8::MAX) {
                return Err(self.p.error(column, "shift count out of imm8 range"));
            }
            if value == 1 {
                (0xd0, 0, value)
            } else {
                (0xc0, 1, value)
            }
        } else {
            let cl = self.p.word();
            if cl.text != "cl" {
                return Err(self
                    .p
                    .error(cl.column, "shift count must be an immediate or cl"));
            }
            (0xd2, 0, 0)
        };
        self.modrm(
            opcode + u32::from(width != 1),
            Reg::new(digit, width),
            dest,
            immediate,
        )?;
        self.emit_word(value, immediate)
    }

    // IMUL: F6/F7 /5, 0F AF /r, and 69/6B /r with immediate.
    fn imul(&mut self) -> Result<()> {
        let dest = self.operand()?;
        if !self.p.take(b',') {
            return self.unary(0xf6, 5, dest);
        }
        let width = dest.reg.width;
        if width < 2 {
            return Err(self.p.error(
                dest.column,
                "imul destination requires a register width of 16, 32 or 64 bits",
            ));
        }
        // In the register/immediate shorthand the register is also the source.
        let has_immediate = starts_number(self.p.peek());
        let src = if has_immediate { dest } else { self.operand()? };
        src.match_width(self.p, width)?;
        if !has_immediate && !self.p.take(b',') {
            return self.modrm(0x0faf, dest.reg, src, 0);
        }
        self.p.peek();
        let column = self.p.column;
        let value = self.p.integer(width, false)?;
        let full = self.full_immediate_width(column, value, width)?;
        let immediate = if fits_sign_extended(value, 1, width) {
            1
        } else {
            full
        };
        self.modrm(
            if immediate == 1 { 0x6b } else { 0x69 },
            dest.reg,
            src,
            immediate,
        )?;
        self.emit_word(value, immediate)
    }

    // LEA: 8D /r.
    fn lea(&mut self) -> Result<()> {
        let dest = self.operand()?;
        if dest.reg.width < 2 {
            return Err(self
                .p
                .error(dest.column, "lea requires a 16-, 32- or 64-bit register"));
        }
        self.p.expect(b',', "expected ',' before lea address")?;
        let src = self.operand()?;
        if !src.is_memory() {
            return Err(self.p.error(src.column, "lea requires a memory operand"));
        }
        src.match_width(self.p, dest.reg.width)?;
        self.modrm(0x8d, dest.reg, src, 0)
    }

    // PUSH/POP: 50/58+rd, FF /6, 8F /0 and PUSH 6A ib or 68 id.
    fn stack(&mut self, push: bool) -> Result<()> {
        let default = usize::from(self.bits() / 8);
        let width = if self.o32 && default != 8 { 4 } else { default };
        let (first, next, _) = self.p.lookahead();
        if push && default == 8 && first.text == "qword" && starts_number(next) {
            self.p.word();
        }
        if push && starts_number(self.p.peek()) {
            let full = width.min(4);
            let value = self.p.integer(full, true)?;
            let immediate = if fits_sign_extended(value, 1, full) {
                1
            } else {
                full
            };
            self.operand_override(width)?;
            self.emit(&[if immediate == 1 { 0x6a } else { 0x68 }])?;
            return self.emit_word(value, immediate);
        }
        let operand = self.operand()?;
        let width = match operand.width() {
            0 => width,
            explicit => explicit,
        };
        if !matches!(width, 2) && width != if default == 8 { 8 } else { 4 } {
            return Err(self
                .p
                .error(operand.column, "invalid push/pop operand width for mode"));
        }
        if operand.is_memory() {
            let digit = Reg::new(if push { 6 } else { 0 }, if width == 8 { 0 } else { width });
            self.modrm(if push { 0xff } else { 0x8f }, digit, operand, 0)
        } else {
            self.operand_override(width)?;
            if operand.reg.number >= 8 {
                self.emit(&[0x41])?;
            }
            let opcode = if push { 0x50 } else { 0x58 };
            self.emit(&[opcode + (operand.reg.number & 7)])
        }
    }

    // Near CALL/JMP: E8/E9 cw/cd or FF /2 and /4. Far indirect JMP: FF /5.
    fn transfer(&mut self, call: bool) -> Result<()> {
        let (name, next, following) = self.p.lookahead();
        if !call && (name.text == "far" || following.text == "far") {
            let first = self.p.word();
            let width = if first.text == "far" {
                if self.o32 {
                    4
                } else {
                    self.default_operand()
                }
            } else {
                self.p.word();
                operand_width(first.text)
            };
            if !matches!(width, 2 | 4) {
                return Err(self
                    .p
                    .error(first.column, "far jump requires a word or dword offset"));
            }
            let dest = self.required_memory(0)?;
            if dest.width() != 0 {
                return Err(self
                    .p
                    .error(dest.column, "specify the far offset size before far"));
            }
            return self.modrm(0xff, Reg::new(5, width), dest, 0);
        }
        let indirect = (name.text.is_empty() && next == b'[')
            || (operand_width(name.text) != 0 && (next == b'[' || following.is_name()))
            || (segment(name.text) != 0 && next == b':')
            || Reg::named(name.text).width != 0;
        if indirect {
            let dest = self.operand()?;
            let width = match dest.width() {
                0 if self.o32 && self.bits() != 64 => 4,
                0 => usize::from(self.bits() / 8),
                width => width,
            };
            let valid = if self.bits() == 64 {
                width == 8
            } else {
                matches!(width, 2 | 4)
            };
            if !valid {
                return Err(self
                    .p
                    .error(dest.column, "invalid near call/jmp operand width for mode"));
            }
            self.modrm(
                0xff,
                Reg::new(if call { 2 } else { 4 }, if width == 8 { 0 } else { width }),
                dest,
                0,
            )
        } else {
            self.relative_branch(if call { 0xe8 } else { 0xe9 }, true)
        }
    }

    fn relative_branch(&mut self, opcode: u32, allow_plt: bool) -> Result<()> {
        let width = if self.o32 { 4 } else { self.default_operand() };
        self.operand_override(width)?;
        self.opcode(opcode)?;
        self.branch_displacement(allow_plt, width)
    }

    // Jcc, SETcc and CMOVcc share condition numbers and aliases.
    fn condition(&mut self) -> Result<bool> {
        const CONDITIONS: &[&[&str]] = &[
            &["o"],
            &["no"],
            &["b", "c", "nae"],
            &["ae", "nb", "nc"],
            &["e", "z"],
            &["ne", "nz"],
            &["be", "na"],
            &["a", "nbe"],
            &["s"],
            &["ns"],
            &["p", "pe"],
            &["np", "po"],
            &["l", "nge"],
            &["ge", "nl"],
            &["le", "ng"],
            &["g", "nle"],
        ];
        let name = self.op.text;
        let (prefix, suffix) = match name {
            name if name.starts_with('j') => name.split_at(1),
            name if name.starts_with("set") => name.split_at(3),
            name if name.starts_with("cmov") => name.split_at(4),
            _ => return Ok(false),
        };
        let Some(condition) = CONDITIONS
            .iter()
            .position(|aliases| aliases.contains(&suffix))
        else {
            return Ok(false);
        };
        let code = condition as u32;
        match prefix {
            "j" => {
                self.relative_branch(0x0f80 + code, false)?;
            }
            "set" => {
                let dest = self.operand()?;
                if !matches!(dest.width(), 0 | 1) {
                    return Err(self.p.error(dest.column, "setcc requires a byte operand"));
                }
                self.modrm(0x0f90 + code, Reg::new(0, 1), dest, 0)?;
            }
            _ => {
                let dest = self.operand()?;
                if dest.reg.width < 2 {
                    return Err(self.p.error(
                        dest.column,
                        "cmovcc destination requires a register width of 16, 32 or 64 bits",
                    ));
                }
                self.p.expect(b',', "expected ',' before cmovcc source")?;
                let src = self.operand()?;
                src.match_width(self.p, dest.reg.width)?;
                self.modrm(0x0f40 + code, dest.reg, src, 0)?;
            }
        }
        Ok(true)
    }

    fn ret(&mut self) -> Result<()> {
        if self.o32 && self.bits() != 64 {
            self.operand_override(4)?;
        }
        if self.p.at_end() {
            return self.emit(&[0xc3]);
        }
        let column = self.p.column;
        if self.p.peek() == b'-' {
            return Err(self.p.error(column, "ret requires an unsigned imm16"));
        }
        let adjustment = self.p.integer(2, false)?;
        self.emit(&[0xc2])?;
        self.emit_word(adjustment, 2)
    }

    // LOCK, XADD, XCHG and CMPXCHG.
    fn exchange(&mut self, opcode: u32) -> Result<()> {
        let dest = self.operand()?;
        self.p
            .expect(b',', "expected ',' between exchange operands")?;
        let src = self.operand()?;
        let (reg, rm) = if opcode == 0x86 && src.is_memory() {
            (dest.reg, src)
        } else {
            (src.reg, dest)
        };
        if reg.width == 0 {
            return Err(self
                .p
                .error(src.column, "exchange requires a register source"));
        }
        if self.locked && !rm.is_memory() {
            return Err(self
                .p
                .error(dest.column, "lock requires a memory destination"));
        }
        rm.match_width(self.p, reg.width)?;
        self.modrm(
            self.lock_prefix() | (opcode + u32::from(reg.width != 1)),
            reg,
            rm,
            0,
        )
    }

    // BT/BTC/BTR/BTS register and immediate forms.
    fn bit(&mut self, digit: u8, opcode: u32) -> Result<()> {
        let dest = self.operand()?;
        self.check_lock(dest)?;
        let lock = self.lock_prefix();
        self.p.expect(b',', "expected ',' before bit index")?;
        if starts_number(self.p.peek()) {
            let width = dest.integer_width(self.p)?;
            if width == 1 {
                return Err(self
                    .p
                    .error(dest.column, "bit operation requires at least 16 bits"));
            }
            if self.p.peek() == b'-' {
                return Err(self
                    .p
                    .error(self.p.column, "expected an unsigned bit index"));
            }
            let index = self.p.integer(1, false)?;
            self.modrm(lock | 0x0fba, Reg::new(digit, width), dest, 1)?;
            self.emit_word(index, 1)
        } else {
            let src = self.operand()?;
            if src.reg.width < 2 {
                return Err(self.p.error(
                    src.column,
                    "bit index requires a 16-, 32- or 64-bit register",
                ));
            }
            dest.match_width(self.p, src.reg.width)?;
            self.modrm(lock | opcode, src.reg, dest, 0)
        }
    }

    pub(super) fn lockable(&mut self) -> Result<bool> {
        match self.op.text {
            "xchg" => self.exchange(0x86)?,
            "xadd" => self.exchange(0x0fc0)?,
            "cmpxchg" => self.exchange(0x0fb0)?,
            "cmpxchg8b" | "cmpxchg16b" => {
                let wide = self.op.text == "cmpxchg16b";
                let rm = self.required_memory(if wide { 16 } else { 8 })?;
                self.modrm(
                    self.lock_prefix() | 0x0fc7,
                    Reg::new(1, if wide { 8 } else { 0 }),
                    rm,
                    0,
                )?;
            }
            "bt" if !self.locked => self.bit(4, 0x0fa3)?,
            "bts" => self.bit(5, 0x0fab)?,
            "btr" => self.bit(6, 0x0fb3)?,
            "btc" => self.bit(7, 0x0fbb)?,
            name => {
                const BINARY: &[&str] = &["add", "or", "adc", "sbb", "and", "sub", "xor", "cmp"];
                let allowed = if self.locked { &BINARY[..7] } else { BINARY };
                if let Some(group) = allowed.iter().position(|&op| op == name) {
                    self.binary(group as u8, false)?;
                } else {
                    // INC/DEC: FE/FF. NOT/NEG/MUL/DIV/IDIV: F6/F7.
                    let (opcode, digit) = match name {
                        "inc" => (0xfe, 0),
                        "dec" => (0xfe, 1),
                        "not" => (0xf6, 2),
                        "neg" => (0xf6, 3),
                        "mul" if !self.locked => (0xf6, 4),
                        "div" if !self.locked => (0xf6, 6),
                        "idiv" if !self.locked => (0xf6, 7),
                        _ => return Ok(false),
                    };
                    let rm = self.operand()?;
                    self.unary(opcode, digit, rm)?;
                }
            }
        }
        Ok(true)
    }

    pub(super) fn integer(&mut self) -> Result<bool> {
        match self.op.text {
            "mov" => self.mov()?,
            "lgdt" => {
                // LGDT's operand attribute is independent of the address
                // attribute (SDM Vol. 2A, LGDT/LIDT).
                let width = if self.bits() == 64 {
                    0
                } else if self.o32 {
                    4
                } else {
                    self.default_operand()
                };
                let address = self.required_memory(0)?;
                self.modrm(0x0f01, Reg::new(2, width), address, 0)?;
            }
            "out" => {
                let port = self.p.word();
                self.p.expect(b',', "expected ',' between out operands")?;
                let value = self.p.word();
                if port.text != "dx" || value.text != "al" {
                    return Err(self.p.error(port.column, "out requires dx, al"));
                }
                self.emit(&[0xee])?;
            }
            "movsx" | "movzx" | "movsxd" => self.extend()?,
            "test" => self.binary(0, true)?,
            "imul" => self.imul()?,
            "lea" => self.lea()?,
            "push" | "pop" => self.stack(self.op.text == "push")?,
            "call" | "jmp" => self.transfer(self.op.text == "call")?,
            "ret" => self.ret()?,
            "rol" => self.shift(0)?,
            "ror" => self.shift(1)?,
            "rcl" => self.shift(2)?,
            "rcr" => self.shift(3)?,
            "shl" | "sal" => self.shift(4)?,
            "shr" => self.shift(5)?,
            "sar" => self.shift(7)?,
            "cbw" | "cwde" | "cwd" | "cdq" => {
                let name = self.op.text;
                self.operand_override(if matches!(name, "cbw" | "cwd") { 2 } else { 4 })?;
                self.emit(&[if matches!(name, "cbw" | "cwde") {
                    0x98
                } else {
                    0x99
                }])?;
            }
            name => {
                if self.bits() != 64 && matches!(name, "cdqe" | "cqo" | "swapgs" | "iretq") {
                    return Err(self
                        .p
                        .error(self.op.column, "instruction requires 64-bit mode"));
                }
                let opcode = match name {
                    "cdqe" => 0x4898,
                    "cqo" => 0x4899,
                    "leave" => 0xc9,
                    "cld" => 0xfc,
                    "std" => 0xfd,
                    "nop" => 0x90,
                    "syscall" => 0x0f05,
                    "swapgs" => 0x0f01f8,
                    "iretq" => 0x48cf,
                    "sysret" => 0x0f07,
                    "wrmsr" => 0x0f30,
                    "rdmsr" => 0x0f32,
                    "hlt" => 0xf4,
                    "cli" => 0xfa,
                    "lfence" => 0x0faee8,
                    "mfence" => 0x0faef0,
                    "sfence" => 0x0faef8,
                    _ => return self.condition(),
                };
                self.opcode(opcode)?;
            }
        }
        Ok(true)
    }
}

fn segment_register(name: &str) -> Option<u8> {
    // MOV Sreg and MOV from Sreg use the same three-bit field.
    ["es", "cs", "ss", "ds", "fs", "gs"]
        .iter()
        .position(|&register| register == name)
        .map(|index| index as u8)
}
