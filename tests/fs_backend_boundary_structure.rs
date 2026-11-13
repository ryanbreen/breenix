use std::fs;
use std::path::{Path, PathBuf};

/// The staged migration in docs/fs/cowfs-design.md section 1.1 lowers this
/// ext2-only ceiling at each stage and never raises it.
/// claim-lint:ok: Staged migration requirement in docs/fs/cowfs-design.md section 1.1.
const CEILING: usize = 184;

fn visit(directory: &Path, ext2_dir: &Path, tally: &mut Vec<(PathBuf, usize)>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read directory {}: {error}", directory.display()))
    {
        let path = entry.expect("read directory entry").path();
        if path == ext2_dir {
            continue;
        }
        if path.is_dir() {
            visit(&path, ext2_dir, tally);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            let count = source
                .lines()
                .filter(|line| line.contains("fs::ext2") || line.contains("ext2::"))
                .count();
            if count > 0 {
                tally.push((path, count));
            }
        }
    }
}

#[test]
fn ext2_references_outside_ext2_stay_under_ceiling() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let kernel = root.join("kernel/src");
    let mut tally = Vec::new();
    visit(&kernel, &kernel.join("fs/ext2"), &mut tally);
    tally.sort();
    let total: usize = tally.iter().map(|(_, count)| count).sum();
    let details = tally
        .iter()
        .map(|(path, count)| format!("{}: {count}", path.strip_prefix(root).unwrap().display()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        total <= CEILING,
        "ext2 references outside fs/ext2: {total} lines (ceiling {CEILING})\n{details}"
    );
}

#[test]
fn read_side_callers_take_errno() {
    use syn::visit::{self, Visit};

    fn error_value(expr: &syn::Expr) -> bool {
        matches!(expr, syn::Expr::Path(path) if path.path.get_ident().is_some_and(
            |name| matches!(name.to_string().as_str(), "e" | "err" | "error" | "errno")
        ))
    }
    fn string(expr: &syn::Expr) -> bool {
        matches!(expr, syn::Expr::Lit(lit) if matches!(lit.lit, syn::Lit::Str(_)))
    }
    #[derive(Default)]
    struct Boundary {
        violations: Vec<&'static str>,
    }
    impl<'ast> Visit<'ast> for Boundary {
        fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
            if matches!(&*expr.func, syn::Expr::Path(path)
                if path.path.segments.last().is_some_and(|part| part.ident == "fs_errno"))
            {
                self.violations.push("fs_errno call");
            }
            visit::visit_expr_call(self, expr);
        }
        fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
            if expr.method == "contains" && error_value(&expr.receiver)
                && expr.args.first().is_some_and(string)
            {
                self.violations.push("error.contains(string)");
            }
            visit::visit_expr_method_call(self, expr);
        }
        fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
            if matches!(expr.op, syn::BinOp::Eq(_))
                && ((error_value(&expr.left) && string(&expr.right))
                    || (string(&expr.left) && error_value(&expr.right)))
            {
                self.violations.push("error == string");
            }
            visit::visit_expr_binary(self, expr);
        }
        fn visit_expr_path(&mut self, expr: &'ast syn::ExprPath) {
            // Passing the mapper to map_err is a call at the ext2 boundary too.
            if expr.path.segments.last().is_some_and(|part| part.ident == "fs_errno") {
                self.violations.push("fs_errno mapper");
            }
            visit::visit_expr_path(self, expr);
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();
    for file in ["fs/namei.rs", "syscall/metadata.rs", "syscall/mmap.rs", "syscall/fifo.rs"] {
        let source = fs::read_to_string(root.join("kernel/src").join(file)).unwrap();
        let mut boundary = Boundary::default();
        boundary.visit_file(&syn::parse_file(&source).unwrap());
        for violation in boundary.violations {
            violations.push(format!("{file}: {violation}"));
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}


#[test]
fn no_ext2_error_strings_above_ext2() {
    use syn::visit::{self, Visit};

    // These strings belong to other subsystems, not the ext2 error boundary.
    // Each exemption names its producer's consumer and the exact string tested.
    const NON_EXT2: &[(&str, &str, &str)] = &[
        ("syscall/handlers.rs", "write_with_limit", "out of memory"), // user-buffer copy
        ("gpu/mod.rs", "native_spans", "native crossing upload backing allocation"),
        ("gpu/mod.rs", "submit_stream", "native crossing allocation"),
        ("drivers/virtio/block_mmio.rs", "test_invalid_sector", "Sector out of range"),
        ("gpu/gk208/gr/channel.rs", "issue_words", "GR control stream not consumed; backing retained"),
    ];

    fn error_value(expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::Path(path) => path.path.get_ident().is_some_and(|name| {
                matches!(name.to_string().as_str(), "e" | "err" | "error" | "errno")
                    || name.to_string().ends_with("_error")
                    || name.to_string().ends_with("_err")
            }),
            syn::Expr::Paren(expr) => error_value(&expr.expr),
            syn::Expr::Reference(expr) => error_value(&expr.expr),
            syn::Expr::Unary(expr) => error_value(&expr.expr),
            _ => false,
        }
    }
    fn string(expr: &syn::Expr) -> Option<String> {
        match expr {
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(text), .. }) => Some(text.value()),
            _ => None,
        }
    }
    struct Boundary<'a> {
        file: &'a str,
        function: String,
        ext2: bool,
        violations: &'a mut Vec<String>,
    }
    impl Boundary<'_> {
        fn check(&mut self, literal: String) {
            if !self.ext2 && !NON_EXT2.iter().any(|&(file, function, text)| {
                file == self.file && function == self.function && text == literal
            }) {
                self.violations.push(format!("{}::{}: error string {literal:?}", self.file, self.function));
            }
        }
    }
    impl<'ast> Visit<'ast> for Boundary<'_> {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if ident == "fs_errno" {
                self.violations.push(format!("{}: fs_errno definition or use", self.file));
            }
        }
        fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
            let old = std::mem::replace(&mut self.function, item.sig.ident.to_string());
            visit::visit_item_fn(self, item);
            self.function = old;
        }
        fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
            let old = std::mem::replace(&mut self.function, item.sig.ident.to_string());
            visit::visit_impl_item_fn(self, item);
            self.function = old;
        }
        fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
            if expr.method == "contains" && error_value(&expr.receiver) {
                if let Some(text) = expr.args.first().and_then(string) { self.check(text); }
            }
            visit::visit_expr_method_call(self, expr);
        }
        fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
            if matches!(expr.op, syn::BinOp::Eq(_)) {
                if error_value(&expr.left) {
                    if let Some(text) = string(&expr.right) { self.check(text); }
                }
                if error_value(&expr.right) {
                    if let Some(text) = string(&expr.left) { self.check(text); }
                }
            }
            visit::visit_expr_binary(self, expr);
        }
    }
    fn walk(directory: &Path, kernel: &Path, violations: &mut Vec<String>) {
        let mut entries = fs::read_dir(directory).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, kernel, violations);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let file = path.strip_prefix(kernel).unwrap().to_str().unwrap();
                let source = fs::read_to_string(&path).unwrap();
                Boundary { file, function: String::new(), ext2: file.starts_with("fs/ext2/"), violations }
                    .visit_file(&syn::parse_file(&source).unwrap_or_else(|e| panic!("{file}: {e}")));
            }
        }
    }
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src");
    let mut violations = Vec::new();
    walk(&kernel, &kernel, &mut violations);
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
