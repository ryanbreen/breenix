//! Variable expansion from POSIX.1-2024 and GNU make 4.4.1, chapters 6 and 8.

use crate::{
    graph::archive_member,
    inference::Pattern,
    make_functions,
    reader::{outside_macro_references, reference_level, Makefile},
    Error, Location,
};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
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
    Automatic,
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
    // eval and shell can define variables during an expansion. Keep borrows
    // short, and hold a copy of the definition being expanded across callbacks.
    definitions: RefCell<Bindings>,
    target_variables: RefCell<HashMap<String, Bindings>>,
    environment: HashMap<String, String>,
    exports: RefCell<HashMap<String, bool>>,
    export_all: Cell<bool>,
    gnu_environment: Cell<bool>,
    environment_overrides: bool,
    pub(crate) include_dirs: Vec<PathBuf>,
    locals: RefCell<Vec<LocalScope>>,
    exporting: RefCell<HashSet<String>>,
    pending_eval: RefCell<Makefile>,
    eval_depth: Cell<usize>,
    reading: Cell<bool>,
}

#[derive(Clone, Default)]
struct LocalScope {
    variables: Bindings,
    call: bool,
}

#[derive(Default)]
pub(crate) struct Automatic<'a> {
    pub target: &'a str,
    pub source: Option<&'a str>,
    pub prerequisites: &'a [&'a str],
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
        let list;
        let value = match name {
            "@" => self.target,
            "<" => self.source.unwrap_or_default(),
            "?" => self.newer,
            "|" => self.order_only,
            "*" => self.stem,
            "%" => self.member.unwrap_or_default(),
            "^" | "+" => {
                let mut seen = HashSet::new();
                let keep_duplicates = name == "+";
                list = self
                    .prerequisites
                    .iter()
                    .copied()
                    .filter(|name| keep_duplicates || seen.insert(*name))
                    .map(|name| archive_member(name).map_or(name, |(_, member)| member))
                    .collect::<Vec<_>>()
                    .join(" ");
                &list
            }
            _ => return None,
        };
        Some(match part {
            Some('D') => separate_words(value)
                .map(|word| match word.rsplit_once('/') {
                    Some(("", _)) => "/",
                    Some((directory, _)) => directory,
                    None => ".",
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
        builtin_variables: bool,
        directory: String,
        include_dirs: Vec<PathBuf>,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut macros = Self {
            environment_overrides,
            include_dirs,
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
        ].into_iter().filter(|_| builtin_variables) {
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
            macros.environment.insert(name.clone(), value.clone());
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

    pub(crate) fn use_posix_compiler_defaults(&self) {
        let mut definitions = self.definitions.borrow_mut();
        for (name, value) in [("CC", "c17"), ("CFLAGS", "-O 1")] {
            if let Some(definition) = definitions.get_mut(name) {
                if definition.origin == Origin::Default {
                    definition.value = value.into();
                }
            }
        }
    }

    pub(crate) fn set_make_defaults(&mut self, make: String, flags: String, goals: &[String]) {
        let location = MacroSource::Makeflags.location();
        if !self.definitions.get_mut().contains_key("MAKE") {
            self.define("MAKE", make, Flavor::Immediate, &location, Origin::Default);
        }
        self.define(
            "MAKEFLAGS",
            flags,
            Flavor::Immediate,
            &location,
            Origin::Default,
        );
        if !goals.is_empty() {
            self.define(
                "MAKECMDGOALS",
                goals.join(" "),
                Flavor::Immediate,
                &location,
                Origin::Default,
            );
        }
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
            self.definitions.get_mut().insert(name, definition);
        }
        Ok(())
    }

    pub(crate) fn assign_makefile(
        &self,
        assignment: &str,
        location: &Location,
    ) -> Result<(), Error> {
        let assignment = Assignment::parse(assignment)
            .ok_or_else(|| Error::new(location, "missing '=' in macro definition"))?;
        self.assign_directive(None, assignment, Modifiers::default(), location)
    }

    pub(crate) fn assign_directive(
        &self,
        target: Option<&str>,
        assignment: Assignment<'_>,
        modifiers: Modifiers,
        location: &Location,
    ) -> Result<(), Error> {
        if target.is_some() || modifiers.export.is_some() || modifiers.overriding {
            self.gnu_environment.set(true);
        }
        let origin = if modifiers.overriding {
            Origin::Override
        } else {
            Origin::Makefile
        };
        let scope = target.and_then(|target| self.target_variables.borrow().get(target).cloned());
        let values = Values {
            macros: self,
            scope: scope.as_ref(),
            target_assignment: target.is_some(),
        };
        let name = values.assignment_name(assignment.name, location)?;
        let definition = values.assigned(&name, assignment, location, origin, modifiers)?;
        if let Some(target) = target {
            if let Some(definition) = definition {
                self.target_variables
                    .borrow_mut()
                    .entry(target.to_owned())
                    .or_default()
                    .insert(name, definition);
            } else if let Some(export) = modifiers.export {
                // A command-line or -e value can win while this target
                // still sets its export policy.
                if let Some(mut definition) = values.get(&name) {
                    definition.export = Some(export);
                    definition.private = modifiers.private;
                    self.target_variables
                        .borrow_mut()
                        .entry(target.to_owned())
                        .or_default()
                        .insert(name, definition);
                }
            }
        } else {
            if let Some(export) = modifiers.export {
                self.exports.borrow_mut().insert(name.clone(), export);
            }
            if let Some(definition) = definition {
                self.definitions.borrow_mut().insert(name, definition);
            }
        }
        Ok(())
    }

    pub(crate) fn set_export(
        &self,
        names: &str,
        export: bool,
        location: &Location,
    ) -> Result<(), Error> {
        self.gnu_environment.set(true);
        if names.is_empty() {
            self.export_all.set(export);
        } else {
            let names = self.expand(names, location, &Automatic::default())?;
            for name in separate_words(&names) {
                if name.contains(['=', '\0']) {
                    return Err(Error::new(
                        location,
                        format!("invalid export name '{name}'"),
                    ));
                }
                self.exports.borrow_mut().insert(name.to_owned(), export);
            }
        }
        Ok(())
    }

    pub(crate) fn has_value(&self, name: &str) -> bool {
        self.view(None)
            .get(name)
            .is_some_and(|definition| !definition.value.is_empty())
    }

    pub(crate) fn target_scope(
        &self,
        target: &str,
        inherited: Option<&Rc<Bindings>>,
    ) -> (Option<Rc<Bindings>>, Option<Rc<Bindings>>) {
        let target_variables = self.target_variables.borrow();
        let Some(local) = target_variables.get(target) else {
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
        &self,
        name: &str,
        value: String,
        flavor: Flavor,
        location: &Location,
        origin: Origin,
    ) {
        self.definitions.borrow_mut().insert(
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

    pub(crate) fn set_reading(&self, reading: bool) {
        self.reading.set(reading);
    }

    pub(crate) fn take_eval(&self) -> Makefile {
        self.pending_eval.take()
    }

    fn eval(&self, text: &str, location: &Location) -> Result<(), Error> {
        let depth = self.eval_depth.get();
        if depth >= MAX_EXPANSION_DEPTH {
            return Err(Error::new(location, "eval nesting is too deep"));
        }
        let mut pending = self.take_eval();
        let mut generated = Makefile::default();
        let text = if text.is_empty() || text.ends_with('\n') {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(format!("{text}\n"))
        };
        self.eval_depth.set(depth + 1);
        let result = generated.read_entries(location, text.as_bytes(), self, depth + 1);
        self.eval_depth.set(depth);
        if !self.reading.get() && (!generated.entries.is_empty() || !generated.includes.is_empty())
        {
            return Err(Error::new(
                location,
                "eval cannot define rules or includes during a recipe",
            ));
        }
        pending.append(generated);
        self.pending_eval.replace(pending);
        result
    }

    fn with_locals<T>(&self, scope: LocalScope, expand: impl FnOnce() -> T) -> T {
        self.locals.borrow_mut().push(scope);
        let result = expand();
        self.locals.borrow_mut().pop();
        result
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Values<'a> {
    macros: &'a Macros,
    scope: Option<&'a Bindings>,
    target_assignment: bool,
}

impl<'a> Values<'a> {
    fn get(self, name: &str) -> Option<Definition> {
        for scope in self.macros.locals.borrow().iter().rev() {
            if let Some(definition) = scope.variables.get(name) {
                return Some(definition.clone());
            }
            if scope.call && name.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
        }
        self.scope
            .and_then(|scope| scope.get(name))
            .cloned()
            .or_else(|| self.macros.definitions.borrow().get(name).cloned())
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
        if previous.as_ref().is_some_and(|previous| {
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
            previous.as_ref()
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
        shell.arg("-c");
        self.set_recipe_environment(&mut shell, location, &Automatic::default())?;
        let output = self.run_shell(shell, command, location)?;
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
        self.set_environment(command, location, automatic, &[])
    }

    fn set_environment(
        self,
        command: &mut Command,
        location: &Location,
        automatic: &Automatic<'_>,
        active: &[String],
    ) -> Result<(), Error> {
        if !self.macros.gnu_environment.get() {
            let flags = self.expand("$(MAKEFLAGS)", location, automatic)?;
            command.env("MAKEFLAGS", flags);
            command.envs(
                self.macros
                    .definitions
                    .borrow()
                    .iter()
                    .filter_map(|(name, definition)| {
                        (definition.origin == Origin::CommandLine
                            && !matches!(name.as_str(), "SHELL" | "MAKEFLAGS"))
                        .then_some((name.as_str(), definition.value.as_str()))
                    }),
            );
            return Ok(());
        }

        let exports = self.macros.exports.borrow().clone();
        for (name, export) in &exports {
            if !export {
                command.env_remove(name);
            }
        }
        let names = {
            let definitions = self.macros.definitions.borrow();
            definitions
                .keys()
                .chain(
                    self.scope
                        .into_iter()
                        .flat_map(|scope| scope.keys())
                        .filter(|name| !definitions.contains_key(*name)),
                )
                .cloned()
                .collect::<Vec<_>>()
        };
        for name in &names {
            let Some(definition) = self.get(name) else {
                continue;
            };
            let policy = definition
                .export
                .or_else(|| exports.get(name).copied());
            if policy == Some(false) {
                command.env_remove(name);
                continue;
            }
            let by_default = matches!(name.as_str(), "MAKEFLAGS" | "MAKEFILES")
                || (name != "SHELL"
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    && (self.macros.environment.contains_key(name)
                        || matches!(
                            definition.origin,
                            Origin::Environment | Origin::Makeflags | Origin::CommandLine
                        )
                        || (self.macros.export_all.get() && definition.origin != Origin::Default)));
            if policy == Some(true) || by_default {
                let value = if active.contains(name)
                    || !self.macros.exporting.borrow_mut().insert(name.clone())
                {
                    self.macros.environment.get(name).cloned().unwrap_or_default()
                } else {
                    let result = Expansion::new(self, automatic, false).value(name, location, 0);
                    self.macros.exporting.borrow_mut().remove(name);
                    result?
                };
                command.env(name, value);
            }
        }
        Ok(())
    }

    fn run_shell(
        self,
        mut shell: Command,
        command: &str,
        location: &Location,
    ) -> Result<std::process::Output, Error> {
        let output = shell
            .arg(command)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|error| {
                Error::new(
                    location,
                    format!(
                        "could not run {}: {error}",
                        shell.get_program().to_string_lossy()
                    ),
                )
            })?;
        let status = output
            .status
            .code()
            .or_else(|| output.status.signal().map(|signal| 128 + signal))
            .unwrap_or(127);
        self.macros.define(
            ".SHELLSTATUS",
            status.to_string(),
            Flavor::Immediate,
            location,
            Origin::Override,
        );
        Ok(output)
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
    active: Vec<String>,
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
                let Some((end, _)) =
                    reference_level(text, Some(close)).find(|&(_, ch)| ch == close)
                else {
                    return Err(Error::new(
                        location,
                        format!("missing '{close}' in macro reference"),
                    ));
                };
                expanded.push_str(&self.reference(&text[..end], close, location, depth + 1)?);
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
        close: char,
        location: &Location,
        depth: usize,
    ) -> Result<String, Error> {
        if let Some((name, arguments)) = reference.split_once([' ', '\t']) {
            if let Some((_, max)) = function_arity(name) {
                let arguments = arguments.trim_start_matches([' ', '\t']);
                let mut start = 0;
                let mut args = Vec::new();
                for (end, _) in reference_level(arguments, Some(close))
                    .filter(|&(_, ch)| ch == ',')
                    .take(max - 1)
                {
                    args.push(&arguments[start..end]);
                    start = end + 1;
                }
                args.push(&arguments[start..]);
                return self.function(name, &args, location, depth, false);
            }
            if crate::inventory::FUNCTIONS.contains(&name) {
                return Err(Error::new(
                    location,
                    format!("unsupported GNU make function '{name}'"),
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

    fn function(
        &mut self,
        name: &str,
        args: &[&str],
        location: &Location,
        depth: usize,
        expanded: bool,
    ) -> Result<String, Error> {
        let (min, max) = function_arity(name).ok_or_else(|| {
            Error::new(location, format!("unsupported GNU make function '{name}'"))
        })?;
        if args.len() < min {
            return Err(Error::new(
                location,
                format!("too few arguments to '{name}'"),
            ));
        }
        let args = &args[..args.len().min(max)];
        let expand = |this: &mut Self, arg: &str| {
            if expanded {
                Ok(arg.to_owned())
            } else {
                this.text(arg, location, depth)
            }
        };
        match name {
            "if" => {
                let condition = expand(self, args[0].trim())?;
                let branch = if condition.is_empty() {
                    args.get(2)
                } else {
                    args.get(1)
                };
                branch.map_or_else(|| Ok(String::new()), |arg| expand(self, arg))
            }
            "and" | "or" => {
                let mut value = String::new();
                for arg in args {
                    value = expand(self, arg)?;
                    if value.is_empty() == (name == "and") {
                        break;
                    }
                }
                Ok(value)
            }
            "foreach" => {
                let variable = expand(self, args[0])?;
                let variable = separate_words(&variable).next().unwrap_or_default();
                let list = expand(self, args[1])?;
                let mut results = Vec::new();
                for word in separate_words(&list) {
                    let mut scope = LocalScope::default();
                    scope.bind(variable.into(), word.into(), location);
                    results.push(
                        self.values
                            .macros
                            .with_locals(scope, || expand(self, args[2]))?,
                    );
                }
                Ok(results.join(" "))
            }
            "call" => {
                let args = args
                    .iter()
                    .map(|arg| expand(self, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let function = separate_words(&args[0]).next().unwrap_or_default();
                if function.is_empty() {
                    return Ok(String::new());
                }
                if function_arity(function).is_some()
                    || crate::inventory::FUNCTIONS.contains(&function)
                {
                    let args = args[1..].iter().map(String::as_str).collect::<Vec<_>>();
                    return self.function(function, &args, location, depth + 1, true);
                }
                let mut scope = LocalScope {
                    call: true,
                    ..LocalScope::default()
                };
                scope.bind("0".into(), function.into(), location);
                for (index, arg) in args[1..].iter().enumerate() {
                    scope.bind((index + 1).to_string(), arg.clone(), location);
                }
                self.values.macros.with_locals(scope, || {
                    self.expand_variable(function, location, depth + 1, true)
                })
            }
            "eval" => {
                let text = expand(self, args[0])?;
                self.values.macros.eval(&text, location)?;
                Ok(String::new())
            }
            "value" | "origin" | "flavor" => {
                let variable = expand(self, args[0])?;
                Ok(self.describe(name, &variable))
            }
            "error" | "warning" | "info" => {
                let text = expand(self, args[0])?;
                match name {
                    "error" => return Err(Error::new(location, text)),
                    "warning" => writeln!(io::stderr().lock(), "{location}: {text}"),
                    _ => writeln!(io::stdout().lock(), "{text}"),
                }
                .map_err(|error| Error::new(location, format!("{name}: {error}")))?;
                Ok(String::new())
            }
            "shell" => {
                let command = expand(self, args[0])?;
                self.shell(&command, location, depth)
            }
            "file" => {
                let operand = expand(self, args[0])?;
                let text = args.get(1).map(|arg| expand(self, arg)).transpose()?;
                make_file(&operand, text.as_deref(), location)
            }
            _ => {
                let values = args
                    .iter()
                    .map(|arg| expand(self, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                make_functions::evaluate(name, &values, location)
            }
        }
    }

    fn describe(&self, function: &str, name: &str) -> String {
        if !self.automatic.target.is_empty() {
            if let Some(value) = self.automatic.get(name) {
                return match function {
                    "value" => value,
                    "origin" => "automatic".into(),
                    _ => "simple".into(),
                };
            }
        }
        let Some(definition) = self.values.get(name) else {
            return if function == "value" { "" } else { "undefined" }.into();
        };
        match function {
            "value" => return definition.value,
            "flavor" => match definition.flavor {
                Flavor::Immediate => "simple",
                Flavor::Delayed => "recursive",
            },
            _ => match definition.origin {
                Origin::Default => "default",
                Origin::Environment if self.values.macros.environment_overrides => {
                    "environment override"
                }
                Origin::Environment => "environment",
                Origin::Makefile => "file",
                Origin::Makeflags | Origin::CommandLine => "command line",
                Origin::Override => "override",
                Origin::Automatic => "automatic",
            },
        }
        .into()
    }

    fn shell(&mut self, command: &str, location: &Location, depth: usize) -> Result<String, Error> {
        if command.trim().is_empty() {
            return Ok(String::new());
        }
        let executable = self.value("SHELL", location, depth + 1)?;
        let flags = self.value(".SHELLFLAGS", location, depth + 1)?;
        let mut shell = Command::new(executable);
        if self.values.get(".SHELLFLAGS").is_some() {
            shell.args(separate_words(&flags));
        } else {
            shell.arg("-c");
        }
        self.values
            .set_environment(&mut shell, location, self.automatic, &self.active)?;
        let output = self.values.run_shell(shell, command, location)?;
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| Error::new(location, format!("shell output is not UTF-8: {error}")))?;
        let mut text = stdout.as_str();
        while let Some(prefix) = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
        {
            text = prefix;
        }
        Ok(text.replace("\r\n", " ").replace('\n', " "))
    }

    fn value(&mut self, name: &str, location: &Location, depth: usize) -> Result<String, Error> {
        self.expand_variable(name, location, depth, false)
    }

    fn expand_variable(
        &mut self,
        name: &str,
        location: &Location,
        depth: usize,
        via_call: bool,
    ) -> Result<String, Error> {
        self.expanded_make |= name == "MAKE";
        if let Some(value) = self.automatic.get(name) {
            return Ok(value);
        }
        let Some(definition) = self.values.get(name) else {
            return Ok(String::new());
        };
        if !via_call {
            if let Some(start) = self.active.iter().position(|active| active == name) {
                let mut cycle = self.active[start..].to_vec();
                cycle.push(name.to_owned());
                return Err(Error::new(
                    location,
                    format!("recursive macro reference: {}", cycle.join(" -> ")),
                ));
            }
        }
        self.active.push(name.to_owned());
        let mut value = String::new();
        let result = self.expand_definition(name, &definition, &mut value, depth);
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
        if !definition.append {
            return self.expand_fragment(definition, result, depth);
        }
        let global = self.values.macros.definitions.borrow().get(name).cloned();
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
                .or(global.as_ref());
        }
        for definition in appends.into_iter().rev() {
            if !result.is_empty() && !definition.value.is_empty() {
                result.push(' ');
            }
            self.expand_fragment(definition, result, depth)?;
        }
        Ok(())
    }

    fn expand_fragment(
        &mut self,
        definition: &Definition,
        result: &mut String,
        depth: usize,
    ) -> Result<(), Error> {
        if definition.flavor == Flavor::Immediate {
            result.push_str(&definition.value);
        } else {
            result.push_str(&self.text(&definition.value, &definition.location, depth)?);
        }
        Ok(())
    }
}

impl LocalScope {
    fn bind(&mut self, name: String, value: String, location: &Location) {
        self.variables.insert(
            name,
            Definition {
                value,
                flavor: Flavor::Immediate,
                location: location.clone(),
                origin: Origin::Automatic,
                export: None,
                private: false,
                append: false,
                parent: None,
            },
        );
    }
}

fn function_arity(name: &str) -> Option<(usize, usize)> {
    Some(match name {
        "if" => (2, 3),
        "foreach" => (3, 3),
        "file" => (1, 2),
        "and" | "or" | "call" => (1, usize::MAX),
        "eval" | "value" | "origin" | "flavor" | "shell" | "error" | "warning" | "info" => (1, 1),
        _ => {
            let arity = make_functions::arity(name)?;
            (arity, arity)
        }
    })
}

// GNU make 4.4.1 §8.7. A missing input reads as empty; other I/O errors are fatal.
fn make_file(operand: &str, text: Option<&str>, location: &Location) -> Result<String, Error> {
    let operand = operand.trim();
    let error = |error| Error::new(location, format!("file '{operand}': {error}"));
    if let Some(path) = operand.strip_prefix('<') {
        if text.is_some() {
            return Err(Error::new(
                location,
                "file read does not take a text argument",
            ));
        }
        return match fs::read_to_string(path.trim()) {
            Ok(mut contents) => {
                if contents.ends_with('\n') {
                    contents.pop();
                    if contents.ends_with('\r') {
                        contents.pop();
                    }
                }
                Ok(contents)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(reason) => Err(error(reason)),
        };
    }
    let (path, append) = if let Some(path) = operand.strip_prefix(">>") {
        (path, true)
    } else if let Some(path) = operand.strip_prefix('>') {
        (path, false)
    } else {
        return Err(Error::new(
            location,
            format!("invalid file operation '{operand}'"),
        ));
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(path.trim())
        .map_err(error)?;
    if let Some(text) = text {
        file.write_all(text.as_bytes()).map_err(error)?;
        if !text.ends_with('\n') {
            file.write_all(b"\n").map_err(error)?;
        }
    }
    Ok(String::new())
}

pub(crate) fn separate_words(text: &str) -> impl Iterator<Item = &str> {
    text.split([' ', '\t', '\n'])
        .filter(|word| !word.is_empty())
}

fn substitute(value: &str, old: &str, new: &str) -> String {
    let pattern = Pattern::parse(old);
    if pattern.has_stem() {
        return make_functions::patsubst(&pattern, new, value);
    }
    let mut result = String::new();
    for part in value.split_inclusive([' ', '\t', '\n']) {
        let word = part.trim_end_matches([' ', '\t', '\n']);
        if !word.is_empty() {
            if let Some(stem) = word.strip_suffix(old) {
                result.push_str(stem);
                result.push_str(new);
            } else {
                result.push_str(word);
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
