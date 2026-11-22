//! N1570 6.10.1 and 6.5: controlling expressions in intmax_t/uintmax_t.
use super::directive::{Diagnostic, Target};
use super::expand::{Expander, Lexeme};
use super::{literal, Kind};

pub(super) fn evaluate(
    macros: &Expander,
    target: Target,
    directive: &Lexeme,
    tokens: &[Lexeme],
) -> Result<bool, Diagnostic> {
    let mut defined = Vec::new();
    let mut input = tokens.iter().peekable();
    while let Some(token) = input.next() {
        if !token.is("defined") {
            defined.push(token.clone());
            continue;
        }
        let parenthesized = input.next_if(|t| t.is("(")).is_some();
        let name = input
            .next()
            .filter(|t| t.kind == Kind::Identifier)
            .ok_or_else(|| token.error("expected an identifier after defined"))?;
        if parenthesized && input.next_if(|t| t.is(")")).is_none() {
            return Err(token.error("expected ')' after defined operand"));
        }
        defined.push(token.respell(
            if macros.is_defined(&name.text) {
                "1"
            } else {
                "0"
            },
            Kind::Number,
        ));
    }

    let tokens = macros.expand(defined)?;
    let mut parser = Parser {
        rest: &tokens,
        directive,
        target,
    };
    let value = parser.parse(0, 0, true)?;
    if let Some(extra) = parser.rest.first() {
        return Err(extra.error("unexpected token in preprocessing expression"));
    }
    Ok(value.truth())
}

// Both target ABIs use a 64-bit intmax_t and uintmax_t. Keep the signedness
// separate so the unselected arm of ?: can still determine the result type.
#[derive(Clone, Copy)]
struct Value {
    bits: u64,
    unsigned: bool,
}

impl Value {
    const ZERO: Self = Self::signed(0);

    const fn signed(value: i64) -> Self {
        Self {
            bits: value as u64,
            unsigned: false,
        }
    }

    fn boolean(value: bool) -> Self {
        Self::signed(i64::from(value))
    }

    fn truth(self) -> bool {
        self.bits != 0
    }
}

struct Parser<'a> {
    rest: &'a [Lexeme],
    directive: &'a Lexeme,
    target: Target,
}

impl<'a> Parser<'a> {
    fn next(&mut self) -> Option<&'a Lexeme> {
        let (first, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(first)
    }

    fn expect(&mut self, spelling: &str, at: &Lexeme) -> Result<(), Diagnostic> {
        match self.next() {
            Some(token) if token.is(spelling) => Ok(()),
            token => Err(token
                .unwrap_or(at)
                .error(format!("expected '{spelling}' in preprocessing expression"))),
        }
    }

    fn parse(&mut self, min: u8, depth: usize, evaluate: bool) -> Result<Value, Diagnostic> {
        let at = self.rest.first().unwrap_or(self.directive);
        if depth >= 128 {
            return Err(at.error("preprocessing expression nesting limit exceeded"));
        }
        let mut left = self.primary(depth, evaluate)?;
        while let Some(operator) = self.rest.first() {
            let Some(precedence) = precedence(&operator.text) else {
                break;
            };
            if precedence < min {
                break;
            }
            self.rest = &self.rest[1..];
            if operator.is("?") {
                let yes = self.parse(0, depth + 1, evaluate && left.truth())?;
                self.expect(":", operator)?;
                let no = self.parse(precedence, depth + 1, evaluate && !left.truth())?;
                let selected = if left.truth() { yes } else { no };
                left = Value {
                    unsigned: yes.unsigned || no.unsigned,
                    ..selected
                };
            } else {
                let evaluate_right = evaluate
                    && (!operator.is("&&") || left.truth())
                    && (!operator.is("||") || !left.truth());
                let right = self.parse(precedence + 1, depth + 1, evaluate_right)?;
                left = binary(operator, left, right, evaluate)?;
            }
        }
        Ok(left)
    }

    fn primary(&mut self, depth: usize, evaluate: bool) -> Result<Value, Diagnostic> {
        let token = self.next().ok_or_else(|| {
            self.directive
                .error("expected an operand in preprocessing expression")
        })?;
        match token.text.as_ref() {
            "(" => {
                let value = self.parse(0, depth + 1, evaluate)?;
                self.expect(")", token)?;
                Ok(value)
            }
            "+" | "-" | "~" | "!" => {
                let value = self.parse(12, depth + 1, evaluate)?;
                unary(token, value, evaluate)
            }
            "defined" => Err(token.error("defined must appear before macro expansion")),
            _ => match token.kind {
                Kind::Number => integer(&token.text).ok_or_else(|| {
                    token.error("invalid integer constant in preprocessing expression")
                }),
                Kind::Character => character(&token.text, self.target).ok_or_else(|| {
                    token.error("invalid character constant in preprocessing expression")
                }),
                Kind::Identifier => Ok(Value::ZERO),
                _ => Err(token.error("expected an integer operand in preprocessing expression")),
            },
        }
    }
}

fn precedence(operator: &str) -> Option<u8> {
    Some(match operator {
        "?" => 1,
        "||" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" => 7,
        "<" | ">" | "<=" | ">=" => 8,
        "<<" | ">>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        _ => return None,
    })
}

fn unary(operator: &Lexeme, value: Value, evaluate: bool) -> Result<Value, Diagnostic> {
    let bits = match operator.text.as_ref() {
        "!" => return Ok(Value::boolean(evaluate && !value.truth())),
        _ if !evaluate => 0,
        "-" if value.unsigned => value.bits.wrapping_neg(),
        "-" => (value.bits as i64)
            .checked_neg()
            .ok_or_else(|| operator.error("signed integer overflow"))? as u64,
        "~" => !value.bits,
        _ => value.bits,
    };
    Ok(Value { bits, ..value })
}

fn binary(
    operator: &Lexeme,
    left: Value,
    right: Value,
    evaluate: bool,
) -> Result<Value, Diagnostic> {
    let op = operator.text.as_ref();
    let a = left.bits;
    let b = right.bits;
    let signed_a = a as i64;
    let signed_b = b as i64;
    let unsigned = left.unsigned || right.unsigned;
    let order = if unsigned {
        a.cmp(&b)
    } else {
        signed_a.cmp(&signed_b)
    };
    let boolean = match op {
        "||" => Some(left.truth() || right.truth()),
        "&&" => Some(left.truth() && right.truth()),
        "==" => Some(order.is_eq()),
        "!=" => Some(order.is_ne()),
        "<" => Some(order.is_lt()),
        "<=" => Some(order.is_le()),
        ">" => Some(order.is_gt()),
        ">=" => Some(order.is_ge()),
        _ => None,
    };
    if let Some(result) = boolean {
        return Ok(Value::boolean(evaluate && result));
    }

    if matches!(op, "<<" | ">>") {
        return shift(operator, left, right, evaluate);
    }
    if !evaluate {
        return Ok(Value { bits: 0, unsigned });
    }
    if matches!(op, "/" | "%") && b == 0 {
        return Err(operator.error("division by zero in preprocessing expression"));
    }
    let bits = match op {
        "&" => a & b,
        "|" => a | b,
        "^" => a ^ b,
        "+" if unsigned => a.wrapping_add(b),
        "-" if unsigned => a.wrapping_sub(b),
        "*" if unsigned => a.wrapping_mul(b),
        "/" if unsigned => a / b,
        "%" if unsigned => a % b,
        _ => {
            let result = match op {
                "+" => signed_a.checked_add(signed_b),
                "-" => signed_a.checked_sub(signed_b),
                "*" => signed_a.checked_mul(signed_b),
                "/" => signed_a.checked_div(signed_b),
                "%" => signed_a.checked_rem(signed_b),
                _ => return Err(operator.error("invalid preprocessing operator")),
            };
            result.ok_or_else(|| operator.error("signed integer overflow"))? as u64
        }
    };
    Ok(Value { bits, unsigned })
}

fn shift(
    operator: &Lexeme,
    left: Value,
    right: Value,
    evaluate: bool,
) -> Result<Value, Diagnostic> {
    if !evaluate {
        return Ok(Value { bits: 0, ..left });
    }
    if right.bits >= u64::from(Target::INTMAX_BITS) {
        return Err(operator.error("shift count out of range"));
    }
    let amount = right.bits as u32;
    let signed = left.bits as i64;
    let bits = match (operator.is("<<"), left.unsigned) {
        (true, true) => left.bits << amount,
        (false, true) => left.bits >> amount,
        (false, false) => (signed >> amount) as u64,
        (true, false) => {
            if signed < 0 {
                return Err(operator.error("left shift of a negative value"));
            }
            i64::try_from(i128::from(signed) << amount)
                .map_err(|_| operator.error("signed integer overflow"))? as u64
        }
    };
    Ok(Value { bits, ..left })
}

// N1570 6.4.4.1 integer candidates at intmax_t and uintmax_t width
// per 6.10.1p4.
fn integer(text: &str) -> Option<Value> {
    let (radix, text) =
        if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (16, hex)
        } else if text.starts_with('0') {
            (8, text)
        } else {
            (10, text)
        };
    let width = text
        .bytes()
        .take_while(|byte| match radix {
            16 => byte.is_ascii_hexdigit(),
            8 => matches!(byte, b'0'..=b'7'),
            _ => byte.is_ascii_digit(),
        })
        .count();
    let (digits, suffix) = text.split_at(width);
    let bits = u64::from_str_radix(digits, radix).ok()?;
    let explicit_unsigned = match suffix {
        "" | "l" | "L" | "ll" | "LL" => false,
        "u" | "U" | "ul" | "uL" | "Ul" | "UL" | "lu" | "lU" | "Lu" | "LU" | "ull" | "uLL"
        | "Ull" | "ULL" | "llu" | "llU" | "LLu" | "LLU" => true,
        _ => return None,
    };
    let exceeds_signed = bits > i64::MAX as u64;
    if radix == 10 && exceeds_signed && !explicit_unsigned {
        return None;
    }
    Some(Value {
        bits,
        unsigned: explicit_unsigned || exceeds_signed,
    })
}

fn character(text: &str, target: Target) -> Option<Value> {
    let (prefix, contents) = text.split_once('\'')?;
    let mut chars = contents.strip_suffix('\'')?.chars().peekable();
    let mut units = 0;
    let mut bits = 0u64;
    while let Some(ch) = chars.next() {
        let (value, numeric) = if ch == '\\' {
            let code = literal::escape(&mut chars)?;
            (code.value, code.numeric)
        } else {
            (u32::from(ch), false)
        };
        if prefix.is_empty() && !numeric {
            let mut buf = [0; 4];
            for byte in char::from_u32(value)?.encode_utf8(&mut buf).bytes() {
                bits = (bits << 8) | u64::from(byte);
                units += 1;
            }
        } else if prefix.is_empty() {
            bits = (bits << 8) | u64::from(u8::try_from(value).ok()?);
            units += 1;
        } else {
            if units != 0 || (prefix == "u" && value > u32::from(u16::MAX)) {
                return None;
            }
            bits = u64::from(value);
            units = 1;
        }
        if units > 4 {
            return None;
        }
    }
    Some(match prefix {
        "" if units == 1 && target.signed_char() => Value::signed(i64::from(bits as i8)),
        "" if units != 0 => Value::signed(i64::from(bits as i32)),
        "L" if target == Target::X86_64 => Value::signed(i64::from(bits as i32)),
        "L" | "u" | "U" => Value {
            bits,
            unsigned: true,
        },
        _ => return None,
    })
}
