//! W1: input ingress is shared, IRQ-safe, and retains the console fallback.
use quote::ToTokens;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::{
    visit::{self, Visit},
    Expr, ImplItem, Item,
};

fn input_sources(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut sources = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources.extend(input_sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            sources.push((path.clone(), fs::read_to_string(path).unwrap()));
        }
    }
    sources
}
fn compact(node: &impl ToTokens) -> String {
    node.to_token_stream()
        .to_string()
        .split_whitespace()
        .collect()
}
#[derive(Default)]
struct Attributes(Vec<String>);
impl<'ast> Visit<'ast> for Attributes {
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if compact(attr).contains("target_arch") {
            self.0.push(compact(attr));
        }
        visit::visit_attribute(self, attr);
    }
}
fn functions(items: &[Item], out: &mut BTreeMap<String, Vec<syn::Block>>) {
    for item in items {
        match item {
            Item::Fn(f) => out
                .entry(f.sig.ident.to_string())
                .or_default()
                .push(*f.block.clone()),
            Item::Impl(i) => {
                for item in &i.items {
                    if let ImplItem::Fn(f) = item {
                        out.entry(f.sig.ident.to_string())
                            .or_default()
                            .push(f.block.clone());
                    }
                }
            }
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    functions(items, out);
                }
            }
            _ => {}
        }
    }
}
#[derive(Default)]
struct Calls {
    masked: bool,
    edges: Vec<(String, bool)>,
    unsafe_locks: Vec<String>,
    console: bool,
}
impl<'ast> Visit<'ast> for Calls {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(path) = &*call.func {
            let name = path.path.segments.last().unwrap().ident.to_string();
            self.console |= compact(&path.path).ends_with("console::keys::key");
            self.edges.push((name.clone(), self.masked));
            if name == "arch_without_interrupts" || name == "without_interrupts" {
                let previous = self.masked;
                self.masked = true;
                for arg in &call.args {
                    self.visit_expr(arg);
                }
                self.masked = previous;
                return;
            }
        }
        visit::visit_expr_call(self, call);
    }
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let name = call.method.to_string();
        if name == "lock" && !self.masked {
            self.unsafe_locks.push(compact(call));
        }
        self.edges.push((name, self.masked));
        visit::visit_expr_method_call(self, call);
    }
}
fn report_lock_errors(defs: &BTreeMap<String, Vec<syn::Block>>) -> Vec<String> {
    let mut pending: Vec<_> = defs
        .keys()
        .filter(|name| name.starts_with("report") || name.as_str() == "publish_hardware")
        .map(|name| (name.clone(), false))
        .collect();
    let mut seen = BTreeSet::new();
    let mut errors = Vec::new();
    while let Some((name, masked)) = pending.pop() {
        if !seen.insert((name.clone(), masked)) {
            continue;
        }
        let Some(blocks) = defs.get(&name) else {
            continue;
        };
        for block in blocks {
            let mut calls = Calls {
                masked,
                ..Calls::default()
            };
            calls.visit_block(block);
            errors.extend(
                calls
                    .unsafe_locks
                    .into_iter()
                    .map(|lock| format!("{name}: {lock}")),
            );
            pending.extend(
                calls
                    .edges
                    .into_iter()
                    .filter(|(name, _)| defs.contains_key(name)),
            );
        }
    }
    errors
}
fn calls(block: &syn::Block) -> Calls {
    let mut calls = Calls::default();
    calls.visit_block(block);
    calls
}
fn console_fallback(defs: &BTreeMap<String, Vec<syn::Block>>) -> bool {
    // The task-context stream selects console keys when no reader holds the
    // grab, then its injection/removal producers deliver those keys to keys.rs.
    let publish = defs.get("publish").is_some_and(|blocks| {
        blocks.iter().any(|b| {
            struct Fallback(bool);
            impl<'ast> Visit<'ast> for Fallback {
                fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
                    self.0 |= compact(&e.cond).contains("holder.is_none()")
                        && compact(&e.then_branch).contains("console.push(");
                    visit::visit_expr_if(self, e);
                }
            }
            let mut fallback = Fallback(false);
            fallback.visit_block(b);
            fallback.0
        })
    });
    publish
        && ["inject", "release_service", "report_key"]
            .iter()
            .all(|name| {
                defs.get(*name)
                    .is_some_and(|blocks| blocks.iter().any(|b| calls(b).console))
            })
}

#[test]
fn input_core_has_no_architecture_conditionals() {
    let root = Path::new(file!())
        .canonicalize()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("kernel/src/input");
    let mut bad = Vec::new();
    for (path, source) in input_sources(&root) {
        let tree = syn::parse_file(&source).unwrap();
        let mut attrs = Attributes::default();
        attrs.visit_file(&tree);
        bad.extend(
            attrs
                .0
                .into_iter()
                .map(|a| format!("{}: {a}", path.display())),
        );
    }
    assert!(
        bad.is_empty(),
        "architecture conditionals in input core: {bad:?}"
    );
}
#[test]
fn report_paths_mask_interrupts_or_try_locks_and_keep_console() {
    let root = Path::new(file!())
        .canonicalize()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("kernel/src/input");
    assert!(
        root.join("stream.rs").is_file(),
        "shared input stream is absent"
    );
    let mut defs = BTreeMap::new();
    for (_, source) in input_sources(&root) {
        functions(&syn::parse_file(&source).unwrap().items, &mut defs);
    }
    let errors = report_lock_errors(&defs);
    assert!(
        errors.is_empty(),
        "unmasked blocking report-path locks: {errors:?}"
    );
    assert!(
        console_fallback(&defs),
        "no-reader path must reach console/keys.rs"
    );
}
#[test]
fn structure_checker_detects_unsafe_report_helpers() {
    let mut defs = BTreeMap::new();
    functions(
        &syn::parse_file("fn report() { helper(); } fn helper() { STATE.lock(); }")
            .unwrap()
            .items,
        &mut defs,
    );
    assert_eq!(report_lock_errors(&defs).len(), 1);
    defs.clear();
    functions(&syn::parse_file("fn report() { crate::arch_without_interrupts(|| helper()); STATE.try_lock(); } fn helper() { STATE.lock(); }").unwrap().items, &mut defs);
    assert!(report_lock_errors(&defs).is_empty());
    assert!(!console_fallback(&defs));
    let mut attrs = Attributes::default();
    attrs.visit_file(
        &syn::parse_file("#[cfg(any(target_arch = \"aarch64\", feature = \"x\"))] fn report() {}")
            .unwrap(),
    );
    assert_eq!(attrs.0.len(), 1);
}
