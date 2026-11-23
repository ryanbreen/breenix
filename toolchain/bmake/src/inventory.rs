//! Source inventory from the pinned GNU make 4.4.1 manual. This pass does not
//! expand variables, resolve includes or select conditional branches.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::PathBuf,
};

const CONDITIONALS: &[&str] = &["ifdef", "ifndef", "ifeq", "ifneq"];
const MODIFIERS: &[&str] = &["override", "export", "unexport", "private"];
const DIRECTIVES: &[&str] = &[
    "define", "endef", "ifdef", "ifndef", "ifeq", "ifneq", "else", "endif", "include", "-include",
    "sinclude", "override", "export", "unexport", "private", "vpath", "undefine", "load", "-load",
];
const FUNCTIONS: &[&str] = &[
    "subst",
    "patsubst",
    "strip",
    "findstring",
    "filter",
    "filter-out",
    "sort",
    "word",
    "wordlist",
    "words",
    "firstword",
    "lastword",
    "dir",
    "notdir",
    "suffix",
    "basename",
    "addsuffix",
    "addprefix",
    "join",
    "wildcard",
    "realpath",
    "abspath",
    "error",
    "warning",
    "info",
    "shell",
    "origin",
    "flavor",
    "foreach",
    "if",
    "or",
    "and",
    "intcmp",
    "call",
    "value",
    "eval",
    "file",
    "let",
    "guile",
];
const SPECIALS: &[&str] = &[
    ".PHONY",
    ".SUFFIXES",
    ".DEFAULT",
    ".PRECIOUS",
    ".INTERMEDIATE",
    ".NOTINTERMEDIATE",
    ".SECONDARY",
    ".SECONDEXPANSION",
    ".DELETE_ON_ERROR",
    ".IGNORE",
    ".LOW_RESOLUTION_TIME",
    ".SILENT",
    ".EXPORT_ALL_VARIABLES",
    ".NOTPARALLEL",
    ".ONESHELL",
    ".POSIX",
    ".WAIT",
];

#[derive(Default)]
struct LogicalLine {
    text: String,
    starts: Vec<usize>,
    first: usize,
}

impl LogicalLine {
    fn push(&mut self, physical: &str, line: usize) -> bool {
        if self.starts.is_empty() {
            self.first = line;
        }
        self.starts.push(self.text.len());
        let slashes = physical
            .bytes()
            .rev()
            .take_while(|&byte| byte == b'\\')
            .count();
        if slashes % 2 == 0 {
            self.text.push_str(physical);
            false
        } else {
            // Manual §3.1.1. A separator suffices for lexical recognition,
            // and the offsets retain the physical line of each token.
            self.text.push_str(&physical[..physical.len() - 1]);
            self.text.push(' ');
            true
        }
    }

    fn line_at(&self, offset: usize) -> usize {
        self.first + self.starts.partition_point(|&start| start <= offset) - 1
    }
}

fn offset_of(part: &str, whole: &str) -> usize {
    part.as_ptr() as usize - whole.as_ptr() as usize
}

enum Operator {
    Assign(&'static str),
    Rule(usize),
}

fn operator(text: &str) -> Option<(usize, Operator)> {
    for (offset, symbol) in text.match_indices([':', '=', '+', '?', '!']) {
        let tail = &text[offset..];
        for name in [":::=", "::=", ":=", "+=", "?=", "!=", "="] {
            if tail.starts_with(name) {
                return Some((offset, Operator::Assign(name)));
            }
        }
        if symbol == ":" {
            return Some((
                offset,
                Operator::Rule(if tail.starts_with("::") { 2 } else { 1 }),
            ));
        }
    }
    None
}

fn head(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    text.split_at(text.find(char::is_whitespace).unwrap_or(text.len()))
}

fn directive(text: &str) -> Option<(&str, &str)> {
    let (name, rest) = head(text);
    // A directive spelling can itself be the variable name in an assignment.
    let assigns_name = matches!(operator(rest.trim_start()), Some((0, Operator::Assign(_))));
    (DIRECTIVES.contains(&name) && !assigns_name).then_some((name, rest))
}

fn target_assignment(mut text: &str) -> Option<(usize, &'static str)> {
    let initial_len = text.len();
    while let Some((_, rest)) = directive(text).filter(|(name, _)| MODIFIERS.contains(name)) {
        text = rest;
    }
    let (offset, Operator::Assign(name)) = operator(text)? else {
        return None;
    };
    let variable = text[..offset].trim();
    if variable.is_empty() || variable.contains(char::is_whitespace) {
        return None;
    }
    Some((initial_len - text.len() + offset, name))
}

fn has_inline_recipe(text: &str) -> bool {
    if directive(text).is_some() {
        return false;
    }
    match operator(text) {
        Some((offset, Operator::Rule(width))) => {
            target_assignment(&text[offset + width..]).is_none()
        }
        _ => false,
    }
}

// Keep a same-width copy with references and quoted punctuation masked so
// they cannot be mistaken for statement separators. §3.1 and §8.1 govern
// comments and reference delimiters. Recipe text keeps its shell comments
// because make expands references before handing the text to the shell (§5.1).
fn scan_lexemes<'a>(
    text: &'a str,
    verbatim: bool,
    mut function: impl FnMut(usize, &'a str),
) -> String {
    let bytes = text.as_bytes();
    let mut code = bytes.to_vec();
    let mut end = if verbatim { 0 } else { bytes.len() };
    let mut delimiters = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let inside_reference = !delimiters.is_empty();
        let mut width = 1;
        if byte == b'$' {
            if let Some(&next) = bytes.get(index + 1) {
                width = if next.is_ascii() { 2 } else { 1 };
                if matches!(next, b'(' | b'{') {
                    delimiters.push(next);
                    let tail = &text[index + 2..];
                    let name_end = tail
                        .find(|ch: char| !(ch.is_ascii_alphabetic() || ch == '-'))
                        .unwrap_or(tail.len());
                    let name = &tail[..name_end];
                    if matches!(tail.as_bytes().get(name_end), Some(b' ' | b'\t'))
                        && FUNCTIONS.contains(&name)
                    {
                        function(index, name);
                    }
                }
            }
        } else if let Some(&open) = delimiters.last() {
            if byte == open {
                delimiters.push(open);
            } else if matches!((open, byte), (b'(', b')') | (b'{', b'}')) {
                delimiters.pop();
            }
        } else if index < end {
            if byte == b'\\'
                && matches!(
                    bytes.get(index + 1),
                    Some(b'\\' | b'#' | b':' | b'%' | b';' | b' ')
                )
            {
                width = 2;
            } else if byte == b'#' {
                end = index;
                break;
            } else if byte == b';'
                && has_inline_recipe(std::str::from_utf8(&code[..index]).expect("ASCII boundary"))
            {
                end = index;
            }
        }
        if inside_reference || byte == b'$' || width == 2 {
            code[index..index + width].fill(b'~');
        }
        index += width;
    }
    code.truncate(end);
    String::from_utf8(code).expect("lexing preserves UTF-8")
}

#[derive(Default)]
struct Context {
    rule: bool,
    defines: usize,
}

impl Context {
    fn scan(&mut self, line: &LogicalLine, mut report: impl FnMut(usize, &'static str, &str)) {
        let tabbed = line.text.starts_with('\t');
        let recipe = self.rule && tabbed;
        let define_body = self.defines != 0;
        let mut hit = |offset, kind, name| report(line.line_at(offset), kind, name);
        let code = scan_lexemes(&line.text, recipe || define_body, |at, name| {
            hit(at, "function", name);
        });
        if recipe {
            return;
        }

        // A define body is literal variable text. Only its nesting
        // delimiters are statements until the closing endef (§6.8).
        if define_body {
            let (word, _) = head(&line.text);
            if !tabbed && matches!(word, "define" | "endef") {
                hit(offset_of(word, &line.text), "directive", word);
                if word == "define" {
                    self.defines += 1;
                } else {
                    self.defines -= 1;
                }
            }
            return;
        }

        let mut rest = code.as_str();
        while let Some((name, args)) = directive(rest) {
            hit(offset_of(name, &code), "directive", name);
            if MODIFIERS.contains(&name) && !args.trim().is_empty() {
                rest = args;
                continue;
            }
            if name == "else" {
                let (next, _) = head(args);
                if CONDITIONALS.contains(&next) {
                    hit(offset_of(next, &code), "directive", next);
                }
            } else if name == "define" {
                self.rule = false;
                self.defines += 1;
                if let Some((at, Operator::Assign(op))) = operator(args) {
                    hit(offset_of(args, &code) + at, "assignment", op);
                }
            }
            return;
        }
        let Some((at, op)) = operator(rest) else {
            return;
        };
        let base = offset_of(rest, &code);
        let width = match op {
            Operator::Assign(name) => {
                hit(base + at, "assignment", name);
                self.rule = false;
                return;
            }
            Operator::Rule(width) => width,
        };

        let targets = &rest[..at];
        let prereqs = &rest[at + width..];
        let prereq_base = base + at + width;
        if let Some((at, name)) = target_assignment(prereqs) {
            let mut modifiers = prereqs;
            while let Some((name, args)) =
                directive(modifiers).filter(|(name, _)| MODIFIERS.contains(name))
            {
                hit(offset_of(name, &code), "directive", name);
                modifiers = args;
            }
            hit(prereq_base + at, "assignment", name);
            self.rule = false;
            return;
        }
        self.rule = true;
        if let Some(percent) = targets.find('%') {
            hit(base + percent, "pattern-rule", "%");
        } else if let Some((colon, Operator::Rule(_))) = operator(prereqs) {
            if let Some(percent) = prereqs[..colon].find('%') {
                hit(prereq_base + percent, "pattern-rule", "static");
            }
        }
        for target in targets.trim_end_matches('&').split_whitespace() {
            if SPECIALS.contains(&target) {
                hit(offset_of(target, &code), "special-target", target);
            }
        }
        for prerequisite in prereqs.split([' ', '\t', '|']) {
            if prerequisite == ".WAIT" {
                hit(
                    offset_of(prerequisite, &code),
                    "special-target",
                    prerequisite,
                );
            }
        }
    }
}

pub(crate) fn print(files: &[PathBuf]) -> Result<(), String> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for file in files {
        let display = file.display();
        let content = fs::read_to_string(file).map_err(|error| format!("{display}: {error}"))?;
        let mut context = Context::default();
        let mut logical = LogicalLine::default();
        let mut hits = BTreeSet::new();
        let mut scan = |line: &LogicalLine| {
            context.scan(line, |at, kind, name| {
                hits.insert((at, kind, name.to_owned()));
            });
        };

        for (index, line) in content.lines().enumerate() {
            if !logical.push(line, index + 1) {
                scan(&logical);
                logical.starts.clear();
                logical.text.clear();
            }
        }
        if !logical.starts.is_empty() {
            scan(&logical);
        }
        for (line, kind, name) in hits {
            writeln!(output, "{display}:{line}\t{kind}\t{name}")
                .map_err(|error| format!("inventory output: {error}"))?;
        }
    }
    output
        .flush()
        .map_err(|error| format!("inventory output: {error}"))
}
