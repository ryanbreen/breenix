//! N1570 6.10.3: replacement, argument prescan, pasting and rescanning.
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::Path;
use std::rc::Rc;

use crate::types::{Float, FloatFormat, Kind as TypeKind};

use super::directive::{Diagnostic, Target};
use super::literal;
use super::token::{token_prefix, LexContext};
use super::{Kind, Location};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub file: Rc<str>,
    pub loc: Location,
}

impl Position {
    pub(crate) fn new(path: &Path, loc: Location) -> Self {
        Self {
            file: path.to_string_lossy().into(),
            loc,
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}:{}:{}", self.file, self.loc.line, self.loc.column)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Lexeme {
    pub text: Rc<str>,
    pub kind: Kind,
    pub space_before: bool,
    pub site: Position,
    origin: Position,
    hidden: Vec<Rc<str>>,
}

impl Lexeme {
    pub(super) fn new(
        text: impl Into<Rc<str>>,
        kind: Kind,
        space_before: bool,
        site: Position,
    ) -> Self {
        Self {
            text: text.into(),
            kind,
            space_before,
            origin: site.clone(),
            site,
            hidden: Vec::new(),
        }
    }

    pub(crate) fn is(&self, text: &str) -> bool {
        self.text.as_ref() == text
    }

    pub(crate) fn error(&self, message: impl Into<String>) -> Diagnostic {
        let mut diagnostic = Diagnostic::new(&self.site, message);
        if self.site != self.origin {
            diagnostic.spelled_at = Some(self.origin.clone());
        }
        diagnostic
    }

    pub(super) fn respell(&self, text: impl Into<Rc<str>>, kind: Kind) -> Self {
        let mut token = self.clone();
        token.text = text.into();
        token.kind = kind;
        token
    }

    fn hash(&self) -> bool {
        matches!(self.text.as_ref(), "#" | "%:")
    }

    fn paste(&self) -> bool {
        matches!(self.text.as_ref(), "##" | "%:%:")
    }
}

pub(super) struct Expander {
    target: Target,
    definitions: HashMap<Rc<str>, Macro>,
}

#[derive(PartialEq, Eq)]
struct Parameters {
    fixed: Vec<Rc<str>>,
    variadic: bool,
}

impl Parameters {
    fn parse(mut tokens: &[Lexeme], name: &Lexeme) -> Result<(Self, usize), Diagnostic> {
        let total = tokens.len();
        let mut fixed = Vec::new();
        let mut variadic = false;
        if tokens.first().is_some_and(|token| !token.is(")")) {
            loop {
                let (first, rest) = tokens
                    .split_first()
                    .ok_or_else(|| name.error("unterminated macro parameter list"))?;
                tokens = rest;
                if first.is("...") {
                    variadic = true;
                    break;
                }
                if first.kind != Kind::Identifier || first.is("__VA_ARGS__") {
                    return Err(first.error("expected a macro parameter"));
                }
                if fixed.contains(&first.text) {
                    return Err(first.error("duplicate macro parameter"));
                }
                fixed.push(Rc::clone(&first.text));
                if let Some(rest) = tokens
                    .split_first()
                    .and_then(|(token, rest)| token.is(",").then_some(rest))
                {
                    tokens = rest;
                } else {
                    break;
                }
            }
        }
        let (close, rest) = tokens
            .split_first()
            .ok_or_else(|| name.error("unterminated macro parameter list"))?;
        if !close.is(")") {
            return Err(close.error("expected ')' after macro parameters"));
        }
        Ok((Self { fixed, variadic }, total - rest.len()))
    }

    fn index(&self, token: &Lexeme) -> Option<usize> {
        if self.variadic && token.is("__VA_ARGS__") {
            Some(self.fixed.len())
        } else {
            self.fixed.iter().position(|name| *name == token.text)
        }
    }

    fn arguments(
        &self,
        input: &mut VecDeque<Lexeme>,
        name: &Lexeme,
    ) -> Result<Invocation, Diagnostic> {
        let mut parens = 0usize;
        let mut args = vec![Vec::new()];
        let mut spaced = false;
        let closing = loop {
            let mut token = input
                .pop_front()
                .ok_or_else(|| name.error("unterminated macro invocation"))?;
            match token.text.as_ref() {
                "\n" => {
                    spaced = true;
                    continue;
                }
                "(" => parens += 1,
                ")" if parens == 0 => break token,
                ")" => parens -= 1,
                "," if parens == 0 && (!self.variadic || args.len() <= self.fixed.len()) => {
                    args.push(Vec::new());
                    spaced = false;
                    continue;
                }
                _ => {}
            }
            token.space_before |= spaced;
            spaced = false;
            if let Some(arg) = args.last_mut() {
                arg.push(token);
            }
        };
        if self.fixed.is_empty() && !self.variadic && args.len() == 1 && args[0].is_empty() {
            args.clear();
        }
        if args.len() != self.fixed.len() + usize::from(self.variadic) {
            return Err(name.error("wrong number of macro arguments"));
        }
        Ok(Invocation { args, closing })
    }
}

struct Macro {
    parameters: Option<Parameters>,
    replacement: Vec<Lexeme>,
}

impl Macro {
    fn identical(&self, other: &Self) -> bool {
        self.parameters == other.parameters
            && self.replacement.len() == other.replacement.len()
            && self
                .replacement
                .iter()
                .zip(&other.replacement)
                .enumerate()
                .all(|(i, (a, b))| a.text == b.text && (i == 0 || a.space_before == b.space_before))
    }

    fn parameter(&self, token: &Lexeme) -> Option<usize> {
        self.parameters.as_ref()?.index(token)
    }
}

struct Invocation {
    args: Vec<Vec<Lexeme>>,
    closing: Lexeme,
}

impl Expander {
    pub(super) fn new(target: Target) -> Self {
        Self {
            target,
            definitions: HashMap::new(),
        }
    }

    pub(super) fn is_defined(&self, name: &str) -> bool {
        is_builtin(name, self.target) || self.definitions.contains_key(name)
    }

    pub(super) fn define(&mut self, directive: &Lexeme, args: &[Lexeme]) -> Result<(), Diagnostic> {
        let Some((name, mut replacement)) = args.split_first() else {
            return Err(directive.error("expected a macro name"));
        };
        if name.kind != Kind::Identifier {
            return Err(name.error("expected a macro name"));
        }
        check_macro_name(name, self.target)?;
        let parameters = match replacement.first() {
            Some(first) if first.is("(") && !first.space_before => {
                let (parameters, length) = Parameters::parse(&replacement[1..], name)?;
                replacement = &replacement[1 + length..];
                Some(parameters)
            }
            Some(first) if !first.space_before => {
                return Err(first.error("expected whitespace after object-like macro name"));
            }
            _ => None,
        };
        let definition = Macro {
            parameters,
            replacement: replacement.to_vec(),
        };
        for (i, token) in replacement.iter().enumerate() {
            if token.paste() && (i == 0 || i + 1 == replacement.len()) {
                return Err(token.error("## cannot begin or end a replacement list"));
            }
            if token.hash()
                && definition.parameters.is_some()
                && replacement
                    .get(i + 1)
                    .and_then(|next| definition.parameter(next))
                    .is_none()
            {
                return Err(token.error("# must be followed by a macro parameter"));
            }
            if token.is("__VA_ARGS__") && definition.parameter(token).is_none() {
                return Err(token.error("__VA_ARGS__ requires a variadic macro"));
            }
        }
        if let Some(old) = self.definitions.get(&name.text) {
            if !old.identical(&definition) {
                return Err(name.error(format!("different redefinition of {}", name.text)));
            }
        } else {
            self.definitions.insert(Rc::clone(&name.text), definition);
        }
        Ok(())
    }

    pub(super) fn undefine(&mut self, name: &Lexeme) -> Result<(), Diagnostic> {
        check_macro_name(name, self.target)?;
        self.definitions.remove(&name.text);
        Ok(())
    }

    pub(super) fn expand(&self, tokens: Vec<Lexeme>) -> Result<Vec<Lexeme>, Diagnostic> {
        self.rescan(tokens, 0)
    }

    fn rescan(&self, tokens: Vec<Lexeme>, depth: usize) -> Result<Vec<Lexeme>, Diagnostic> {
        let mut input = VecDeque::from(tokens);
        let mut result = Vec::new();
        while let Some(token) = input.pop_front() {
            if token.kind != Kind::Identifier || token.hidden.contains(&token.text) {
                result.push(token);
                continue;
            }
            if expand_builtin(&token, self.target, &mut result) {
                continue;
            }
            let Some(mac) = self.definitions.get(&token.text) else {
                result.push(token);
                continue;
            };
            let mut hidden = token.hidden.clone();
            let args = if let Some(parameters) = &mac.parameters {
                let next = input.iter().position(|token| token.kind != Kind::Newline);
                let Some(open) = next.filter(|i| input[*i].is("(")) else {
                    result.push(token);
                    continue;
                };
                input.drain(..=open);
                let invocation = parameters.arguments(&mut input, &token)?;
                // The closing token marks how far the replacement's rescan reached.
                // A name suppressed throughout both ends stays suppressed inside.
                hidden.retain(|name| invocation.closing.hidden.contains(name));
                invocation.args
            } else {
                Vec::new()
            };
            hidden.push(Rc::clone(&token.text));

            let replacement = self.substitute(mac, &token, &args, depth)?;
            if replacement.is_empty() && token.space_before {
                if let Some(next) = input.front_mut() {
                    next.space_before = true;
                }
            }
            for (index, mut item) in replacement.into_iter().enumerate().rev() {
                if index == 0 {
                    item.space_before = token.space_before;
                }
                for name in &hidden {
                    if !item.hidden.contains(name) {
                        item.hidden.push(Rc::clone(name));
                    }
                }
                input.push_front(item);
            }
        }
        Ok(result)
    }

    fn substitute(
        &self,
        mac: &Macro,
        invocation: &Lexeme,
        args: &[Vec<Lexeme>],
        depth: usize,
    ) -> Result<Vec<Lexeme>, Diagnostic> {
        let mut result = Vec::new();
        let mut prescanned = vec![None; args.len()];
        let mut replacement = mac.replacement.iter().enumerate().peekable();
        let mut paste = false;
        while let Some((index, original)) = replacement.next() {
            if original.paste() {
                if paste {
                    return Err(invocation.error("missing operand for ##"));
                }
                paste = true;
                continue;
            }
            let mut at_call = original.clone();
            at_call.site = invocation.site.clone();
            let mut part = if original.hash() && mac.parameters.is_some() {
                let argument = replacement
                    .next()
                    .and_then(|(_, token)| mac.parameter(token))
                    .ok_or_else(|| invocation.error("missing argument for #"))?;
                vec![stringize(&args[argument], &at_call)?]
            } else if let Some(argument) = mac.parameter(original) {
                let adjacent = paste || replacement.peek().is_some_and(|(_, token)| token.paste());
                if adjacent {
                    args[argument].clone()
                } else {
                    if prescanned[argument].is_none() {
                        if depth >= 128 {
                            return Err(
                                invocation.error("macro argument expansion nesting limit exceeded")
                            );
                        }
                        prescanned[argument] =
                            Some(self.rescan(args[argument].clone(), depth + 1)?);
                    }
                    prescanned[argument].as_ref().cloned().unwrap_or_default()
                }
            } else {
                vec![at_call]
            };
            if let Some(first) = part.first_mut() {
                first.space_before = original.space_before;
            }
            let mut part = part.into_iter();
            let first = part.next();
            if paste {
                let left = result.pop().flatten();
                result.push(paste_tokens(left, first, invocation)?);
                paste = false;
            } else if first.is_some() || mac.replacement.get(index + 1).is_some_and(Lexeme::paste) {
                result.push(first);
            }
            result.extend(part.map(Some));
        }
        Ok(result.into_iter().flatten().collect())
    }
}

// N1570 6.10.3.2: preserve the spelling of raw tokens, escaping quotes and
// backslashes inside character and string literals. Trim edge whitespace.
fn stringize(tokens: &[Lexeme], at: &Lexeme) -> Result<Lexeme, Diagnostic> {
    let mut text = String::from("\"");
    for (index, token) in tokens.iter().enumerate() {
        if index != 0 && token.space_before {
            text.push(' ');
        }
        if matches!(token.kind, Kind::String | Kind::Character) {
            literal::escape_into(&mut text, &token.text);
        } else {
            text.push_str(&token.text);
        }
    }
    text.push('"');
    // The listing example in 6.10.9 stringizes opaque text and then
    // stringizes it again. Validate the quoting here, before escape
    // interpretation is needed by any consumer of the resulting literal.
    validate_token(&text, at, LexContext::Pragma)?;
    Ok(at.respell(text, Kind::String))
}

fn paste_tokens(
    left: Option<Lexeme>,
    right: Option<Lexeme>,
    at: &Lexeme,
) -> Result<Option<Lexeme>, Diagnostic> {
    let (mut left, right) = match (left, right) {
        (Some(left), Some(right)) => (left, right),
        (left, right) => return Ok(left.or(right)),
    };
    let text = format!("{}{}", left.text, right.text);
    let kind = validate_token(&text, at, LexContext::Ordinary)?;
    left.hidden.retain(|name| right.hidden.contains(name));
    left.site = at.site.clone();
    Ok(Some(left.respell(text, kind)))
}

fn validate_token(text: &str, at: &Lexeme, context: LexContext) -> Result<Kind, Diagnostic> {
    match token_prefix(text, context) {
        Ok((kind, width)) if width == text.len() && kind != Kind::Other => Ok(kind),
        _ => Err(at.error(format!(
            "macro replacement does not form one preprocessing token: {text}"
        ))),
    }
}

fn builtin(name: &str, target: Target) -> Option<(&str, Kind)> {
    let entry = match name {
        "__FILE__" => ("", Kind::String),
        "__LINE__" => ("", Kind::Number),
        "__STDC__" => ("1", Kind::Number),
        "__STDC_VERSION__" => ("201112L", Kind::Number),
        "__STDC_HOSTED__" | "__STRICT_ANSI__" => ("1", Kind::Number),
        "__x86_64__" if target == Target::X86_64 => ("1", Kind::Number),
        "__aarch64__" if target == Target::Aarch64 => ("1", Kind::Number),
        // The shipped limits.h selects CHAR_MIN and CHAR_MAX with this macro.
        "__CHAR_UNSIGNED__" if !target.signed_char() => ("1", Kind::Number),
        // Breenix LP64 support headers select these target ABI types.
        "__WCHAR_TYPE__" if target == Target::X86_64 => ("int", Kind::Identifier),
        "__WCHAR_TYPE__" => ("unsigned", Kind::Identifier),
        "__WCHAR_MAX__" if target == Target::X86_64 => ("2147483647", Kind::Number),
        "__WCHAR_MAX__" | "__WINT_MAX__" => ("4294967295U", Kind::Number),
        "__WCHAR_UNSIGNED__" if target == Target::Aarch64 => ("1", Kind::Number),
        "__WINT_UNSIGNED__" => ("1", Kind::Number),
        "__ATOMIC_RELAXED" => ("0", Kind::Number),
        "__ATOMIC_CONSUME" => ("1", Kind::Number),
        "__ATOMIC_ACQUIRE" => ("2", Kind::Number),
        "__ATOMIC_RELEASE" => ("3", Kind::Number),
        "__ATOMIC_ACQ_REL" => ("4", Kind::Number),
        "__ATOMIC_SEQ_CST" => ("5", Kind::Number),
        "__BREENIX_BCC__" => ("1", Kind::Number),
        // N1570 6.10.8.3. Native ARM atomics are scheduled for ARM 25.9d.
        "__STDC_NO_COMPLEX__" | "__STDC_NO_THREADS__" | "__STDC_NO_VLA__" => ("1", Kind::Number),
        "__STDC_NO_ATOMICS__" if target == Target::Aarch64 => ("1", Kind::Number),
        // Use N1570 6.10.8.1's fallback when translation time is unavailable.
        "__DATE__" => ("\"Jan  1 1970\"", Kind::String),
        "__TIME__" => ("\"00:00:00\"", Kind::String),
        _ => return None,
    };
    Some(entry)
}

// N1570 5.2.4.2.2. The floating limits are exact hexadecimal constants
// with the type suffix required by the header.
fn float_builtin(name: &str, target: Target) -> Option<String> {
    let (prefix, property) = name
        .strip_prefix("__")?
        .strip_suffix("__")?
        .split_once('_')?;
    let (ty, suffix) = match prefix {
        "FLT" => (TypeKind::Float(Float::Single), "F"),
        "DBL" => (TypeKind::Float(Float::Double), ""),
        "LDBL" => (TypeKind::LongDouble, "L"),
        _ => return None,
    };
    let format = FloatFormat::of(&ty, target);
    let fraction = format.fraction as i32;
    let min = format.min_exponent();
    let max = format.bias;
    let value = match property {
        "RADIX" if prefix == "FLT" => 2,
        "MANT_DIG" => fraction + 1,
        "DIG" => format.decimal_digits(),
        "DECIMAL_DIG" => format.roundtrip_digits(),
        "MIN_EXP" => min + 1,
        "MAX_EXP" => max + 1,
        "MIN_10_EXP" => format.min_decimal_exponent(),
        "MAX_10_EXP" => format.max_decimal_exponent(),
        "HAS_DENORM" => 1,
        "MIN" => return Some(format!("0x1p{min}{suffix}")),
        "MAX" => {
            let significand = (1_u128 << (fraction + 1)) - 1;
            return Some(format!("0x{significand:x}p{}{suffix}", max - fraction));
        }
        "EPSILON" => return Some(format!("0x1p-{fraction}{suffix}")),
        "DENORM_MIN" => {
            return Some(format!("0x1p{}{suffix}", format.min_subnormal_exponent()));
        }
        _ => return None,
    };
    Some(value.to_string())
}

fn is_builtin(name: &str, target: Target) -> bool {
    builtin(name, target).is_some() || float_builtin(name, target).is_some()
}

fn expand_builtin(token: &Lexeme, target: Target, result: &mut Vec<Lexeme>) -> bool {
    if let Some((text, kind)) = builtin(&token.text, target) {
        result.push(match token.text.as_ref() {
            "__FILE__" => token.respell(literal::quote(&token.site.file), kind),
            "__LINE__" => token.respell(token.site.loc.line.to_string(), kind),
            _ => token.respell(text, kind),
        });
    } else if let Some(text) = float_builtin(&token.text, target) {
        if let Some(magnitude) = text.strip_prefix('-') {
            result.push(token.respell("(", Kind::Punctuator));
            for (text, kind) in [
                ("-", Kind::Punctuator),
                (magnitude, Kind::Number),
                (")", Kind::Punctuator),
            ] {
                let mut part = token.respell(text, kind);
                part.space_before = false;
                result.push(part);
            }
        } else {
            result.push(token.respell(text, Kind::Number));
        }
    } else {
        return false;
    }
    true
}

fn check_macro_name(name: &Lexeme, target: Target) -> Result<(), Diagnostic> {
    if is_builtin(&name.text, target)
        || matches!(name.text.as_ref(), "defined" | "__VA_ARGS__" | "_Pragma")
    {
        return Err(name.error("reserved preprocessing identifier"));
    }
    Ok(())
}
