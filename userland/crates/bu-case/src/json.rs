//! The path-only workspace uses this JSON codec for specs and run records.

use std::collections::BTreeMap;
use std::fmt::{self, Write};

#[derive(Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut parser = Parser { rest: text };
        let value = parser.value(0)?;
        parser.space();
        if !parser.rest.is_empty() {
            return Err("trailing data after JSON value".into());
        }
        Ok(value)
    }

    pub fn object<const N: usize>(fields: [(&str, Json); N]) -> Self {
        Self::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    pub fn number(value: impl fmt::Display) -> Self {
        Self::Number(value.to_string())
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

fn quote(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in value.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            '\0'..='\u{1f}' => write!(f, "\\u{:04x}", c as u32)?,
            _ => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Number(value) => f.write_str(value),
            Self::String(value) => quote(f, value),
            Self::Array(values) => {
                f.write_char('[')?;
                for (i, value) in values.iter().enumerate() {
                    if i != 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{value}")?;
                }
                f.write_char(']')
            }
            Self::Object(fields) => {
                f.write_char('{')?;
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i != 0 {
                        f.write_char(',')?;
                    }
                    quote(f, key)?;
                    write!(f, ":{value}")?;
                }
                f.write_char('}')
            }
        }
    }
}

struct Parser<'a> {
    rest: &'a str,
}

impl<'a> Parser<'a> {
    fn space(&mut self) {
        self.rest = self.rest.trim_start_matches([' ', '\n', '\r', '\t']);
    }

    fn eat(&mut self, token: &str) -> bool {
        if let Some(rest) = self.rest.strip_prefix(token) {
            self.rest = rest;
            return true;
        }
        false
    }

    fn require(&mut self, token: &str) -> Result<(), String> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(format!("expected JSON {token:?}"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth == 32 {
            return Err("JSON nesting exceeds 32 levels".into());
        }
        self.space();
        match self.rest.as_bytes().first() {
            Some(b'{') => {
                self.eat("{");
                let mut fields = BTreeMap::new();
                self.space();
                if !self.eat("}") {
                    loop {
                        let key = self.string()?;
                        self.space();
                        self.require(":")?;
                        let value = self.value(depth + 1)?;
                        if fields.insert(key.clone(), value).is_some() {
                            return Err(format!("duplicate JSON key {key:?}"));
                        }
                        self.space();
                        if self.eat("}") {
                            break;
                        }
                        self.require(",")?;
                        self.space();
                    }
                }
                Ok(Json::Object(fields))
            }
            Some(b'[') => {
                self.eat("[");
                let mut values = Vec::new();
                self.space();
                if !self.eat("]") {
                    loop {
                        values.push(self.value(depth + 1)?);
                        self.space();
                        if self.eat("]") {
                            break;
                        }
                        self.require(",")?;
                    }
                }
                Ok(Json::Array(values))
            }
            Some(b'"') => self.string().map(Json::String),
            Some(b'-' | b'0'..=b'9') => self.number().map(Json::Number),
            _ if self.eat("true") => Ok(Json::Bool(true)),
            _ if self.eat("false") => Ok(Json::Bool(false)),
            _ if self.eat("null") => Ok(Json::Null),
            _ => Err("expected JSON value".into()),
        }
    }

    fn next_char(&mut self) -> Result<char, String> {
        let c = self.rest.chars().next().ok_or("unterminated JSON string")?;
        self.rest = &self.rest[c.len_utf8()..];
        Ok(c)
    }

    fn hex_unit(&mut self) -> Result<u16, String> {
        let digits = self
            .rest
            .get(..4)
            .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("invalid JSON Unicode escape")?;
        self.rest = &self.rest[4..];
        u16::from_str_radix(digits, 16).map_err(|e| e.to_string())
    }

    fn unicode(&mut self) -> Result<char, String> {
        let head = self.hex_unit()?;
        let code = if (0xd800..=0xdbff).contains(&head) {
            self.require("\\u")?;
            let tail = self.hex_unit()?;
            if !(0xdc00..=0xdfff).contains(&tail) {
                return Err("invalid JSON surrogate pair".into());
            }
            0x10000 + (u32::from(head - 0xd800) << 10) + u32::from(tail - 0xdc00)
        } else {
            u32::from(head)
        };
        char::from_u32(code).ok_or_else(|| "invalid JSON Unicode scalar".into())
    }

    fn string(&mut self) -> Result<String, String> {
        self.require("\"")?;
        let mut text = String::new();
        loop {
            let c = match self.next_char()? {
                '"' => return Ok(text),
                '\\' => match self.next_char()? {
                    c @ ('"' | '\\' | '/') => c,
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => self.unicode()?,
                    _ => return Err("invalid JSON string escape".into()),
                },
                '\0'..='\u{1f}' => return Err("control character in JSON string".into()),
                c => c,
            };
            text.push(c);
        }
    }

    fn digits(&mut self) -> Result<(), String> {
        let end = self.rest.bytes().take_while(u8::is_ascii_digit).count();
        if end == 0 {
            return Err("expected JSON number digits".into());
        }
        self.rest = &self.rest[end..];
        Ok(())
    }

    fn number(&mut self) -> Result<String, String> {
        let start = self.rest;
        self.eat("-");
        if !self.eat("0") {
            self.digits()?;
        }
        if self.eat(".") {
            self.digits()?;
        }
        if self.eat("e") || self.eat("E") {
            if !self.eat("+") {
                self.eat("-");
            }
            self.digits()?;
        }
        Ok(start[..start.len() - self.rest.len()].into())
    }
}
