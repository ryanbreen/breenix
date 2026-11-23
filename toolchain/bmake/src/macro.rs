//! Assignment, expansion and internal macros from POSIX.1-2024 make.

use crate::{graph::archive_member, reader::outside_macro_references, Error, Location};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
    rc::Rc,
};

const MAX_EXPANSION_DEPTH: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Default,
    Environment,
    Makefile,
    Makeflags,
    CommandLine,
    Override,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacroSource {
    Makeflags,
    CommandLine,
}

impl MacroSource {
    pub(crate) fn location(self) -> Location {
        let path = match self {
            Self::Makeflags => "<environment>",
            Self::CommandLine => "<command line>",
        };
        Location {
            path: Path::new(path).into(),
            line: 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Delayed,
    Immediate,
}

#[derive(Clone)]
pub(crate) struct Definition {
    value: String,
    flavor: Flavor,
    location: Location,
    origin: Origin,
    export: Option<bool>,
    private: bool,
    append: bool,
    parent: Option<Rc<Definition>>,
}

pub(crate) type Bindings = HashMap<String, Definition>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub overriding: bool,
    pub export: Option<bool>,
    pub private: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Assignment<'a> {
    pub name: &'a str,
    pub operator: &'a str,
    pub value: &'a str,
}

impl<'a> Assignment<'a> {
    pub fn parse(text: &'a str) -> Option<Self> {
        let (equals, _) = outside_macro_references(text).find(|&(_, ch)| ch == '=')?;
        let raw_name = &text[..equals];
        let (name, operator) = [":::", "::", ":", "+", "?", "!"]
            .into_iter()
            .find_map(|operator| raw_name.strip_suffix(operator).map(|name| (name, operator)))
            .unwrap_or((raw_name, ""));
        Some(Self {
            name,
            operator,
            value: text[equals + 1..].trim_start_matches([' ', '\t']),
        })
    }
}

#[derive(Clone, Default)]
pub struct Macros {
    definitions: Bindings,
    target_variables: HashMap<String, Bindings>,
    environment_names: HashSet<String>,
    exports: HashMap<String, bool>,
    export_all: bool,
    gnu_environment: bool,
    environment_overrides: bool,
}

#[derive(Default)]
pub(crate) struct Automatic<'a> {
    pub target: &'a str,
    pub source: Option<&'a str>,
    pub newer: &'a str,
    pub order_only: &'a str,
    pub stem: &'a str,
    pub member: Option<&'a str>,
}

impl<'a> Automatic<'a> {
    pub(crate) fn for_target(name: &'a str) -> Self {
        let (target, member) =
            archive_member(name).map_or((name, None), |(file, member)| (file, Some(member)));
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
            "|" => self.order_only,
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
            macros.environment_names.insert(name.clone());
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

    pub(crate) fn set_make_defaults(&mut self, make: String, flags: String) {
        let location = MacroSource::Makeflags.location();
        if !self.definitions.contains_key("MAKE") {
            self.define("MAKE", make, Flavor::Immediate, &location, Origin::Default);
        }
        self.define(
            "MAKEFLAGS",
            flags,
            Flavor::Immediate,
            &location,
            Origin::Default,
        );
    }

    pub(crate) fn assign_operand(
        &mut self,
        assignment: &str,
        source: MacroSource,
    ) -> Result<(), Error> {
        let origin = match source {
            MacroSource::Makeflags => Origin::Makeflags,
            MacroSource::CommandLine => Origin::CommandLine,
        };
        let location = source.location();
        let assignment = Assignment::parse(assignment)
            .ok_or_else(|| Error::new(&location, "missing '=' in macro definition"))?;
        let values = self.view(None);
        let name = values.assignment_name(assignment.name, &location)?;
        if let Some(definition) =
            values.assigned(&name, assignment, &location, origin, Modifiers::default())?
        {
            self.definitions.insert(name, definition);
        }
        Ok(())
    }

    pub(crate) fn assign_makefile(
        &mut self,
        assignment: &str,
        location: &Location,
    ) -> Result<(), Error> {
        let assignment = Assignment::parse(assignment)
            .ok_or_else(|| Error::new(location, "missing '=' in macro definition"))?;
        self.assign_directive(None, assignment, Modifiers::default(), location)
    }

    pub(crate) fn assign_directive(
        &mut self,
        target: Option<&str>,
        assignment: Assignment<'_>,
        modifiers: Modifiers,
        location: &Location,
    ) -> Result<(), Error> {
        self.gnu_environment |=
            target.is_some() || modifiers.export.is_some() || modifiers.overriding;
        let origin = if modifiers.overriding {
            Origin::Override
        } else {
            Origin::Makefile
        };
        let scope = target.and_then(|target| self.target_variables.get(target));
        let values = Values {
            macros: self,
            scope,
            target_assignment: target.is_some(),
        };
        let name = values.assignment_name(assignment.name, location)?;
        let definition = values.assigned(&name, assignment, location, origin, modifiers)?;
        if let Some(target) = target {
            if let Some(definition) = definition {
                self.target_variables
                    .entry(target.to_owned())
                    .or_default()
                    .insert(name, definition);
            } else if let Some(export) = modifiers.export {
                // A command-line or -e value can win while this target
                // still sets its export policy.
                if let Some(mut definition) = values.get(&name).cloned() {
                    definition.export = Some(export);
                    definition.private = modifiers.private;
                    self.target_variables
                        .entry(target.to_owned())
                        .or_default()
                        .insert(name, definition);
                }
            }
        } else {
            if let Some(export) = modifiers.export {
                self.exports.insert(name.clone(), export);
            }
            if let Some(definition) = definition {
                self.definitions.insert(name, definition);
            }
        }
        Ok(())
    }

    pub(crate) fn set_export(
        &mut self,
        names: &str,
        export: bool,
        location: &Location,
    ) -> Result<(), Error> {
        self.gnu_environment = true;
        if names.is_empty() {
            self.export_all = export;
        } else {
            let names = self.expand(names, location, &Automatic::default())?;
            for name in separate_words(&names) {
                if name.contains(['=', '\0']) {
                    return Err(Error::new(
                        location,
                        format!("invalid export name '{name}'"),
                    ));
                }
                self.exports.insert(name.to_owned(), export);
            }
        }
        Ok(())
    }

    pub(crate) fn has_value(&self, name: &str) -> bool {
        self.definitions
            .get(name)
            .is_some_and(|definition| !definition.value.is_empty())
    }

    pub(crate) fn target_scope(
        &self,
        target: &str,
        inherited: Option<&Rc<Bindings>>,
    ) -> (Option<Rc<Bindings>>, Option<Rc<Bindings>>) {
        let Some(local) = self.target_variables.get(target) else {
            return (inherited.cloned(), inherited.cloned());
        };
        let mut recipe = inherited.map_or_else(Bindings::new, |scope| (**scope).clone());
        let mut descendants = None;
        for (name, local) in local {
            let mut definition = local.clone();
            let parent = inherited.and_then(|scope| scope.get(name));
            definition.export = definition
                .export
                .or_else(|| parent.and_then(|parent| parent.export));
            if definition.append {
                definition.parent = parent.cloned().map(Rc::new);
            }
            recipe.insert(name.clone(), definition);
            if local.private {
                descendants.get_or_insert_with(Vec::new).push(name);
            }
        }
        let descendant_scope = descendants.map(|private| {
            let mut scope = recipe.clone();
            for name in private {
                if let Some(previous) = inherited.and_then(|scope| scope.get(name)) {
                    scope.insert(name.clone(), previous.clone());
                } else {
                    scope.remove(name);
                }
            }
            Rc::new(scope)
        });
        let recipe = Rc::new(recipe);
        let descendants = descendant_scope.unwrap_or_else(|| Rc::clone(&recipe));
        (Some(recipe), Some(descendants))
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
                export: None,
                private: false,
                append: false,
                parent: None,
            },
        );
    }

    pub(crate) fn view<'a>(&'a self, scope: Option<&'a Bindings>) -> Values<'a> {
        Values {
            macros: self,
            scope,
            target_assignment: false,
        }
    }

    pub(crate) fn expand(
        &self,
        text: &str,
        location: &Location,
        automatic: &Automatic<'_>,
    ) -> Result<String, Error> {
        self.view(None).expand(text, location, automatic)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Values<'a> {
    macros: &'a Macros,
    scope: Option<&'a Bindings>,
    target_assignment: bool,
}

impl<'a> Values<'a> {
    fn get(self, name: &str) -> Option<&'a Definition> {
        self.entry(name).map(|(_, definition)| definition)
    }

    fn entry(self, name: &str) -> Option<(&'a String, &'a Definition)> {
        self.scope
            .and_then(|scope| scope.get_key_value(name))
            .or_else(|| self.macros.definitions.get_key_value(name))
    }

    fn assignment_name(self, raw_name: &str, location: &Location) -> Result<String, Error> {
        let name = self.expand(raw_name, location, &Automatic::default())?;
        let name = name.trim_matches([' ', '\t']);
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
        Ok(name.to_owned())
    }

    fn assigned(
        self,
        name: &str,
        assignment: Assignment<'_>,
        location: &Location,
        origin: Origin,
        modifiers: Modifiers,
    ) -> Result<Option<Definition>, Error> {
        let Assignment {
            operator, value, ..
        } = assignment;
        let previous = self.get(name);
        let local = self.scope.and_then(|scope| scope.get(name));
        if previous.is_some_and(|previous| {
            operator == "?"
                || (origin == Origin::Makefile
                    && (matches!(previous.origin, Origin::CommandLine | Origin::Makeflags)
                        || (previous.origin == Origin::Override
                            && (!self.target_assignment || local.is_some()))
                        || (previous.origin == Origin::Environment
                            && self.macros.environment_overrides
                            && name != "MAKE")))
        }) {
            return Ok(None);
        }
        let append = if self.target_assignment && operator == "+" {
            local.is_none_or(|definition| definition.append)
        } else {
            false
        };
        let previous = if self.target_assignment && operator == "+" {
            local
        } else {
            previous
        };
        let automatic = Automatic::default();
        let (value, flavor) = match (operator, previous) {
            (":" | "::", _) => (self.expand(value, location, &automatic)?, Flavor::Immediate),
            (":::", _) => (
                Expansion::new(self, &automatic, true).text(value, location, 0)?,
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
        Ok(Some(Definition {
            value,
            flavor,
            origin,
            location: location.clone(),
            export: modifiers
                .export
                .or_else(|| local.and_then(|definition| definition.export)),
            private: modifiers.private,
            append,
            parent: None,
        }))
    }

    fn shell_assignment(self, command: &str, location: &Location) -> Result<String, Error> {
        let mut shell = Command::new("/bin/sh");
        self.set_recipe_environment(&mut shell, location, &Automatic::default())?;
        let output = shell
            .args(["-c", command])
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

    pub(crate) fn set_recipe_environment(
        self,
        command: &mut Command,
        location: &Location,
        automatic: &Automatic<'_>,
    ) -> Result<(), Error> {
        if !self.macros.gnu_environment {
            let flags = self.expand("$(MAKEFLAGS)", location, automatic)?;
            command.env("MAKEFLAGS", flags);
            command.envs(
                self.macros
                    .definitions
                    .iter()
                    .filter_map(|(name, definition)| {
                        (definition.origin == Origin::CommandLine
                            && !matches!(name.as_str(), "SHELL" | "MAKEFLAGS"))
                        .then_some((name.as_str(), definition.value.as_str()))
                    }),
            );
            return Ok(());
        }

        for (name, export) in &self.macros.exports {
            if !export {
                command.env_remove(name);
            }
        }
        let names = self.macros.definitions.keys().chain(
            self.scope
                .into_iter()
                .flat_map(|scope| scope.keys())
                .filter(|name| !self.macros.definitions.contains_key(*name)),
        );
        for name in names {
            let definition = self.get(name).expect("known variable");
            let policy = definition
                .export
                .or_else(|| self.macros.exports.get(name).copied());
            if policy == Some(false) {
                command.env_remove(name);
                continue;
            }
            let by_default = matches!(name.as_str(), "MAKEFLAGS" | "MAKEFILES")
                || (name != "SHELL"
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    && (self.macros.environment_names.contains(name)
                        || matches!(
                            definition.origin,
                            Origin::Environment | Origin::Makeflags | Origin::CommandLine
                        )
                        || (self.macros.export_all && definition.origin != Origin::Default)));
            if policy == Some(true) || by_default {
                let value = Expansion::new(self, automatic, false).value(name, location, 0)?;
                command.env(name, value);
            }
        }
        Ok(())
    }

    fn expand(
        self,
        text: &str,
        location: &Location,
        automatic: &Automatic<'_>,
    ) -> Result<String, Error> {
        self.expand_recipe(text, location, automatic)
            .map(|(text, _)| text)
    }

    pub(crate) fn expand_recipe(
        self,
        text: &str,
        location: &Location,
        automatic: &Automatic<'_>,
    ) -> Result<(String, bool), Error> {
        let mut expansion = Expansion::new(self, automatic, false);
        let text = expansion.text(text, location, 0)?;
        Ok((text, expansion.expanded_make))
    }
}

struct Expansion<'a, 'b> {
    values: Values<'a>,
    automatic: &'a Automatic<'b>,
    preserve_dollar_pairs: bool,
    active: Vec<&'a str>,
    expanded_make: bool,
}

impl<'a, 'b> Expansion<'a, 'b> {
    fn new(values: Values<'a>, automatic: &'a Automatic<'b>, preserve_dollar_pairs: bool) -> Self {
        Self {
            values,
            automatic,
            preserve_dollar_pairs,
            active: Vec::new(),
            expanded_make: false,
        }
    }

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
        if let Some((name, _)) = reference.split_once([' ', '\t']) {
            if crate::inventory::FUNCTIONS.contains(&name) {
                return Err(Error::new(
                    location,
                    format!("GNU make function '{name}' is deferred to the 25.11c function units"),
                ));
            }
        }
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
        self.expanded_make |= name == "MAKE";
        if let Some(value) = self.automatic.get(name) {
            return Ok(value);
        }
        let Some((name, definition)) = self.values.entry(name) else {
            return Ok(String::new());
        };
        if let Some(start) = self.active.iter().position(|&active| active == name) {
            let mut cycle = self.active[start..].to_vec();
            cycle.push(name);
            return Err(Error::new(
                location,
                format!("recursive macro reference: {}", cycle.join(" -> ")),
            ));
        }
        self.active.push(name);
        let mut value = String::new();
        let result = self.expand_definition(name, definition, &mut value, depth);
        self.active.pop();
        result.map(|()| value)
    }

    fn expand_definition(
        &mut self,
        name: &str,
        definition: &Definition,
        result: &mut String,
        depth: usize,
    ) -> Result<(), Error> {
        let mut appends = Vec::new();
        let mut current = Some(definition);
        while let Some(definition) = current {
            appends.push(definition);
            if !definition.append {
                break;
            }
            current = definition
                .parent
                .as_deref()
                .or_else(|| self.values.macros.definitions.get(name));
        }
        for definition in appends.into_iter().rev() {
            if !result.is_empty() && !definition.value.is_empty() {
                result.push(' ');
            }
            if definition.flavor == Flavor::Immediate {
                result.push_str(&definition.value);
            } else {
                result.push_str(&self.text(&definition.value, &definition.location, depth)?);
            }
        }
        Ok(())
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
