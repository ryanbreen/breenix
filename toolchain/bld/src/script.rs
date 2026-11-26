//! GNU ld 2.45 §§3.4.1, 3.6.4, 3.6.8, 3.8 and 3.10: the subset used by
//! Breenix's userspace linker script.

use std::path::Path;

use crate::{Result, IMAGE_BASE, PAGE_SIZE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadRole {
    Text,
    Rodata,
    Data,
}

pub(crate) struct Selector {
    patterns: Vec<Vec<u8>>,
    pub(crate) keep: bool,
}

pub(crate) struct Rule {
    pub(crate) name: Vec<u8>,
    pub(crate) align: u64,
    pub(crate) load: LoadRole,
    pub(crate) tls: bool,
    pub(crate) selectors: Vec<Selector>,
}

pub(crate) struct Script {
    pub(crate) entry: Vec<u8>,
    pub(crate) base: u64,
    pub(crate) explicit: bool,
    pub(crate) tls: bool,
    pub(crate) eh_frame: bool,
    pub(crate) implicit_unwind_outputs: usize,
    pub(crate) rules: Vec<Rule>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            entry: b"_start".to_vec(),
            base: IMAGE_BASE,
            explicit: false,
            tls: true,
            eh_frame: true,
            implicit_unwind_outputs: 0,
            rules: Vec::new(),
        }
    }
}

impl Script {
    pub(crate) fn parse(path: &Path, bytes: &[u8]) -> Result<Self> {
        if bytes.contains(&0) {
            return Err(format!(
                "{}: linker script contains a NUL byte",
                path.display()
            ));
        }
        let mut parser = Parser {
            rest: bytes,
            token: b"",
            line: 1,
        };
        let script = parser
            .advance()
            .and_then(|()| parser.script())
            .map_err(|()| {
                format!(
                    "{}:{}: invalid or unsupported Breenix linker script construct",
                    path.display(),
                    parser.line
                )
            })?;
        if script.base % PAGE_SIZE != 0 || script.base > u64::MAX - PAGE_SIZE {
            return Err(format!(
                "{}: invalid page-aligned image base",
                path.display()
            ));
        }
        Ok(script)
    }

    pub(crate) fn unwind_output(&mut self, name: &[u8]) -> Result<(usize, usize)> {
        let mut matching = self
            .rules
            .iter_mut()
            .enumerate()
            .filter(|(_, rule)| rule.name == name);
        if let Some((index, rule)) = matching.next() {
            if matching.next().is_some() {
                return Err(format!(
                    "bld: multiple {} output rules",
                    crate::display(name)
                ));
            }
            if rule.load != LoadRole::Rodata {
                return Err(format!(
                    "bld: {} must be in the read-only load",
                    crate::display(name)
                ));
            }
            let selector = rule.selectors.len();
            rule.selectors.push(Selector {
                patterns: Vec::new(),
                keep: false,
            });
            return Ok((index, selector));
        }
        let index = self.rules.len();
        self.implicit_unwind_outputs += 1;
        self.rules.push(Rule {
            name: name.to_vec(),
            align: 0,
            load: LoadRole::Rodata,
            tls: false,
            selectors: vec![Selector {
                patterns: Vec::new(),
                keep: false,
            }],
        });
        Ok((index, 0))
    }

    pub(crate) fn match_section(&self, name: &[u8]) -> Option<(usize, usize)> {
        self.rules.iter().enumerate().find_map(|(rule, output)| {
            output
                .selectors
                .iter()
                .position(|selector| {
                    selector
                        .patterns
                        .iter()
                        .any(|pattern| matches_pattern(pattern, name))
                })
                .map(|selector| (rule, selector))
        })
    }
}

struct Parser<'a> {
    rest: &'a [u8],
    token: &'a [u8],
    line: usize,
}

type Parsed<T> = std::result::Result<T, ()>;

const PUNCTUATION: &[u8] = b"{}():;=+";

impl<'a> Parser<'a> {
    fn advance(&mut self) -> Parsed<()> {
        loop {
            while let Some((&byte, rest)) = self.rest.split_first() {
                if !byte.is_ascii_whitespace() {
                    break;
                }
                self.line += usize::from(byte == b'\n');
                self.rest = rest;
            }
            let Some(comment) = self.rest.strip_prefix(b"/*") else {
                break;
            };
            let end = comment.windows(2).position(|pair| pair == b"*/");
            let before_end = &comment[..end.unwrap_or(comment.len())];
            self.line += before_end.iter().filter(|&&byte| byte == b'\n').count();
            self.rest = &comment[end.ok_or(())? + 2..];
        }
        let length = match self.rest.first() {
            None => 0,
            Some(byte) if PUNCTUATION.contains(byte) => 1,
            Some(_) => (1..self.rest.len())
                .find(|&index| {
                    let current = &self.rest[index..];
                    current[0].is_ascii_whitespace()
                        || PUNCTUATION.contains(&current[0])
                        || current.starts_with(b"/*")
                })
                .unwrap_or(self.rest.len()),
        };
        (self.token, self.rest) = self.rest.split_at(length);
        Ok(())
    }

    fn take(&mut self, word: &[u8]) -> Parsed<()> {
        if self.token != word {
            return Err(());
        }
        self.advance()
    }

    fn word(&mut self) -> Parsed<&'a [u8]> {
        let word = self.token;
        if word.is_empty() || PUNCTUATION.contains(&word[0]) {
            return Err(());
        }
        self.advance()?;
        Ok(word)
    }

    fn number(&mut self) -> Parsed<u64> {
        let mut token = self.token;
        if !token.first().is_some_and(u8::is_ascii_digit) {
            return Err(());
        }
        let scale = match token.last() {
            Some(b'k' | b'K') => 1 << 10,
            Some(b'm' | b'M') => 1 << 20,
            _ => 1,
        };
        if scale != 1 {
            token = &token[..token.len() - 1];
        }
        let (digits, radix) = if let Some(hex) = token
            .strip_prefix(b"0x")
            .or_else(|| token.strip_prefix(b"0X"))
        {
            (hex, 16)
        } else {
            (token, if token.starts_with(b"0") { 8 } else { 10 })
        };
        let value = u64::from_str_radix(std::str::from_utf8(digits).map_err(|_| ())?, radix)
            .ok()
            .and_then(|value| value.checked_mul(scale))
            .ok_or(())?;
        self.advance()?;
        Ok(value)
    }

    fn phdrs(&mut self) -> Parsed<[Option<&'a [u8]>; 6]> {
        self.take(b"PHDRS")?;
        self.take(b"{")?;
        let mut names = [None; 6];
        while self.token != b"}" {
            let name = self.word()?;
            let kind = self.word()?;
            let headers = self.token == b"FILEHDR";
            if headers {
                self.take(b"FILEHDR")?;
                self.take(b"PHDRS")?;
            }
            let flags = if kind == b"PT_GNU_EH_FRAME" && self.token == b";" {
                4
            } else {
                self.take(b"FLAGS")?;
                self.take(b"(")?;
                let flags = self.number()?;
                self.take(b")")?;
                flags
            };
            self.take(b";")?;
            let role = match (kind, flags, headers) {
                (b"PT_LOAD", 5, true) => 0,
                (b"PT_LOAD", 4, false) => 1,
                (b"PT_LOAD", 6, false) => 2,
                (b"PT_TLS", 4, false) => 3,
                (b"PT_GNU_STACK", 6, false) => 4,
                (b"PT_GNU_EH_FRAME", 4, false) => 5,
                _ => return Err(()),
            };
            if names[role].is_some() || names.contains(&Some(name)) {
                return Err(());
            }
            names[role] = Some(name);
        }
        if [0, 1, 2, 4].into_iter().any(|role| names[role].is_none()) {
            return Err(());
        }
        self.take(b"}")?;
        Ok(names)
    }

    fn selector(&mut self) -> Parsed<Selector> {
        let keep = self.token == b"KEEP";
        if keep {
            self.take(b"KEEP")?;
            self.take(b"(")?;
        }
        self.take(b"*")?;
        self.take(b"(")?;
        let mut patterns = vec![self.word()?.to_vec()];
        while self.token != b")" {
            patterns.push(self.word()?.to_vec());
        }
        self.take(b")")?;
        if keep {
            self.take(b")")?;
        }
        Ok(Selector { patterns, keep })
    }

    fn rule(&mut self, names: [Option<&[u8]>; 6]) -> Parsed<Rule> {
        let name = self.word()?.to_vec();
        self.take(b":")?;
        let align = if self.token == b"ALIGN" {
            self.take(b"ALIGN")?;
            self.take(b"(")?;
            let align = self.number()?;
            self.take(b")")?;
            if !align.is_power_of_two() {
                return Err(());
            }
            align
        } else {
            0
        };
        self.take(b"{")?;
        let mut selectors = Vec::new();
        while self.token != b"}" {
            selectors.push(self.selector()?);
        }
        self.take(b"}")?;
        self.take(b":")?;
        let segment = self.word()?;
        let load = [LoadRole::Text, LoadRole::Rodata, LoadRole::Data]
            .into_iter()
            .zip(names)
            .find_map(|(role, name)| (name == Some(segment)).then_some(role))
            .ok_or(())?;
        let mut tls = false;
        if self.token == b":" {
            self.take(b":")?;
            let segment = self.word()?;
            match (load, name.as_slice()) {
                (LoadRole::Data, _) if names[3] == Some(segment) => tls = true,
                (LoadRole::Rodata, b".eh_frame_hdr") if names[5] == Some(segment) => {}
                _ => return Err(()),
            }
        }
        Ok(Rule {
            name,
            align,
            load,
            tls,
            selectors,
        })
    }

    fn script(&mut self) -> Parsed<Script> {
        self.take(b"ENTRY")?;
        self.take(b"(")?;
        let entry = self.word()?.to_vec();
        self.take(b")")?;
        let names = self.phdrs()?;
        self.take(b"SECTIONS")?;
        self.take(b"{")?;
        self.take(b".")?;
        self.take(b"=")?;
        let base = self.number()?;
        self.take(b"+")?;
        self.take(b"SIZEOF_HEADERS")?;
        self.take(b";")?;
        let mut rules = Vec::new();
        while self.token != b"}" {
            rules.push(self.rule(names)?);
        }
        self.take(b"}")?;
        if !self.token.is_empty() {
            return Err(());
        }
        Ok(Script {
            entry,
            base,
            explicit: true,
            tls: names[3].is_some(),
            eh_frame: names[5].is_some(),
            implicit_unwind_outputs: 0,
            rules,
        })
    }
}

// GNU ld §3.6.4.3: section name wildcard matching. Backtrack only to the
// most recent '*', so repeated stars do not introduce recursive searches.
fn matches_pattern(mut pattern: &[u8], mut name: &[u8]) -> bool {
    let mut retry = None::<(&[u8], &[u8])>;
    while let Some((&byte, rest)) = name.split_first() {
        if let Some(next) = pattern.strip_prefix(b"*") {
            pattern = next;
            retry = Some((pattern, name));
        } else if let Some(width) = pattern_width(pattern, byte) {
            pattern = &pattern[width..];
            name = rest;
        } else if let Some((next, [_, rest @ ..])) = retry {
            pattern = next;
            name = rest;
            retry = Some((next, rest));
        } else {
            return false;
        }
    }
    pattern.iter().all(|&byte| byte == b'*')
}

fn pattern_width(pattern: &[u8], byte: u8) -> Option<usize> {
    match pattern {
        [b'?', ..] => Some(1),
        [b'\\', literal, ..] => (*literal == byte).then_some(2),
        [b'[', tail @ ..] => match bracket(tail, byte) {
            Some((size, matched)) => matched.then_some(size + 1),
            None => (byte == b'[').then_some(1),
        },
        [literal, ..] => (*literal == byte).then_some(1),
        [] => None,
    }
}

fn bracket(mut rest: &[u8], byte: u8) -> Option<(usize, bool)> {
    let initial_len = rest.len();
    let negated = matches!(rest.first(), Some(b'!' | b'^'));
    if negated {
        rest = &rest[1..];
    }
    let mut matched = false;
    let mut first = true;
    loop {
        if !first && rest.starts_with(b"]") {
            return Some((initial_len - rest.len() + 1, matched != negated));
        }
        let (low, next) = bracket_byte(rest)?;
        first = false;
        rest = next;
        if let [b'-', high, ..] = rest {
            if *high != b']' {
                let (high, next) = bracket_byte(&rest[1..])?;
                matched |= (low..=high).contains(&byte);
                rest = next;
                continue;
            }
        }
        matched |= low == byte;
    }
}

fn bracket_byte(rest: &[u8]) -> Option<(u8, &[u8])> {
    match rest {
        [b'\\', byte, rest @ ..] | [byte, rest @ ..] => Some((*byte, rest)),
        [] => None,
    }
}

#[cfg(test)]
mod tests {
    use super::Script;
    use std::path::Path;

    #[test]
    fn malformed_script_is_error_not_panic() {
        let path = Path::new("broken.ld");
        let script = include_bytes!("../../../userspace/programs/linker.ld").trim_ascii_end();
        assert!(Script::parse(path, script).is_ok());
        for end in 0..script.len() {
            assert!(
                matches!(Script::parse(path, &script[..end]), Err(error)
                if error.starts_with("broken.ld:")),
                "truncation at {end}"
            );
        }

        for (from, to) in [
            ("0x40000000", "0x10000000000000000"),
            ("4K", "3K"),
            ("FLAGS(6)", "FLAGS(7)"),
            (":data :tls", ":rodata :tls"),
            ("/* Userspace", "/*\0 Userspace"),
        ] {
            let corrupt = String::from_utf8_lossy(script).replace(from, to);
            assert!(
                matches!(Script::parse(path, corrupt.as_bytes()), Err(error)
                if error.starts_with("broken.ld:")),
                "{from} → {to}"
            );
        }
    }
}
