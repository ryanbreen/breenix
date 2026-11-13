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
