//! Integer and escape spellings from N1570 6.4.4, and 6.10.9 destringizing.

pub(crate) struct IntegerConstant {
    pub value: u64,
    pub decimal: bool,
    pub unsigned: bool,
    pub longs: usize,
}

// N1570 6.4.4.1: separate the spelling from the context's candidate types.
pub(crate) fn integer_constant(text: &str) -> Option<IntegerConstant> {
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
    let value = u64::from_str_radix(digits, radix).ok()?;
    let unsigned = suffix.starts_with(['u', 'U']) || suffix.ends_with(['u', 'U']);
    let longs = suffix
        .strip_prefix(['u', 'U'])
        .or_else(|| suffix.strip_suffix(['u', 'U']))
        .unwrap_or(suffix);
    let longs = match longs {
        "" => 0,
        "l" | "L" => 1,
        "ll" | "LL" => 2,
        _ => return None,
    };
    Some(IntegerConstant {
        value,
        decimal: radix == 10,
        unsigned,
        longs,
    })
}

pub(super) fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
}

pub(super) fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

pub(super) fn destringize(text: &str) -> Option<String> {
    let (_, contents) = text.split_once('"')?;
    let mut chars = contents.strip_suffix('"')?.chars().peekable();
    let mut result = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' && matches!(chars.peek(), Some('"' | '\\')) {
            result.push(chars.next()?);
        } else {
            result.push(ch);
        }
    }
    Some(result)
}

pub(super) fn decode(text: &str) -> Option<String> {
    let mut chars = text.chars().peekable();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        let ch = if ch == '\\' {
            char::from_u32(escape(&mut chars)?.value)?
        } else {
            ch
        };
        // A presumed filename must fit on a diagnostic line.
        if matches!(ch, '\0' | '\r' | '\n') {
            return None;
        }
        out.push(ch);
    }
    Some(out)
}

pub(crate) struct Escape {
    pub value: u32,
    pub numeric: bool,
}

pub(crate) fn escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<Escape> {
    let lead = chars.next()?;
    let value = match lead {
        '\'' | '"' | '?' | '\\' => u32::from(lead),
        'a' => 7,
        'b' => 8,
        'f' => 12,
        'n' => 10,
        'r' => 13,
        't' => 9,
        'v' => 11,
        '0'..='7' => {
            let mut num = lead.to_digit(8)?;
            for _ in 0..2 {
                let Some(digit) = chars.peek().and_then(|ch| ch.to_digit(8)) else {
                    break;
                };
                chars.next();
                num = num * 8 + digit;
            }
            num
        }
        'x' => {
            let mut num = chars.next()?.to_digit(16)?;
            while let Some(digit) = chars.peek().and_then(|ch| ch.to_digit(16)) {
                chars.next();
                num = num.checked_mul(16)?.checked_add(digit)?;
            }
            num
        }
        'u' | 'U' => {
            let mut num = 0;
            for _ in 0..if lead == 'u' { 4 } else { 8 } {
                num = num * 16 + chars.next()?.to_digit(16)?;
            }
            u32::from(char::from_u32(num)?)
        }
        _ => return None,
    };
    Some(Escape {
        value,
        numeric: matches!(lead, '0'..='7' | 'x'),
    })
}
