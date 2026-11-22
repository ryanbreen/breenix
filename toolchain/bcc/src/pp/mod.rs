//! N1570 5.1.1.2 phases 1–3: source mapping, splicing and pp-tokens.

mod normalize;
mod token;

use std::io::{self, Write};
use std::ops::Range;

use normalize::{normalize, phase_one, PhysicalCursor};
use token::token_prefix;

/// A physical source position with a 1-based line and byte column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Header,
    Identifier,
    Number,
    Character,
    String,
    Punctuator,
    Other,
    Newline,
}

/// A preprocessing token with a half-open physical range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token<'a> {
    pub spelling: &'a str,
    pub kind: Kind,
    pub space_before: bool,
    pub begin: Location,
    pub end: Location,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Error {
    pub location: Location,
    pub message: &'static str,
}

#[derive(Debug)]
struct TokenRecord {
    range: Range<usize>,
    kind: Kind,
    space_before: bool,
    begin: Location,
    end: Location,
}

/// The token stream and the normalized buffer holding its spellings.
#[derive(Debug)]
pub struct Source {
    text: String,
    tokens: Vec<TokenRecord>,
}

impl Source {
    pub fn tokens(&self) -> impl ExactSizeIterator<Item = Token<'_>> {
        self.tokens.iter().map(|token| Token {
            spelling: &self.text[token.range.clone()],
            kind: token.kind,
            space_before: token.space_before,
            begin: token.begin,
            end: token.end,
        })
    }

    /// Write the phase-3 token stream with normalized spacing and line endings.
    pub fn write(&self, mut out: impl Write) -> io::Result<()> {
        let mut backslash = false;
        for token in self.tokens() {
            // Keep a surviving backslash from becoming a splice in the output.
            if token.space_before || (backslash && token.kind == Kind::Newline) {
                out.write_all(b" ")?;
            }
            out.write_all(token.spelling.as_bytes())?;
            backslash = token.spelling == "\\";
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum LineContext {
    Start,
    AfterHash,
    Include,
    Body,
}

impl LineContext {
    fn after(self, kind: Kind, spelling: &str) -> Self {
        match (self, kind, spelling) {
            (_, Kind::Newline, _) => Self::Start,
            (Self::Start, Kind::Punctuator, "#" | "%:") => Self::AfterHash,
            (Self::AfterHash, Kind::Identifier, "include") => Self::Include,
            _ => Self::Body,
        }
    }
}

/// Scan a UTF-8 source file through translation phase 3.
pub fn scan(bytes: &[u8]) -> Result<Source, Error> {
    let mut physical = PhysicalCursor::new(bytes);
    let input = std::str::from_utf8(bytes).map_err(|error| Error {
        location: physical.locate(error.valid_up_to()),
        message: "invalid UTF-8 in source",
    })?;
    let normalized = normalize(input)?;
    let text = normalized.text;
    let origins = normalized.origins;
    let mut tokens = Vec::new();
    let mut context = LineContext::Start;
    let mut space = false;
    let mut start = 0;
    while start < text.len() {
        let rest = &text[start..];
        if rest.starts_with([' ', '\t', '\x0b', '\x0c']) {
            space = true;
            start += 1;
            continue;
        }
        // A comment is one space. A // comment leaves its terminating newline
        // to be tokenized, while a /* comment consumes its entire contents.
        if let Some(body) = rest.strip_prefix("//") {
            space = true;
            start += 2 + body.find('\n').unwrap_or(body.len());
            continue;
        }
        if let Some(body) = rest.strip_prefix("/*") {
            let close = body.find("*/").ok_or_else(|| Error {
                location: physical.locate(origins[start]),
                message: "unterminated comment",
            })?;
            space = true;
            start += 2 + close + 2;
            continue;
        }

        let (kind, width) =
            token_prefix(rest, matches!(context, LineContext::Include)).map_err(|error| Error {
                location: physical.locate(origins[start + error.offset]),
                message: error.message,
            })?;
        let end = start + width;
        let last = origins[end - 1];
        // The end follows the last contributing physical character, before any
        // splice or whitespace between this token and the next one.
        let last_width = phase_one(&input[last..]).map_or(0, |(_, width)| width);
        tokens.push(TokenRecord {
            range: start..end,
            kind,
            space_before: space && kind != Kind::Newline,
            begin: physical.locate(origins[start]),
            end: physical.locate(last + last_width),
        });
        context = context.after(kind, &text[start..end]);
        space = false;
        start = end;
    }
    Ok(Source { text, tokens })
}
