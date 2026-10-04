//! Effort-suite manifests (`docs/suites/<id>.json`, see `docs/suites/README.md`).
//!
//! Every manifest has the shape Vigil reads, with ids that are lowercase words joined by
//! '-'. Each manifest's suite binary exists and is built and installed, and the
//! categories and cases in the table its `main` runs (the `static` Suite in
//! `userspace/programs/src/suite_<id>.rs`, read from the parsed source) are the
//! manifest's, in the same order with the same titles. Every suite source has a manifest.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Lowercase words of a-z and 0-9 joined by single '-'.
fn is_id(id: &str) -> bool {
    !id.is_empty()
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
    let mut expected = vec!["id", "title", "area", "summary", "binary", "categories"];
    if let Some(checks) = object.get("diskChecks") {
        expected.push("diskChecks");
        match checks.as_array() {
            Some(checks) if !checks.is_empty() => {
                let mut paths = HashSet::new();
                for check in checks {
                    let Some(check) = check.as_object() else {
                        errors.push(format!("{at}: disk check must be an object"));
                        continue;
                    };
                    keys(check, &["path", "length", "byte"], &at, errors);
                    let path = text(check, "path", &at, errors);
                    if !path.starts_with('/') || !paths.insert(path) {
                        errors.push(format!("{at}: disk check needs a unique absolute guest path"));
                    }
                    if !check.get("length").and_then(Value::as_u64).is_some_and(|n| n > 0)
                        || !check.get("byte").and_then(Value::as_u64).is_some_and(|n| n <= 255)
                    {
                        errors.push(format!("{at}: disk check needs a positive length and a byte"));
                    }
                }
            }
            _ => errors.push(format!("{at}: diskChecks must be a nonempty array")),
        }
    }
    keys(object, &expected, &at, errors);
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

/// Why a suite source's table could not be read.
fn table_error(what: &str) -> String {
    format!(
        "{what}; a suite's source must run its table from main as `NAME.run()` and declare it as \
         `static NAME: Suite = suite(\"id\", \"Title\", &[category(\"id\", \"Title\", &[case(\"id\", \"Title\", function), ...]), ...])`, \
         optionally followed by `.case_limit_ms(N)`, with no attributes inside it"
    )
}

/// Whether `attrs` holds anything but doc comments (a `cfg` could drop the item).
fn has_attributes(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| !attr.path().is_ident("doc"))
}

/// The call `name(args...)` that `expr` is, with its arguments.
fn call<'a>(expr: &'a syn::Expr, name: &str) -> Result<Vec<&'a syn::Expr>, String> {
    let syn::Expr::Call(call) = expr else {
        return Err(table_error(&format!("expected a call to {name}(...)")));
    };
    let syn::Expr::Path(path) = &*call.func else {
        return Err(table_error(&format!("expected a call to {name}(...)")));
    };
    if !call.attrs.is_empty() || path.path.segments.last().map(|segment| segment.ident.to_string()) != Some(name.to_string()) {
        return Err(table_error(&format!("expected a call to {name}(...)")));
    }
    Ok(call.args.iter().collect())
}

/// The string literal `expr` is.
fn string(expr: &syn::Expr) -> Result<String, String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(text), attrs }) if attrs.is_empty() => Ok(text.value()),
        _ => Err(table_error("expected a string literal")),
    }
}

/// The elements of `&[a, b, ...]`.
fn slice(expr: &syn::Expr) -> Result<Vec<&syn::Expr>, String> {
    if let syn::Expr::Reference(reference) = expr {
        if let syn::Expr::Array(array) = &*reference.expr {
            if reference.attrs.is_empty() && array.attrs.is_empty() && reference.mutability.is_none() {
                return Ok(array.elems.iter().collect());
            }
        }
    }
    Err(table_error("expected a slice literal &[...]"))
}

/// An `(id, title)` pair from a call's first two arguments.
fn id_and_title(args: &[&syn::Expr], name: &str, arity: usize) -> Result<Entry, String> {
    if args.len() != arity {
        return Err(table_error(&format!("{name}(...) takes {arity} arguments")));
    }
    Ok(Entry { id: string(args[0])?, title: string(args[1])? })
}

/// The suite id, title and categories in the table the suite's `main` runs:
/// `main` must be `NAME.run()`, and `static NAME: Suite` is read from the parsed
/// source, so comments, disabled code and calls outside the table play no part.
fn source_suite(source: &str) -> Result<(Entry, Vec<Category>), String> {
    let file = syn::parse_file(source).map_err(|error| format!("does not parse as Rust: {error}"))?;
    let main = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "main" => Some(function),
            _ => None,
        })
        .ok_or_else(|| table_error("no fn main"))?;
    let run_target = match main.block.stmts.as_slice() {
        [syn::Stmt::Expr(syn::Expr::MethodCall(method), _)] if method.method == "run" && method.args.is_empty() => {
            match &*method.receiver {
                syn::Expr::Path(path) if path.path.get_ident().is_some() => path.path.get_ident().cloned(),
                _ => None,
            }
        }
        _ => None,
    };
    let name = run_target.ok_or_else(|| table_error("fn main is not `NAME.run()`"))?;
    if has_attributes(&main.attrs) {
        return Err(table_error("fn main has attributes"));
    }
    let table = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Static(item) if item.ident == name => Some(item),
            _ => None,
        })
        .ok_or_else(|| table_error(&format!("no static {name}")))?;
    if has_attributes(&table.attrs) {
        return Err(table_error(&format!("static {name} has attributes")));
    }
    let mut expr = &*table.expr;
    while let syn::Expr::MethodCall(method) = expr {
        if method.method != "case_limit_ms" || method.args.len() != 1 || !method.attrs.is_empty() {
            return Err(table_error(&format!("unexpected method .{}(...)", method.method)));
        }
        expr = &method.receiver;
    }
    let suite_args = call(expr, "suite")?;
    let suite = id_and_title(&suite_args, "suite", 3)?;
    let mut categories = Vec::new();
    for category_expr in slice(suite_args[2])? {
        let args = call(category_expr, "category")?;
        let entry = id_and_title(&args, "category", 3)?;
        let mut cases = Vec::new();
        for case_expr in slice(args[2])? {
            let case_args = call(case_expr, "case")?;
            cases.push(id_and_title(&case_args, "case", 3)?);
        }
        categories.push(Category { entry, cases });
    }
    Ok((suite, categories))
}

/// The `[[bin]]` tables in a Cargo.toml, as (name, path).
fn cargo_bins(cargo_toml: &str) -> Vec<(String, String)> {
    let mut bins = Vec::new();
    let mut current: Option<(Option<String>, Option<String>)> = None;
    let value = |line: &str, key: &str| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
    };
    for line in cargo_toml.lines().map(|line| line.split('#').next().unwrap_or("").trim()) {
        if line.starts_with('[') {
            if let Some((Some(name), Some(path))) = current.take() {
                bins.push((name, path));
            }
            if line == "[[bin]]" {
                current = Some((None, None));
            }
        } else if let Some((name, path)) = current.as_mut() {
            if let Some(found) = value(line, "name") {
                *name = Some(found);
            } else if let Some(found) = value(line, "path") {
                *path = Some(found);
            }
        }
    }
    if let Some((Some(name), Some(path))) = current {
        bins.push((name, path));
    }
    bins
}

/// The entries of build.sh's `STD_BINARIES=( ... )` array, unquoted.
fn std_binaries(build_sh: &str) -> Vec<String> {
    build_sh
        .lines()
        .skip_while(|line| line.trim() != "STD_BINARIES=(")
        .skip(1)
        .take_while(|line| line.trim() != ")")
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .flat_map(|line| line.split_whitespace())
        .map(|entry| entry.trim_matches('"').to_string())
        .collect()
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
    let bins = cargo_bins(&fs::read_to_string(root.join("userspace/programs/Cargo.toml")).unwrap());
    let binaries = std_binaries(&fs::read_to_string(root.join("userspace/programs/build.sh")).unwrap());
    assert!(!binaries.is_empty(), "no STD_BINARIES=( ... ) array in userspace/programs/build.sh");
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
        let bin = (format!("suite-{id}"), format!("src/suite_{module}.rs"));
        if !bins.contains(&bin) {
            errors.push(format!(
                "{id}: userspace/programs/Cargo.toml has no [[bin]] name = {:?}, path = {:?}",
                bin.0, bin.1
            ));
        }
        let entry = format!("suite-{id}");
        if !binaries.iter().any(|listed| *listed == entry || *listed == format!("{entry}:{entry}")) {
            errors.push(format!("{id}: \"{entry}\" is not in STD_BINARIES in userspace/programs/build.sh"));
        }

        let (suite, categories) = match source_suite(&source) {
            Ok(table) => table,
            Err(error) => {
                errors.push(format!("{id}: {source_rel}: {error}"));
                continue;
            }
        };
        if suite != (Entry { id: id.clone(), title: manifest.title.clone() }) {
            errors.push(format!(
                "{id}: {source_rel} runs suite({:?}, {:?}); the manifest says suite({id:?}, {:?})",
                suite.id, suite.title, manifest.title
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
    assert!(is_id(&"x".repeat(80)));
    for bad in ["", "Files", "files_io", "files io", "-files", "files-", "a--b", "smoke\n"] {
        assert!(!is_id(bad), "{bad}");
    }
}

#[test]
fn the_table_main_runs_is_read() {
    let source = r#"
        use libbreenix::suite::{case, category, suite};
        // case("commented", "out", c)
        /* category("block", "Comment", &[case("x", "y", x)]) */
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
        ]).case_limit_ms(500);
        static UNUSED: Suite = suite("other", "Other", &[category("x", "X", &[case("y", "Y", y)])]);
        fn not_a_case() { let _ = case("loose", "Loose", a); }
        fn main() { S.run() }
    "#;
    let (suite, categories) = source_suite(source).unwrap();
    assert_eq!(suite, Entry { id: "demo".to_string(), title: "Demo".to_string() });
    let ids: Vec<(&str, &str, Vec<&str>)> = categories
        .iter()
        .map(|c| (c.entry.id.as_str(), c.entry.title.as_str(), c.cases.iter().map(|k| k.id.as_str()).collect()))
        .collect();
    assert_eq!(ids, [("one", "First \"quoted\"", vec!["a", "b"]), ("two", "Second", vec!["c"])]);
}

#[test]
fn tables_that_could_differ_from_what_runs_are_refused() {
    for source in [
        r#"#[cfg(any())] static S: Suite = suite("d", "D", &[]); fn main() { S.run() }"#,
        r#"static S: Suite = suite("d", "D", &[#[cfg(any())] category("c", "C", &[])]); fn main() { S.run() }"#,
        r#"static S: Suite = suite("d", "D", CATEGORIES); fn main() { S.run() }"#,
        r#"static S: Suite = suite("d", "D", &[]); fn main() { if true { S.run() } }"#,
        r#"static S: Suite = suite("d", "D", &[]); fn main() {}"#,
    ] {
        assert!(source_suite(source).is_err(), "{source}");
    }
}

#[test]
fn build_lists_are_parsed_exactly() {
    let toml = "[[bin]]\nname = \"suite-smoke\"\npath = \"src/suite_smoke.rs\"\n\n[[bin]]\nname = \"x\" # c\npath = \"src/x.rs\"\n[dependencies]\nname = \"y\"\n";
    assert_eq!(cargo_bins(toml), [
        ("suite-smoke".to_string(), "src/suite_smoke.rs".to_string()),
        ("x".to_string(), "src/x.rs".to_string()),
    ]);
    let build = "A=(\n\"suite-nope\"\n)\nSTD_BINARIES=(\n    # \"suite-commented\"\n    \"a:b\"\n    \"suite-smoke\"\n)\n\"suite-after\"\n";
    assert_eq!(std_binaries(build), ["a:b", "suite-smoke"]);
}
