//! Logical lines and target rules from POSIX.1-2024 make, "Makefile Syntax",
//! "Target Rules", "Macros" and "Include Lines".

use crate::{
    graph::archive_member,
    inference::static_rules,
    r#macro::{separate_words, Assignment, Automatic, Macros, Modifiers},
    Error, Location,
};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    str,
};

const MAX_INCLUDE_DEPTH: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct Makefile {
    pub entries: Vec<Entry>,
    pub(crate) includes: Vec<IncludeFile>,
}

#[derive(Clone, Debug)]
pub(crate) struct IncludeFile {
    pub name: String,
    pub location: Location,
    pub optional: bool,
    pub contents: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub enum Entry {
    Rule(Rule),
    Macro(SourceLine),
    Expansion(SourceLine),
    Include {
        paths: SourceLine,
        optional: bool,
    },
    Export {
        names: SourceLine,
        export: bool,
    },
    Variable {
        targets: Vec<String>,
        name: SourceLine,
        operator: String,
        value: String,
        modifiers: Modifiers,
    },
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub location: Location,
    pub targets: Vec<String>,
    pub prerequisites: Vec<String>,
    pub order_only: Vec<String>,
    pub recipes: Vec<SourceLine>,
    pub(crate) pattern: Option<String>,
    pub(crate) terminal: bool,
    pub(crate) inference: Option<Inference>,
}

#[derive(Clone, Debug)]
pub(crate) struct Inference {
    pub source: String,
    pub stem: String,
    pub implicit: bool,
}

#[derive(Clone, Debug)]
pub struct SourceLine {
    pub location: Location,
    pub text: String,
}

/// Read makefiles in order, inserting includes where their directives appear.
pub fn load(paths: &[PathBuf], macros: &mut Macros) -> Result<Makefile, Error> {
    reload(paths, macros, &mut HashMap::new())
}

pub(crate) fn reload(
    paths: &[PathBuf],
    macros: &mut Macros,
    streams: &mut HashMap<PathBuf, Vec<u8>>,
) -> Result<Makefile, Error> {
    macros.set_reading(true);
    let mut makefile = Makefile::default();
    for path in paths {
        let location = Location {
            path: path.as_path().into(),
            line: 1,
        };
        if let Some(source) = streams.get(path) {
            makefile.read_entries(&location, source, macros, 0)?;
            continue;
        }
        let (source, stream) = if path == Path::new("-") {
            let mut bytes = Vec::new();
            io::stdin()
                .read_to_end(&mut bytes)
                .map_err(|error| Error::new(&location, error.to_string()))?;
            (bytes, true)
        } else {
            let file =
                File::open(path).map_err(|error| Error::new(&location, error.to_string()))?;
            let stream = !file
                .metadata()
                .map_err(|error| Error::new(&location, error.to_string()))?
                .is_file();
            (read_source(file, &location)?, stream)
        };
        makefile.read_entries(&location, &source, macros, 0)?;
        if stream {
            streams.insert(path.clone(), source);
        }
    }
    macros.set_reading(false);
    Ok(makefile)
}

pub(crate) fn check_target(name: &str, location: &Location) -> Result<(), Error> {
    if name.contains(['(', ')']) && archive_member(name).is_none() {
        return Err(Error::new(
            location,
            format!("invalid archive member '{name}'"),
        ));
    }
    Ok(())
}

fn read_source(mut file: File, location: &Location) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| Error::new(location, error.to_string()))?;
    Ok(bytes)
}

fn open_include(name: &str, directories: &[PathBuf]) -> io::Result<Option<(PathBuf, File)>> {
    let direct = Path::new(name);
    let search = directories
        .iter()
        .filter(|_| direct.is_relative())
        .map(|dir| dir.join(name));
    for path in std::iter::once(direct.to_owned()).chain(search) {
        match File::open(&path) {
            Ok(file) => return Ok(Some((path, file))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

impl Makefile {
    pub(crate) fn append(&mut self, mut other: Self) {
        self.entries.append(&mut other.entries);
        self.includes.append(&mut other.includes);
    }

    pub(crate) fn read_entries(
        &mut self,
        location: &Location,
        bytes: &[u8],
        macros: &Macros,
        depth: usize,
    ) -> Result<(), Error> {
        let mut source = SourceReader::new(location.clone(), bytes)?;
        let mut leading_posix = depth == 0;
        let automatic = Automatic::default();
        while let Some(mut entry) = source.next(macros, leading_posix)? {
            leading_posix = false;
            self.append(macros.take_eval());
            if let Entry::Expansion(line) = entry {
                let expanded = macros.expand(&line.text, &line.location, &automatic)?;
                self.append(macros.take_eval());
                if expanded.trim_matches(is_blank).is_empty() {
                    continue;
                }
                entry = read_entry(&expanded, line.location.clone())?
                    .filter(|entry| !matches!(entry, Entry::Expansion(_)))
                    .ok_or_else(|| Error::new(&line.location, "missing ':' in target rule"))?;
            }
            match entry {
                Entry::Expansion(_) => unreachable!(),
                Entry::Macro(line) => macros.assign_makefile(&line.text, &line.location)?,
                Entry::Export { names, export } => {
                    macros.set_export(&names.text, export, &names.location)?;
                }
                Entry::Variable {
                    mut targets,
                    name,
                    operator,
                    value,
                    modifiers,
                } => {
                    let global = targets.is_empty();
                    expand_words(&mut targets, macros, &name.location)?;
                    let assignment = Assignment {
                        name: &name.text,
                        operator: &operator,
                        value: &value,
                    };
                    if global {
                        macros.assign_directive(None, assignment, modifiers, &name.location)?;
                    } else {
                        for target in &targets {
                            macros.assign_directive(
                                Some(target),
                                assignment,
                                modifiers,
                                &name.location,
                            )?;
                        }
                    }
                }
                Entry::Rule(mut rule) => {
                    for words in [
                        &mut rule.targets,
                        &mut rule.prerequisites,
                        &mut rule.order_only,
                    ] {
                        expand_words(words, macros, &rule.location)?;
                    }
                    self.append(macros.take_eval());
                    if rule.targets.is_empty() {
                        continue;
                    }
                    if let Some(pattern) = rule.pattern.take() {
                        let pattern = macros.expand(&pattern, &rule.location, &automatic)?;
                        self.append(macros.take_eval());
                        self.entries
                            .extend(static_rules(rule, &pattern)?.map(Entry::Rule));
                        continue;
                    }
                    self.entries.push(Entry::Rule(rule));
                }
                Entry::Include { paths, optional } => {
                    let expanded = macros.expand(&paths.text, &paths.location, &automatic)?;
                    self.append(macros.take_eval());
                    for name in separate_words(&expanded) {
                        check_target(name, &paths.location)?;
                        let file = match open_include(name, &macros.include_dirs) {
                            Ok(file) => file,
                            Err(_) if optional => continue,
                            Err(error) => {
                                return Err(Error::new(
                                    &paths.location,
                                    format!("include '{name}': {error}"),
                                ))
                            }
                        };
                        let index = self.includes.len();
                        self.includes.push(IncludeFile {
                            name: file.as_ref().map_or_else(
                                || name.to_owned(), |(path, _)| path.to_string_lossy().into_owned(),
                            ),
                            location: paths.location.clone(),
                            optional,
                            contents: None,
                        });
                        let Some((path, file)) = file else {
                            continue;
                        };
                        if depth >= MAX_INCLUDE_DEPTH {
                            return Err(Error::new(
                                &paths.location,
                                format!("include nesting is too deep at '{name}'"),
                            ));
                        }
                        let location = Location {
                            path: path.as_path().into(),
                            line: 1,
                        };
                        let source = read_source(file, &location)?;
                        self.read_entries(&location, &source, macros, depth + 1)?;
                        self.includes[index].contents = Some(source);
                    }
                }
            }
        }
        self.append(macros.take_eval());
        Ok(())
    }
}

fn expand_words(
    words: &mut Vec<String>,
    macros: &Macros,
    location: &Location,
) -> Result<(), Error> {
    let mut expanded = Vec::new();
    for word in words.iter() {
        let value = macros.expand(word, location, &Automatic::default())?;
        for word in separate_words(&value) {
            check_target(word, location)?;
            expanded.push(word.to_owned());
        }
    }
    *words = expanded;
    Ok(())
}

/// Read unexpanded entries from a makefile without opening includes.
pub fn parse(path: impl AsRef<Path>, source: &[u8]) -> Result<Makefile, Error> {
    let location = Location {
        path: path.as_ref().into(),
        line: 1,
    };
    let mut source = SourceReader::new(location, source)?;
    let macros = Macros::default();
    let mut makefile = Makefile::default();
    while let Some(entry) = source.next(&macros, false)? {
        makefile.entries.push(entry);
    }
    Ok(makefile)
}

struct SourceReader<'a> {
    location: Location,
    remaining: &'a str,
    pending: Option<Entry>,
    conditionals: Vec<Conditional>,
}

impl<'a> SourceReader<'a> {
    fn new(mut location: Location, source: &'a [u8]) -> Result<Self, Error> {
        let remaining = str::from_utf8(source).map_err(|error| {
            location.line += source[..error.valid_up_to()]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count();
            Error::new(&location, "invalid UTF-8")
        })?;
        Ok(Self {
            location,
            remaining,
            pending: None,
            conditionals: Vec::new(),
        })
    }

    fn line(&mut self) -> Result<(&'a str, Location), Error> {
        let location = self.location.clone();
        let line = take_logical_line(&mut self.remaining, &mut self.location)?;
        Ok((line, location))
    }

    fn active(&self) -> bool {
        self.conditionals.last().is_none_or(|frame| frame.active)
    }

    fn next(&mut self, macros: &Macros, leading_posix: bool) -> Result<Option<Entry>, Error> {
        if self.pending.is_some() && !matches!(self.pending, Some(Entry::Rule(_))) {
            return Ok(self.pending.take());
        }
        while !self.remaining.is_empty() {
            let (line, location) = self.line()?;
            if let Some(recipe) = line.strip_prefix('\t') {
                if self.active() {
                    if let Some(Entry::Rule(rule)) = &mut self.pending {
                        rule.recipes.push(recipe_line(location, recipe));
                    } else if !recipe.trim_matches(is_blank).is_empty() {
                        return Err(Error::new(&location, "recipe before any rule"));
                    }
                }
                continue;
            }

            let header = directive_header(line);
            let (keyword, args) = directive_word(&header);
            if self.conditional(keyword, args, &location, macros)? {
                continue;
            }
            let (modifiers, statement) = split_modifiers(&header);
            let (word, args) = directive_word(statement);
            let entry = if word == "define" {
                let value = self.define_body(&location)?;
                if !self.active() {
                    continue;
                }
                let assignment = Assignment::parse(args).unwrap_or(Assignment {
                    name: args,
                    operator: "",
                    value: "",
                });
                if !assignment.value.is_empty() {
                    return Err(Error::new(
                        &location,
                        "unexpected text after define operator",
                    ));
                }
                Some(variable_entry(
                    Vec::new(),
                    assignment,
                    value,
                    modifiers,
                    location,
                ))
            } else if !self.active() {
                continue;
            } else if word == "endef" {
                return Err(Error::new(&location, "endef without define"));
            } else if modifiers != Modifiers::default() {
                Some(modified_entry(statement, modifiers, location)?)
            } else {
                read_entry(line, location)?
            };
            if let Some(entry) = entry {
                if leading_posix
                    && self.pending.is_none()
                    && matches!(&entry, Entry::Rule(rule) if rule.targets == [".POSIX"])
                {
                    macros.use_posix_compiler_defaults();
                }
                if let Some(previous) = self.pending.replace(entry) {
                    return Ok(Some(previous));
                }
                if !matches!(self.pending, Some(Entry::Rule(_))) {
                    return Ok(self.pending.take());
                }
            }
        }

        if let Some(frame) = self.conditionals.last() {
            return Err(Error::new(&frame.location, "missing endif"));
        }
        Ok(self.pending.take())
    }

    fn define_body(&mut self, definition: &Location) -> Result<String, Error> {
        let mut value = String::new();
        let mut depth = 1;
        while !self.remaining.is_empty() {
            let (line, location) = self.line()?;
            if !line.starts_with('\t') {
                let header = directive_header(line);
                let (_, statement) = split_modifiers(&header);
                match directive_word(statement) {
                    ("define", _) => depth += 1,
                    ("endef", "") => {
                        depth -= 1;
                        if depth == 0 {
                            value.pop(); // Drop the last body's newline.
                            return Ok(value);
                        }
                    }
                    ("endef", _) => {
                        return Err(Error::new(&location, "unexpected text after endef"))
                    }
                    _ => (),
                }
            }
            value.push_str(&fold_header(line));
            value.push('\n');
        }
        Err(Error::new(definition, "missing endef"))
    }

    // GNU make 4.4.1 §7.2. Each source (including an include) owns its stack.
    fn conditional(
        &mut self,
        keyword: &str,
        args: &str,
        location: &Location,
        macros: &Macros,
    ) -> Result<bool, Error> {
        match keyword {
            "else" => {
                let frame = self
                    .conditionals
                    .last_mut()
                    .ok_or_else(|| Error::new(location, "else without conditional"))?;
                if frame.final_else {
                    return Err(Error::new(location, "extra else"));
                }
                frame.final_else = args.is_empty();
                let (keyword, args) = directive_word(args);
                if !frame.final_else && !is_condition(keyword) {
                    return Err(Error::new(location, "expected conditional after else"));
                }
                frame.active = frame.parent_active
                    && !frame.matched
                    && (frame.final_else || evaluate_condition(keyword, args, location, macros)?);
                frame.matched |= frame.active;
            }
            "endif" => {
                if !args.is_empty() {
                    return Err(Error::new(location, "unexpected text after endif"));
                }
                self.conditionals
                    .pop()
                    .ok_or_else(|| Error::new(location, "endif without conditional"))?;
            }
            _ if is_condition(keyword) => {
                let parent_active = self.active();
                let active = parent_active && evaluate_condition(keyword, args, location, macros)?;
                self.conditionals.push(Conditional {
                    location: location.clone(),
                    parent_active,
                    active,
                    matched: active,
                    final_else: false,
                });
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

struct Conditional {
    location: Location,
    parent_active: bool,
    active: bool,
    matched: bool,
    final_else: bool,
}

fn is_condition(word: &str) -> bool {
    matches!(word, "ifeq" | "ifneq" | "ifdef" | "ifndef")
}

fn evaluate_condition(
    keyword: &str,
    args: &str,
    location: &Location,
    macros: &Macros,
) -> Result<bool, Error> {
    let automatic = Automatic::default();
    if matches!(keyword, "ifdef" | "ifndef") {
        let name = macros.expand(args, location, &automatic)?;
        let name = name.trim_matches(is_blank);
        if args.is_empty() || name.contains(is_blank) {
            return Err(Error::new(
                location,
                format!("expected one variable name after {keyword}"),
            ));
        }
        return Ok(macros.has_value(name) == (keyword == "ifdef"));
    }
    let (left, right) = comparison_arguments(args)
        .ok_or_else(|| Error::new(location, format!("invalid {keyword} arguments")))?;
    let left = macros.expand(left, location, &automatic)?;
    let right = macros.expand(right, location, &automatic)?;
    Ok((left == right) == (keyword == "ifeq"))
}

fn comparison_arguments(args: &str) -> Option<(&str, &str)> {
    if let Some(args) = args.strip_prefix('(') {
        let mut nesting = 0;
        let mut comma = None;
        for (index, character) in outside_macro_references(args) {
            match character {
                '(' => nesting += 1,
                ')' if nesting != 0 => nesting -= 1,
                ')' if args[index + 1..].trim_matches(is_blank).is_empty() => {
                    let comma = comma?;
                    return Some((
                        args[..comma].trim_end_matches(is_blank),
                        args[comma + 1..index].trim_start_matches(is_blank),
                    ));
                }
                ')' => return None,
                ',' if nesting == 0 && comma.is_none() => comma = Some(index),
                _ => (),
            }
        }
        return None;
    }
    fn quoted(text: &str) -> Option<(&str, &str)> {
        let text = text.trim_start_matches(is_blank);
        let quote = text.chars().next().filter(|ch| matches!(ch, '\'' | '"'))?;
        let text = &text[1..];
        let (end, _) = outside_macro_references(text).find(|&(_, ch)| ch == quote)?;
        Some((&text[..end], &text[end + 1..]))
    }
    let (left, rest) = quoted(args)?;
    let (right, rest) = quoted(rest)?;
    rest.trim_matches(is_blank)
        .is_empty()
        .then_some((left, right))
}

fn directive_header(line: &str) -> String {
    let comment = outside_macro_references(line).find(|&(_, ch)| ch == '#');
    fold_header(&line[..comment.map_or(line.len(), |(at, _)| at)])
}

fn directive_word(text: &str) -> (&str, &str) {
    let text = text.trim_start_matches(is_blank);
    let (head, rest) = text.split_at(text.find(is_blank).unwrap_or(text.len()));
    let rest = rest.trim_start_matches(is_blank);
    // The keyword can also be an ordinary variable or target name.
    if rest.starts_with(['=', ':']) || assignment_operator(rest) {
        ("", text)
    } else {
        (head, rest)
    }
}

fn assignment_operator(text: &str) -> bool {
    ["=", ":=", "::=", ":::=", "+=", "?=", "!="]
        .iter()
        .any(|operator| text.starts_with(operator))
}

fn split_modifiers(mut text: &str) -> (Modifiers, &str) {
    let mut modifiers = Modifiers::default();
    loop {
        let (word, rest) = directive_word(text);
        match word {
            "override" => modifiers.overriding = true,
            "export" => modifiers.export = Some(true),
            "unexport" => modifiers.export = Some(false),
            "private" => modifiers.private = true,
            _ => return (modifiers, text),
        }
        text = rest;
    }
}

fn variable_entry(
    targets: Vec<String>,
    assignment: Assignment<'_>,
    value: String,
    modifiers: Modifiers,
    location: Location,
) -> Entry {
    Entry::Variable {
        targets,
        name: SourceLine {
            text: assignment.name.to_owned(),
            location,
        },
        operator: assignment.operator.to_owned(),
        value,
        modifiers,
    }
}

fn modified_entry(text: &str, modifiers: Modifiers, location: Location) -> Result<Entry, Error> {
    if let Some(assignment) = Assignment::parse(text) {
        Ok(variable_entry(
            Vec::new(),
            assignment,
            assignment.value.to_owned(),
            modifiers,
            location,
        ))
    } else if let Some(export) = modifiers
        .export
        .filter(|_| !modifiers.overriding && !modifiers.private)
    {
        Ok(Entry::Export {
            names: SourceLine {
                location,
                text: text.trim_matches(is_blank).to_owned(),
            },
            export,
        })
    } else {
        Err(Error::new(
            &location,
            "expected variable assignment after modifier",
        ))
    }
}

fn take_logical_line<'a>(
    remaining: &mut &'a str,
    location: &mut Location,
) -> Result<&'a str, Error> {
    let start = *remaining;
    let starting_location = location.clone();
    loop {
        let (line, rest) = remaining
            .split_once('\n')
            .ok_or_else(|| Error::new(location, "missing newline at end of makefile"))?;
        if line.contains('\0') {
            return Err(Error::new(location, "NUL byte in makefile"));
        }
        *remaining = rest;
        location.line += 1;

        if !line.ends_with('\\') {
            return Ok(&start[..start.len() - rest.len() - 1]);
        }
        if rest.is_empty() {
            return Err(Error::new(&starting_location, "unfinished continuation"));
        }
    }
}

fn read_entry(line: &str, location: Location) -> Result<Option<Entry>, Error> {
    let header = directive_header(line);
    let (keyword, paths) = directive_word(&header);
    if matches!(keyword, "include" | "-include" | "sinclude") {
        return Ok(Some(Entry::Include {
            paths: SourceLine {
                text: paths.to_owned(),
                location,
            },
            optional: keyword != "include",
        }));
    }

    let mut colon = None;
    let mut assignment = false;
    let mut end = line.len();
    let mut recipe = None;

    for (index, character) in outside_macro_references(line) {
        match character {
            '#' => {
                end = index;
                break;
            }
            '=' if colon.is_none() => assignment = true,
            ':' if colon.is_none() && !assignment => {
                // The colon characters in these assignment operators belong to the macro.
                if assignment_operator(&line[index..]) {
                    assignment = true;
                } else {
                    colon = Some(index);
                }
            }
            ';' if colon.is_some() => {
                recipe = Some(&line[index + 1..]);
                end = index;
                break;
            }
            _ => (),
        }
    }

    if let Some(colon) = colon {
        let targets = words(&fold_header(&line[..colon]));
        if targets.is_empty() {
            return Err(Error::new(&location, "missing target before ':'"));
        }
        let terminal = line[colon + 1..end].starts_with(':');
        let contents = fold_header(&line[colon + 1 + usize::from(terminal)..end]);
        let (modifiers, text) = split_modifiers(&contents);
        if let Some(assignment) = Assignment::parse(text).filter(|assignment| {
            let name = assignment.name.trim_matches(is_blank);
            !name.is_empty()
                && !outside_macro_references(name)
                    .any(|(_, ch)| is_blank(ch) || matches!(ch, ':' | '|'))
        }) {
            let value = match recipe {
                Some(inline) => format!("{};{inline}", assignment.value),
                None => assignment.value.to_owned(),
            };
            return Ok(Some(variable_entry(
                targets, assignment, value, modifiers, location,
            )));
        }
        let (pattern, contents) =
            match outside_macro_references(&contents).find(|&(_, ch)| ch == ':') {
                Some((at, _)) => (Some(contents[..at].trim().to_owned()), &contents[at + 1..]),
                None => (None, contents.as_str()),
            };
        let separator = outside_macro_references(contents).find(|&(_, ch)| ch == '|');
        let (prerequisites, order_only) = match separator {
            Some((at, _)) => (words(&contents[..at]), words(&contents[at + 1..])),
            None => (words(contents), Vec::new()),
        };
        let recipes = recipe
            .map(|text| recipe_line(location.clone(), text))
            .into_iter()
            .collect();
        Ok(Some(Entry::Rule(Rule {
            location,
            targets,
            prerequisites,
            order_only,
            recipes,
            pattern,
            terminal,
            inference: None,
        })))
    } else {
        let text = fold_header(&line[..end]);
        if assignment {
            Ok(Some(Entry::Macro(SourceLine { location, text })))
        } else if text.trim_matches(is_blank).is_empty() {
            Ok(None)
        } else if text.contains('$') {
            Ok(Some(Entry::Expansion(SourceLine { location, text })))
        } else {
            Err(Error::new(&location, "missing ':' in target rule"))
        }
    }
}

fn recipe_line(location: Location, text: &str) -> SourceLine {
    // Preserve the shell's escaped newline, dropping one leading recipe tab.
    SourceLine {
        location,
        text: text.replace("\\\n\t", "\\\n"),
    }
}

fn fold_header(text: &str) -> String {
    let mut physical_lines = text.split("\\\n");
    let mut folded = physical_lines.next().unwrap_or_default().to_owned();
    for line in physical_lines {
        folded.push(' ');
        folded.push_str(line.trim_start_matches(is_blank));
    }
    folded
}

fn is_blank(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut start = 0;
    for (index, _) in outside_macro_references(text).filter(|&(_, character)| is_blank(character)) {
        if start < index {
            words.push(text[start..index].to_owned());
        }
        start = index + 1;
    }
    if start < text.len() {
        words.push(text[start..].to_owned());
    }
    words
}

// Unexpanded references can contain colons, equals signs and blanks, including
// in a substitution or a nested reference. Comments still end the header.
pub(crate) fn outside_macro_references(text: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    reference_level(text, None)
}

// With a function delimiter, report separators and the closing delimiter at
// that level. Raw matching pairs inside a function can protect commas too.
pub(crate) fn reference_level(
    text: &str,
    delimiter: Option<char>,
) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut characters = text.char_indices().peekable();
    let mut closing = Vec::new();
    std::iter::from_fn(move || {
        while let Some((index, character)) = characters.next() {
            if character == '$' {
                match characters.peek().map(|&(_, next)| next) {
                    Some('(') => closing.push(')'),
                    Some('{') => closing.push('}'),
                    Some('$') => (),
                    _ => continue,
                }
                characters.next();
            } else if closing.last() == Some(&character) {
                closing.pop();
            } else if let Some(close) = closing
                .last()
                .copied()
                .or(delimiter)
                .filter(|&close| matches!((character, close), ('(', ')') | ('{', '}')))
            {
                closing.push(close);
            } else if closing.is_empty() {
                return Some((index, character));
            }
        }
        None
    })
}
