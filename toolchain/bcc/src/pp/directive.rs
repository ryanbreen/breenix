//! N1570 6.10: directives are recognized before macro replacement.
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::expand::{Expander, Lexeme, Position};
use super::{expression, literal, scan_pragma, scan_prefix, Kind, Location, Token};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Target {
    #[default]
    X86_64,
    Aarch64,
}

impl Target {
    // The Breenix C targets are LP64 in the SysV AMD64 psABI and AAPCS64.
    pub(super) const INTMAX_BITS: u32 = 64;

    pub(super) fn signed_char(self) -> bool {
        self == Self::X86_64
    }
}

#[derive(Debug, Default)]
pub struct Options {
    pub target: Target,
    pub include_dirs: Vec<PathBuf>,
    pub system_dirs: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Diagnostic {
    site: Position,
    message: String,
    pub(super) spelled_at: Option<Position>,
    included_from: Vec<Position>,
}

impl Diagnostic {
    pub(super) fn new(site: &Position, message: impl Into<String>) -> Self {
        Self {
            site: site.clone(),
            message: message.into(),
            spelled_at: None,
            included_from: Vec::new(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}: error: {}", self.site, self.message)?;
        if let Some(site) = &self.spelled_at {
            write!(out, "\n{site}: note: macro token spelled here")?;
        }
        for site in &self.included_from {
            write!(out, "\n{site}: note: included from here")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}

#[derive(Debug)]
enum Output {
    Token(Lexeme),
    Pragma(String),
}

#[derive(Debug, Default)]
pub struct Preprocessed(Vec<Output>);

impl Preprocessed {
    /// Write the phase-4 result with separators that preserve token boundaries.
    pub fn write(&self, mut out: impl Write) -> io::Result<()> {
        let mut in_line = false;
        let mut backslash = false;
        for item in &self.0 {
            match item {
                Output::Token(token) if token.kind == Kind::Newline => {
                    end_line(&mut out, backslash)?;
                    in_line = false;
                    backslash = false;
                }
                Output::Token(token) => {
                    if in_line {
                        out.write_all(b" ")?;
                    }
                    out.write_all(token.text.as_bytes())?;
                    in_line = true;
                    backslash = token.is("\\");
                }
                Output::Pragma(text) => {
                    if in_line {
                        end_line(&mut out, backslash)?;
                    }
                    writeln!(out, "#pragma {text}")?;
                    in_line = false;
                    backslash = false;
                }
            }
        }
        if in_line {
            end_line(&mut out, backslash)?;
        }
        Ok(())
    }
}

fn end_line(out: &mut impl Write, backslash: bool) -> io::Result<()> {
    // Preserve a final backslash if the output is read again as C source.
    out.write_all(if backslash { b" \n" } else { b"\n" })
}

/// Preprocess a translation unit and its includes using only the supplied search paths.
pub fn preprocess(path: &Path, options: &Options) -> Result<Preprocessed, Diagnostic> {
    let start = Position::new(path, Location { line: 1, column: 1 });
    let bytes = fs::read(path).map_err(|error| Diagnostic::new(&start, error.to_string()))?;
    preprocess_source(path, &bytes, options)
}

/// Preprocess source bytes named by `path`, which also anchors quoted includes.
pub fn preprocess_source(
    path: &Path,
    bytes: &[u8],
    options: &Options,
) -> Result<Preprocessed, Diagnostic> {
    let mut processor = Processor {
        options,
        macros: Expander::new(options.target),
        output: Preprocessed::default(),
    };
    processor.file(path, bytes, 0)?;
    Ok(processor.output)
}

struct LineMap {
    file: Rc<str>,
    physical_base: usize,
    presumed_base: usize,
}

impl LineMap {
    fn position(&self, physical: Location) -> Position {
        Position {
            file: Rc::clone(&self.file),
            loc: Location {
                line: self.presumed_base + physical.line - self.physical_base,
                column: physical.column,
            },
        }
    }

    fn token(&self, token: Token<'_>) -> Lexeme {
        Lexeme::new(
            token.spelling,
            token.kind,
            token.space_before,
            self.position(token.begin),
        )
    }

    fn change(
        &mut self,
        args: &[Lexeme],
        directive: &Lexeme,
        next_physical: usize,
    ) -> Result<(), Diagnostic> {
        let Some(number) = args.first() else {
            return Err(directive.error("expected a line number"));
        };
        let base = number
            .text
            .parse::<u32>()
            .ok()
            .filter(|n| *n != 0 && *n <= i32::MAX as u32)
            .filter(|_| number.text.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| number.error("invalid line number"))?;
        if let Some(name) = args.get(1) {
            let contents = name
                .text
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or_else(|| {
                    name.error("expected an ordinary string literal for the file name")
                })?;
            self.file = literal::decode(contents)
                .ok_or_else(|| name.error("invalid file name in line directive"))?
                .into();
        }
        require_empty(args.get(2..).unwrap_or_default())?;
        self.physical_base = next_physical;
        self.presumed_base = base as usize;
        Ok(())
    }
}

struct Conditional {
    opening: Position,
    parent_active: bool,
    taken: bool,
    active: bool,
    saw_else: bool,
}

struct Processor<'a> {
    options: &'a Options,
    macros: Expander,
    output: Preprocessed,
}

impl Processor<'_> {
    fn file(&mut self, path: &Path, bytes: &[u8], depth: usize) -> Result<(), Diagnostic> {
        let mut lines = LineMap {
            file: path.to_string_lossy().into(),
            physical_base: 1,
            presumed_base: 1,
        };
        let (source, lex_error) = scan_prefix(bytes);
        let mut conditionals = Vec::<Conditional>::new();
        let mut body = Vec::new();
        let tokens: Vec<_> = source.tokens().collect();

        for row in tokens.split_inclusive(|token| token.kind == Kind::Newline) {
            let active = conditionals.last().is_none_or(|group| group.active);
            let Some(first) = row.first() else { continue };
            let last = row.last().unwrap_or(first);
            if lex_error.is_some() && last.kind != Kind::Newline {
                break;
            }
            if !matches!(first.spelling, "#" | "%:") {
                if active {
                    body.extend(row.iter().map(|token| lines.token(*token)));
                }
                continue;
            }

            self.flush(&mut body)?;
            let (directive, next_physical) = if last.kind == Kind::Newline {
                (&row[1..row.len() - 1], last.end.line)
            } else {
                (&row[1..], last.end.line + 1)
            };
            let directive: Vec<_> = directive.iter().map(|token| lines.token(*token)).collect();
            let Some((name, args)) = directive.split_first() else {
                continue;
            };

            match name.text.as_ref() {
                "if" | "ifdef" | "ifndef" => {
                    let selected = active && self.condition(name, args)?;
                    conditionals.push(Conditional {
                        opening: name.site.clone(),
                        parent_active: active,
                        taken: selected,
                        active: selected,
                        saw_else: false,
                    });
                }
                "elif" | "else" => {
                    let group = conditionals
                        .last_mut()
                        .ok_or_else(|| name.error("conditional branch without #if"))?;
                    if group.saw_else {
                        return Err(name.error("conditional branch after #else"));
                    }
                    group.saw_else = name.is("else");
                    if group.saw_else && group.parent_active {
                        require_empty(args)?;
                    }
                    group.active = group.parent_active
                        && !group.taken
                        && (group.saw_else || self.condition(name, args)?);
                    group.taken |= group.active;
                }
                "endif" => {
                    let group = conditionals
                        .pop()
                        .ok_or_else(|| name.error("#endif without #if"))?;
                    if group.parent_active {
                        require_empty(args)?;
                    }
                }
                _ if !active => {}
                "define" => self.macros.define(name, args)?,
                "undef" => self.macros.undefine(identifier(name, args)?)?,
                "include" => self.include(path, name, args, depth)?,
                "line" => {
                    let args = self.macros.expand(args.to_vec())?;
                    lines.change(&args, name, next_physical)?;
                }
                "error" => return Err(name.error(spell(args))),
                "pragma" => self.output.0.push(Output::Pragma(spell(args))),
                _ => return Err(name.error("unknown preprocessing directive")),
            }
        }
        if let Some(error) = lex_error {
            return Err(Diagnostic::new(
                &lines.position(error.location),
                error.message,
            ));
        }
        self.flush(&mut body)?;
        if let Some(group) = conditionals.last() {
            return Err(Diagnostic::new(
                &group.opening,
                "unterminated conditional directive",
            ));
        }
        Ok(())
    }

    fn condition(&self, name: &Lexeme, args: &[Lexeme]) -> Result<bool, Diagnostic> {
        match name.text.as_ref() {
            "ifdef" | "ifndef" => {
                let defined = self.macros.is_defined(&identifier(name, args)?.text);
                Ok(defined != name.is("ifndef"))
            }
            _ => expression::evaluate(&self.macros, self.options.target, name, args),
        }
    }

    fn include(
        &mut self,
        path: &Path,
        directive: &Lexeme,
        args: &[Lexeme],
        depth: usize,
    ) -> Result<(), Diagnostic> {
        let original = args
            .first()
            .ok_or_else(|| directive.error("expected a header name"))?;
        let expanded = self.macros.expand(args.to_vec())?;
        let (name, quoted) =
            header_name(&expanded).ok_or_else(|| original.error("expected a header name"))?;
        if depth >= 64 {
            return Err(original.error("include nesting limit exceeded"));
        }
        let local = path.parent().filter(|_| quoted);
        let dirs = local.into_iter().chain(
            self.options
                .include_dirs
                .iter()
                .chain(&self.options.system_dirs)
                .map(PathBuf::as_path),
        );
        for dir in dirs {
            let candidate = dir.join(&name);
            match fs::read(&candidate) {
                Ok(bytes) => {
                    return self
                        .file(&candidate, &bytes, depth + 1)
                        .map_err(|mut error| {
                            error.included_from.push(original.site.clone());
                            error
                        });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(original.error(format!("cannot read header {name}: {error}")))
                }
            }
        }
        Err(original.error(format!("header {name} not found")))
    }

    fn flush(&mut self, body: &mut Vec<Lexeme>) -> Result<(), Diagnostic> {
        let tokens = self.macros.expand(std::mem::take(body))?;
        let mut cursor = tokens.into_iter().peekable();
        while let Some(token) = cursor.next() {
            if !token.is("_Pragma") {
                self.output.0.push(Output::Token(token));
                continue;
            }
            // N1570 6.10.3.4p3 and 6.10.9: process the operator after replacement.
            let mut operand = cursor.by_ref().filter(|t| t.kind != Kind::Newline);
            let (open, string, close) = (operand.next(), operand.next(), operand.next());
            let (Some(open), Some(string), Some(close)) = (open, string, close) else {
                return Err(token.error("incomplete _Pragma operator"));
            };
            if !open.is("(") || string.kind != Kind::String || !close.is(")") {
                return Err(token.error("expected _Pragma(\"text\")"));
            }
            let payload = literal::destringize(&string.text)
                .ok_or_else(|| string.error("expected a pragma string literal"))?;
            let source = scan_pragma(payload).map_err(|error| string.error(error.message))?;
            let contents = source
                .tokens()
                .map(|t| t.spelling)
                .collect::<Vec<_>>()
                .join(" ");
            self.output.0.push(Output::Pragma(contents));
        }
        Ok(())
    }
}

fn header_name(tokens: &[Lexeme]) -> Option<(String, bool)> {
    if let [token] = tokens {
        if let Some(name) = token
            .text
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
        {
            return (!name.is_empty()).then(|| (name.to_owned(), true));
        }
        if token.kind == Kind::Header {
            return token
                .text
                .strip_prefix('<')?
                .strip_suffix('>')
                .map(|s| (s.to_owned(), false));
        }
    }
    let [opening, inside @ .., closing] = tokens else {
        return None;
    };
    if !opening.is("<")
        || !closing.is(">")
        || inside.is_empty()
        || inside.iter().any(|token| token.is(">"))
    {
        return None;
    }
    Some((spell(inside), false))
}

fn identifier<'a>(directive: &Lexeme, args: &'a [Lexeme]) -> Result<&'a Lexeme, Diagnostic> {
    let name = args
        .first()
        .ok_or_else(|| directive.error("expected an identifier"))?;
    if name.kind != Kind::Identifier {
        return Err(name.error("expected an identifier"));
    }
    require_empty(&args[1..])?;
    Ok(name)
}

fn require_empty(args: &[Lexeme]) -> Result<(), Diagnostic> {
    match args.first() {
        Some(first) => Err(first.error("unexpected tokens in directive")),
        None => Ok(()),
    }
}

pub(super) fn spell(tokens: &[Lexeme]) -> String {
    let mut out = String::new();
    for token in tokens {
        if token.space_before && !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&token.text);
    }
    out
}
