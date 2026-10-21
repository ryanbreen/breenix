//! Effort-suite manifests (`docs/suites/<id>.json`, see `docs/suites/README.md`).
//!
//! Each manifest has the shape Vigil reads, with ids that are lowercase words joined by
//! '-'. A manifest with cases has a suite binary that is built and installed, and the
//! categories and cases in the table its `main` runs (the `static` Suite in
//! `userspace/programs/src/suite_<id>.rs`, read from the parsed source) are the manifest's
//! categories that have cases, in the same order with the same titles. Each suite source has
//! a manifest. A category with no cases yet is planned: the binary leaves it out, and a
//! manifest in which each category is planned is a skeleton with no suite source
//! (docs/suites/README.md, "Planned categories").
//!
//! The efforts in `docs/efforts/path.json` that name their targets name known ones (`targets`,
//! docs/efforts/README.md).

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
    host_runner: Option<String>,
}

impl Manifest {
    /// Each category is planned: the suite has no cases, no source and no binary yet.
    fn is_skeleton(&self) -> bool {
        self.categories.iter().all(|category| category.cases.is_empty())
    }
}

/// A manifest value in which each category has an empty case list (`Manifest::is_skeleton`).
fn is_skeleton_value(manifest: &Value) -> bool {
    manifest["categories"].as_array().into_iter().flatten()
        .all(|category| category["cases"].as_array().is_some_and(|cases| cases.is_empty()))
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
    // A suite whose cases measure time waits for a quiet host before it boots.
    if let Some(sensitive) = object.get("timingSensitive") {
        expected.push("timingSensitive");
        if sensitive != &Value::Bool(true) {
            errors.push(format!("{at}: timingSensitive is true when present"));
        }
    }
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
        // The host reads the disk after the boot, so in a boot of several suites no suite may run after this one.
        if object.get("sequence") != Some(&Value::String("last".into())) {
            errors.push(format!("{at}: a suite with diskChecks runs last in a multi-suite boot: set \"sequence\": \"last\""));
        }
    }
    // Where the suite must run in a boot of several suites (docs/suites/README.md, "Several suites in one boot").
    if let Some(place) = object.get("sequence") {
        expected.push("sequence");
        if place != "first" && place != "last" {
            errors.push(format!("{at}: sequence is \"first\" or \"last\" when present"));
        }
    }
    // A suite that renders through a 3D GPU; launchers for targets without one leave it out of a resolved list.
    if let Some(needs) = object.get("needs3dGpu") {
        expected.push("needs3dGpu");
        if needs != &Value::Bool(true) {
            errors.push(format!("{at}: needs3dGpu is true when present"));
        }
    }
    if let Some(host) = object.get("hostRunner") {
        expected.push("hostRunner");
        if host != &Value::Bool(true) {
            errors.push(format!("{at}: hostRunner is true when present"));
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
    let host_runner = (object.get("hostRunner") == Some(&Value::Bool(true))).then(|| binary.to_string());
    if host_runner.is_some() {
        if binary != "scripts/metal/server-suite" || id != "server" {
            errors.push(format!("{at}: unrecognized host runner {binary:?}"));
        }
    } else if binary != format!("/sbin/suite-{id}") {
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
                // An empty list is a planned category: its cases arrive with the lane that implements it.
                match value.get("cases") {
                    Some(Value::Array(case_list)) => {
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
                    _ => errors.push(format!("{cat_at}: \"cases\" must be an array")),
                }
                categories.push(Category { entry: category, cases });
            }
        }
        _ => errors.push(format!("{at}: \"categories\" must be a non-empty array")),
    }
    Some(Manifest { id, title, categories, host_runner })
}

/// Why a suite source's table could not be read.
fn table_error(what: &str) -> String {
    format!(
        "{what}; a suite's source must run its table from main as `NAME.run()` and declare it as \
         `static NAME: Suite = suite(\"id\", \"Title\", &[category(\"id\", \"Title\", &[case(\"id\", \"Title\", function), ...]), ...])`, \
         optionally followed by `.case_limit_ms(N)` or `.defer_display()`, each category optionally followed \
         by `.idle_before(N)`, with no attributes inside it"
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

/// Setup may prepare cases, but cannot print or bypass the suite runner.
/// Follow local functions (including callbacks) so a helper cannot hide output
/// or process termination. Returns and `?` in helpers return to their caller;
/// either in main would instead bypass the runner.
struct SetupCheck<'a> {
    file: &'a syn::File,
    active: HashSet<String>,
    errors: Vec<String>,
    in_main: bool,
}
impl<'ast> syn::visit::Visit<'ast> for SetupCheck<'ast> {
    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        if self.in_main && matches!(expr, syn::Expr::Return(_) | syn::Expr::Try(_)) {
            self.errors.push("setup can return before SUITE.run()".into());
        }
        if let syn::Expr::Call(call) = expr {
            if matches!(&*call.func, syn::Expr::Path(p) if p.path.is_ident("sc"))
                && !matches!(call.args.first(), Some(syn::Expr::Path(p)) if
                    p.path.segments.last().is_some_and(|s| s.ident == "CLOCK_GETTIME" || s.ident == "NANOSLEEP"))
            {
                self.errors.push("setup syscall is not proven silent and non-exiting".into());
            }
        }
        if let syn::Expr::MethodCall(call) = expr {
            if call.method == "unwrap" || call.method == "expect" {
                self.errors.push("setup can panic".into());
            }
        }
        syn::visit::visit_expr(self, expr);
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let name = mac.path.segments.last().unwrap().ident.to_string();
        // The networking setup reads monotonic time and sleeps through sc().
        // Its one-instruction syscall assembly is silent and returns; calls
        // from setup are limited below to those two non-exiting syscalls.
        let syscall = name == "asm"
            && mac.tokens.to_string().starts_with("\"syscall\" ,")
            && !mac.tokens.to_string().contains("noreturn");
        if name != "format" && !syscall {
            self.errors.push(format!("setup macro {name} is not proven silent and non-exiting"));
        }
    }
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        let name = path.path.segments.last().unwrap().ident.to_string();
        if ["exit", "abort", "_exit", "panic", "print", "println", "eprint", "eprintln"].contains(&name.as_str()) {
            self.errors.push(format!("setup calls {name}"));
        }
        if self.active.insert(name.clone()) {
            if let Some(function) = self.file.items.iter().find_map(|item| match item {
                syn::Item::Fn(f) if f.sig.ident == name => Some(f), _ => None,
            }) {
                let in_main = self.in_main;
                self.in_main = false;
                self.visit_block(&function.block);
                self.in_main = in_main;
            }
            self.active.remove(&name);
        }
    }
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
    let run_target = match main.block.stmts.last() {
        Some(syn::Stmt::Expr(syn::Expr::MethodCall(method), _)) if method.method == "run" && method.args.is_empty() => {
            match &*method.receiver {
                syn::Expr::Path(path) if path.path.get_ident().is_some() => path.path.get_ident().cloned(),
                _ => None,
            }
        }
        _ => None,
    };
    let mut setup = SetupCheck { file: &file, active: HashSet::new(), errors: Vec::new(), in_main: true };
    for stmt in main.block.stmts.iter().take(main.block.stmts.len().saturating_sub(1)) {
        syn::visit::Visit::visit_stmt(&mut setup, stmt);
    }
    if !setup.errors.is_empty() { return Err(table_error(&setup.errors.join("; "))); }
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
        if !((method.method == "case_limit_ms" && method.args.len() == 1)
            || (method.method == "defer_display" && method.args.is_empty()))
            || !method.attrs.is_empty()
        {
            return Err(table_error(&format!("unexpected method .{}(...)", method.method)));
        }
        expr = &method.receiver;
    }
    let suite_args = call(expr, "suite")?;
    let suite = id_and_title(&suite_args, "suite", 3)?;
    let mut categories = Vec::new();
    for category_expr in slice(suite_args[2])? {
        let mut category_expr = category_expr;
        if let syn::Expr::MethodCall(method) = category_expr {
            if method.method != "idle_before" || method.args.len() != 1 || !method.attrs.is_empty() {
                return Err(table_error(&format!("unexpected method .{}(...)", method.method)));
            }
            category_expr = &method.receiver;
        }
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
        if manifest.is_skeleton() {
            // A suite with no cases cannot run, so it has no source, binary or install entry yet.
            if root.join(&source_rel).exists() || bins.iter().any(|(name, _)| *name == format!("suite-{id}")) {
                errors.push(format!("{id}: each category is planned, but {source_rel} or its binary exists; list its cases"));
            }
            continue;
        }
        if let Some(runner) = &manifest.host_runner {
            let checked = std::process::Command::new("python3").arg(root.join(runner))
                .arg("--check-manifest").arg(path).output().expect("host runner starts");
            if !checked.status.success() {
                errors.push(format!("{id}: host case table differs: {}", String::from_utf8_lossy(&checked.stderr)));
            }
            continue;
        }
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
        // The binary runs populated categories for incremental development. This is not
        // completion: suite-verdict.py blocks full-suite acceptance while any category
        // is planned; a milestone must have cases in each category it measures.
        let populated: Vec<Category> =
            manifest.categories.into_iter().filter(|category| !category.cases.is_empty()).collect();
        if categories != populated {
            errors.push(format!(
                "{id}: the categories and cases in {source_rel} differ from docs/suites/{id}.json\n\
                 binary:\n{}\nmanifest:\n{}",
                describe(&categories),
                describe(&populated)
            ));
        }
    }

    for (name, _) in &bins {
        if let Some(id) = name.strip_prefix("suite-") {
            if !manifest_ids.contains(id) {
                errors.push(format!("Cargo binary {name} has no manifest docs/suites/{id}.json"));
            }
        }
    }

    assert!(errors.is_empty(), "suite manifest problems:\n{}", errors.join("\n"));
}

/// The case ids in a libc-suite C source's `libc_cases[]` table, in order, from its
/// `{ "<id>", <function> },` rows up to the `{ 0, 0 }` that ends it.
fn c_case_table(source: &str) -> Result<Vec<String>, String> {
    let start = source.find("const struct libc_case libc_cases[] = {").ok_or("no libc_cases[] table")?;
    let mut ids = Vec::new();
    for line in source[start..].lines().skip(1) {
        let row = line.trim();
        if row == "{ 0, 0 }," {
            return Ok(ids);
        }
        let id = row.strip_prefix("{ \"").and_then(|rest| rest.split_once('"')).map(|(id, _)| id)
            .ok_or_else(|| format!("unexpected row {row:?} before {{ 0, 0 }}"))?;
        ids.push(id.to_string());
    }
    Err("the table has no { 0, 0 } row".to_string())
}

/// The libc suite runs each case of every category but busybox as the C function of the
/// same id in `userspace/libc-suite/<category>.c`; those tables list the manifest's cases.
#[test]
fn libc_c_case_tables_match_the_manifest() {
    let root = repo_root();
    let manifest: Value = serde_json::from_str(&fs::read_to_string(root.join("docs/suites/libc.json")).unwrap()).unwrap();
    let mut errors = Vec::new();
    for category in manifest["categories"].as_array().unwrap() {
        let id = category["id"].as_str().unwrap();
        if id == "busybox" {
            continue;
        }
        let want: Vec<String> = category["cases"].as_array().unwrap().iter()
            .map(|case| case["id"].as_str().unwrap().to_string()).collect();
        let rel = format!("userspace/libc-suite/{}.c", id.replace('-', "_"));
        match fs::read_to_string(root.join(&rel)).map_err(|e| e.to_string()).and_then(|s| c_case_table(&s)) {
            Ok(ids) if ids == want => {}
            Ok(ids) => errors.push(format!("{rel}: cases {ids:?}, manifest {want:?}")),
            Err(error) => errors.push(format!("{rel}: {error}")),
        }
    }
    assert!(errors.is_empty(), "libc C case tables differ from docs/suites/libc.json:\n{}", errors.join("\n"));
}

/// The host records scripts/sdk-suite.py writes (its `HOST_CASES`), in order.
fn sdk_harness_cases(source: &str) -> Vec<String> {
    let start = source.find("\nHOST_CASES = (\n").expect("scripts/sdk-suite.py has a HOST_CASES table");
    let body = &source[start..];
    let end = body.find("\n)\n").expect("HOST_CASES ends with a closing parenthesis");
    body[..end].split('\'').skip(1).step_by(2).map(String::from).collect()
}

/// The host records suite_sdk.rs reads: the cases whose function is `host_case`, and
/// every `category/case` string literal it names.
fn sdk_guest_reads(source: &str) -> Vec<String> {
    let mut reads = Vec::new();
    let mut category = String::new();
    for line in source.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("category(\"") {
            category = rest.split('"').next().unwrap_or("").to_string();
        } else if let (Some(rest), true) = (line.strip_prefix("case(\""), line.ends_with("host_case),")) {
            reads.push(format!("{category}/{}", rest.split('"').next().unwrap_or("")));
        }
    }
    fn literals(tokens: proc_macro2::TokenStream, out: &mut Vec<String>) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Group(group) => literals(group.stream(), out),
                proc_macro2::TokenTree::Literal(literal) => {
                    let text = literal.to_string();
                    let Some(value) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else { continue };
                    let mut parts = value.split('/');
                    if let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) {
                        if is_id(a) && is_id(b) {
                            out.push(value.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    literals(source.parse().expect("suite_sdk.rs tokenizes"), &mut reads);
    reads.sort();
    reads.dedup();
    reads
}

/// Every record the SDK harness writes names a case of docs/suites/sdk.json, and the
/// guest reads exactly the records the harness writes, so no host step goes unreported
/// and no guest case waits on a record nothing writes.
#[test]
fn sdk_host_records_match_the_manifest_and_the_guest() {
    let root = repo_root();
    let manifest: Value = serde_json::from_str(&fs::read_to_string(root.join("docs/suites/sdk.json")).unwrap()).unwrap();
    let mut ids = Vec::new();
    for category in manifest["categories"].as_array().unwrap() {
        for case in category["cases"].as_array().unwrap() {
            ids.push(format!("{}/{}", category["id"].as_str().unwrap(), case["id"].as_str().unwrap()));
        }
    }
    let mut written = sdk_harness_cases(&fs::read_to_string(root.join("scripts/sdk-suite.py")).unwrap());
    let unknown: Vec<&String> = written.iter().filter(|id| !ids.contains(id)).collect();
    assert!(unknown.is_empty(), "scripts/sdk-suite.py records cases docs/suites/sdk.json lacks: {unknown:?}");
    let in_order: Vec<&String> = ids.iter().filter(|id| written.contains(id)).collect();
    assert_eq!(written.iter().collect::<Vec<_>>(), in_order, "HOST_CASES is not in manifest order");
    written.sort();
    let read = sdk_guest_reads(&fs::read_to_string(root.join("userspace/programs/src/suite_sdk.rs")).unwrap());
    assert_eq!(read, written, "the records suite_sdk.rs reads differ from those scripts/sdk-suite.py writes");
}

#[test]
fn sdk_record_tables_are_read_exactly() {
    let harness = "X = 1\nHOST_CASES = (\n    'a/b', 'c/d',\n    'e/f',\n)\nY = ('g/h',)\n";
    assert_eq!(sdk_harness_cases(harness), ["a/b", "c/d", "e/f"]);
    let guest = "static S: Suite = suite(\"s\", \"S\", &[\n    category(\"one\", \"One\", &[\n        case(\"x\", \"X\", host_case),\n        case(\"y\", \"Y\", y),\n    ]),\n]);\nfn y() { host(\"two/z\"); let _ = \"not/a/record\"; let _ = \"Not/This\"; }\n";
    assert_eq!(sdk_guest_reads(guest), ["one/x", "two/z"]);
}

#[test]
fn planned_categories_have_empty_case_lists_only() {
    let dir = std::env::temp_dir().join(format!("suite-manifests-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let read = |categories: &str| {
        let path = dir.join("s.json");
        fs::write(&path, format!(r#"{{"id": "s", "title": "S", "area": "A", "summary": "S.", "binary": "/sbin/suite-s", "categories": {categories}}}"#)).unwrap();
        let mut errors = Vec::new();
        let manifest = read_manifest(&path, &mut errors);
        (manifest, errors)
    };
    let (manifest, errors) = read(r#"[{"id": "later", "title": "Later", "cases": []}]"#);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(manifest.unwrap().is_skeleton());
    let (manifest, errors) = read(r#"[{"id": "now", "title": "Now", "cases": [{"id": "a", "title": "A"}]}, {"id": "later", "title": "Later", "cases": []}]"#);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!manifest.unwrap().is_skeleton());
    for bad in [r#"[{"id": "later", "title": "Later"}]"#, r#"[{"id": "later", "title": "Later", "cases": {}}]"#, "[]"] {
        assert!(!read(bad).1.is_empty(), "{bad}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn c_case_tables_are_read_exactly() {
    let table = "const struct libc_case libc_cases[] = {\n    { \"a-b\", a_b },\n    { \"c\", c },\n    { 0, 0 },\n};\n";
    assert_eq!(c_case_table(table).unwrap(), ["a-b", "c"]);
    assert!(c_case_table("const struct libc_case libc_cases[] = {\n    { \"a\", a },\n};\n").is_err());
    assert!(c_case_table("const struct libc_case libc_cases[] = {\n    /* x */\n    { 0, 0 },\n};\n").is_err());
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
        ]).case_limit_ms(500).defer_display();
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
        r#"static S: Suite = suite("d", "D", &[]).defer_display(true); fn main() { S.run() }"#,
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

/// Clock reads and bounded concurrency helpers make host scheduling observable.
/// Walk Rust tokens so comments and string literals cannot classify a suite.
fn uses_time_or_concurrency(tokens: proc_macro2::TokenStream) -> bool {
    tokens.into_iter().any(|token| match token {
        proc_macro2::TokenTree::Group(group) => uses_time_or_concurrency(group.stream()),
        proc_macro2::TokenTree::Ident(ident) => matches!(ident.to_string().as_str(),
            "CLOCK_GETTIME" | "clock_gettime" | "now_monotonic" | "sleep_ms"
            | "nanosleep" | "clock_nanosleep" | "pthread_create" | "spawn"
            | "poll" | "select" | "ppoll" | "alarm" | "setitimer"
            | "sched_yield" | "fork" | "POLL" | "SELECT" | "PPOLL"
            | "ALARM" | "SETITIMER" | "CLOCK_NANOSLEEP" | "SCHED_YIELD" | "FORK"),
        _ => false,
    })
}

#[test]
fn suites_using_time_or_concurrency_require_timing_metadata() {
    let root = repo_root();
    let mut checked = 0;
    for entry in fs::read_dir(root.join("docs/suites")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "json") { continue; }
        let manifest: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        if is_skeleton_value(&manifest) { continue; }
        if manifest.get("hostRunner") == Some(&Value::Bool(true)) {
            // Host timeouts bound external tools; no clock/concurrency assertions on the guest.
            continue;
        }
        let id = manifest["id"].as_str().unwrap();
        let source = fs::read_to_string(root.join(format!(
            "userspace/programs/src/suite_{}.rs", id.replace('-', "_")
        ))).unwrap();
        let tokens = source.parse::<proc_macro2::TokenStream>().unwrap();
        if uses_time_or_concurrency(tokens) {
            checked += 1;
            assert_eq!(manifest.get("timingSensitive"), Some(&Value::Bool(true)),
                "{id}: cases use clocks or concurrency; set timingSensitive=true to classify this workload");
        }
    }
    assert!(checked > 0, "no timing-sensitive suite was checked");
}

#[test]
fn timing_classification_ignores_comments_and_literals() {
    assert!(!uses_time_or_concurrency(
        "// now_monotonic\nfn case() { let text = \"pthread_create\"; }".parse().unwrap()
    ));
    for name in ["poll", "select", "ppoll", "alarm", "setitimer",
                 "clock_nanosleep", "sched_yield", "fork"] {
        assert!(uses_time_or_concurrency(format!("fn case() {{ {name}(); }}").parse().unwrap()));
        assert!(uses_time_or_concurrency(format!("fn case() {{ sc(nr::{}, &[]); }}", name.to_uppercase()).parse().unwrap()));
    }
    for source in ["fn case() { time::now_monotonic(); }",
                   "fn case() { sc(nr::CLOCK_GETTIME, &[]); }",
                   "fn case() { std::thread::spawn(|| {}); }"] {
        assert!(uses_time_or_concurrency(source.parse().unwrap()));
    }
}

/// The target ids an effort in `docs/efforts/path.json` may list, in the order a list gives them: the ids
/// `scripts/effort-target-guard.py` uses (docs/efforts/README.md, "Targets"). `x86-metal` marks an effort that
/// x86-64 leads on bare metal; alone it belongs in `metal.json`.
const TARGETS: [&str; 5] = ["arm64-qemu", "parallels", "vmware", "x86", "x86-metal"];

/// Problems with the efforts' optional `targets` and `targetsNote`: a list must be non-empty, known ids in
/// `TARGETS` order without repeats; a note is one line of text and only sits beside a list.
/// Vigil BreenixPath.swift:168,232-234 treats profiles without x86 as outOfScope.
fn effort_target_errors(path: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    for effort in path["efforts"].as_array().into_iter().flatten() {
        let id = effort["id"].as_str().unwrap_or("?");
        if effort.get("needsGpu").is_some_and(|value| value != &Value::Bool(true)) {
            errors.push(format!("{id}: \"needsGpu\" must be true or omitted"));
        }
        let note = effort.get("targetsNote");
        let Some(targets) = effort.get("targets") else {
            if note.is_some() {
                errors.push(format!("{id}: \"targetsNote\" without \"targets\""));
            }
            continue;
        };
        let names: Vec<&str> = match targets.as_array() {
            Some(list) if !list.is_empty() => list.iter().map(|t| t.as_str().unwrap_or("")).collect(),
            _ => {
                errors.push(format!("{id}: \"targets\" must be a non-empty list"));
                continue;
            }
        };
        let positions: Vec<Option<usize>> = names.iter().map(|n| TARGETS.iter().position(|t| t == n)).collect();
        for (name, position) in names.iter().zip(&positions) {
            if position.is_none() {
                errors.push(format!("{id}: unknown target {name:?} (expected one of {TARGETS:?})"));
            }
        }
        let known: Vec<usize> = positions.into_iter().flatten().collect();
        if known.windows(2).any(|pair| pair[0] >= pair[1]) {
            errors.push(format!("{id}: targets {names:?} repeat or are out of the order {TARGETS:?}"));
        } else if names == ["x86-metal"] {
            errors.push(format!("{id}: an effort measured on x86-metal alone belongs in docs/efforts/metal.json"));
        }
        match note {
            None => {}
            Some(Value::String(text)) if !text.trim().is_empty() && !text.contains('\n') => {}
            Some(_) => errors.push(format!("{id}: \"targetsNote\" must be non-empty text on one line")),
        }
    }
    errors
}

#[test]
fn effort_targets_are_known_and_ordered() {
    let root = repo_root();
    let guard = fs::read_to_string(root.join("scripts/effort-target-guard.py")).unwrap();
    let line = guard.lines().find(|line| line.starts_with("TARGETS = (")).expect("TARGETS in the guard");
    let mut guarded: Vec<&str> = line.split('\'').skip(1).step_by(2).collect();
    guarded.sort_unstable();
    let mut ours = TARGETS.to_vec();
    ours.sort_unstable();
    assert_eq!(guarded, ours, "the guard's TARGETS and this test's differ");

    let path: Value =
        serde_json::from_str(&fs::read_to_string(root.join("docs/efforts/path.json")).unwrap()).unwrap();
    let errors = effort_target_errors(&path);
    assert!(errors.is_empty(), "docs/efforts/path.json targets:\n{}", errors.join("\n"));
    let listed = path["efforts"].as_array().unwrap().iter().filter(|e| e.get("targets").is_some()).count();
    assert!(listed > 0, "no effort lists targets, so the check above read nothing");
}

#[test]
fn effort_target_lists_are_checked() {
    let check = |effort: Value| effort_target_errors(&serde_json::json!({ "efforts": [effort] }));
    assert!(check(serde_json::json!({"needsGpu": true})).is_empty());
    for value in [serde_json::json!(false), serde_json::json!("true"), Value::Null, serde_json::json!(1)] {
        assert_eq!(check(serde_json::json!({"needsGpu": value})).len(), 1);
    }
    let profiles = serde_json::json!([{ "id": "hw", "measure": { "profiles": "first-userspace" } }]);
    assert!(check(serde_json::json!({ "id": "a" })).is_empty());
    assert!(check(serde_json::json!({ "id": "a", "targets": ["arm64-qemu", "vmware"], "targetsNote": "later" }))
        .is_empty());
    assert!(check(serde_json::json!({ "id": "a", "targets": ["arm64-qemu", "parallels", "vmware"], "milestones": profiles })).is_empty());
    assert!(check(serde_json::json!({ "id": "a", "targets": ["arm64-qemu", "parallels", "vmware", "x86-metal"] })).is_empty());
    for bad in [
        serde_json::json!({ "id": "a", "targetsNote": "no list" }),
        serde_json::json!({ "id": "a", "targets": [] }),
        serde_json::json!({ "id": "a", "targets": "vmware" }),
        serde_json::json!({ "id": "a", "targets": ["qemu"] }),
        serde_json::json!({ "id": "a", "targets": ["x86-64"] }),
        serde_json::json!({ "id": "a", "targets": ["x86-metal"] }),
        serde_json::json!({ "id": "a", "targets": ["vmware", "parallels"] }),
        serde_json::json!({ "id": "a", "targets": ["x86-metal", "arm64-qemu"] }),
        serde_json::json!({ "id": "a", "targets": ["vmware", "vmware"] }),
        serde_json::json!({ "id": "a", "targets": ["vmware"], "targetsNote": "" }),
        serde_json::json!({ "id": "a", "targets": ["vmware"], "targetsNote": "two\nlines" }),
    ] {
        assert_eq!(check(bad.clone()).len(), 1, "{bad}");
    }
}

fn read_definition(root: &Path, file: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(root.join(file)).unwrap()).unwrap()
}

/// Validate the independent stream without changing the main path's planned suite semantics.
fn metal_definition_errors(root: &Path, main: &Value, metal: &Value, boot: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let mut ids = HashSet::new();
    for path in [main, metal] {
        for effort in path["efforts"].as_array().unwrap() {
            let id = effort["id"].as_str().unwrap_or("");
            if !is_id(id) || !ids.insert(id) {
                errors.push(format!("duplicate or invalid effort id {id:?}"));
            }
        }
    }
    for (key, expected) in [("catalog", "xtask/src/boot_stages.rs"), ("gateArch", "x86_64"),
                            ("bootProfile", "production"), ("scoring", "modes")] {
        if boot[key].as_str() != Some(expected) {
            errors.push(format!("metal boot path {key} must be {expected}"));
        }
    }
    let modes = boot["modes"].as_object().unwrap();
    let mut mode_names: Vec<&str> = modes.keys().map(String::as_str).collect();
    mode_names.sort_unstable();
    if mode_names != ["console", "default", "persistent", "persistent-install", "probe", "shell", "suite"] {
        errors.push("metal modes must be console, default, persistent, persistent-install, probe, shell and suite".into());
    }
    let catalog = read_definition(root,
        "tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json");
    let catalog_names: HashSet<&str> = catalog["stages"].as_array().unwrap().iter()
        .map(|s| s["name"].as_str().unwrap()).collect();
    let host_events = ["console-ready", "power-on", "firmware-text", "post", "image-complete", "handoff"];
    let mut milestones = HashSet::new();
    let mut stage_names = HashSet::new();
    for milestone in boot["milestones"].as_array().unwrap() {
        let id = milestone["id"].as_str().unwrap_or("");
        if !is_id(id) || !milestones.insert(id) {
            errors.push(format!("duplicate or invalid metal boot milestone {id:?}"));
        }
        if milestone.get("stub").is_some_and(|stub| stub != true) {
            errors.push(format!("{id}: stub must be true or absent when finalized"));
        }
        let shapes = milestone["bootShapes"].as_array();
        if !shapes.is_some_and(|shapes| !shapes.is_empty()
            && shapes.iter().all(|shape| matches!(shape.as_str(), Some("ramdisk" | "install" | "disk-root" | "persistent")))
            && shapes.iter().collect::<HashSet<_>>().len() == shapes.len()) {
            errors.push(format!("{id}: bootShapes must name unique supported boot shapes"));
        }
        if let Some(boot_modes) = milestone.get("bootModes") {
            if !boot_modes.as_array().is_some_and(|names| !names.is_empty()
                && names.iter().all(|name| name.as_str().is_some_and(|name| modes.contains_key(name)))
                && names.iter().collect::<HashSet<_>>().len() == names.len()) {
                errors.push(format!("{id}: bootModes must name unique supported boot modes"));
            }
        }
        let lists = milestone["stages"].as_object().unwrap();
        if lists.is_empty() {
            errors.push(format!("{id}: stages must not be empty, including stubs"));
        }
        if milestone["kernel"] == true && !lists.contains_key("x86_64") {
            errors.push(format!("{id}: kernel milestone needs x86_64 stages"));
        }
        for (mode, stages) in lists {
            if mode != "x86_64" && !modes.contains_key(mode) {
                errors.push(format!("{id}: unknown metal stage mode {mode}"));
            }
            if mode == "x86_64" && milestone["kernel"] != true {
                errors.push(format!("{id}: x86_64 stages need kernel=true"));
            }
            let Some(stages) = stages.as_array().filter(|s| !s.is_empty()) else {
                errors.push(format!("{id}: stage list {mode} must not be empty"));
                continue;
            };
            for stage in stages {
                if let Some(name) = stage.as_str() {
                    stage_names.insert(name);
                    if !catalog_names.contains(name) {
                        errors.push(format!("{id}: unknown x86 catalog stage {name}"));
                    }
                    continue;
                }
                let name = stage["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    errors.push(format!("{id}: inline stage needs a name"));
                }
                stage_names.insert(name);
                match (stage["marker"].as_str(), stage["host"].as_str()) {
                    (Some(marker), None) if !marker.is_empty() => {
                        if marker.split('|').any(|part| part.is_empty() || part.len() > 128) {
                            errors.push(format!("{id}: invalid serial marker"));
                        }
                        if stage.get("match").is_some() && stage["match"] != "line" {
                            errors.push(format!("{id}: unknown marker match rule"));
                        }
                    }
                    (None, Some(host)) if host_events.contains(&host) => {}
                    _ => errors.push(format!("{id}: stage needs one serial marker or known host event")),
                }
            }
        }
    }
    for effort in metal["efforts"].as_array().unwrap() {
        let id = effort["id"].as_str().unwrap_or("");
        if !id.starts_with("metal-") || effort["targets"] != serde_json::json!(["x86-metal"]) {
            errors.push(format!("{id}: metal effort requires metal- id and exactly x86-metal"));
        }
        let mut own_ids = HashSet::new();
        for milestone in effort["milestones"].as_array().unwrap() {
            let mid = milestone["id"].as_str().unwrap_or("");
            if !is_id(mid) || !own_ids.insert(mid) {
                errors.push(format!("{id}: duplicate or invalid milestone {mid}"));
            }
            let measure = milestone["measure"].as_object().unwrap();
            if let Some(boot_id) = measure.get("bootPath").and_then(Value::as_str) {
                if measure.len() != 1 || !milestones.contains(boot_id) {
                    errors.push(format!("{id}/{mid}: unknown or mixed bootPath measure"));
                }
            } else if let Some(suite) = measure.get("suite").and_then(Value::as_str) {
                let file = root.join(format!("docs/suites/{suite}.json"));
                if !is_id(suite) || !file.is_file() {
                    errors.push(format!("{id}/{mid}: unknown suite {suite}"));
                    continue;
                }
                let manifest: Value = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
                let known: HashSet<&str> = manifest["categories"].as_array().unwrap().iter()
                    .map(|c| c["id"].as_str().unwrap()).collect();
                let categories = measure.get("categories").and_then(Value::as_array);
                let mut seen = HashSet::new();
                if measure.len() != 2 || categories.is_none_or(|c| c.is_empty()) {
                    errors.push(format!("{id}/{mid}: suite needs categories"));
                }
                for category in categories.into_iter().flatten() {
                    let category = category.as_str().unwrap_or("");
                    if !known.contains(category) || !seen.insert(category) {
                        errors.push(format!("{id}/{mid}: unknown or repeated suite category {category}"));
                    }
                }
            } else {
                errors.push(format!("{id}/{mid}: unsupported metal measure"));
            }
        }
    }
    let mut first_ids = HashSet::new();
    for first in boot["firsts"].as_array().unwrap() {
        let id = first["id"].as_str().unwrap_or("");
        if !is_id(id) || !first_ids.insert(id) ||
            !stage_names.contains(first["stage"].as_str().unwrap_or("")) {
            errors.push(format!("{id}: invalid first or unknown stage"));
        }
        if let Some(suite) = first.get("suite").and_then(Value::as_str) {
            if !is_id(suite) || !root.join(format!("docs/suites/{suite}.json")).is_file() {
                errors.push(format!("{id}: unknown first suite"));
            }
        }
    }
    errors
}

#[test]
fn metal_stream_is_independent_and_resolves_its_measures() {
    let root = repo_root();
    let main = read_definition(&root, "docs/efforts/path.json");
    let metal = read_definition(&root, "docs/efforts/metal.json");
    let boot = read_definition(&root, "docs/metal/boot-path.json");
    let errors = metal_definition_errors(&root, &main, &metal, &boot);
    assert!(errors.is_empty(), "metal definitions:\n{}", errors.join("\n"));
    assert_eq!(metal["efforts"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap())
        .collect::<Vec<_>>(), ["metal-bring-up", "metal-userspace", "metal-network", "metal-storage", "metal-console"]);
    assert_eq!(boot["milestones"][0]["id"], "delivery");
    assert_eq!(boot["milestones"][1]["id"], "first-light");
    assert!(boot["milestones"][0].get("stub").is_none());
    assert!(boot["milestones"][1].get("stub").is_none());
    let milestone = |id: &str| boot["milestones"].as_array().unwrap().iter()
        .find(|m| m["id"] == id).unwrap();
    assert_eq!(milestone("smp")["stages"]["x86_64"][1]["marker"], "[smp] online=4 reported=4");
    assert_eq!(boot["firsts"][2]["stage"], "Four CPUs online and reported");
    assert_eq!(milestone("ramdisk-root")["stages"]["x86_64"].as_array().unwrap().len(), 3);
    for id in ["sata-ahci", "disk-io", "disk-filesystem"] {
        assert!(metal["efforts"][3]["milestones"].as_array().unwrap().iter()
            .any(|m| m["measure"]["bootPath"] == id));
        assert!(!milestone(id)["stages"]["probe"].as_array().unwrap().is_empty());
    }
    let suite_markers: HashSet<&str> = milestone("suite-finished")["stages"]["suite"][0]["marker"]
        .as_str().unwrap().split('|').collect();
    let expected_markers: HashSet<String> = fs::read_dir(root.join("docs/suites")).unwrap()
        .map(Result::unwrap).map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            let manifest: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            format!("SUITE {} DONE", manifest["id"].as_str().unwrap())
        }).collect();
    assert_eq!(suite_markers, expected_markers.iter().map(String::as_str).collect());
    assert_eq!(boot["firsts"][7]["suite"], "networking");
    assert_eq!(boot["firsts"].as_array().unwrap().last().unwrap()["stage"], "Logged-in computation answered");
    for id in ["fb-console", "xhci", "usb-keyboard", "console-login"] {
        assert!(metal["efforts"][4]["milestones"].as_array().unwrap().iter()
            .any(|m| m["measure"]["bootPath"] == id));
        let shapes = if id == "console-login" {
            serde_json::json!(["ramdisk", "install", "disk-root"])
        } else {
            serde_json::json!(["ramdisk", "install", "disk-root", "persistent"])
        };
        assert_eq!(milestone(id)["bootShapes"], shapes);
    }
    assert_eq!(boot["milestones"][0]["stages"]["x86_64"].as_array().unwrap().iter()
        .filter_map(|s| s["host"].as_str()).collect::<Vec<_>>(),
        ["console-ready", "power-on", "firmware-text", "post", "image-complete"]);
    assert_eq!(boot["fatal"], serde_json::json!([
        "panicked at kernel/src/", "!!! SOFT LOCKUP DETECTED !!!"]));
}

#[test]
fn metal_definition_contract_rejects_invalid_inputs() {
    let root = repo_root();
    let main = read_definition(&root, "docs/efforts/path.json");
    let metal = read_definition(&root, "docs/efforts/metal.json");
    let boot = read_definition(&root, "docs/metal/boot-path.json");
    for (pointer, value) in [
        ("/efforts/0/id", main["efforts"][0]["id"].clone()),
        ("/efforts/0/targets", serde_json::json!(["x86-metal", "x86"])),
        ("/efforts/0/targets", Value::Null),
        ("/efforts/0/milestones/0/measure/bootPath", serde_json::json!("missing")),
        ("/efforts/1/milestones/3/measure/suite", serde_json::json!("missing")),
        ("/efforts/1/milestones/3/measure/categories", serde_json::json!(["missing"])),
    ] {
        let mut bad = metal.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        assert!(!metal_definition_errors(&root, &main, &bad, &boot).is_empty(), "{pointer}");
    }
    for value in [
        serde_json::json!([]),
        serde_json::json!(["sutie"]),
        serde_json::json!(["shell", "shell"]),
        serde_json::json!("shell"),
        serde_json::json!([null]),
    ] {
        let mut bad = boot.clone();
        bad["milestones"][0]["bootModes"] = value;
        assert!(metal_definition_errors(&root, &main, &metal, &bad)
            .iter().any(|error| error.contains("bootModes")));
    }
    let mut finalized = boot.clone();
    for milestone in finalized["milestones"].as_array_mut().unwrap() {
        milestone.as_object_mut().unwrap().remove("stub");
    }
    assert!(metal_definition_errors(&root, &main, &metal, &finalized).is_empty());
    for (pointer, value) in [
        ("/milestones/0/bootShapes", serde_json::json!(["unknown"])),
        ("/milestones/0", {
            let mut invalid = boot["milestones"][0].clone();
            invalid["stub"] = serde_json::json!("true");
            invalid
        }),
        ("/scoring", serde_json::json!("arch")),
        ("/gateArch", serde_json::json!("aarch64")),
        ("/milestones/0/stages/x86_64/0/host", serde_json::json!("missing")),
        ("/milestones/1/stages/x86_64/0", serde_json::json!("ARM64 kernel starting")),
        ("/milestones/1/stages", serde_json::json!({"desktop": ["Kernel entry point"]})),
        ("/milestones/2/stages", serde_json::json!({})),
        ("/firsts/0/stage", serde_json::json!("missing")),
    ] {
        let mut bad = boot.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        assert!(!metal_definition_errors(&root, &main, &metal, &bad).is_empty(), "{pointer}");
    }
}

#[test]
fn setup_is_silent_and_cannot_bypass_the_table() {
    let table = r#"static S: Suite = suite("d", "D", &[]);"#;
    assert!(source_suite(&format!("{table} fn prepare() -> Result<(), ()> {{ Ok(()) }} fn main() {{ let _ = prepare(); S.run() }}")).is_ok());
    for setup in ["return;", r#"println!("early");"#, "std::process::exit(0);", "let _ = prepare()?;", "prepare();"] {
        assert!(source_suite(&format!(r#"{table} fn prepare() {{ eprintln!("hidden"); }} fn main() {{ {setup} S.run() }}"#)).is_err(), "{setup}");
    }
}
