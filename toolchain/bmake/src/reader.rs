//! Logical lines and target rules from POSIX.1-2024 make, "Makefile Syntax",
//! "Target Rules", "Macros" and "Include Lines".

use crate::{
    r#macro::{separate_words, Automatic, Macros},
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

#[derive(Debug, Default)]
pub struct Makefile {
    pub entries: Vec<Entry>,
    pub(crate) includes: Vec<IncludeFile>,
}

#[derive(Debug)]
pub(crate) struct IncludeFile {
    pub name: String,
    pub location: Location,
    pub optional: bool,
    pub contents: Option<Vec<u8>>,
}

#[derive(Debug)]
pub enum Entry {
    Rule(Rule),
    Macro(SourceLine),
    Include { paths: SourceLine, optional: bool },
}

#[derive(Debug)]
pub struct Rule {
    pub location: Location,
    pub targets: Vec<String>,
    pub prerequisites: Vec<String>,
    pub recipes: Vec<SourceLine>,
    pub(crate) inference: Option<Inference>,
}

#[derive(Debug)]
pub(crate) struct Inference {
    pub source: String,
    pub stem: String,
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
    let mut makefile = Makefile::default();
    for path in paths {
        if let Some(source) = streams.get(path) {
            makefile.read_entries(path, source, macros, 0)?;
            continue;
        }
        let location = Location {
            path: path.as_path().into(),
            line: 1,
        };
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
        makefile.read_entries(path, &source, macros, 0)?;
        if stream {
            streams.insert(path.clone(), source);
        }
    }
    Ok(makefile)
}

pub(crate) fn check_target(name: &str, location: &Location) -> Result<(), Error> {
    if matches!(name, ".WAIT" | ".NOTPARALLEL") || name.contains(['(', ')']) {
        return Err(Error::deferred(location, name));
    }
    Ok(())
}

fn read_source(mut file: File, location: &Location) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| Error::new(location, error.to_string()))?;
    Ok(bytes)
}

impl Makefile {
    fn read_entries(
        &mut self,
        path: &Path,
        bytes: &[u8],
        macros: &mut Macros,
        depth: usize,
    ) -> Result<(), Error> {
        let parsed = parse(path, bytes)?;
        if depth == 0
            && matches!(parsed.entries.first(), Some(Entry::Rule(rule)) if rule.targets == [".POSIX"])
        {
            macros.use_posix_compiler_defaults();
        }
        let automatic = Automatic::default();
        for entry in parsed.entries {
            match entry {
                Entry::Macro(line) => macros.assign_makefile(&line.text, &line.location)?,
                Entry::Rule(mut rule) => {
                    for words in [&mut rule.targets, &mut rule.prerequisites] {
                        let mut expanded = Vec::new();
                        for word in words.iter() {
                            let value = macros.expand(word, &rule.location, &automatic)?;
                            expanded.extend(separate_words(&value).map(str::to_owned));
                        }
                        for word in &expanded {
                            check_target(word, &rule.location)?;
                        }
                        *words = expanded;
                    }
                    if rule.targets.is_empty() {
                        return Err(Error::new(
                            &rule.location,
                            "missing target after macro expansion",
                        ));
                    }
                    self.entries.push(Entry::Rule(rule));
                }
                Entry::Include { paths, optional } => {
                    let expanded = macros.expand(&paths.text, &paths.location, &automatic)?;
                    for name in separate_words(&expanded) {
                        check_target(name, &paths.location)?;
                        let file = match File::open(name) {
                            Ok(file) => Some(file),
                            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
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
                            name: name.to_owned(),
                            location: paths.location.clone(),
                            optional,
                            contents: None,
                        });
                        let Some(file) = file else {
                            continue;
                        };
                        if depth >= MAX_INCLUDE_DEPTH {
                            return Err(Error::new(
                                &paths.location,
                                format!("include nesting is too deep at '{name}'"),
                            ));
                        }
                        let path = Path::new(name);
                        let location = Location {
                            path: path.into(),
                            line: 1,
                        };
                        let source = read_source(file, &location)?;
                        self.read_entries(path, &source, macros, depth + 1)?;
                        self.includes[index].contents = Some(source);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Read a makefile as UTF-8 text, retaining each entry's starting line.
pub fn parse(path: impl AsRef<Path>, source: &[u8]) -> Result<Makefile, Error> {
    let mut location = Location {
        path: path.as_ref().into(),
        line: 1,
    };
    let mut remaining = str::from_utf8(source).map_err(|error| {
        location.line += source[..error.valid_up_to()]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        Error::new(&location, "invalid UTF-8")
    })?;
    let mut makefile = Makefile::default();

    while !remaining.is_empty() {
        let starting_location = location.clone();
        let line = take_logical_line(&mut remaining, &mut location)?;

        if let Some(recipe) = line.strip_prefix('\t') {
            if let Some(Entry::Rule(rule)) = makefile.entries.last_mut() {
                rule.recipes.push(recipe_line(starting_location, recipe));
            } else if !recipe.trim_matches(is_blank).is_empty() {
                return Err(Error::new(&starting_location, "recipe before any rule"));
            }
        } else if let Some(entry) = read_entry(line, starting_location)? {
            makefile.entries.push(entry);
        }
    }
    Ok(makefile)
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
    for (keyword, optional) in [("include", false), ("-include", true)] {
        if let Some(paths) = line
            .strip_prefix(keyword)
            .filter(|rest| rest.starts_with(is_blank))
        {
            let paths = paths.split_once('#').map_or(paths, |(paths, _)| paths);
            return Ok(Some(Entry::Include {
                paths: SourceLine {
                    text: fold_header(paths),
                    location,
                },
                optional,
            }));
        }
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
                if line[index..].starts_with("::=") || line[index..].starts_with(":::=") {
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
        let prerequisites = words(&fold_header(&line[colon + 1..end]));
        let recipes = recipe
            .map(|text| recipe_line(location.clone(), text))
            .into_iter()
            .collect();
        Ok(Some(Entry::Rule(Rule {
            location,
            targets,
            prerequisites,
            recipes,
            inference: None,
        })))
    } else {
        let text = fold_header(&line[..end]);
        if assignment {
            Ok(Some(Entry::Macro(SourceLine { location, text })))
        } else if text.trim_matches(is_blank).is_empty() {
            Ok(None)
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
            } else if character == '#' {
                return Some((index, character));
            } else if closing.last() == Some(&character) {
                closing.pop();
            } else if closing.is_empty() {
                return Some((index, character));
            }
        }
        None
    })
}
