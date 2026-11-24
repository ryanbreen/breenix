//! NASM 3.02 manual §§3.2.5, 3.9, 5.3.1, 5.6 and 8.1.

use std::collections::BTreeMap;

use crate::object::Object;
use crate::parser::Parser;
use crate::{Location, Result};

const MAX_DEPTH: usize = 64;
const MAX_EXPANDED_BYTES: usize = 1024 * 1024;
const MAX_BODY_LINES_PER_CALL: usize = 65_536;

pub(crate) struct Syntax {
    pub(crate) bits: u8,
    pub(crate) scope: Option<String>,
}

struct Define<'a> {
    parameters: Option<Vec<&'a [u8]>>,
    body: &'a [u8],
}

struct Macro<'a> {
    arity: usize,
    body: Vec<&'a [u8]>,
}

struct Definition<'a> {
    name: &'a [u8],
    at: Location,
    expansion: Macro<'a>,
}

pub(crate) struct Preprocessor<'a> {
    defines: BTreeMap<&'a [u8], Define<'a>>,
    macros: BTreeMap<&'a [u8], Macro<'a>>,
    pending: Option<Definition<'a>>,
    syntax: Syntax,
}

impl<'a> Preprocessor<'a> {
    pub(crate) fn new() -> Self {
        Self {
            defines: BTreeMap::new(),
            macros: BTreeMap::new(),
            pending: None,
            syntax: Syntax {
                bits: 64,
                scope: None,
            },
        }
    }

    pub(crate) fn line(
        &mut self,
        line: &'a [u8],
        number: usize,
        object: &mut Object,
    ) -> Result<()> {
        let mut tokens = Tokens::new(line);
        let Some(first) = tokens.next() else {
            return Ok(());
        };
        let at = first.location(number);
        let rest = &line[first.end..];
        if let Some(mut definition) = self.pending.take() {
            match first.text {
                b"%endmacro" => {
                    if tokens.next().is_some() {
                        return Err(at.error("unexpected input after %endmacro"));
                    }
                    self.macros.insert(definition.name, definition.expansion);
                }
                b"%macro" => return Err(at.error("nested %macro definition")),
                _ => {
                    definition.expansion.body.push(line);
                    self.pending = Some(definition);
                }
            }
            return Ok(());
        }

        match first.text {
            b"%define" => {
                let (name, define) = define(rest, at)?;
                self.defines.insert(name, define);
            }
            b"%macro" => {
                let name = macro_name(tokens.next(), at)?;
                let arity = tokens.next().and_then(|token| decimal(token.text));
                let arity = arity.ok_or_else(|| at.error("expected a fixed %macro arity"))?;
                if tokens.next().is_some() {
                    return Err(at.error("unexpected input after %macro arity"));
                }
                self.pending = Some(Definition {
                    name,
                    at,
                    expansion: Macro {
                        arity,
                        body: Vec::new(),
                    },
                });
            }
            b"%endmacro" => return Err(at.error("%endmacro without %macro")),
            _ => {
                let mut remaining_lines = MAX_BODY_LINES_PER_CALL;
                let mut assembly = Expansion {
                    defines: &self.defines,
                    macros: &self.macros,
                    object,
                    syntax: &mut self.syntax,
                };
                assembly.line(line, number, None, 0, &mut remaining_lines)?;
            }
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<()> {
        if let Some(definition) = self.pending {
            return Err(definition.at.error("unterminated %macro"));
        }
        Ok(())
    }
}

struct Expansion<'a, 'src> {
    defines: &'a BTreeMap<&'src [u8], Define<'src>>,
    macros: &'a BTreeMap<&'src [u8], Macro<'src>>,
    object: &'a mut Object,
    syntax: &'a mut Syntax,
}

impl Expansion<'_, '_> {
    fn line(
        &mut self,
        line: &[u8],
        number: usize,
        origin: Option<Location>,
        depth: usize,
        remaining_lines: &mut usize,
    ) -> Result<()> {
        if line.trim_ascii_start().starts_with(b"#") {
            return Ok(());
        }
        let mut expanded = Vec::new();
        let expansion_site = self.inline(line, number, origin, 0, &mut expanded)?;
        if let Some(op) = statement(&expanded) {
            if let Some(mac) = self.macros.get(op.text) {
                let at = origin
                    .or(expansion_site)
                    .unwrap_or_else(|| op.location(number));
                if depth == MAX_DEPTH {
                    return Err(at.error("macro expansion is too deeply nested"));
                }
                let (arguments, _) = arguments(&expanded[op.end..], false, at)?;
                if arguments.len() != mac.arity {
                    return Err(at.error("wrong number of macro arguments"));
                }
                if let Some(remaining) = remaining_lines.checked_sub(mac.body.len()) {
                    *remaining_lines = remaining;
                } else {
                    return Err(at.error("too many expanded macro lines"));
                }
                Parser::new(&expanded[..op.start], number, self.object)
                    .with_nasm(self.syntax, origin.or(expansion_site))
                    .assemble_line()?;
                for body_line in &mac.body {
                    let substituted = substitute(body_line, at, |token| {
                        let Some(parameter) = token.strip_prefix(b"%") else {
                            return Ok(None);
                        };
                        if !parameter.first().is_some_and(u8::is_ascii_digit) {
                            return Ok(None);
                        }
                        decimal(parameter)
                            .and_then(|index| index.checked_sub(1))
                            .and_then(|index| arguments.get(index).copied())
                            .map(Some)
                            .ok_or_else(|| at.error("macro parameter is out of range"))
                    })?;
                    self.line(&substituted, number, Some(at), depth + 1, remaining_lines)?;
                }
                return Ok(());
            }
        }
        Parser::new(&expanded, number, self.object)
            .with_nasm(self.syntax, origin.or(expansion_site))
            .assemble_line()
    }

    fn inline(
        &self,
        line: &[u8],
        number: usize,
        origin: Option<Location>,
        depth: usize,
        expanded: &mut Vec<u8>,
    ) -> Result<Option<Location>> {
        let mut tokens = Tokens::new(line);
        let mut cursor = 0;
        let mut first_site = None;
        while let Some(token) = tokens.next() {
            let Some(definition) = self.defines.get(token.text) else {
                continue;
            };
            let at = origin.unwrap_or_else(|| token.location(number));
            let replacement;
            let body = if let Some(parameters) = &definition.parameters {
                let after_name = &line[token.end..];
                let Some(call) = after_name.trim_ascii_start().strip_prefix(b"(") else {
                    continue;
                };
                let (arguments, end) = arguments(call, true, at)?;
                if parameters.len() != arguments.len() {
                    return Err(at.error("wrong number of macro arguments"));
                }
                tokens.offset = line.len() - call.len() + end;
                replacement = substitute(definition.body, at, |word| {
                    Ok(parameters
                        .iter()
                        .position(|&name| word == name)
                        .map(|i| arguments[i]))
                })?;
                &replacement
            } else {
                definition.body
            };
            if depth == MAX_DEPTH {
                return Err(at.error("macro expansion is too deeply nested"));
            }
            first_site.get_or_insert(at);
            append(expanded, &line[cursor..token.start], at)?;
            self.inline(body, number, Some(at), depth + 1, expanded)?;
            cursor = tokens.offset;
        }
        append(
            expanded,
            &line[cursor..],
            first_site.or(origin).unwrap_or(Location {
                line: number,
                column: 1,
            }),
        )?;
        Ok(first_site)
    }
}

fn define(line: &[u8], at: Location) -> Result<(&[u8], Define<'_>)> {
    let mut tokens = Tokens::new(line);
    let name = macro_name(tokens.next(), at)?;
    let rest = &line[tokens.offset..];
    let (parameters, rest) = if let Some(list) = rest.strip_prefix(b"(") {
        let (parameters, end) = arguments(list, true, at)?;
        for (index, &name) in parameters.iter().enumerate() {
            if !is_name(name) || parameters[..index].contains(&name) {
                return Err(at.error("expected distinct macro parameter names"));
            }
        }
        (Some(parameters), &list[end..])
    } else {
        (None, rest)
    };
    let body = &rest[..Tokens::new(rest).last().map_or(0, |token| token.end)];
    Ok((
        name,
        Define {
            parameters,
            body: body.trim_ascii(),
        },
    ))
}

fn macro_name<'a>(token: Option<Token<'a>>, at: Location) -> Result<&'a [u8]> {
    token
        .filter(|token| is_name(token.text))
        .map(|token| token.text)
        .ok_or_else(|| at.error("expected a macro name"))
}

fn is_name(word: &[u8]) -> bool {
    word.first()
        .is_some_and(|&byte| byte.is_ascii_alphabetic() || byte == b'_')
        && word.iter().copied().all(word_byte)
}

fn decimal(text: &[u8]) -> Option<usize> {
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(text).ok()?.parse().ok()
}

fn arguments(line: &[u8], parenthesized: bool, at: Location) -> Result<(Vec<&[u8]>, usize)> {
    let mut args = Vec::new();
    let mut nesting = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for token in Tokens::new(line) {
        match token.text {
            b"(" => nesting.push(b')'),
            b"[" => nesting.push(b']'),
            b")" if parenthesized && nesting.is_empty() => {
                push_argument(&mut args, line[start..token.start].trim_ascii(), false, at)?;
                return Ok((args, token.end));
            }
            b")" | b"]" => {
                if nesting.pop() != token.text.first().copied() {
                    return Err(at.error("unbalanced macro argument"));
                }
            }
            b"," if nesting.is_empty() => {
                push_argument(&mut args, line[start..token.start].trim_ascii(), true, at)?;
                start = token.end;
            }
            _ => {}
        }
        end = token.end;
    }
    if parenthesized || !nesting.is_empty() {
        return Err(at.error("unterminated macro argument list"));
    }
    push_argument(&mut args, line[start..end].trim_ascii(), false, at)?;
    Ok((args, end))
}

fn push_argument<'a>(
    args: &mut Vec<&'a [u8]>,
    arg: &'a [u8],
    required: bool,
    at: Location,
) -> Result<()> {
    if !arg.is_empty() {
        args.push(arg);
    } else if required || !args.is_empty() {
        return Err(at.error("empty macro argument"));
    }
    Ok(())
}

fn substitute<'a>(
    line: &[u8],
    at: Location,
    mut replacement: impl FnMut(&[u8]) -> Result<Option<&'a [u8]>>,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    for token in Tokens::new(line) {
        if let Some(value) = replacement(token.text)? {
            append(&mut output, &line[cursor..token.start], at)?;
            append(&mut output, value, at)?;
            cursor = token.end;
        }
    }
    append(&mut output, &line[cursor..], at)?;
    Ok(output)
}

fn append(output: &mut Vec<u8>, text: &[u8], at: Location) -> Result<()> {
    if text.len() > MAX_EXPANDED_BYTES - output.len() {
        return Err(at.error("expanded macro line is too long"));
    }
    output.extend_from_slice(text);
    Ok(())
}

pub(crate) fn is_marker(line: &[u8]) -> bool {
    statement(line).is_some_and(|op| {
        op.text.starts_with(b"%")
            || op.text.eq_ignore_ascii_case(b"bits")
            || op.text.eq_ignore_ascii_case(b"times")
    })
}

fn statement(line: &[u8]) -> Option<Token<'_>> {
    let mut tokens = Tokens::new(line).peekable();
    while let Some(token) = tokens.next() {
        if tokens.next_if(|token| token.text == b":").is_none() {
            return Some(token);
        }
    }
    None
}

fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$')
}

struct Token<'a> {
    text: &'a [u8],
    start: usize,
    end: usize,
}

impl Token<'_> {
    fn location(&self, line: usize) -> Location {
        Location {
            line,
            column: self.start + 1,
        }
    }
}

struct Tokens<'a> {
    line: &'a [u8],
    offset: usize,
}

impl<'a> Tokens<'a> {
    fn new(line: &'a [u8]) -> Self {
        Self { line, offset: 0 }
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = &self.line[self.offset..];
        let trimmed = rest.trim_ascii_start();
        let (&first, tail) = trimmed.split_first()?;
        let width = match first {
            b';' => return None,
            b'\'' | b'"' | b'`' => {
                let mut end = 1;
                let mut escaped = false;
                for &byte in tail {
                    end += 1;
                    if !escaped && byte == first {
                        break;
                    }
                    escaped = !escaped && byte == b'\\';
                }
                end
            }
            b'%' if tail.first().is_some_and(u8::is_ascii_digit) => {
                1 + tail.iter().take_while(|byte| byte.is_ascii_digit()).count()
            }
            byte if word_byte(byte) || byte == b'%' => {
                1 + tail.iter().take_while(|&&byte| word_byte(byte)).count()
            }
            _ => 1,
        };
        let start = self.line.len() - trimmed.len();
        self.offset = start + width;
        Some(Token {
            text: &trimmed[..width],
            start,
            end: self.offset,
        })
    }
}
