//! Effort-suite manifests (`docs/suites/<id>.json`, see `docs/suites/README.md`).
//!
//! Every manifest has the shape Vigil reads, with ids that are lowercase words joined by
//! '-'. Each manifest's suite binary exists and is built and installed, and the
//! categories and cases its source registers (the `suite(...)`, `category(...)` and
//! `case(...)` calls in `userspace/programs/src/suite_<id>.rs`) are the manifest's, in the
//! same order with the same titles. Every suite source has a manifest.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Lowercase words of a-z and 0-9 joined by single '-', at most 40 bytes.
fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .split('-')
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

/// A category or case as the manifest or the source names it.
#[derive(Debug, PartialEq)]
struct Entry {
    id: String,
    title: String,
}

#[derive(Debug, PartialEq)]
struct Category {
    entry: Entry,
    cases: Vec<Entry>,
}

struct Manifest {
    id: String,
    title: String,
    categories: Vec<Category>,
}

fn text<'a>(object: &'a Map<String, Value>, key: &str, at: &str, errors: &mut Vec<String>) -> &'a str {
    match object.get(key) {
        Some(Value::String(text)) if !text.trim().is_empty() && !text.contains('\n') => text,
        Some(Value::String(_)) => {
            errors.push(format!("{at}: \"{key}\" must be non-empty text on one line"));
            ""
        }
        _ => {
            errors.push(format!("{at}: \"{key}\" must be a string"));
            ""
        }
    }
}

fn keys(object: &Map<String, Value>, expected: &[&str], at: &str, errors: &mut Vec<String>) {
    for key in object.keys() {
        if !expected.contains(&key.as_str()) {
            errors.push(format!("{at}: unexpected key \"{key}\" (expected {expected:?})"));
        }
    }
    for key in expected {
        if !object.contains_key(*key) {
            errors.push(format!("{at}: missing key \"{key}\""));
        }
    }
}

fn entry(value: &Value, at: &str, extra: &[&str], errors: &mut Vec<String>) -> Option<Entry> {
    let Value::Object(object) = value else {
        errors.push(format!("{at}: must be an object"));
        return None;
    };
    let mut expected = vec!["id", "title"];
    expected.extend_from_slice(extra);
    keys(object, &expected, at, errors);
    let id = text(object, "id", at, errors);
    if !id.is_empty() && !is_id(id) {
        errors.push(format!("{at}: id {id:?} is not lowercase words joined by '-'"));
    }
    let title = text(object, "title", at, errors);
    Some(Entry { id: id.to_string(), title: title.to_string() })
}

/// Check one manifest's shape, returning what it lists.
fn read_manifest(path: &Path, errors: &mut Vec<String>) -> Option<Manifest> {
    let at = path.file_name().unwrap().to_string_lossy().to_string();
    let source = fs::read_to_string(path).unwrap_or_default();
    let value: Value = match serde_json::from_str(&source) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("{at}: not JSON: {error}"));
            return None;
        }
    };
    let Value::Object(object) = &value else {
        errors.push(format!("{at}: must be a JSON object"));
        return None;
    };
    keys(object, &["id", "title", "area", "summary", "binary", "categories"], &at, errors);
    let id = text(object, "id", &at, errors).to_string();
    if !id.is_empty() && !is_id(&id) {
        errors.push(format!("{at}: suite id {id:?} is not lowercase words joined by '-'"));
    }
    if at != format!("{id}.json") {
        errors.push(format!("{at}: the file must be named after the suite id, {id}.json"));
    }
    let title = text(object, "title", &at, errors).to_string();
    text(object, "area", &at, errors);
    text(object, "summary", &at, errors);
    let binary = text(object, "binary", &at, errors);
    if binary != format!("/sbin/suite-{id}") {
        errors.push(format!("{at}: binary is {binary:?}, expected \"/sbin/suite-{id}\""));
    }
    let mut categories = Vec::new();
    match object.get("categories") {
        Some(Value::Array(list)) if !list.is_empty() => {
            let mut category_ids = HashSet::new();
            for (c, value) in list.iter().enumerate() {
                let cat_at = format!("{at} categories[{c}]");
                let Some(category) = entry(value, &cat_at, &["cases"], errors) else { continue };
                if !category_ids.insert(category.id.clone()) {
                    errors.push(format!("{cat_at}: category id {:?} appears twice", category.id));
                }
                let mut cases = Vec::new();
                match value.get("cases") {
                    Some(Value::Array(case_list)) if !case_list.is_empty() => {
                        let mut case_ids = HashSet::new();
                        for (k, case_value) in case_list.iter().enumerate() {
                            let case_at = format!("{cat_at} cases[{k}]");
                            if let Some(case) = entry(case_value, &case_at, &[], errors) {
                                if !case_ids.insert(case.id.clone()) {
                                    errors.push(format!("{case_at}: case id {:?} appears twice in {:?}", case.id, category.id));
                                }
                                cases.push(case);
                            }
                        }
                    }
                    _ => errors.push(format!("{cat_at}: \"cases\" must be a non-empty array")),
                }
                categories.push(Category { entry: category, cases });
            }
        }
        _ => errors.push(format!("{at}: \"categories\" must be a non-empty array")),
    }
    Some(Manifest { id, title, categories })
}

/// Parse a Rust string literal starting at `chars[*at]` (which must be `"`).
fn string_literal(chars: &[char], at: &mut usize) -> Option<String> {
    if chars.get(*at) != Some(&'"') {
        return None;
    }
    *at += 1;
    let mut out = String::new();
    while let Some(&c) = chars.get(*at) {
        *at += 1;
        match c {
            '"' => return Some(out),
            '\\' => {
                let escaped = *chars.get(*at)?;
                *at += 1;
                out.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                });
            }
            _ => out.push(c),
        }
    }
    None
}

fn skip_space(chars: &[char], at: &mut usize) {
    while chars.get(*at).is_some_and(|c| c.is_whitespace()) {
        *at += 1;
    }
}

/// Every `name("first", "second"` call in `source`, in order, as (name, first, second).
/// Line comments are ignored.
fn calls(source: &str, names: &[&str]) -> Vec<(String, String, String)> {
    let code: String = source
        .lines()
        .map(|line| if line.trim_start().starts_with("//") { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n");
    let chars: Vec<char> = code.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '.');
        let name = names.iter().find(|name| {
            let len = name.chars().count();
            boundary && chars[i..].iter().take(len).copied().eq(name.chars())
        });
        if let Some(name) = name {
            let mut at = i + name.chars().count();
            skip_space(&chars, &mut at);
            if chars.get(at) == Some(&'(') {
                at += 1;
                skip_space(&chars, &mut at);
                if let Some(first) = string_literal(&chars, &mut at) {
                    skip_space(&chars, &mut at);
                    if chars.get(at) == Some(&',') {
                        at += 1;
                        skip_space(&chars, &mut at);
                        if let Some(second) = string_literal(&chars, &mut at) {
                            found.push((name.to_string(), first, second));
                            i = at;
                            continue;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    found
}

/// The suite id, title and categories a suite's source registers.
fn source_suite(source: &str) -> (Vec<(String, String)>, Vec<Category>) {
    let mut suites = Vec::new();
    let mut categories: Vec<Category> = Vec::new();
    for (name, id, title) in calls(source, &["suite", "category", "case"]) {
        match name.as_str() {
            "suite" => suites.push((id, title)),
            "category" => categories.push(Category { entry: Entry { id, title }, cases: Vec::new() }),
            _ => match categories.last_mut() {
                Some(category) => category.cases.push(Entry { id, title }),
                None => categories.push(Category {
                    entry: Entry { id: String::from("(no category)"), title: String::new() },
                    cases: vec![Entry { id, title }],
                }),
            },
        }
    }
    (suites, categories)
}

fn describe(categories: &[Category]) -> String {
    categories
        .iter()
        .map(|category| {
            let cases: Vec<String> =
                category.cases.iter().map(|case| format!("    {} | {}", case.id, case.title)).collect();
            format!("  {} | {}\n{}", category.entry.id, category.entry.title, cases.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn suite_manifests_match_their_binaries() {
    let root = repo_root();
    let cargo_toml = fs::read_to_string(root.join("userspace/programs/Cargo.toml")).unwrap();
    let build_sh = fs::read_to_string(root.join("userspace/programs/build.sh")).unwrap();
    let mut errors = Vec::new();
    let mut manifest_ids = HashSet::new();

    let mut paths: Vec<PathBuf> = fs::read_dir(root.join("docs/suites"))
        .expect("docs/suites exists")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no manifests in docs/suites");

    for path in &paths {
        let Some(manifest) = read_manifest(path, &mut errors) else { continue };
        if manifest.id.is_empty() {
            continue;
        }
        manifest_ids.insert(manifest.id.clone());
        let id = &manifest.id;
        let module = id.replace('-', "_");
        let source_rel = format!("userspace/programs/src/suite_{module}.rs");
        let Ok(source) = fs::read_to_string(root.join(&source_rel)) else {
            errors.push(format!("{id}: no suite source at {source_rel}"));
            continue;
        };
        let bin = format!("[[bin]]\nname = \"suite-{id}\"\npath = \"src/suite_{module}.rs\"");
        if !cargo_toml.contains(&bin) {
            errors.push(format!("{id}: userspace/programs/Cargo.toml has no binary:\n{bin}"));
        }
        if !build_sh.contains(&format!("\"suite-{id}\"")) {
            errors.push(format!("{id}: \"suite-{id}\" is not in STD_BINARIES in userspace/programs/build.sh"));
        }

        let (suites, categories) = source_suite(&source);
        if suites != [(id.clone(), manifest.title.clone())] {
            errors.push(format!(
                "{id}: {source_rel} must declare suite({id:?}, {:?}) once; found {suites:?}",
                manifest.title
            ));
        }
        if categories != manifest.categories {
            errors.push(format!(
                "{id}: the categories and cases in {source_rel} differ from docs/suites/{id}.json\n\
                 binary:\n{}\nmanifest:\n{}",
                describe(&categories),
                describe(&manifest.categories)
            ));
        }
    }

    for entry in fs::read_dir(root.join("userspace/programs/src")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        if let Some(module) = name.strip_prefix("suite_").and_then(|rest| rest.strip_suffix(".rs")) {
            let id = module.replace('_', "-");
            if !manifest_ids.contains(&id) {
                errors.push(format!("userspace/programs/src/{name} has no manifest docs/suites/{id}.json"));
            }
        }
    }

    assert!(errors.is_empty(), "suite manifest problems:\n{}", errors.join("\n"));
}

#[test]
fn ids_are_lowercase_words_joined_by_dashes() {
    for good in ["smoke", "files-io", "ipc2", "a-b-c"] {
        assert!(is_id(good), "{good}");
    }
    for bad in ["", "Files", "files_io", "files io", "-files", "files-", "a--b", &"x".repeat(41)] {
        assert!(!is_id(bad), "{bad}");
    }
}

#[test]
fn source_calls_are_read_in_order() {
    let source = r#"
        use libbreenix::suite::{case, category, suite};
        // case("commented", "out")
        static S: Suite = suite("demo", "Demo", &[
            category("one", "First \"quoted\"", &[
                case("a", "Case a", a),
                case(
                    "b",
                    "Case b",
                    b,
                ),
            ]),
            category("two", "Second", &[ case("c", "Case c", c) ]),
        ]);
        fn not_a_case() { showcase("x", "y"); }
    "#;
    let (suites, categories) = source_suite(source);
    assert_eq!(suites, [("demo".to_string(), "Demo".to_string())]);
    let ids: Vec<(&str, &str, Vec<&str>)> = categories
        .iter()
        .map(|c| (c.entry.id.as_str(), c.entry.title.as_str(), c.cases.iter().map(|k| k.id.as_str()).collect()))
        .collect();
    assert_eq!(ids, [("one", "First \"quoted\"", vec!["a", "b"]), ("two", "Second", vec!["c"])]);
}
