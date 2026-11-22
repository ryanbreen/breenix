//! N1570 5.1.1.2 phases 1–4: preprocessing tokens, macros and directives.

mod directive;
mod expand;
mod expression;
mod literal;
mod normalize;
mod token;

use std::io::{self, Write};
use std::ops::Range;

use normalize::{normalize, phase_one, PhysicalCursor};
use token::{token_prefix, LexContext};

pub(crate) use directive::read_preprocessed;
pub use directive::{
    preprocess, preprocess_source, Diagnostic, MacroOption, Options, Preprocessed, Target,
};
pub(crate) use expand::Lexeme;
pub use expand::Position;
pub(crate) fn character_bits(text: &str, target: Target) -> Option<u64> {
    expression::character_bits(text, target)
}
pub(crate) use literal::{escape, integer_constant};

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
    Pragma,
    Body,
}

impl LineContext {
    fn after(self, kind: Kind, spelling: &str) -> Self {
        match (self, kind, spelling) {
            (_, Kind::Newline, _) => Self::Start,
            (Self::Start, Kind::Punctuator, "#" | "%:") => Self::AfterHash,
            (Self::AfterHash, Kind::Identifier, "include") => Self::Include,
            (Self::AfterHash, Kind::Identifier, "pragma") => Self::Pragma,
            (Self::Pragma, _, _) => Self::Pragma,
            _ => Self::Body,
        }
    }
}

/// Scan a UTF-8 source file through translation phase 3.
pub fn scan(bytes: &[u8]) -> Result<Source, Error> {
    let (source, error) = scan_prefix(bytes);
    error.map_or(Ok(source), Err)
}

// Keep completed lines available so phase 4 can apply preceding #line
// directives before reporting a lexer error.
fn scan_prefix(bytes: &[u8]) -> (Source, Option<Error>) {
    let (input, encoding_error) = match std::str::from_utf8(bytes) {
        Ok(input) => (input, None),
        Err(error) => {
            let offset = error.valid_up_to();
            let diagnostic = Error {
                location: PhysicalCursor::new(bytes).locate(offset),
                message: "invalid UTF-8 in source",
            };
            (
                std::str::from_utf8(&bytes[..offset]).unwrap_or_default(),
                Some(diagnostic),
            )
        }
    };
    let (normalized, normalization_error) = normalize(input);
    let (source, token_error) = scan_tokens(Some(input), normalized, LineContext::Start);
    (
        source,
        encoding_error.or(normalization_error).or(token_error),
    )
}

// N1570 6.10.9 starts destringized pragma text at phase 3.
fn scan_pragma(text: String) -> Result<Source, Error> {
    scan_string(text, LineContext::Pragma)
}

fn scan_string(text: String, context: LineContext) -> Result<Source, Error> {
    let origins = text
        .char_indices()
        .flat_map(|(offset, ch)| std::iter::repeat_n(offset, ch.len_utf8()))
        .collect();
    let (source, error) = scan_tokens(None, normalize::Normalized { text, origins }, context);
    error.map_or(Ok(source), Err)
}

fn scan_tokens(
    input: Option<&str>,
    normalized: normalize::Normalized,
    mut context: LineContext,
) -> (Source, Option<Error>) {
    let normalize::Normalized { text, origins } = normalized;
    let mut physical = PhysicalCursor::new(input.unwrap_or(&text).as_bytes());
    let mut tokens = Vec::new();
    let mut space = false;
    let mut start = 0;
    let error = loop {
        if start == text.len() {
            break None;
        }
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
            let Some(close) = body.find("*/") else {
                break Some(Error {
                    location: physical.locate(origins[start]),
                    message: "unterminated comment",
                });
            };
            space = true;
            start += 2 + close + 2;
            continue;
        }

        let lex_context = match context {
            LineContext::Include => LexContext::Include,
            LineContext::Pragma => LexContext::Pragma,
            _ => LexContext::Ordinary,
        };
        let (kind, width) = match token_prefix(rest, lex_context) {
            Ok(token) => token,
            Err(error) => {
                break Some(Error {
                    location: physical.locate(origins[start + error.offset]),
                    message: error.message,
                });
            }
        };
        let end = start + width;
        let last = origins[end - 1];
        // The end follows the last contributing physical character, before any
        // splice or whitespace between this token and the next one.
        let last_width = if let Some(input) = input {
            phase_one(&input[last..]).map_or(0, |(_, width)| width)
        } else {
            text[last..].chars().next().map_or(0, char::len_utf8)
        };
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
    };
    (Source { text, tokens }, error)
}
