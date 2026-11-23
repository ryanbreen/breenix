//! Assignment, expansion and internal macros from POSIX.1-2024 make.

use crate::{reader::outside_macro_references, Error, Location};
use std::{
    collections::HashMap,
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};

const MAX_EXPANSION_DEPTH: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Default,
    Environment,
    Makefile,
    CommandLine,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Delayed,
    Immediate,
}

struct Definition {
    value: String,
    flavor: Flavor,
    location: Location,
    origin: Origin,
}

#[derive(Default)]
pub struct Macros {
    definitions: HashMap<String, Definition>,
    environment_overrides: bool,
}

#[derive(Default)]
pub(crate) struct Automatic<'a> {
    pub target: &'a str,
    pub source: Option<&'a str>,
    pub newer: &'a str,
    pub stem: &'a str,
    pub member: Option<&'a str>,
}

impl<'a> Automatic<'a> {
    pub(crate) fn for_target(name: &'a str) -> Self {
        let archive = name.strip_suffix(')').and_then(|name| name.split_once('('));
        let (target, member) = archive.map_or((name, None), |(file, member)| (file, Some(member)));
        Self {
            target,
            member,
            ..Self::default()
        }
    }

    fn get(&self, name: &str) -> Option<String> {
        let (name, part) = match name.strip_suffix(['D', 'F']) {
            Some(short) => (short, name.chars().last()),
            None => (name, None),
        };
        let value = match name {
            "@" => self.target,
            "<" => self.source.unwrap_or_default(),
            "?" => self.newer,
            "*" => self.stem,
            "%" => self.member.unwrap_or_default(),
            _ => return None,
        };
        Some(match part {
            Some('D') => separate_words(value)
                .map(|word| {
                    Path::new(word)
                        .parent()
                        .and_then(Path::to_str)
                        .filter(|part| !part.is_empty())
                        .unwrap_or(".")
                })
                .collect::<Vec<_>>()
                .join(" "),
            Some('F') => separate_words(value)
                .map(|word| word.rsplit('/').next().unwrap_or(word))
                .collect::<Vec<_>>()
                .join(" "),
            _ => value.to_owned(),
        })
    }
}

impl Macros {
    pub(crate) fn new(
        environment_overrides: bool,
        directory: String,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut macros = Self {
            environment_overrides,
            ..Self::default()
        };
        let location = Location {
            path: Path::new("<environment>").into(),
            line: 1,
        };
        macros.define(
            "SHELL",
            "/bin/sh".into(),
            Flavor::Delayed,
            &location,
            Origin::Default,
        );
        macros.define(
            "CURDIR",
            directory,
            Flavor::Delayed,
            &location,
            Origin::Default,
        );
        let defaults = Location {
            path: Path::new("<default rules>").into(),
            line: 1,
        };
        // POSIX.1-2024 make, "Default Rules", plus the assembler macros.
        for (name, value) in [
            ("AR", "ar"),
            ("ARFLAGS", "-rv"),
            ("AS", "as"),
            ("ASFLAGS", ""),
            ("CC", "cc"),
            ("CFLAGS", ""),
            ("YACC", "yacc"),
            ("YFLAGS", ""),
            ("LEX", "lex"),
            ("LFLAGS", ""),
            ("LDFLAGS", ""),
        ] {
            macros.define(
                name,
                value.into(),
                Flavor::Delayed,
                &defaults,
                Origin::Default,
            );
        }
        for (name, value) in environment {
            if matches!(name.as_str(), "SHELL" | "MAKEFLAGS")
                || (name == "CURDIR" && !environment_overrides)
            {
                continue;
            }
            macros.define(
                &name,
                value,
                Flavor::Delayed,
                &location,
                Origin::Environment,
            );
        }
        macros
    }

    pub(crate) fn use_posix_compiler_defaults(&mut self) {
        for (name, value) in [("CC", "c17"), ("CFLAGS", "-O 1")] {
            if let Some(definition) = self.definitions.get_mut(name) {
                if definition.origin == Origin::Default {
                    definition.value = value.into();
                }
            }
        }
    }

    pub(crate) fn assign_command_line(&mut self, assignment: &str) -> Result<(), Error> {
        let location = Location {
            path: Path::new("<command line>").into(),
            line: 1,
        };
        self.assign(assignment, &location, Origin::CommandLine)
    }

    pub(crate) fn assign_makefile(
        &mut self,
        assignment: &str,
        location: &Location,
    ) -> Result<(), Error> {
        self.assign(assignment, location, Origin::Makefile)
    }

    fn assign(
        &mut self,
        assignment: &str,
        location: &Location,
        origin: Origin,
    ) -> Result<(), Error> {
        let Some((equals, _)) = outside_macro_references(assignment).find(|&(_, ch)| ch == '=')
        else {
            return Err(Error::new(location, "missing '=' in macro definition"));
        };
        let raw_name = &assignment[..equals];
        let (raw_name, operator) = [":::", "::", "+", "?", "!"]
            .into_iter()
            .find_map(|operator| raw_name.strip_suffix(operator).map(|name| (name, operator)))
            .unwrap_or((raw_name, ""));
        let automatic = Automatic::default();
        let name = self.expand(raw_name, location, &automatic)?;
        let name = name.trim_matches([' ', '\t']);
        if name == "MAKEFLAGS" {
            return Err(Error::deferred(location, name));
        }
        if name.is_empty()
            || name.contains(|ch: char| ch.is_whitespace() || ch.is_control() || ch == '=')
        {
            return Err(Error::new(location, format!("invalid macro name '{name}'")));
        }
        if name.ends_with([':', '!', '?', '+']) {
            return Err(Error::new(
                location,
                "unsupported macro assignment operator",
            ));
        }
        let previous = self.definitions.get(name);
        if previous.is_some_and(|previous| {
            operator == "?"
                || (origin == Origin::Makefile
                    && (previous.origin == Origin::CommandLine
                        || (previous.origin == Origin::Environment && self.environment_overrides)))
        }) {
            return Ok(());
        }
        let value = assignment[equals + 1..].trim_start_matches([' ', '\t']);
        let (value, flavor) = match (operator, previous) {
            ("::", _) => (self.expand(value, location, &automatic)?, Flavor::Immediate),
            (":::", _) => (
                self.expand_with_dollar_pairs(value, location, &automatic, true)?,
                Flavor::Delayed,
            ),
            ("!", _) => {
                let command = self.expand(value, location, &automatic)?;
                (self.shell_assignment(&command, location)?, Flavor::Delayed)
            }
            ("+", Some(previous)) => {
                let appended = match previous.flavor {
                    Flavor::Immediate => self.expand(value, location, &automatic)?,
                    Flavor::Delayed => value.to_owned(),
                };
                (format!("{} {appended}", previous.value), previous.flavor)
            }
            _ => (value.to_owned(), Flavor::Delayed),
        };
        self.define(name, value, flavor, location, origin);
        Ok(())
    }

    fn shell_assignment(&self, command: &str, location: &Location) -> Result<String, Error> {
        let output = Command::new("/bin/sh")
            .args(["-c", command])
            .envs(self.recipe_environment())
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|error| Error::new(location, format!("could not run /bin/sh: {error}")))?;
        // POSIX keeps the captured output even when the command exits nonzero.
        if !output.status.success() {
            let reason = match output.status.code() {
                Some(code) => format!("exited with status {code}"),
                None => output.status.to_string(),
            };
            let _ = writeln!(io::stderr().lock(), "{location}: != command {reason}");
        }
        let stdout = String::from_utf8(output.stdout).map_err(|error| {
            Error::new(location, format!("!= command output is not UTF-8: {error}"))
        })?;
        let stdout = stdout.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
        Ok(stdout
            .strip_suffix('\n')
            .unwrap_or(stdout)
            .replace('\n', " "))
    }

    fn define(
        &mut self,
        name: &str,
        value: String,
        flavor: Flavor,
        location: &Location,
        origin: Origin,
    ) {
        self.definitions.insert(
            name.to_owned(),
            Definition {
                value,
                flavor,
                location: location.clone(),
                origin,
            },
        );
    }

    pub(crate) fn recipe_environment(&self) -> impl Iterator<Item = (&str, &str)> {
        self.definitions.iter().filter_map(|(name, definition)| {
            (definition.origin == Origin::CommandLine
                && !matches!(name.as_str(), "SHELL" | "MAKEFLAGS"))
            .then_some((name.as_str(), definition.value.as_str()))
        })
    }

    pub(crate) fn expand(
        &self,
        text: &str,
        location: &Location,
        automatic: &Automatic<'_>,
    ) -> Result<String, Error> {
        self.expand_with_dollar_pairs(text, location, automatic, false)
    }

    fn expand_with_dollar_pairs(
        &self,
        text: &str,
        location: &Location,
        automatic: &Automatic<'_>,
        preserve_dollar_pairs: bool,
    ) -> Result<String, Error> {
        Expansion {
            macros: self,
            automatic,
            preserve_dollar_pairs,
            active: Vec::new(),
        }
        .text(text, location, 0)
    }
}

struct Expansion<'a, 'b> {
    macros: &'a Macros,
    automatic: &'a Automatic<'b>,
    preserve_dollar_pairs: bool,
    active: Vec<&'a str>,
}

impl Expansion<'_, '_> {
    fn text(&mut self, mut text: &str, location: &Location, depth: usize) -> Result<String, Error> {
        if depth > MAX_EXPANSION_DEPTH {
            return Err(Error::new(location, "macro expansion nesting is too deep"));
        }
        let mut expanded = String::new();
        while let Some(dollar) = text.find('$') {
            expanded.push_str(&text[..dollar]);
            text = &text[dollar + 1..];
            let Some(next) = text.chars().next() else {
                return Err(Error::new(location, "missing macro name after '$'"));
            };
            text = &text[next.len_utf8()..];
            if next == '$' {
                if self.preserve_dollar_pairs {
                    expanded.push('$');
                }
                expanded.push('$');
                continue;
            }
            if matches!(next, '(' | '{') {
                let close = if next == '(' { ')' } else { '}' };
                let Some((end, _)) = outside_macro_references(text).find(|&(_, ch)| ch == close)
                else {
                    return Err(Error::new(
                        location,
                        format!("missing '{close}' in macro reference"),
                    ));
                };
                expanded.push_str(&self.reference(&text[..end], location, depth + 1)?);
                text = &text[end + 1..];
            } else {
                expanded.push_str(&self.value(
                    next.encode_utf8(&mut [0; 4]),
                    location,
                    depth + 1,
                )?);
            }
        }
        expanded.push_str(text);
        Ok(expanded)
    }

    fn reference(
        &mut self,
        reference: &str,
        location: &Location,
        depth: usize,
    ) -> Result<String, Error> {
        let colon = outside_macro_references(reference).find(|&(_, ch)| ch == ':');
        let name_end = colon.map_or(reference.len(), |(index, _)| index);
        let name = self.text(&reference[..name_end], location, depth)?;
        let value = self.value(&name, location, depth)?;
        match colon {
            Some((index, _)) => {
                let substitution = self.text(&reference[index + 1..], location, depth)?;
                let Some((old, new)) = substitution.split_once('=') else {
                    return Err(Error::new(location, "missing '=' in macro substitution"));
                };
                Ok(substitute(&value, old, new))
            }
            None => Ok(value),
        }
    }

    fn value(&mut self, name: &str, location: &Location, depth: usize) -> Result<String, Error> {
        if matches!(name, "MAKEFLAGS" | "MAKE") {
            return Err(Error::deferred(location, name));
        }
        if let Some(value) = self.automatic.get(name) {
            return Ok(value);
        }
        let Some((name, definition)) = self.macros.definitions.get_key_value(name) else {
            return Ok(String::new());
        };
        if definition.flavor == Flavor::Immediate {
            return Ok(definition.value.clone());
        }
        if let Some(start) = self.active.iter().position(|&active| active == name) {
            let mut cycle = self.active[start..].to_vec();
            cycle.push(name);
            return Err(Error::new(
                location,
                format!("recursive macro reference: {}", cycle.join(" -> ")),
            ));
        }
        self.active.push(name);
        let expanded = self.text(&definition.value, &definition.location, depth);
        self.active.pop();
        expanded
    }
}

pub(crate) fn separate_words(text: &str) -> impl Iterator<Item = &str> {
    text.split([' ', '\t', '\n'])
        .filter(|word| !word.is_empty())
}

fn substitute(value: &str, old: &str, new: &str) -> String {
    let pattern = old.split_once('%');
    let mut result = String::new();
    for part in value.split_inclusive([' ', '\t', '\n']) {
        let word = part.trim_end_matches([' ', '\t', '\n']);
        if !word.is_empty() {
            match pattern {
                Some((prefix, suffix)) => {
                    if let Some(stem) = word
                        .strip_prefix(prefix)
                        .and_then(|rest| rest.strip_suffix(suffix))
                    {
                        result.push_str(&new.replacen('%', stem, 1));
                    } else {
                        result.push_str(word);
                    }
                }
                None => {
                    if let Some(stem) = word.strip_suffix(old) {
                        result.push_str(stem);
                        result.push_str(new);
                    } else {
                        result.push_str(word);
                    }
                }
            }
        }
        result.push_str(&part[word.len()..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_automatic_variables() {
        let automatic = Automatic::for_target("lib.a(member.o)");
        let location = Location {
            path: Path::new("project.mk").into(),
            line: 2,
        };
        assert_eq!(
            Macros::default()
                .expand("$@|$%", &location, &automatic)
                .unwrap(),
            "lib.a|member.o"
        );
    }
}
