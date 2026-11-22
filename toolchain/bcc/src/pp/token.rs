//! Preprocessing token grammar from N1570 6.4.

use super::Kind;

pub(super) struct TokenError {
    pub offset: usize,
    pub message: &'static str,
}

// N1570 6.4p4: longest match, including the digraph spellings in 6.4.6.
const PUNCTUATORS: &[&str] = &[
    "%:%:", "<<=", ">>=", "...", "->", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||",
    "*=", "/=", "%=", "+=", "-=", "&=", "^=", "|=", "##", "<:", ":>", "<%", "%>", "%:",
];

pub(super) fn token_prefix(text: &str, include: bool) -> Result<(Kind, usize), TokenError> {
    let Some(first) = text.chars().next() else {
        return Err(TokenError {
            offset: 0,
            message: "expected a preprocessing token",
        });
    };
    if first == '\n' {
        return Ok((Kind::Newline, 1));
    }
    if include {
        if let Some(width) = header_width(text) {
            return Ok((Kind::Header, width));
        }
    }
    if let Some(opening) = literal_opening(text.as_bytes()) {
        return literal(text, opening);
    }

    let bytes = text.as_bytes();
    if matches!(bytes, [b'0'..=b'9', ..] | [b'.', b'0'..=b'9', ..]) {
        return Ok((Kind::Number, number_width(bytes)));
    }
    let leading = nondigit_width(bytes);
    if leading != 0 {
        let mut end = leading;
        while let Some(byte) = bytes.get(end) {
            let width = if byte.is_ascii_digit() {
                1
            } else {
                nondigit_width(&bytes[end..])
            };
            if width == 0 {
                break;
            }
            end += width;
        }
        return Ok((Kind::Identifier, end));
    }
    if let Some(punctuator) = PUNCTUATORS.iter().find(|p| text.starts_with(**p)) {
        return Ok((Kind::Punctuator, punctuator.len()));
    }
    if "[](){}.&*+-~!/%<>^|?:;=,#".contains(first) {
        return Ok((Kind::Punctuator, 1));
    }
    Ok((Kind::Other, first.len_utf8()))
}

// N1570 6.4.2.1 and 6.4.3. Recognize UCN spellings without translating them.
fn nondigit_width(bytes: &[u8]) -> usize {
    match bytes {
        [b'a'..=b'z' | b'A'..=b'Z' | b'_', ..] => 1,
        _ => ucn_width(bytes),
    }
}

fn ucn_width(bytes: &[u8]) -> usize {
    let digits = match bytes {
        [b'\\', b'u', rest @ ..] => rest.get(..4),
        [b'\\', b'U', rest @ ..] => rest.get(..8),
        _ => None,
    };
    match digits {
        Some(digits) if digits.iter().all(u8::is_ascii_hexdigit) => 2 + digits.len(),
        _ => 0,
    }
}

// N1570 6.4.8. A pp-number need not have a numerical value.
fn number_width(bytes: &[u8]) -> usize {
    let mut end = 0;
    loop {
        let width = match &bytes[end..] {
            [b'e' | b'E' | b'p' | b'P', b'+' | b'-', ..] => 2,
            [b'0'..=b'9' | b'.', ..] => 1,
            rest => nondigit_width(rest),
        };
        if width == 0 {
            return end;
        }
        end += width;
    }
}

// N1570 6.4.4.4 and 6.4.5: u8 prefixes strings, while u, U and L prefix both.
fn literal_opening(bytes: &[u8]) -> Option<usize> {
    match bytes {
        [b'\'' | b'"', ..] => Some(0),
        [b'u' | b'U' | b'L', b'\'' | b'"', ..] => Some(1),
        [b'u', b'8', b'"', ..] => Some(2),
        _ => None,
    }
}

fn literal(text: &str, opening: usize) -> Result<(Kind, usize), TokenError> {
    let quote = text.as_bytes()[opening];
    let (kind, unterminated) = if quote == b'"' {
        (Kind::String, "unterminated string literal")
    } else {
        (Kind::Character, "unterminated character constant")
    };
    let content = &text.as_bytes()[opening + 1..];
    let closing = closing_quote(content, quote).ok_or(TokenError {
        offset: opening,
        message: unterminated,
    })?;
    if kind == Kind::Character && closing == 0 {
        return Err(TokenError {
            offset: opening,
            message: "empty character constant",
        });
    }
    if let Some(offset) = invalid_escape(&content[..closing]) {
        return Err(TokenError {
            offset: opening + 1 + offset,
            message: "invalid escape sequence",
        });
    }
    Ok((kind, opening + 1 + closing + 1))
}

fn closing_quote(content: &[u8], quote: u8) -> Option<usize> {
    let mut escaped = false;
    for (index, &byte) in content.iter().enumerate() {
        if byte == b'\n' {
            break;
        }
        if byte == quote && !escaped {
            return Some(index);
        }
        escaped = byte == b'\\' && !escaped;
    }
    None
}

fn invalid_escape(content: &[u8]) -> Option<usize> {
    let mut offset = 0;
    while offset < content.len() {
        let width = match &content[offset..] {
            [b'\\', b'\''
            | b'"'
            | b'?'
            | b'\\'
            | b'a'
            | b'b'
            | b'f'
            | b'n'
            | b'r'
            | b't'
            | b'v'
            | b'0'..=b'7', ..] => 2,
            [b'\\', b'x', digit, ..] if digit.is_ascii_hexdigit() => 3,
            rest @ [b'\\', ..] => ucn_width(rest),
            _ => 1,
        };
        if width == 0 {
            return Some(offset);
        }
        offset += width;
    }
    None
}

// N1570 6.4.7. A header name is recognized only in its directive context.
fn header_width(text: &str) -> Option<usize> {
    let closing = match text.as_bytes() {
        [b'<', ..] => '>',
        [b'"', ..] => '"',
        _ => return None,
    };
    let index = text[1..].find([closing, '\n'])? + 1;
    (index > 1 && text.as_bytes()[index] == closing as u8).then_some(index + 1)
}
