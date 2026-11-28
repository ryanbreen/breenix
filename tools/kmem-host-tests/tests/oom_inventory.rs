//! E1-4: each runtime use of an infallible allocation API in kernel/src has
//! an owner in docs/plans/e1-oom-inventory.json (E1 plan, 7.4 and E1-11 to
//! E1-20).
//!
//! The scan parses each kernel source file with `syn` and records, per file
//! and function, the calls that allocate and abort on failure: the
//! constructors, methods and macros in `ALLOCATING_*` below. Code under
//! `#[cfg(test)]` or `#[test]` is outside the rule. Macro bodies are scanned
//! when they parse as a comma-separated expression list, which covers the
//! logging and assertion macros. A site is matched to an inventory row by its
//! file and function path (`Type::method`, `outer::inner`, `mod::function`).
//!
//! `BREENIX_OOM_INVENTORY_DUMP=<file>` writes the unowned sites as JSON, which
//! `scripts/kmem-oom-inventory-fill.py` turns into rows.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use syn::visit::Visit;

/// `Type::function` calls that allocate infallibly.
const ALLOCATING_CALLS: &[(&str, &str)] = &[
    ("Box", "new"),
    ("Box", "pin"),
    ("Arc", "new"),
    ("Rc", "new"),
    ("String", "from"),
    ("String", "with_capacity"),
    ("Vec", "with_capacity"),
    ("VecDeque", "with_capacity"),
];

/// Methods of the `alloc` collections and conversions that allocate
/// infallibly. A same-named method of another type is reported as well; its
/// row says so when it is converted.
const ALLOCATING_METHODS: &[&str] = &[
    "push",
    "push_back",
    "push_front",
    "push_str",
    "insert",
    "extend",
    "extend_from_slice",
    "append",
    "resize",
    "reserve",
    "reserve_exact",
    "collect",
    "to_vec",
    "to_string",
    "to_owned",
    "into_boxed_slice",
];

/// Macros that allocate infallibly.
const ALLOCATING_MACROS: &[&str] = &["vec", "format"];

/// The owners a row may name: E1-4 for boot sites, a conversion PR, or `test`
/// for code built only with a test feature.
const OWNERS: &[&str] = &[
    "E1-4", "E1-11", "E1-12", "E1-13", "E1-14", "E1-15", "E1-16", "E1-17", "E1-18", "E1-19",
    "E1-20", "test",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Whether a `cfg` predicate can hold only in a test build.
fn requires_test(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) => {
            let Ok(nested) = list.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return false;
            };
            if list.path.is_ident("all") {
                nested.iter().any(requires_test)
            } else if list.path.is_ident("any") {
                !nested.is_empty() && nested.iter().all(requires_test)
            } else {
                false
            }
        }
        syn::Meta::NameValue(_) => false,
    }
}

fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("test") {
            return true;
        }
        attr.path().is_ident("cfg")
            && attr.parse_args::<syn::Meta>().is_ok_and(|meta| requires_test(&meta))
    })
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| match &attr.meta {
        syn::Meta::NameValue(nv) if nv.path.is_ident("path") => match &nv.value {
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
            _ => None,
        },
        _ => None,
    })
}

/// The file an out-of-line `mod name;` in `file` names.
fn module_file(file: &Path, name: &str, attrs: &[syn::Attribute]) -> Option<PathBuf> {
    let dir = file.parent()?;
    if let Some(path) = path_attr(attrs) {
        return Some(dir.join(path));
    }
    let stem = file.file_stem()?.to_str()?;
    let base = if matches!(stem, "mod" | "lib" | "main") { dir.to_path_buf() } else { dir.join(stem) };
    [base.join(format!("{name}.rs")), base.join(name).join("mod.rs")]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

#[derive(Default)]
struct Scan {
    /// (file, function) -> one line where an allocating API appears.
    sites: BTreeMap<(String, String), usize>,
    /// Out-of-line modules declared under `#[cfg(test)]`.
    test_modules: BTreeSet<PathBuf>,
    /// The out-of-line module declarations: (declaring file, declared file).
    children: Vec<(PathBuf, PathBuf)>,
}

struct Visitor<'a> {
    file: &'a Path,
    rel: String,
    scope: Vec<String>,
    in_fn: usize,
    test_depth: usize,
    scan: &'a mut Scan,
}

impl Visitor<'_> {
    fn record(&mut self, line: usize) {
        if self.test_depth > 0 || self.in_fn == 0 {
            return;
        }
        let key = (self.rel.clone(), self.scope.join("::"));
        self.scan.sites.entry(key).or_insert(line);
    }

    fn scoped(&mut self, name: String, is_fn: bool, test: bool, f: impl FnOnce(&mut Self)) {
        self.scope.push(name);
        self.in_fn += usize::from(is_fn);
        self.test_depth += usize::from(test);
        f(self);
        self.test_depth -= usize::from(test);
        self.in_fn -= usize::from(is_fn);
        self.scope.pop();
    }

    fn macro_body(&mut self, mac: &syn::Macro) {
        let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        if ALLOCATING_MACROS.contains(&name.as_str()) {
            self.record(line_of(mac.path.segments.last().unwrap().ident.span()));
        }
        let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
        if let Ok(exprs) = mac.parse_body_with(parser) {
            for expr in &exprs {
                self.visit_expr(expr);
            }
        }
    }
}

fn line_of(span: proc_macro2::Span) -> usize {
    span.start().line
}

fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_else(|| "_".into()),
        syn::Type::Reference(reference) => type_name(&reference.elem),
        _ => "_".into(),
    }
}

impl<'ast> Visit<'ast> for Visitor<'_> {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let test = is_test_only(&item.attrs);
        self.scoped(item.sig.ident.to_string(), true, test, |v| syn::visit::visit_item_fn(v, item));
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let test = is_test_only(&item.attrs);
        self.scoped(item.sig.ident.to_string(), true, test, |v| {
            syn::visit::visit_impl_item_fn(v, item)
        });
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        let test = is_test_only(&item.attrs);
        self.scoped(item.sig.ident.to_string(), true, test, |v| {
            syn::visit::visit_trait_item_fn(v, item)
        });
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let test = is_test_only(&item.attrs);
        self.scoped(type_name(&item.self_ty), false, test, |v| syn::visit::visit_item_impl(v, item));
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        let test = is_test_only(&item.attrs);
        self.scoped(item.ident.to_string(), false, test, |v| syn::visit::visit_item_trait(v, item));
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        let test = is_test_only(&item.attrs);
        if item.content.is_none() {
            if let Some(child) = module_file(self.file, &item.ident.to_string(), &item.attrs) {
                if test || self.test_depth > 0 {
                    self.scan.test_modules.insert(child.clone());
                }
                self.scan.children.push((self.file.to_path_buf(), child));
            }
            return;
        }
        self.scoped(item.ident.to_string(), false, test, |v| syn::visit::visit_item_mod(v, item));
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        let test = is_test_only(&item.attrs);
        self.scoped(format!("static {}", item.ident), true, test, |v| {
            syn::visit::visit_item_static(v, item)
        });
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        let test = is_test_only(&item.attrs);
        self.scoped(format!("const {}", item.ident), true, test, |v| {
            syn::visit::visit_item_const(v, item)
        });
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        let attrs: &[syn::Attribute] = match stmt {
            syn::Stmt::Local(local) => &local.attrs,
            syn::Stmt::Macro(mac) => &mac.attrs,
            _ => &[],
        };
        let test = is_test_only(attrs);
        self.test_depth += usize::from(test);
        syn::visit::visit_stmt(self, stmt);
        self.test_depth -= usize::from(test);
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        let attrs: &[syn::Attribute] = match expr {
            syn::Expr::Block(b) => &b.attrs,
            syn::Expr::Call(c) => &c.attrs,
            syn::Expr::MethodCall(m) => &m.attrs,
            syn::Expr::Macro(m) => &m.attrs,
            _ => &[],
        };
        let test = is_test_only(attrs);
        self.test_depth += usize::from(test);
        syn::visit::visit_expr(self, expr);
        self.test_depth -= usize::from(test);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*call.func {
            let segments: Vec<String> = path.path.segments.iter().map(|s| s.ident.to_string()).collect();
            if let [.., ty, function] = segments.as_slice() {
                if ALLOCATING_CALLS.iter().any(|(t, f)| t == ty && f == function) {
                    self.record(line_of(path.path.segments.last().unwrap().ident.span()));
                }
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if ALLOCATING_METHODS.contains(&call.method.to_string().as_str()) {
            self.record(line_of(call.method.span()));
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.macro_body(mac);
    }
}

fn scan_kernel() -> Scan {
    let root = repo();
    let mut files = Vec::new();
    rust_files(&root.join("kernel/src"), &mut files);
    let mut scan = Scan::default();
    let mut per_file = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).unwrap();
        let parsed = syn::parse_file(&source)
            .unwrap_or_else(|error| panic!("{} does not parse: {error}", file.display()));
        let rel = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let rel = rel.trim_start_matches("./").to_string();
        let mut file_scan = Scan::default();
        Visitor { file, rel, scope: Vec::new(), in_fn: 0, test_depth: 0, scan: &mut file_scan }
            .visit_file(&parsed);
        scan.test_modules.extend(file_scan.test_modules.iter().cloned());
        scan.children.extend(file_scan.children.iter().cloned());
        per_file.push((file.clone(), file_scan.sites));
    }
    // A module declared from a test-only file is test-only too.
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let mut test_files: BTreeSet<PathBuf> = scan.test_modules.iter().map(|p| canonical(p)).collect();
    loop {
        let before = test_files.len();
        for (parent, child) in &scan.children {
            if test_files.contains(&canonical(parent)) {
                test_files.insert(canonical(child));
            }
        }
        if test_files.len() == before {
            break;
        }
    }
    for (file, sites) in per_file {
        if !test_files.contains(&canonical(&file)) {
            scan.sites.extend(sites);
        }
    }
    scan
}

#[derive(Debug)]
struct Row {
    file: String,
    functions: Vec<String>,
}

/// Parse and check the inventory's rows: each names a known owner, the error
/// surfaced, the rollback point and at least one function.
fn rows(inventory: &Value) -> Result<Vec<Row>, String> {
    let rows = inventory.get("rows").and_then(Value::as_array).ok_or("no `rows` array")?;
    let mut out = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let text = |key: &str| -> Result<String, String> {
            match row.get(key).and_then(Value::as_str).map(str::trim) {
                Some(value) if !value.is_empty() => Ok(value.to_string()),
                _ => Err(format!("row {index} has no `{key}`: {row}")),
            }
        };
        let (file, owner) = (text("file")?, text("owner")?);
        text("error")?;
        text("rollback")?;
        if !OWNERS.contains(&owner.as_str()) {
            return Err(format!("row {index} names unknown owner `{owner}`"));
        }
        let functions: Vec<String> = row
            .get("functions")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        if functions.is_empty() || functions.iter().any(|f| f.trim().is_empty()) {
            return Err(format!("row {index} names no function: {row}"));
        }
        out.push(Row { file, functions });
    }
    Ok(out)
}

fn inventory() -> Value {
    let path = repo().join("docs/plans/e1-oom-inventory.json");
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn every_runtime_allocation_site_has_an_owner() {
    let scan = scan_kernel();
    let rows = rows(&inventory()).unwrap();
    let owned: BTreeSet<(&str, &str)> = rows
        .iter()
        .flat_map(|row| row.functions.iter().map(move |f| (row.file.as_str(), f.as_str())))
        .collect();
    let unowned: Vec<(&(String, String), &usize)> = scan
        .sites
        .iter()
        .filter(|((file, function), _)| !owned.contains(&(file.as_str(), function.as_str())))
        .collect();
    if let Ok(dump) = std::env::var("BREENIX_OOM_INVENTORY_DUMP") {
        let list: Vec<Value> = unowned
            .iter()
            .map(|((file, function), line)| serde_json::json!({"file": file, "function": function, "line": line}))
            .collect();
        std::fs::write(dump, serde_json::to_string_pretty(&list).unwrap()).unwrap();
    }
    let shown: Vec<String> = unowned
        .iter()
        .take(40)
        .map(|((file, function), line)| format!("  {file}:{line} {function}"))
        .collect();
    assert!(
        unowned.is_empty(),
        "{} of {} runtime allocation sites have no inventory row, for example:\n{}",
        unowned.len(),
        scan.sites.len(),
        shown.join("\n")
    );
}

#[test]
fn a_row_without_owner_error_or_rollback_is_refused() {
    let complete = serde_json::json!({
        "file": "kernel/src/x.rs", "owner": "E1-14", "error": "ENOMEM",
        "rollback": "before the fd is published", "functions": ["open"]
    });
    assert!(rows(&serde_json::json!({ "rows": [complete.clone()] })).is_ok());
    for key in ["owner", "error", "rollback"] {
        for value in [Value::Null, Value::String(String::new())] {
            let mut row = complete.clone();
            row[key] = value;
            assert!(
                rows(&serde_json::json!({ "rows": [row] })).is_err(),
                "a row whose `{key}` is empty passed"
            );
        }
    }
    let mut unknown = complete.clone();
    unknown["owner"] = Value::String("E1-99".into());
    assert!(rows(&serde_json::json!({ "rows": [unknown] })).is_err());
    let mut bare = complete;
    bare["functions"] = serde_json::json!([]);
    assert!(rows(&serde_json::json!({ "rows": [bare] })).is_err());
}

#[test]
fn the_inventory_rows_are_complete() {
    rows(&inventory()).unwrap();
}
