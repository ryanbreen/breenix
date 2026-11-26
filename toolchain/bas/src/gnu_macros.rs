//! GNU macro, conditional and local-label expansion for AArch64 assembly.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::rc::Rc;

use super::{gnu::Constants, Parser};
use crate::object::Object;
use crate::{check_source_line, Location, Result};

const MAX_MACRO_DEPTH: usize = 64;
const MAX_EXPANDED_LINES: usize = 1_048_576;
const MAX_EXPANDED_LINE_BYTES: usize = 1_048_576;

struct Macro<'src> {
    parameters: Vec<String>,
    body: Vec<Line<'src>>,
}

#[derive(Clone, Copy)]
struct Line<'src> {
    bytes: &'src [u8],
    number: usize,
}

struct Repeat {
    start: usize,
    remaining: u64,
    at: Location,
}

struct Expander<'src> {
    constants: Constants,
    macros: BTreeMap<String, Rc<Macro<'src>>>,
    label_prefix: String,
    next_scope: usize,
    expanded_lines: usize,
}

pub(crate) fn assemble(source: &[u8], object: &mut Object) -> Result<()> {
    let lines: Vec<_> = source
        .split(|&byte| byte == b'\n')
        .enumerate()
        .map(|(index, bytes)| Line {
            bytes,
            number: index + 1,
        })
        .collect();
    // Pick a prefix absent from the source so ordinary named labels cannot
    // accidentally share a generated label's ELF name.
    let dollars = source
        .split(|&byte| byte == b'.')
        .filter_map(|suffix| suffix.strip_prefix(b"Lbas"))
        .map(|suffix| suffix.iter().take_while(|&&byte| byte == b'$').count())
        .max()
        .unwrap_or(0);
    Expander {
        constants: Constants::new(),
        macros: BTreeMap::new(),
        label_prefix: format!(".Lbas{}", "$".repeat(dollars + 1)),
        next_scope: 0,
        expanded_lines: 0,
    }
    .lines(&lines, None, object)
}

struct Invocation<'a, 'src> {
    definition: &'a Macro<'src>,
    arguments: Vec<&'a [u8]>,
    depth: usize,
}

impl<'src> Expander<'src> {
    fn lines(
        &mut self,
        lines: &[Line<'src>],
        invocation: Option<Invocation<'_, 'src>>,
        object: &mut Object,
    ) -> Result<()> {
        let depth = invocation.as_ref().map_or(0, |call| call.depth);
        let mut labels = NumericLabels {
            prefix: self.label_prefix.clone(),
            scope: self.next_scope,
            numbers: BTreeMap::new(),
        };
        self.next_scope += 1;

        let mut conditionals = Vec::<(Location, bool)>::new();
        let mut repeats = Vec::<Repeat>::new();
        let mut skipped_repeat_depth = 0;
        let mut cursor = 0;
        while let Some(&Line { bytes, number }) = lines.get(cursor) {
            cursor += 1;
            check_source_line(bytes, number)?;
            let line = match &invocation {
                Some(call) => call.substitute(bytes, number)?,
                None => Cow::Borrowed(bytes),
            };
            let mut parser = Parser::new(&line, number, object);
            let op = parser.word();
            let at = parser.location(op.column);
            if depth != 0 || !repeats.is_empty() {
                self.expanded_lines += 1;
                if self.expanded_lines > MAX_EXPANDED_LINES {
                    return Err(at.error("GNU assembly expansion exceeds the supported line limit"));
                }
            }
            if skipped_repeat_depth != 0 {
                match op.text {
                    ".rept" => skipped_repeat_depth += 1,
                    ".endr" => skipped_repeat_depth -= 1,
                    _ => {}
                }
                if skipped_repeat_depth != 0 {
                    continue;
                }
            }

            let active = conditionals.last().is_none_or(|&(_, active)| active);
            match op.text {
                ".ifndef" => {
                    let name = parser.word();
                    if !name.is_name() {
                        return Err(parser.error(name.column, "expected a symbol after .ifndef"));
                    }
                    parser.require_end()?;
                    conditionals.push((at, active && !self.constants.contains_key(name.text)));
                }
                ".endif" => {
                    parser.require_end()?;
                    conditionals
                        .pop()
                        .ok_or_else(|| at.error(".endif without .ifndef"))?;
                }
                ".macro" => {
                    let body = macro_body(lines, &mut cursor, parser.object, at)?;
                    if active {
                        let name = parser.word();
                        if !name.is_name() {
                            return Err(parser.error(name.column, "expected a macro name"));
                        }
                        let parameters = macro_parameters(&mut parser)?;
                        if self.macros.contains_key(name.text) {
                            return Err(at.error("duplicate macro definition"));
                        }
                        self.macros.insert(
                            name.text.to_owned(),
                            Rc::new(Macro {
                                parameters,
                                body: body.to_vec(),
                            }),
                        );
                    }
                }
                _ if !active => {}
                ".endm" => return Err(at.error(".endm without .macro")),
                ".rept" => {
                    parser.constants = Some(&mut self.constants);
                    let remaining = parser.count()?;
                    parser.require_end()?;
                    repeats.push(Repeat {
                        start: cursor,
                        remaining,
                        at,
                    });
                    skipped_repeat_depth = usize::from(remaining == 0);
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
                    let operands = parser.column;
                    let rewritten = labels.rewrite(&line, number)?;
                    if let Some(definition) = self.macros.get(op.text).cloned() {
                        if depth == MAX_MACRO_DEPTH {
                            return Err(
                                at.error("macro expansion depth exceeds the supported limit")
                            );
                        }
                        // Resolve numeric arguments in the caller's scope before
                        // substituting them into the callee's private label scope.
                        let arguments = macro_arguments(&rewritten, operands, at)?;
                        if arguments.len() != definition.parameters.len() {
                            return Err(at.error("wrong number of macro arguments"));
                        }
                        self.lines(
                            &definition.body,
                            Some(Invocation {
                                definition: &definition,
                                arguments,
                                depth: depth + 1,
                            }),
                            parser.object,
                        )?;
                    } else {
                        let mut parser = Parser::new(&rewritten, number, parser.object);
                        parser.constants = Some(&mut self.constants);
                        parser.assemble_line()?;
                    }
                }
            }
        }
        if let Some(repeat) = repeats.last() {
            return Err(repeat.at.error(".rept without .endr"));
        }
        if let Some(&(at, _)) = conditionals.last() {
            return Err(at.error(".ifndef without .endif"));
        }
        labels.finish()
    }
}

fn macro_body<'a, 'src>(
    lines: &'a [Line<'src>],
    cursor: &mut usize,
    object: &mut Object,
    at: Location,
) -> Result<&'a [Line<'src>]> {
    let start = *cursor;
    while let Some(&Line { bytes, number }) = lines.get(*cursor) {
        let end = *cursor;
        *cursor += 1;
        check_source_line(bytes, number)?;
        let mut parser = Parser::new(bytes, number, object);
        let op = parser.word();
        match op.text {
            ".endm" => {
                parser.require_end()?;
                return Ok(&lines[start..end]);
            }
            ".macro" => return Err(parser.error(op.column, "nested .macro definition")),
            _ => {}
        }
    }
    Err(at.error(".macro without .endm"))
}

fn macro_parameters(parser: &mut Parser<'_, '_>) -> Result<Vec<String>> {
    let mut parameters = Vec::new();
    if parser.at_end() {
        return Ok(parameters);
    }
    loop {
        let parameter = parser.word();
        if !parameter.is_name() {
            return Err(parser.error(parameter.column, "expected a macro parameter name"));
        }
        if parameters.iter().any(|name| name == parameter.text) {
            return Err(parser.error(parameter.column, "duplicate macro parameter"));
        }
        parameters.push(parameter.text.to_owned());
        if !parser.take(b',') {
            parser.require_end()?;
            return Ok(parameters);
        }
    }
}

fn macro_arguments(line: &[u8], operands: usize, at: Location) -> Result<Vec<&[u8]>> {
    let (_, end) = words(line).last().unwrap_or((0, operands));
    let line = line[operands..end].trim_ascii();
    if line.is_empty() {
        return Ok(Vec::new());
    }
    line.split(|&byte| byte == b',')
        .map(|argument| {
            let argument = argument.trim_ascii();
            if argument.is_empty() {
                Err(at.error("expected a macro argument"))
            } else {
                Ok(argument)
            }
        })
        .collect()
}

impl Invocation<'_, '_> {
    fn substitute<'a>(&self, line: &'a [u8], number: usize) -> Result<Cow<'a, [u8]>> {
        let mut result = Vec::new();
        let mut cursor = 0;
        for (start, end) in words(line) {
            let Some(parameter) = line[start..end].strip_prefix(b"\\") else {
                continue;
            };
            let at = Location {
                line: number,
                column: start + 1,
            };
            let index = self
                .definition
                .parameters
                .iter()
                .position(|name| name.as_bytes() == parameter)
                .ok_or_else(|| at.error("unknown macro parameter"))?;
            let argument = self.arguments[index];
            if result.len() + (start - cursor) + argument.len() > MAX_EXPANDED_LINE_BYTES {
                return Err(at.error("expanded macro line is too long"));
            }
            result.extend_from_slice(&line[cursor..start]);
            result.extend_from_slice(argument);
            cursor = end;
        }
        Ok(rewritten_line(line, result, cursor))
    }
}

#[derive(Default)]
struct NumericLabel {
    definitions: usize,
    pending_forward: Option<Location>,
}

struct NumericLabels {
    prefix: String,
    scope: usize,
    numbers: BTreeMap<String, NumericLabel>,
}

impl NumericLabels {
    fn rewrite<'a>(&mut self, line: &'a [u8], number: usize) -> Result<Cow<'a, [u8]>> {
        let mut result = Vec::new();
        let mut cursor = 0;
        let mut label_end = Some(0);
        for (start, end) in words(line) {
            if label_end.is_some_and(|previous| start < previous) {
                continue;
            }
            let word = &line[start..end];
            let tail = line[end..].trim_ascii_start();
            let definition = label_end.is_some_and(|previous| {
                line[previous..start].trim_ascii().is_empty() && tail.starts_with(b":")
            });
            label_end = definition.then(|| line.len() - tail.len() + 1);
            let (digits, direction) = if definition {
                (word, b':')
            } else if let Some((direction @ (b'f' | b'b'), digits)) = word.split_last() {
                (digits, *direction)
            } else {
                continue;
            };
            if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                continue;
            }
            let name = std::str::from_utf8(digits)
                .expect("numeric label contains only ASCII digits")
                .trim_start_matches('0');
            let at = Location {
                line: number,
                column: start + 1,
            };
            let label = self.numbers.entry(name.to_owned()).or_default();
            let instance = match direction {
                b':' => {
                    label.definitions += 1;
                    label.pending_forward = None;
                    label.definitions
                }
                b'f' => {
                    label.pending_forward.get_or_insert(at);
                    label.definitions + 1
                }
                _ if label.definitions != 0 => label.definitions,
                _ => return Err(at.error("no preceding numeric label")),
            };
            result.extend_from_slice(&line[cursor..start]);
            result.extend_from_slice(
                format!("{}{}_${name}_${instance}", self.prefix, self.scope).as_bytes(),
            );
            cursor = end;
        }
        Ok(rewritten_line(line, result, cursor))
    }

    fn finish(self) -> Result<()> {
        if let Some(at) = self
            .numbers
            .values()
            .filter_map(|label| label.pending_forward)
            .min_by_key(|at| (at.line, at.column))
        {
            return Err(at.error("no following numeric label"));
        }
        Ok(())
    }
}

fn rewritten_line(line: &[u8], mut bytes: Vec<u8>, cursor: usize) -> Cow<'_, [u8]> {
    if cursor == 0 {
        return Cow::Borrowed(line);
    }
    bytes.extend_from_slice(&line[cursor..]);
    Cow::Owned(bytes)
}

// Token ranges outside comments and strings. Retain the backslash at the
// start of parameter tokens, and the punctuation ranges for macro operands.
fn words(line: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let line = line.trim_ascii_end();
    let mut cursor = if line.trim_ascii_start().starts_with(b"#") {
        line.len()
    } else {
        0
    };
    std::iter::from_fn(move || {
        let rest = line[cursor..].trim_ascii_start();
        let &first = rest.first()?;
        if first == b';' || rest.starts_with(b"//") {
            return None;
        }
        let start = line.len() - rest.len();
        cursor = start + 1;
        if first == b'"' {
            while let Some(&byte) = line.get(cursor) {
                cursor += 1;
                match byte {
                    b'"' => break,
                    b'\\' if cursor < line.len() => cursor += 1,
                    _ => {}
                }
            }
        } else if first == b'\\' || word_byte(first) {
            while line.get(cursor).is_some_and(|&byte| word_byte(byte)) {
                cursor += 1;
            }
        }
        Some((start, cursor))
    })
}

fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$')
}
