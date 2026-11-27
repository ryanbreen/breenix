//! The byte-oriented source grammar and directives documented in bas/README.md.

use bobj::elf::amd64::{R_32, R_32S, R_64, R_GOTPCREL, R_PLT32, R_TPOFF32};
use bobj::elf::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHT_NOBITS, SHT_PROGBITS,
    STB_GLOBAL, STB_WEAK, STT_FUNC, STT_NOTYPE, STT_OBJECT, STT_TLS,
};

use crate::aarch64::Relocation;
use crate::nasm::Syntax;
use crate::object::{Endpoint, LabelDifference, LiteralValue, Object};
use crate::{Diagnostic, Location, Result, Target};

#[path = "aarch64/gnu.rs"]
mod gnu;
#[path = "gnu_macros.rs"]
mod gnu_macros;
pub(crate) use gnu_macros::assemble as assemble_aarch64;

#[derive(Clone, Copy)]
pub(crate) struct Token<'a> {
    pub(crate) text: &'a str,
    pub(crate) column: usize,
}

impl Token<'_> {
    pub(crate) fn is_name(self) -> bool {
        self.text != "."
            && self
                .text
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'.'))
    }
}

pub(crate) fn starts_number(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'+' | b'-')
}

pub(crate) fn signed_value(value: u64, negative: bool) -> Option<i64> {
    if value > i64::MAX as u64 + u64::from(negative) {
        return None;
    }
    Some(if negative {
        value.wrapping_neg() as i64
    } else {
        value as i64
    })
}

pub(crate) struct Parser<'src, 'obj> {
    line: &'src [u8],
    start: Location,
    pub(crate) column: usize,
    pub(crate) object: &'obj mut Object,
    nasm: Option<&'obj mut Syntax>,
    constants: Option<&'obj mut gnu::Constants>,
    origin: Option<Location>,
}

impl<'src, 'obj> Parser<'src, 'obj> {
    pub(crate) fn new(line: &'src [u8], line_number: usize, object: &'obj mut Object) -> Self {
        Self::new_at(
            line,
            Location {
                line: line_number,
                column: 1,
            },
            object,
        )
    }

    fn new_at(line: &'src [u8], start: Location, object: &'obj mut Object) -> Self {
        Self {
            line,
            start,
            column: 0,
            object,
            nasm: None,
            constants: None,
            origin: None,
        }
    }

    pub(crate) fn with_nasm(mut self, syntax: &'obj mut Syntax, origin: Option<Location>) -> Self {
        self.nasm = Some(syntax);
        self.origin = origin;
        self
    }

    fn location(&self, column: usize) -> Location {
        self.origin.unwrap_or(Location {
            line: self.start.line,
            column: self.start.column + column,
        })
    }

    pub(crate) fn error(&self, column: usize, message: &'static str) -> Diagnostic {
        self.location(column).error(message)
    }

    fn byte(&self) -> u8 {
        self.line.get(self.column).copied().unwrap_or(0)
    }

    pub(crate) fn peek(&mut self) -> u8 {
        while matches!(self.byte(), b' ' | b'\t'..=b'\r') {
            self.column += 1;
        }
        self.byte()
    }

    pub(crate) fn take(&mut self, byte: u8) -> bool {
        if self.peek() != byte {
            return false;
        }
        self.column += 1;
        true
    }

    pub(crate) fn expect(&mut self, byte: u8, message: &'static str) -> Result<()> {
        if self.take(byte) {
            Ok(())
        } else {
            Err(self.error(self.column, message))
        }
    }

    pub(crate) fn at_end(&mut self) -> bool {
        match self.peek() {
            0 => true,
            b';' => self.object.target == Target::X86_64,
            b'/' if self.object.target == Target::Aarch64 => {
                self.line[self.column..].starts_with(b"//")
            }
            _ => false,
        }
    }

    pub(crate) fn word(&mut self) -> Token<'src> {
        self.peek();
        let start = self.column;
        while self.byte().is_ascii_alphanumeric() || matches!(self.byte(), b'_' | b'.' | b'$') {
            self.column += 1;
        }
        let text = std::str::from_utf8(&self.line[start..self.column])
            .expect("the tokenizer only accepts ASCII word bytes");
        Token {
            text,
            column: start,
        }
    }

    pub(crate) fn lookahead(&mut self) -> (Token<'src>, u8, Token<'src>) {
        let start = self.column;
        let name = self.word();
        let next = self.peek();
        let following = self.word();
        self.column = start;
        (name, next, following)
    }

    pub(crate) fn magnitude(&mut self) -> Result<u64> {
        if !self.peek().is_ascii_digit() {
            return Err(self.error(self.column, "expected an integer"));
        }
        let start = self.column;
        let rest = &self.line[start..];
        let (digits, radix) = match rest {
            [b'0', b'b' | b'B', digits @ ..] if self.constants.is_some() => (digits, 2),
            [b'0', b'x' | b'X', digits @ ..] if self.constants.is_some() => (digits, 16),
            [b'0', b'x' | b'X', first, ..] if first.is_ascii_hexdigit() => (&rest[2..], 16),
            _ => (rest, 10),
        };
        let mut value = 0u64;
        let mut length = rest.len() - digits.len();
        for &byte in digits {
            let Some(digit) = char::from(byte).to_digit(radix) else {
                break;
            };
            value = value
                .checked_mul(u64::from(radix))
                .and_then(|value| value.checked_add(u64::from(digit)))
                .ok_or_else(|| self.error(start, "integer out of range"))?;
            length += 1;
        }
        if digits.len() != rest.len() && length == 2 {
            return Err(self.error(start, "expected digits after radix prefix"));
        }
        self.column = start + length;
        Ok(value)
    }

    pub(crate) fn integer(&mut self, width: usize, signed_only: bool) -> Result<u64> {
        self.peek();
        let start = self.column;
        if self.constants.is_some() {
            let value = self.gnu_expression()?;
            let sign_bit = 1i128 << (width * 8 - 1);
            let maximum = if signed_only { sign_bit - 1 } else { 2 * sign_bit - 1 };
            if (-sign_bit..=maximum).contains(&value) {
                return Ok(value as u64);
            }
            return Err(self.error(start, "integer out of range for operand width"));
        }
        let negative = self.take(b'-');
        if !negative {
            self.take(b'+');
        }
        let mut magnitude = self.magnitude()?;
        if self.uses_nasm() && self.take(b'<') {
            self.expect(b'<', "expected '<<' in integer expression")?;
            magnitude = u32::try_from(self.magnitude()?)
                .ok()
                .filter(|&shift| shift < 64 && magnitude <= u64::MAX >> shift)
                .map(|shift| magnitude << shift)
                .ok_or_else(|| self.error(start, "integer shift out of range"))?;
        }
        let sign_bit = 1u64 << (width * 8 - 1);
        let maximum = if negative {
            sign_bit
        } else if signed_only {
            sign_bit - 1
        } else {
            (sign_bit - 1) * 2 + 1
        };
        if magnitude > maximum {
            return Err(self.error(start, "integer out of range for operand width"));
        }
        Ok(if negative {
            magnitude.wrapping_neg()
        } else {
            magnitude
        })
    }

    pub(crate) fn count(&mut self) -> Result<u64> {
        if self.constants.is_some() {
            self.peek();
            let column = self.column;
            return u64::try_from(self.gnu_expression()?)
                .map_err(|_| self.error(column, "expected a nonnegative integer"));
        }
        self.take(b'+');
        self.magnitude()
    }

    pub(crate) fn addend(&mut self) -> Result<i64> {
        let sign = self.peek();
        if !matches!(sign, b'+' | b'-') {
            return Ok(0);
        }
        let column = self.column;
        self.column += 1;
        signed_value(self.magnitude()?, sign == b'-')
            .ok_or_else(|| self.error(column, "symbol addend out of range"))
    }

    pub(crate) fn relocation_modifier(&mut self) -> Result<u32> {
        if !self.take(b'@') {
            return Ok(0);
        }
        let name = self.word();
        match name.text {
            "plt" => Ok(R_PLT32),
            "gotpcrel" => Ok(R_GOTPCREL),
            "32s" => Ok(R_32S),
            "tpoff" => Ok(R_TPOFF32),
            _ => Err(self.error(name.column, "unknown relocation modifier")),
        }
    }

    fn symbol(&mut self, name: Token<'_>) -> Result<usize> {
        if !name.is_name() {
            return Err(self.error(name.column, "expected a symbol name"));
        }
        let at = self.location(name.column);
        if let Some(syntax) = &self.nasm {
            if name.text.starts_with('.') && !name.text.starts_with("..") {
                let scope = syntax.scope.as_deref().ok_or_else(|| {
                    at.error("local label has no preceding non-local label")
                })?;
                return Ok(self.object.symbol(&format!("{scope}{}", name.text), at));
            }
        }
        Ok(self.object.symbol(name.text, at))
    }

    pub(crate) fn reference(
        &mut self,
        label: Token<'_>,
        kind: u32,
        addend: i64,
        trailing: usize,
    ) -> Result<()> {
        let symbol = self.symbol(label)?;
        self.object
            .reference(symbol, kind, addend, trailing, self.location(label.column))
    }

    pub(crate) fn label_difference(&mut self) -> Result<Option<LabelDifference>> {
        let saved = self.column;
        let mut groups = self.size_groups();
        let end = self.word();
        while groups > 0 && self.take(b')') {
            groups -= 1;
        }
        if !end.is_name() || !self.take(b'-') {
            self.column = saved;
            return Ok(None);
        }
        groups += self.size_groups();
        let start = self.word();
        if !start.is_name() {
            self.column = saved;
            return Ok(None);
        }
        let mut addend = 0i64;
        loop {
            if groups > 0 && self.take(b')') {
                groups -= 1;
                continue;
            }
            let before_addend = self.column;
            if (self.take(b'+') || self.take(b'-')) && self.peek().is_ascii_digit() {
                self.column = before_addend;
                addend = addend
                    .checked_add(self.addend()?)
                    .ok_or_else(|| self.error(end.column, "symbol addend out of range"))?;
            } else {
                self.column = before_addend;
                break;
            }
        }
        if groups != 0 {
            return Err(self.error(self.column, "expected ')' in label difference"));
        }
        Ok(Some(LabelDifference {
            end: Endpoint::Symbol(self.symbol(end)?),
            start: Endpoint::Symbol(self.symbol(start)?),
            addend,
        }))
    }

    pub(crate) fn constant_field(
        &mut self,
        column: usize,
        difference: LabelDifference,
        width: usize,
        signed: bool,
    ) -> Result<()> {
        self.object
            .constant_field(difference, width, signed, self.location(column))
    }

    pub(crate) fn rel16(&mut self, label: Token<'_>, addend: i64) -> Result<()> {
        let difference = LabelDifference {
            end: Endpoint::Symbol(self.symbol(label)?),
            start: Endpoint::Here,
            addend: addend
                .checked_sub(2)
                .ok_or_else(|| self.error(label.column, "relative displacement out of range"))?,
        };
        self.constant_field(label.column, difference, 2, true)
    }

    pub(crate) fn emit(&mut self, column: usize, bytes: &[u8]) -> Result<()> {
        self.object.emit_bytes(self.location(column), bytes)
    }

    pub(crate) fn emit_aarch64(&mut self, column: usize, word: u32) -> Result<()> {
        self.object.emit_aarch64(self.location(column), word)
    }

    pub(crate) fn emit_aarch64_literal_load(&mut self, word: u32) -> Result<()> {
        self.peek();
        let at = self.location(self.column);
        let value = if self.starts_gnu_expression() {
            LiteralValue::Constant(self.integer(8, false)?)
        } else {
            let name = self.word();
            let symbol = self.symbol(name)?;
            LiteralValue::Symbol {
                symbol,
                addend: self.addend()?,
            }
        };
        self.object.aarch64_literal_load(word, value, at)
    }

    pub(crate) fn emit_aarch64_reference(
        &mut self,
        label: Token<'_>,
        kind: Relocation,
        word: u32,
    ) -> Result<()> {
        if label.text == "." && matches!(kind, Relocation::Jump26) {
            // DDI 0487 M.b C6.2.35: B measures its displacement from this
            // instruction, so only the addend contributes to a branch to '.'.
            let addend = self.addend()?;
            let mut bytes = word.to_le_bytes();
            kind.patch_local(&mut bytes, i128::from(addend), self.location(label.column))?;
            return self.emit_aarch64(label.column, u32::from_le_bytes(bytes));
        }
        let symbol = self.symbol(label)?;
        let addend = self.addend()?;
        if matches!(kind, Relocation::AdrGotPage | Relocation::Ld64GotLo12Nc) && addend != 0 {
            return Err(self.error(label.column, "GOT relocation requires a zero addend"));
        }
        self.object.aarch64_reference(
            symbol,
            kind,
            addend,
            Some(word),
            self.location(label.column),
        )
    }

    pub(crate) fn emit_word(&mut self, column: usize, value: u64, width: usize) -> Result<()> {
        self.emit(column, &value.to_le_bytes()[..width])
    }

    fn hex_digit(&mut self) -> Result<u8> {
        let digit = char::from(self.byte())
            .to_digit(16)
            .ok_or_else(|| self.error(self.column, "expected two hex digits after \\x"))?;
        self.column += 1;
        Ok(digit as u8)
    }

    fn string_byte(&mut self) -> Result<u8> {
        let byte = self.byte();
        if byte == 0 {
            return Err(self.error(self.column, "unterminated string"));
        }
        self.column += 1;
        if byte != b'\\' {
            return Ok(byte);
        }
        let escape = self.byte();
        self.column += 1;
        match escape {
            b'\\' | b'"' => Ok(escape),
            b'0' => Ok(0),
            b'n' => Ok(b'\n'),
            b'r' => Ok(b'\r'),
            b't' => Ok(b'\t'),
            b'x' => Ok((self.hex_digit()? << 4) | self.hex_digit()?),
            _ => Err(self.error(self.column - 1, "unsupported string escape")),
        }
    }

    fn emit_string(&mut self, terminated: bool) -> Result<()> {
        self.peek();
        let start = self.column;
        self.expect(b'"', "expected a quoted string")?;
        while self.byte() != b'"' {
            let byte = self.string_byte()?;
            self.emit(start, &[byte])?;
        }
        self.column += 1;
        if terminated {
            self.emit(start, &[0])?;
        }
        Ok(())
    }

    fn section(&mut self) -> Result<()> {
        self.peek();
        let at = self.location(self.column);
        let mut quoted_name = Vec::new();
        let name = if self.take(b'"') {
            while self.byte() != b'"' {
                quoted_name.push(self.string_byte()?);
            }
            self.column += 1;
            std::str::from_utf8(&quoted_name).map_err(|_| at.error("section name must be UTF-8"))?
        } else {
            self.word().text
        };
        if name.is_empty() || name.contains('\0') {
            return Err(at.error("invalid section name"));
        }
        let attributes = if self.take(b',') {
            let flags = self.word();
            let flags = match flags.text {
                "a" => SHF_ALLOC,
                "ax" => SHF_ALLOC | SHF_EXECINSTR,
                "aw" => SHF_ALLOC | SHF_WRITE,
                "awT" => SHF_ALLOC | SHF_WRITE | SHF_TLS,
                _ => return Err(self.error(flags.column, "unsupported section flags")),
            };
            self.expect(b',', "expected ',' before section type")?;
            self.expect(b'@', "expected '@' before section type")?;
            let kind = self.word();
            let kind = match kind.text {
                "progbits" => SHT_PROGBITS,
                "nobits" => SHT_NOBITS,
                _ => return Err(self.error(kind.column, "unsupported section type")),
            };
            Some((kind, flags))
        } else if self.uses_nasm() && name.starts_with(".text.") {
            // NASM 3.02 §9.10.2: .text.* falls under "section other".
            Some((SHT_PROGBITS, SHF_ALLOC))
        } else {
            None
        };
        self.object.select_section(name, attributes, at)
    }

    fn emit_data_list(&mut self, width: usize) -> Result<()> {
        loop {
            self.peek();
            let column = self.column;
            if self.peek() == b'"' && width == 1 {
                self.emit_string(false)?;
            } else if starts_number(self.peek()) || self.starts_gnu_expression() {
                let value = self.integer(width, false)?;
                self.emit_word(column, value, width)?;
            } else if self.object.target == Target::Aarch64 {
                self.emit_aarch64_data_reference(width)?;
            } else if let Some(difference) = self.label_difference()? {
                self.constant_field(column, difference, width, false)?;
            } else {
                let label = self.word();
                let kind = match (width, self.relocation_modifier()?) {
                    (8, 0) => R_64,
                    (4, 0) => R_32,
                    (4, R_32S) => R_32S,
                    _ => {
                        return Err(self.error(
                            label.column,
                            "unsupported absolute relocation for this field",
                        ))
                    }
                };
                let addend = self.addend()?;
                self.reference(label, kind, addend, 0)?;
            }
            if !self.take(b',') {
                return Ok(());
            }
        }
    }

    fn emit_aarch64_data_reference(&mut self, width: usize) -> Result<()> {
        let label = self.word();
        let symbol = self.symbol(label)?;
        // AAELF64 §5.7.5: P is the address of the data field, denoted by '.'.
        let after_label = self.column;
        let relative = self.take(b'-') && self.word().text == ".";
        if !relative {
            self.column = after_label;
        }
        let kind = match (width, relative) {
            (8, false) => Relocation::Abs64,
            (4, false) => Relocation::Abs32,
            (4, true) => Relocation::Prel32,
            _ => {
                return Err(self.error(
                    label.column,
                    "unsupported AArch64 data relocation for this field",
                ))
            }
        };
        let addend = self.addend()?;
        self.object
            .aarch64_reference(symbol, kind, addend, None, self.location(label.column))
    }

    fn align(&mut self) -> Result<()> {
        self.peek();
        let column = self.column;
        let alignment = self.count()?;
        if !alignment.is_power_of_two() {
            return Err(self.error(column, "alignment must be a positive power of two"));
        }
        let fill = if self.take(b',') {
            self.integer(1, false)? as u8
        } else if self.uses_nasm() && self.object.in_progbits() {
            // NASM ALIGN defaults to NOP padding (manual §6.15.1).
            0x90
        } else {
            self.object.default_fill()
        };
        self.object.align(self.location(column), alignment, fill)
    }

    fn set_type(&mut self) -> Result<()> {
        let name = self.word();
        let index = self.symbol(name)?;
        self.expect(b',', "expected ',' before symbol type")?;
        self.take(b'@');
        let kind = self.word();
        self.object.symbols[index].kind = match kind.text {
            "function" | "func" => STT_FUNC,
            "object" => STT_OBJECT,
            "tls" => STT_TLS,
            "notype" => STT_NOTYPE,
            _ => return Err(self.error(kind.column, "unsupported symbol type")),
        };
        Ok(())
    }

    fn size_term(&mut self) -> Result<Endpoint> {
        let term = self.word();
        if term.text == "." {
            Ok(Endpoint::Here)
        } else {
            self.symbol(term).map(Endpoint::Symbol)
        }
    }

    fn set_size(&mut self) -> Result<()> {
        let name = self.word();
        let index = self.symbol(name)?;
        let at = self.location(name.column);
        if self.object.symbols[index].size_at.is_some() {
            return Err(at.error("duplicate symbol size"));
        }
        self.object.symbols[index].size_at = Some(at);
        self.expect(b',', "expected ',' before symbol size")?;
        if starts_number(self.peek()) {
            self.object.symbols[index].size = self.count()?;
        } else {
            // NASM macro bodies can wrap either term or the full difference.
            let mut groups = self.size_groups();
            let end = self.size_term()?;
            while groups > 0 && self.take(b')') {
                groups -= 1;
            }
            self.expect(b'-', "expected 'END - START' after symbol name")?;
            groups += self.size_groups();
            let start = self.size_term()?;
            for _ in 0..groups {
                self.expect(b')', "expected ')' in symbol size")?;
            }
            self.object.size_expression(index, end, start, at);
        }
        Ok(())
    }

    fn size_groups(&mut self) -> usize {
        let mut groups = 0;
        while self.nasm.is_some() && self.take(b'(') {
            groups += 1;
        }
        groups
    }

    fn directive(&mut self, op: Token<'src>) -> Result<bool> {
        if self.constants.is_some() && self.gnu_directive(op)? {
            return Ok(true);
        }
        let name = op.text.strip_prefix('.').unwrap_or(op.text);
        match name {
            "section" => self.section()?,
            "extern" if op.text == ".extern" && self.constants.is_some() => {
                // GNU A64 treats .extern as an optional declaration.
                // A reference, not the declaration, creates the symbol.
                let symbol = self.word();
                if !symbol.is_name() {
                    return Err(self.error(symbol.column, "expected a symbol after .extern"));
                }
            }
            "global" | "globl" | "extern" | "weak" => {
                let symbol = self.word();
                let index = self.symbol(symbol)?;
                let binding = &mut self.object.symbols[index].binding;
                if name == "weak" {
                    *binding = STB_WEAK;
                } else if *binding != STB_WEAK {
                    *binding = STB_GLOBAL;
                }
            }
            "internal" | "hidden" | "protected" => {
                let symbol = self.word();
                let index = self.symbol(symbol)?;
                // ELF gABI table 5.4.
                self.object.symbols[index].visibility = match name {
                    "internal" => 1,
                    "hidden" => 2,
                    _ => 3,
                };
            }
            "alias" => {
                let symbol = self.word();
                let index = self.symbol(symbol)?;
                self.expect(b',', "expected ',' before alias target")?;
                let target = self.word();
                let target = self.symbol(target)?;
                self.object.define_alias(index, target, self.location(symbol.column))?;
            }
            "db" | "byte" => self.emit_data_list(1)?,
            "dw" | "word" => self.emit_data_list(2)?,
            "dd" | "long" => self.emit_data_list(4)?,
            "dq" | "quad" => self.emit_data_list(8)?,
            "ascii" | "asciz" => loop {
                self.emit_string(name == "asciz")?;
                if !self.take(b',') {
                    break;
                }
            },
            "zero" => {
                let count = self.count()?;
                let column = op.column + usize::from(op.text.starts_with('.'));
                self.object.fill(self.location(column), count, 0)?;
            }
            "align" | "balign" => self.align()?,
            "type" => self.set_type()?,
            "size" => self.set_size()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(crate) fn assemble_line(&mut self) -> Result<()> {
        // A standalone hash comment, without affecting A64 #immediates.
        if self.peek() == b'#' {
            return Ok(());
        }
        while !self.at_end() {
            let op = self.word();
            if self.take(b':') {
                let index = self.symbol(op)?;
                self.object.define_label(index, self.location(op.column))?;
                if let Some(syntax) = &mut self.nasm {
                    if !op.text.starts_with('.') {
                        syntax.scope = Some(op.text.to_owned());
                    }
                }
                continue;
            }
            if self.nasm.is_some() && op.text.eq_ignore_ascii_case("bits") {
                self.set_bits()?;
            } else if self.nasm.is_some() && op.text.eq_ignore_ascii_case("times") {
                self.repeat()?;
            } else if !self.directive(op)? {
                self.instruction(op)?;
            }
            self.require_end()?;
            break;
        }
        Ok(())
    }

    fn require_end(&mut self) -> Result<()> {
        if self.at_end() {
            Ok(())
        } else {
            Err(self.error(self.column, "unexpected input after operands"))
        }
    }

    fn set_bits(&mut self) -> Result<()> {
        let mode = self.word();
        let bits = match mode.text {
            "16" => 16,
            "32" => 32,
            "64" => 64,
            _ => return Err(self.error(mode.column, "expected BITS 16, 32 or 64")),
        };
        if let Some(syntax) = &mut self.nasm {
            syntax.bits = bits;
        }
        Ok(())
    }

    fn repeat(&mut self) -> Result<()> {
        self.peek();
        let count_at = self.column;
        let count = self.count()?;
        if count > 1_048_576 {
            return Err(self.error(count_at, "TIMES count exceeds the supported limit"));
        }
        if !self.byte().is_ascii_whitespace() {
            return Err(self.error(self.column, "expected whitespace after TIMES count"));
        }
        let op = self.word();
        if op.text.is_empty() || self.peek() == b':' {
            return Err(self.error(op.column, "expected data or an instruction after TIMES"));
        }
        let operands = self.column;
        for _ in 0..count {
            self.column = operands;
            match op.text {
                "db" => self.emit_data_list(1)?,
                "dw" => self.emit_data_list(2)?,
                "dd" => self.emit_data_list(4)?,
                "dq" => self.emit_data_list(8)?,
                _ => self.instruction(op)?,
            }
            if !self.at_end() {
                return Err(self.error(self.column, "unexpected input after operands"));
            }
        }
        if count == 0 {
            self.column = self.line.len();
        }
        Ok(())
    }

    fn instruction(&mut self, op: Token<'src>) -> Result<()> {
        match self.object.target {
            Target::X86_64 => crate::x86::instruction(self, op),
            Target::Aarch64 => crate::aarch64::instruction(self, op),
        }
    }

    pub(crate) fn uses_nasm(&self) -> bool {
        self.nasm.is_some()
    }

    pub(crate) fn x86_bits(&self) -> u8 {
        self.nasm.as_ref().map_or(64, |syntax| syntax.bits)
    }
}
