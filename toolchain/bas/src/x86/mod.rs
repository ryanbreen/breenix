//! Intel SDM Vol. 2A §§2.1–2.2, Tables 2-2–2-5 and 2-7 (prefixes,
//! ModR/M, SIB and register codes). Individual forms use the Vol. 2 opcode
//! tables.

mod integer;
mod vector;

use bobj::elf::amd64::{R_32, R_32S, R_GOTPCREL, R_PC32, R_PLT32, R_TPOFF32};

use crate::object::LabelDifference;
use crate::parser::{signed_value, starts_number, Parser, Token};
use crate::Result;

#[derive(Clone, Copy, Default)]
struct Reg {
    number: u8,
    width: usize,
    high_byte: bool,
    low_byte_rex: bool,
}

impl Reg {
    fn new(number: u8, width: usize) -> Self {
        Self {
            number,
            width,
            high_byte: false,
            low_byte_rex: false,
        }
    }

    fn named(name: &str) -> Self {
        const NAMES: [[&str; 16]; 4] = [
            [
                "al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil", "r8b", "r9b", "r10b", "r11b",
                "r12b", "r13b", "r14b", "r15b",
            ],
            [
                "ax", "cx", "dx", "bx", "sp", "bp", "si", "di", "r8w", "r9w", "r10w", "r11w",
                "r12w", "r13w", "r14w", "r15w",
            ],
            [
                "eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi", "r8d", "r9d", "r10d",
                "r11d", "r12d", "r13d", "r14d", "r15d",
            ],
            [
                "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11",
                "r12", "r13", "r14", "r15",
            ],
        ];
        for (row, names) in NAMES.iter().enumerate() {
            if let Some(number) = names.iter().position(|&candidate| candidate == name) {
                return Self {
                    low_byte_rex: row == 0 && number >= 4,
                    ..Self::new(number as u8, 1 << row)
                };
            }
        }
        if let Some(index) = ["ah", "ch", "dh", "bh"]
            .iter()
            .position(|&candidate| candidate == name)
        {
            return Self {
                number: index as u8 + 4,
                width: 1,
                high_byte: true,
                low_byte_rex: false,
            };
        }
        Self::default()
    }

    fn needs_rex_byte(self) -> bool {
        self.low_byte_rex
    }

    fn needs_long_mode(self) -> bool {
        self.width == 8 || self.number >= 8 || self.needs_rex_byte()
    }
}

#[derive(Clone, Copy, Default)]
struct Address<'a> {
    base: Option<u8>,
    index: Option<u8>,
    scale: u8,
    width: usize,
    addr_width: usize,
    segment: u8,
    ip_relative: bool,
    displacement: i64,
    label: Option<Token<'a>>,
    difference: Option<(usize, LabelDifference)>,
    relocation: u32,
}

#[derive(Clone, Copy)]
struct Operand<'a> {
    column: usize,
    reg: Reg,
    memory: Address<'a>,
}

impl Operand<'_> {
    fn width(self) -> usize {
        if self.reg.width != 0 {
            self.reg.width
        } else {
            self.memory.width
        }
    }

    fn is_memory(self) -> bool {
        self.reg.width == 0
    }

    fn match_width(self, p: &Parser<'_, '_>, width: usize) -> Result<()> {
        let actual = self.width();
        if actual != 0 && actual != width {
            return Err(p.error(self.column, "operand widths differ"));
        }
        Ok(())
    }

    fn explicit_width(self, p: &Parser<'_, '_>) -> Result<usize> {
        let width = self.width();
        if width == 0 {
            return Err(p.error(self.column, "memory operand requires an explicit width"));
        }
        Ok(width)
    }

    fn integer_width(self, p: &Parser<'_, '_>) -> Result<usize> {
        let width = self.explicit_width(p)?;
        if width > 8 {
            return Err(p.error(self.column, "integer operand width exceeds 64 bits"));
        }
        Ok(width)
    }
}

fn segment(name: &str) -> u8 {
    match name {
        "es" => 0x26,
        "cs" => 0x2e,
        "ss" => 0x36,
        "ds" => 0x3e,
        "fs" => 0x64,
        "gs" => 0x65,
        _ => 0,
    }
}

fn operand_width(name: &str) -> usize {
    match name {
        "byte" => 1,
        "word" => 2,
        "dword" => 4,
        "qword" => 8,
        "tbyte" => 10,
        "xmmword" | "oword" => 16,
        _ => 0,
    }
}

impl<'a> Address<'a> {
    fn register(&mut self, p: &mut Parser<'a, '_>, name: Token<'a>, reg: Reg) -> Result<()> {
        let legal_width = if p.x86_bits() == 64 {
            matches!(reg.width, 4 | 8)
        } else {
            matches!(reg.width, 2 | 4) && !reg.needs_long_mode()
        };
        if self.ip_relative || !legal_width {
            return Err(p.error(name.column, "invalid base or index register in this mode"));
        }
        if self.addr_width != 0 && self.addr_width != reg.width {
            return Err(p.error(name.column, "mixed address register widths"));
        }
        self.addr_width = reg.width;
        let scaled = p.take(b'*');
        if scaled && reg.width == 2 {
            return Err(p.error(name.column, "16-bit address registers cannot be scaled"));
        }
        if !scaled && self.base.is_none() {
            self.base = Some(reg.number);
            return Ok(());
        }

        if self.index.is_some() {
            return Err(p.error(name.column, "too many address registers"));
        }
        // An unscaled stack pointer can be the base instead of the index.
        let index = if !scaled && reg.number == 4 {
            self.base.replace(4)
        } else {
            Some(reg.number)
        };
        if index == Some(4) {
            return Err(p.error(name.column, "stack pointer cannot be an index register"));
        }
        self.index = index;
        if scaled {
            p.peek();
            let column = p.column;
            self.scale = match p.magnitude()? {
                1 => 0,
                2 => 1,
                4 => 2,
                8 => 3,
                _ => return Err(p.error(column, "index scale must be 1, 2, 4 or 8")),
            };
        }
        Ok(())
    }

    fn term(&mut self, p: &mut Parser<'a, '_>, negative: bool) -> Result<()> {
        p.peek();
        let column = p.column;
        if let Some(difference) = p.label_difference()? {
            if negative || self.difference.is_some() || self.label.is_some() {
                return Err(p.error(column, "expected one additive label difference"));
            }
            self.difference = Some((column, difference));
            return Ok(());
        }
        let mut groups = 0;
        while p.take(b'(') {
            groups += 1;
        }
        self.simple_term(p, negative)?;
        for _ in 0..groups {
            p.expect(b')', "expected ')' in address expression")?;
        }
        Ok(())
    }

    fn simple_term(&mut self, p: &mut Parser<'a, '_>, negative: bool) -> Result<()> {
        if p.peek().is_ascii_digit() {
            let column = p.column;
            let mut magnitude = p.magnitude()?;
            while p.take(b'*') {
                magnitude = magnitude.checked_mul(p.magnitude()?)
                    .ok_or_else(|| p.error(column, "address displacement out of range"))?;
            }
            self.displacement = signed_value(magnitude, negative)
                .and_then(|offset| self.displacement.checked_add(offset))
                .ok_or_else(|| p.error(column, "address displacement out of range"))?;
            return Ok(());
        }
        let name = p.word();
        if negative || !name.is_name() {
            return Err(p.error(
                name.column,
                "expected an address register, label or displacement",
            ));
        }
        if matches!(name.text, "rip" | "eip") {
            if p.x86_bits() != 64 || self.addr_width != 0 {
                return Err(p.error(
                    name.column,
                    "instruction pointer addressing requires 64-bit mode and no other registers",
                ));
            }
            self.ip_relative = true;
            self.addr_width = if name.text == "rip" { 8 } else { 4 };
        } else {
            let reg = Reg::named(name.text);
            if reg.width != 0 {
                return self.register(p, name, reg);
            }
            if self.label.is_some() || self.difference.is_some() {
                return Err(p.error(name.column, "expected at most one address label"));
            }
            self.label = Some(name);
            self.relocation = p.relocation_modifier()?;
        }
        Ok(())
    }

    fn parse(p: &mut Parser<'a, '_>, mut name: Token<'a>) -> Result<Self> {
        let mut address = Self {
            width: operand_width(name.text),
            ..Self::default()
        };
        if address.width != 0 {
            name = p.word();
            if name.text == "ptr" {
                name = p.word();
            }
        }
        if !name.text.is_empty() {
            address.segment = segment(name.text);
            if address.segment == 0 {
                return Err(p.error(name.column, "expected a register or memory operand"));
            }
            p.expect(b':', "expected ':' after segment override")?;
        }
        p.expect(b'[', "expected '[' before memory address")?;
        let (inside, next, _) = p.lookahead();
        if next == b':' {
            let inner_segment = segment(inside.text);
            if inner_segment == 0 || address.segment != 0 {
                return Err(p.error(inside.column, "invalid or duplicate segment override"));
            }
            p.word();
            p.expect(b':', "expected ':' after segment override")?;
            address.segment = inner_segment;
        }
        let mut negative = p.take(b'-');
        if !negative {
            p.take(b'+');
        }
        loop {
            address.term(p, negative)?;
            if p.take(b']') {
                break;
            }
            negative = p.take(b'-');
            if !negative {
                p.expect(b'+', "expected '+', '-' or ']' in memory address")?;
            }
        }
        if address.addr_width == 0 {
            address.addr_width = usize::from(p.x86_bits() / 8);
        }
        if let Some(label) = address.label {
            if address.addr_width == 2 {
                return Err(p.error(
                    label.column,
                    "16-bit addresses require a constant displacement",
                ));
            }
            if address.relocation == 0 {
                address.relocation = if address.ip_relative {
                    R_PC32
                } else if address.addr_width == 4 {
                    R_32
                } else {
                    R_32S
                };
            } else if !matches!(address.relocation, R_32S | R_TPOFF32 | R_GOTPCREL) {
                return Err(p.error(label.column, "unsupported memory relocation"));
            } else if address.ip_relative != (address.relocation == R_GOTPCREL) {
                return Err(p.error(
                    label.column,
                    "relocation modifier conflicts with address form",
                ));
            }
        } else {
            let (minimum, maximum) = match address.addr_width {
                2 => (i64::from(i16::MIN), i64::from(u16::MAX)),
                4 if !address.ip_relative => (i64::from(i32::MIN), i64::from(u32::MAX)),
                _ => (i64::from(i32::MIN), i64::from(i32::MAX)),
            };
            if !(minimum..=maximum).contains(&address.displacement) {
                let reason = if address.addr_width == 2 {
                    "memory displacement out of disp16 range"
                } else {
                    "memory displacement out of disp32 range"
                };
                return Err(p.error(name.column, reason));
            }
        }
        Ok(address)
    }

    // Intel SDM Vol. 2A Table 2-1. BP alone uses mod=01, disp8=0.
    fn rm16(self) -> Option<u8> {
        match (self.base, self.index) {
            (Some(3), Some(6)) | (Some(6), Some(3)) => Some(0),
            (Some(3), Some(7)) | (Some(7), Some(3)) => Some(1),
            (Some(5), Some(6)) | (Some(6), Some(5)) => Some(2),
            (Some(5), Some(7)) | (Some(7), Some(5)) => Some(3),
            (Some(6), None) => Some(4),
            (Some(7), None) => Some(5),
            (Some(5) | None, None) => Some(6),
            (Some(3), None) => Some(7),
            _ => None,
        }
    }
}

struct Encoder<'borrow, 'src, 'obj> {
    p: &'borrow mut Parser<'src, 'obj>,
    op: Token<'src>,
    locked: bool,
    o32: bool,
    sized: bool,
}

impl<'src> Encoder<'_, 'src, '_> {
    fn operand(&mut self) -> Result<Operand<'src>> {
        let name = self.p.word();
        let reg = Reg::named(name.text);
        if self.bits() != 64 && reg.needs_long_mode() {
            return Err(self.p.error(name.column, "register requires 64-bit mode"));
        }
        let memory = if reg.width == 0 {
            Address::parse(self.p, name)?
        } else {
            Address::default()
        };
        Ok(Operand {
            column: name.column,
            reg,
            memory,
        })
    }

    fn required_memory(&mut self, width: usize) -> Result<Operand<'src>> {
        let rm = self.operand()?;
        if !rm.is_memory() {
            return Err(self.p.error(rm.column, "expected a memory operand"));
        }
        if width != 0 {
            rm.match_width(self.p, width)?;
        }
        Ok(rm)
    }

    fn emit(&mut self, bytes: &[u8]) -> Result<()> {
        self.p.emit(self.op.column, bytes)
    }

    fn emit_word(&mut self, value: u64, width: usize) -> Result<()> {
        self.p.emit_word(self.op.column, value, width)
    }

    fn opcode(&mut self, opcode: u32) -> Result<()> {
        let bytes = opcode.to_be_bytes();
        let first = (opcode.leading_zeros() / 8).min(3) as usize;
        self.emit(&bytes[first..])
    }

    fn prefixes(&mut self, reg: Reg, rm: Operand<'_>, mut rex: u8, mandatory: u8) -> Result<()> {
        if self.bits() != 64 && (rex != 0 || reg.width == 8) {
            return Err(self.p.error(rm.column, "register requires 64-bit mode"));
        }
        if mandatory == 0xf0 {
            self.emit(&[mandatory])?;
        }
        if rm.memory.segment != 0 {
            self.emit(&[rm.memory.segment])?;
        }
        if mandatory != 0 && mandatory != 0xf0 {
            if self.o32 {
                return Err(self
                    .p
                    .error(self.op.column, "o32 conflicts with the opcode prefix"));
            }
            self.emit(&[mandatory])?;
        } else {
            self.operand_override(reg.width)?;
        }
        if rm.memory.addr_width != 0 && rm.memory.addr_width != usize::from(self.bits() / 8) {
            self.emit(&[0x67])?;
        }
        if reg.width == 8 {
            rex |= 8;
        }
        if self.bits() == 64 && (rex != 0 || reg.needs_rex_byte() || rm.reg.needs_rex_byte()) {
            if reg.high_byte || rm.reg.high_byte {
                return Err(self
                    .p
                    .error(rm.column, "high byte register cannot be encoded with REX"));
            }
            self.emit(&[0x40 | rex])?;
        }
        Ok(())
    }

    fn bits(&self) -> u8 {
        self.p.x86_bits()
    }

    fn default_operand(&self) -> usize {
        if self.bits() == 16 {
            2
        } else {
            4
        }
    }

    fn operand_override(&mut self, width: usize) -> Result<()> {
        self.sized = true;
        if self.o32 && width != 4 {
            return Err(self
                .p
                .error(self.op.column, "o32 requires a 32-bit operand size"));
        }
        if matches!(width, 2 | 4) && width != self.default_operand() {
            self.emit(&[0x66])?;
        }
        Ok(())
    }

    fn modrm(&mut self, opcode: u32, reg: Reg, rm: Operand<'_>, trailing: usize) -> Result<()> {
        let mandatory = (opcode >> 16) as u8;
        let opcode = opcode & 0xffff;
        let mut rex = u8::from(reg.number >= 8) << 2;
        let reg_field = (reg.number & 7) << 3;

        if !rm.is_memory() {
            rex |= u8::from(rm.reg.number >= 8);
            self.prefixes(reg, rm, rex, mandatory)?;
            self.opcode(opcode)?;
            return self.emit(&[0xc0 | reg_field | (rm.reg.number & 7)]);
        }

        let addr = rm.memory;
        let legacy16 = addr.addr_width == 2;
        let base = if legacy16 {
            addr.rm16()
                .ok_or_else(|| self.p.error(rm.column, "invalid 16-bit address registers"))?
        } else {
            addr.base.map_or(5, |number| number & 7)
        };
        let index = addr.index.map_or(4, |number| number & 7);
        let full_width = if legacy16 { 2 } else { 4 };
        let no_disp_base = if legacy16 { 6 } else { 5 };
        let (mode, disp_width) = if addr.base.is_none() {
            (0, full_width)
        } else if addr.label.is_some() || addr.difference.is_some() {
            (2, full_width)
        } else if addr.displacement == 0 && base != no_disp_base {
            (0, 0)
        } else if i8::try_from(addr.displacement).is_ok() {
            (1, 1)
        } else {
            (2, full_width)
        };
        let sib = !legacy16
            && !addr.ip_relative
            && (addr.index.is_some()
                || (addr.base.is_none() && self.bits() == 64)
                || (addr.base.is_some() && base == 4));
        rex |= u8::from(addr.base.is_some_and(|number| number >= 8));
        rex |= u8::from(addr.index.is_some_and(|number| number >= 8)) << 1;
        self.prefixes(reg, rm, rex, mandatory)?;
        self.opcode(opcode)?;
        self.emit(&[(mode << 6) | reg_field | if sib { 4 } else { base }])?;
        if sib {
            self.emit(&[(addr.scale << 6) | (index << 3) | base])?;
        }
        if let Some(label) = addr.label {
            self.p
                .reference(label, addr.relocation, addr.displacement, trailing)
        } else if let Some((column, mut difference)) = addr.difference {
            difference.addend = difference
                .addend
                .checked_add(addr.displacement)
                .ok_or_else(|| self.p.error(column, "symbol addend out of range"))?;
            self.p.constant_field(
                column,
                difference,
                disp_width,
                addr.ip_relative || addr.addr_width == 8,
            )
        } else {
            self.emit_word(addr.displacement as u64, disp_width)
        }
    }

    fn branch_displacement(&mut self, allow_plt: bool, width: usize) -> Result<()> {
        if starts_number(self.p.peek()) {
            let value = self.p.integer(width, true)?;
            return self.emit_word(value, width);
        }
        let label = self.p.word();
        if !label.is_name() {
            return Err(self
                .p
                .error(label.column, "expected a branch label or displacement"));
        }
        let kind = match self.p.relocation_modifier()? {
            0 => R_PC32,
            R_PLT32 if allow_plt && width == 4 => R_PLT32,
            _ => return Err(self.p.error(label.column, "unsupported branch relocation")),
        };
        let addend = self.p.addend()?;
        if width == 2 {
            self.p.rel16(label, addend)
        } else {
            self.p.reference(label, kind, addend, 0)
        }
    }

    fn lock_prefix(&self) -> u32 {
        if self.locked {
            0xf00000
        } else {
            0
        }
    }

    fn check_lock(&self, dest: Operand<'_>) -> Result<()> {
        if self.locked && !dest.is_memory() {
            return Err(self
                .p
                .error(dest.column, "lock requires a memory destination"));
        }
        Ok(())
    }

    fn string_instruction(&mut self) -> Result<bool> {
        let (repeat, conditional) = match self.op.text {
            "rep" => (0xf3, false),
            "repe" | "repz" => (0xf3, true),
            "repne" | "repnz" => (0xf2, true),
            _ => (0, false),
        };
        let op = if repeat == 0 { self.op } else { self.p.word() };
        let (stem, suffix) = op.text.split_at(op.text.len().saturating_sub(1));
        let form = match stem {
            "movs" => Some((0xa4, false)),
            "cmps" => Some((0xa6, true)),
            "stos" => Some((0xaa, false)),
            "lods" => Some((0xac, false)),
            "scas" => Some((0xae, true)),
            _ => None,
        };
        if let Some((opcode, compares)) = form.filter(|_| matches!(suffix, "b" | "w" | "d" | "q")) {
            if repeat != 0 {
                if conditional != compares {
                    return Err(self
                        .p
                        .error(op.column, "repeat prefix conflicts with string operation"));
                }
                self.emit(&[repeat])?;
            } else if !self.p.at_end() {
                // MOVSD and CMPSD also have SSE forms.
                return Ok(false);
            }
            match suffix {
                "w" => self.operand_override(2)?,
                "d" => self.operand_override(4)?,
                "q" if self.bits() == 64 => self.emit(&[0x48])?,
                "q" => {
                    return Err(self
                        .p
                        .error(op.column, "qword string operation requires 64-bit mode"))
                }
                _ => (),
            }
            self.emit(&[opcode + u8::from(suffix != "b")])?;
            return Ok(true);
        }
        if repeat != 0 {
            return Err(self
                .p
                .error(op.column, "repeat prefix requires a string instruction"));
        }
        Ok(false)
    }
}

pub(crate) fn instruction<'a>(p: &mut Parser<'a, '_>, mut op: Token<'a>) -> Result<()> {
    if !(p.object.in_text() || p.uses_nasm() && p.object.in_progbits()) {
        return Err(p.error(op.column, "instructions require .text"));
    }
    if op.text == "o64" {
        let next = p.word();
        if next.text != "sysret" || p.x86_bits() != 64 {
            return Err(p.error(next.column, "o64 requires sysret"));
        }
        return p.emit(op.column, &[0x48, 0x0f, 0x07]);
    }
    let o32 = op.text == "o32";
    if o32 {
        op = p.word();
    }
    let locked = op.text == "lock";
    if locked {
        op = p.word();
    }
    let mut encoder = Encoder {
        p,
        op,
        locked,
        o32,
        sized: false,
    };
    let lockable = encoder.lockable()?;
    if locked && !lockable {
        return Err(encoder
            .p
            .error(op.column, "instruction does not accept lock"));
    }
    if lockable
        || encoder.string_instruction()?
        || encoder.integer()?
        || encoder.sse()?
        || encoder.x87()?
    {
        if o32 && !encoder.sized {
            return Err(encoder
                .p
                .error(op.column, "instruction does not accept o32"));
        }
        return Ok(());
    }
    Err(encoder
        .p
        .error(op.column, "unsupported instruction or directive"))
}
