//! Text and file-name functions from the GNU make 4.4.1 manual, §§8.1–8.3.

use crate::{inference::Pattern, Error, Location};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(crate) fn arity(name: &str) -> Option<usize> {
    Some(match name {
        "subst" | "patsubst" | "wordlist" => 3,
        "findstring" | "filter" | "filter-out" | "word" | "addsuffix" | "addprefix" | "join" => 2,
        "strip" | "sort" | "words" | "firstword" | "lastword" | "dir" | "notdir" | "suffix"
        | "basename" | "wildcard" | "abspath" | "realpath" => 1,
        _ => return None,
    })
}

pub(crate) fn evaluate(
    name: &str,
    arguments: &[String],
    location: &Location,
) -> Result<String, Error> {
    let first = arguments[0].as_str();
    let words = || first.split_ascii_whitespace();
    let index = |argument: &str, minimum: usize| {
        argument
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|&n| n >= minimum)
            .ok_or_else(|| {
                Error::new(
                    location,
                    format!("invalid word index '{argument}' in '{name}'"),
                )
            })
    };
    Ok(match name {
        "subst" if first.is_empty() => format!("{}{}", arguments[2], arguments[1]),
        "subst" => arguments[2].replace(first, &arguments[1]),
        "patsubst" => patsubst(&Pattern::parse(first), &arguments[1], &arguments[2]),
        "strip" => words().collect::<Vec<_>>().join(" "),
        "findstring" => {
            if arguments[1].contains(first) {
                first.into()
            } else {
                String::new()
            }
        }
        "filter" | "filter-out" => {
            let patterns: Vec<_> = words().map(Pattern::parse).collect();
            let include_matches = name == "filter";
            arguments[1]
                .split_ascii_whitespace()
                .filter(|word| {
                    patterns.iter().any(|pattern| pattern.stem(word).is_some()) == include_matches
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        "sort" => {
            let mut words: Vec<_> = words().collect();
            words.sort_unstable();
            words.dedup();
            words.join(" ")
        }
        "word" => arguments[1]
            .split_ascii_whitespace()
            .nth(index(first, 1)? - 1)
            .unwrap_or_default()
            .into(),
        "wordlist" => wordlist(&arguments[2], index(first, 1)?, index(&arguments[1], 0)?).into(),
        "words" => words().count().to_string(),
        "firstword" => words().next().unwrap_or_default().into(),
        "lastword" => words().next_back().unwrap_or_default().into(),
        "dir" => words()
            .map(|word| word.rfind('/').map_or("./", |at| &word[..=at]))
            .collect::<Vec<_>>()
            .join(" "),
        "notdir" => words().map(filename).collect::<Vec<_>>().join(" "),
        "suffix" => words()
            .filter_map(|word| extension(word).map(|at| &word[at..]))
            .collect::<Vec<_>>()
            .join(" "),
        "basename" => words()
            .map(|word| extension(word).map_or(word, |at| &word[..at]))
            .collect::<Vec<_>>()
            .join(" "),
        "addsuffix" => arguments[1]
            .split_ascii_whitespace()
            .map(|word| format!("{word}{first}"))
            .collect::<Vec<_>>()
            .join(" "),
        "addprefix" => arguments[1]
            .split_ascii_whitespace()
            .map(|word| format!("{first}{word}"))
            .collect::<Vec<_>>()
            .join(" "),
        "join" => {
            let mut left = words();
            let mut right = arguments[1].split_ascii_whitespace();
            std::iter::from_fn(|| match (left.next(), right.next()) {
                (None, None) => None,
                (a, b) => Some(format!(
                    "{}{}",
                    a.unwrap_or_default(),
                    b.unwrap_or_default()
                )),
            })
            .collect::<Vec<_>>()
            .join(" ")
        }
        "wildcard" => words().flat_map(wildcard).collect::<Vec<_>>().join(" "),
        "abspath" => {
            let current =
                std::env::current_dir().map_err(|error| Error::new(location, error.to_string()))?;
            words()
                .map(|word| absolute(&current, word))
                .collect::<Vec<_>>()
                .join(" ")
        }
        "realpath" => words()
            .filter_map(|word| fs::canonicalize(word).ok())
            .filter_map(|path| path.into_os_string().into_string().ok())
            .collect::<Vec<_>>()
            .join(" "),
        _ => unreachable!("function without an evaluator: {name}"),
    })
}

pub(crate) fn patsubst(pattern: &Pattern, replacement: &str, value: &str) -> String {
    let replacement_literal = replacement;
    let replacement = pattern.has_stem().then(|| Pattern::parse(replacement));
    value
        .split_ascii_whitespace()
        .map(|word| match pattern.stem(word) {
            Some(stem) => replacement.as_ref().map_or_else(
                || replacement_literal.to_owned(),
                |replacement| replacement.fill("", stem),
            ),
            None => word.to_owned(),
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn wordlist(text: &str, start: usize, end: usize) -> &str {
    if start > end {
        return "";
    }
    let mut remaining = text;
    let mut left = text.len();
    let mut right = left;
    for index in 1..=end {
        let rest = remaining.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if rest.is_empty() {
            break;
        }
        if index == start {
            left = text.len() - rest.len();
        }
        remaining = rest.trim_start_matches(|c: char| !c.is_ascii_whitespace());
        right = text.len() - remaining.len();
    }
    if left <= right {
        &text[left..right]
    } else {
        ""
    }
}

fn filename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

fn extension(path: &str) -> Option<usize> {
    let name = filename(path);
    name.rfind('.').map(|at| path.len() - name.len() + at)
}

fn absolute(current: &Path, name: &str) -> String {
    let mut absolute = PathBuf::from("/");
    for component in current.join(name).components() {
        match component {
            Component::ParentDir => {
                absolute.pop();
            }
            Component::Normal(part) => absolute.push(part),
            _ => (),
        }
    }
    absolute.to_string_lossy().into_owned()
}

// GNU make §4.3.3 sorts each input pattern's matches separately, preserving
// input pattern order. Traverse one directory component at a time.
fn wildcard(pattern: &str) -> Vec<String> {
    let home;
    let expanded;
    let pattern = if let Some(rest) = pattern
        .strip_prefix("~/")
        .or_else(|| (pattern == "~").then_some(""))
    {
        home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            pattern
        } else {
            expanded = if pattern == "~" {
                home
            } else {
                format!("{home}/{rest}")
            };
            &expanded
        }
    } else {
        pattern
    };
    let mut paths = vec![String::new()];
    let mut components = pattern.split('/').peekable();
    while let Some(part) = components.next() {
        let more = components.peek().is_some();
        if part.is_empty() {
            if more {
                paths.iter_mut().for_each(|path| path.push('/'));
            }
            continue;
        }
        let pattern = Glob::parse(part);
        let mut matches = Vec::new();
        for prefix in paths {
            if let Some(literal) = &pattern.literal {
                let path = format!("{prefix}{literal}");
                if if more {
                    Path::new(&path).is_dir()
                } else {
                    fs::symlink_metadata(&path).is_ok()
                } {
                    matches.push(path);
                }
                continue;
            }
            let directory = if prefix.is_empty() { "." } else { &prefix };
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if pattern.matches(&name) && (!more || entry.path().is_dir()) {
                    matches.push(format!("{prefix}{name}"));
                }
            }
        }
        paths = matches;
        if more {
            paths.iter_mut().for_each(|path| path.push('/'));
        }
    }
    paths.sort_unstable();
    paths
}

#[derive(PartialEq)]
enum Token {
    Literal(char),
    Any,
    Star,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

struct Glob {
    tokens: Vec<Token>,
    literal: Option<String>,
}

impl Glob {
    fn parse(component: &str) -> Self {
        let mut chars = component.chars();
        let mut tokens = Vec::new();
        while let Some(character) = chars.next() {
            let token = match character {
                '*' => Token::Star,
                '?' => Token::Any,
                '\\' => Token::Literal(chars.next().unwrap_or(character)),
                '[' => {
                    let mut candidate = chars.clone();
                    match Self::class(&mut candidate) {
                        Some(class) => {
                            chars = candidate;
                            class
                        }
                        None => Token::Literal(character),
                    }
                }
                _ => Token::Literal(character),
            };
            tokens.push(token);
        }
        let literal = tokens
            .iter()
            .map(|token| match token {
                Token::Literal(character) => Some(*character),
                _ => None,
            })
            .collect();
        Self { tokens, literal }
    }

    fn class(chars: &mut std::str::Chars<'_>) -> Option<Token> {
        let negated = chars.as_str().starts_with(['!', '^']);
        if negated {
            chars.next();
        }
        let mut ranges = Vec::new();
        while let Some(character) = chars.next() {
            if character == ']' && !ranges.is_empty() {
                return Some(Token::Class { negated, ranges });
            }
            let first = if character == '\\' {
                chars.next()?
            } else {
                character
            };
            let last = if chars.as_str().starts_with('-') && !chars.as_str().starts_with("-]") {
                chars.next();
                match chars.next()? {
                    '\\' => chars.next()?,
                    last => last,
                }
            } else {
                first
            };
            ranges.push((first, last));
        }
        None
    }

    fn matches(&self, name: &str) -> bool {
        if name.starts_with('.') && self.tokens.first() != Some(&Token::Literal('.')) {
            return false;
        }
        let name: Vec<_> = name.chars().collect();
        let (mut token, mut character) = (0, 0);
        let mut star = None;
        while character < name.len() {
            let actual = name[character];
            let matched = match self.tokens.get(token) {
                Some(Token::Star) => {
                    token += 1;
                    star = Some((token, character));
                    continue;
                }
                Some(Token::Literal(expected)) => *expected == actual,
                Some(Token::Any) => true,
                Some(Token::Class { negated, ranges }) => {
                    ranges
                        .iter()
                        .any(|&(start, end)| (start..=end).contains(&actual))
                        != *negated
                }
                None => false,
            };
            if matched {
                token += 1;
                character += 1;
            } else if let Some((after_star, consumed)) = &mut star {
                *consumed += 1;
                token = *after_star;
                character = *consumed;
            } else {
                return false;
            }
        }
        self.tokens[token..]
            .iter()
            .all(|token| *token == Token::Star)
    }
}
