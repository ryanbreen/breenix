//! Absolute constants and layout directives for the AArch64 kernel sources.
//! Directive syntax and operator precedence follow black-box llvm-mc.

use std::collections::BTreeMap;

use super::{starts_number, Parser, Token};
use crate::object::Object;
use crate::{check_source_line, Location, Result};

pub(super) type Constants<'src> = BTreeMap<&'src str, i128>;

struct Repeat {
    start: usize,
    remaining: u64,
    at: Location,
}

pub(crate) fn assemble(source: &[u8], object: &mut Object) -> Result<()> {
    let lines: Vec<_> = source.split(|&byte| byte == b'\n').collect();
    let mut constants = Constants::new();
    let mut repeats = Vec::<Repeat>::new();
    let mut skipped_depth = 0usize;
    let mut cursor = 0;
    // Bound expansion even when repeated lines emit no bytes or nest.
    let mut expanded_lines = 0;
    while let Some(&line) = lines.get(cursor) {
        cursor += 1;
        check_source_line(line, cursor)?;
        let mut parser = Parser::new(line, cursor, object);
        parser.constants = Some(&mut constants);
        let op = parser.word();
        let at = parser.location(op.column);
        if !repeats.is_empty() {
            expanded_lines += 1;
            if expanded_lines > 1_048_576 {
                return Err(at.error(".rept expansion exceeds the supported limit"));
            }
        }
        if skipped_depth != 0 {
            match op.text {
                ".rept" => skipped_depth += 1,
                ".endr" => skipped_depth -= 1,
                _ => {}
            }
            if skipped_depth != 0 {
                continue;
            }
        }
        match op.text {
            ".rept" => {
                let remaining = parser.count()?;
                parser.require_end()?;
                repeats.push(Repeat {
                    start: cursor,
                    remaining,
                    at,
                });
                skipped_depth = usize::from(remaining == 0);
            }
            ".endr" => {
                parser.require_end()?;
                let repeat = repeats
                    .last_mut()
                    .ok_or_else(|| at.error(".endr without .rept"))?;
                if repeat.remaining > 1 {
                    repeat.remaining -= 1;
                    cursor = repeat.start;
                } else {
                    repeats.pop();
                }
            }
            _ => {
                parser.column = 0;
                parser.assemble_line()?;
            }
        }
    }
    if let Some(repeat) = repeats.last() {
        return Err(repeat.at.error(".rept without .endr"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Operator {
    Add,
    Sub,
    Or,
    And,
    Left,
    Right,
}

impl Operator {
    // GNU-style groups: shifts first, then bitwise operators, then addition
    // and subtraction. Operators within a group associate left-to-right.
    fn precedence(self) -> u8 {
        match self {
            Self::Add | Self::Sub => 1,
            Self::Or | Self::And => 2,
            Self::Left | Self::Right => 3,
        }
    }

    fn apply(self, left: i128, right: i128) -> Option<i128> {
        // i128 gives a 64-bit input room to shift left by up to 63 bits
        // before the result is checked against the assembler's range.
        match self {
            Self::Add => Some(left + right),
            Self::Sub => Some(left - right),
            Self::Or => Some(left | right),
            Self::And => Some(left & right),
            Self::Left => Some(left << shift_amount(right)?),
            Self::Right => Some(i128::from((left as u64) >> shift_amount(right)?)),
        }
    }
}

fn shift_amount(value: i128) -> Option<u32> {
    u32::try_from(value).ok().filter(|&shift| shift < 64)
}

impl<'src> Parser<'src, '_> {
    pub(super) fn gnu_directive(&mut self, op: Token<'src>) -> Result<bool> {
        match op.text {
            ".equ" | ".set" => {
                let name = self.word();
                if !name.is_name() {
                    return Err(self.error(name.column, "expected a constant name"));
                }
                self.expect(b',', "expected ',' before constant expression")?;
                let value = self.gnu_expression()?;
                if let Some(constants) = &mut self.constants {
                    constants.insert(name.text, value);
                }
            }
            ".skip" => {
                let count = self.count()?;
                let fill = if self.take(b',') {
                    self.integer(1, false)? as u8
                } else {
                    0
                };
                self.object.fill(self.location(op.column), count, fill)?;
            }
            ".inst" => loop {
                let word = self.integer(4, false)? as u32;
                self.emit_aarch64(op.column, word)?;
                if !self.take(b',') {
                    break;
                }
            },
            ".balign" => {
                self.peek();
                let column = self.column;
                let alignment = self.count()?;
                if !alignment.is_power_of_two() {
                    return Err(self.error(column, "alignment must be a positive power of two"));
                }
                let mut fill = None;
                let mut maximum = None;
                if self.take(b',') {
                    if self.peek() != b',' {
                        fill = Some(self.integer(1, false)? as u8);
                    }
                    if self.take(b',') {
                        maximum = Some(self.count()?).filter(|&value| value != 0);
                    }
                }
                self.object
                    .align_aarch64(self.location(column), alignment, fill, maximum)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(super) fn starts_gnu_expression(&mut self) -> bool {
        if self.constants.is_none() {
            return false;
        }
        let next = self.peek();
        if starts_number(next) || matches!(next, b'(' | b'~') {
            return true;
        }
        let column = self.column;
        let name = self.word();
        self.column = column;
        self.constant(name.text).is_some()
    }

    fn constant(&self, name: &str) -> Option<i128> {
        self.constants.as_ref()?.get(name).copied()
    }

    pub(super) fn gnu_expression(&mut self) -> Result<i128> {
        self.expression(1, 0)
    }

    fn expression(&mut self, precedence: u8, depth: usize) -> Result<i128> {
        let mut left = self.term(depth)?;
        while let Some((operator, length)) = self.operator() {
            let level = operator.precedence();
            if level < precedence {
                break;
            }
            let column = self.column;
            self.column += length;
            let right = self.expression(level + 1, depth)?;
            left = self.checked_value(operator.apply(left, right), column)?;
        }
        Ok(left)
    }

    fn term(&mut self, depth: usize) -> Result<i128> {
        let byte = self.peek();
        let column = self.column;
        if depth == 64 {
            return Err(self.error(column, "constant expression is nested too deeply"));
        }
        match byte {
            b'(' => {
                self.column += 1;
                let value = self.expression(1, depth + 1)?;
                self.expect(b')', "expected ')' after constant expression")?;
                Ok(value)
            }
            b'+' | b'-' | b'~' => {
                self.column += 1;
                let value = self.term(depth + 1)?;
                match byte {
                    b'-' => self.checked_value(Some(-value), column),
                    b'~' => Ok(i128::from(!(value as u64) as i64)),
                    _ => Ok(value),
                }
            }
            b'0'..=b'9' => self.magnitude().map(i128::from),
            _ => {
                let name = self.word();
                self.constant(name.text)
                    .ok_or_else(|| self.error(column, "expected an integer or defined constant"))
            }
        }
    }

    fn operator(&mut self) -> Option<(Operator, usize)> {
        let byte = self.peek();
        let rest = &self.line[self.column..];
        match byte {
            b'+' => Some((Operator::Add, 1)),
            b'-' => Some((Operator::Sub, 1)),
            b'|' => Some((Operator::Or, 1)),
            b'&' => Some((Operator::And, 1)),
            b'<' if rest.starts_with(b"<<") => Some((Operator::Left, 2)),
            b'>' if rest.starts_with(b">>") => Some((Operator::Right, 2)),
            _ => None,
        }
    }

    fn checked_value(&self, value: Option<i128>, column: usize) -> Result<i128> {
        value
            .filter(|value| (i128::from(i64::MIN)..=i128::from(u64::MAX)).contains(value))
            .ok_or_else(|| self.error(column, "constant expression out of range"))
    }
}
