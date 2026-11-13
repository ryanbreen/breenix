use std::fs;
use std::path::{Path, PathBuf};

/// The staged migration in docs/fs/cowfs-design.md section 1.1 lowers this
/// ext2-only ceiling at each stage and never raises it.
/// claim-lint:ok: Staged migration requirement in docs/fs/cowfs-design.md section 1.1.
const CEILING: usize = 122;

#[test]
fn link_and_rename_use_the_trait() {
    use syn::visit::{self, Visit};

    #[derive(Default)]
    struct Boundary { backend: Vec<String>, link: bool, rename: bool }
    impl<'ast> Visit<'ast> for Boundary {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if matches!(ident.to_string().as_str(), "ext2" | "ext2_errno" | "RenameError"
                | "FileType" | "read_inode" | "lookup_in_dir" | "link_in_dir" | "verify") {
                self.backend.push(ident.to_string());
            }
        }
        fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
            self.link |= expr.method == "link";
            self.rename |= expr.method == "rename";
            visit::visit_expr_method_call(self, expr);
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/syscall/fs.rs");
    let parsed = syn::parse_file(&fs::read_to_string(path).unwrap()).unwrap();
    let mut boundary = Boundary::default();
    for name in ["sys_link_from", "sys_rename_from", "sys_linkat", "sys_renameat", "sys_renameat2"] {
        let function = parsed.items.iter().find_map(|item| match item {
            syn::Item::Fn(f) if f.sig.ident == name => Some(f), _ => None,
        });
        if let Some(function) = function {
            boundary.visit_item_fn(function);
        } else {
            assert_eq!(name, "sys_renameat2", "missing fs.rs::{name}");
        }
    }
    assert!(boundary.backend.is_empty(), "backend names: {}", boundary.backend.join(", "));
    assert!(boundary.link && boundary.rename, "link and rename must call the filesystem trait");
}

#[test]
fn unix_socket_names_use_the_trait() {
    use syn::visit::Visit;

    #[derive(Default)]
    struct BackendNames(Vec<String>);
    impl<'ast> Visit<'ast> for BackendNames {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if matches!(ident.to_string().as_str(),
                "ext2" | "FileHandle" | "read_inode"
                | "create_node_prepared" | "pin_loaded_inode")
            {
                self.0.push(ident.to_string());
            }
        }
    }

    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("kernel/src/socket/unix_ns.rs")).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    let mut names = BackendNames::default();
    names.visit_file(&parsed);
    assert!(names.0.is_empty(), "unix_ns.rs names {}", names.0.join(", "));
}

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
    fn string(expr: &syn::Expr, constants: &std::collections::BTreeMap<String, String>) -> Option<String> {
        match expr {
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(text), .. }) => Some(text.value()),
            syn::Expr::Path(path) => constants.get(&path.path.segments.last()?.ident.to_string()).cloned(),
            syn::Expr::Paren(expr) => string(&expr.expr, constants),
            syn::Expr::Reference(expr) => string(&expr.expr, constants),
            _ => None,
        }
    }
    #[derive(Default)]
    struct Constants(std::collections::BTreeMap<String, String>);
    impl<'ast> Visit<'ast> for Constants {
        fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
            if let Some(text) = string(&item.expr, &self.0) {
                self.0.insert(item.ident.to_string(), text);
            }
            visit::visit_item_const(self, item);
        }
    }
    struct Boundary<'a> {
        file: &'a str,
        constants: &'a std::collections::BTreeMap<String, String>,
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
            if ["contains", "starts_with", "ends_with"].iter().any(|method| expr.method == method) && error_value(&expr.receiver) {
                if let Some(text) = expr.args.first().and_then(|arg| string(arg, self.constants)) { self.check(text); }
            }
            visit::visit_expr_method_call(self, expr);
        }
        fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
            if matches!(expr.op, syn::BinOp::Eq(_)) {
                if error_value(&expr.left) {
                    if let Some(text) = string(&expr.right, self.constants) { self.check(text); }
                }
                if error_value(&expr.right) {
                    if let Some(text) = string(&expr.left, self.constants) { self.check(text); }
                }
            }
            visit::visit_expr_binary(self, expr);
        }
    }
    fn walk(directory: &Path, kernel: &Path, constants: &Constants, violations: &mut Vec<String>) {
        let mut entries = fs::read_dir(directory).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, kernel, constants, violations);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let file = path.strip_prefix(kernel).unwrap().to_str().unwrap();
                let source = fs::read_to_string(&path).unwrap();
                let parsed = syn::parse_file(&source).unwrap_or_else(|e| panic!("{file}: {e}"));
                let mut local = Constants(constants.0.clone());
                local.visit_file(&parsed);
                Boundary { file, constants: &local.0, function: String::new(), ext2: file.starts_with("fs/ext2/"), violations }
                    .visit_file(&parsed);
            }
        }
    }
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src");
    let mut violations = Vec::new();
    let mut constants = Constants::default();
    constants.visit_file(&syn::parse_file(&fs::read_to_string(kernel.join("fs/ext2/mod.rs")).unwrap()).unwrap());
    walk(&kernel, &kernel, &constants, &mut violations);
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn inode_numbers_above_ext2_are_u64() {
    use syn::visit::{self, Visit};

    fn integer(ty: &syn::Type, name: &str) -> bool {
        matches!(ty, syn::Type::Path(path) if path.path.is_ident(name))
    }
    fn inode_name(name: &str) -> bool {
        matches!(name, "ino" | "inode_num" | "inode_number" | "parent_ino")
    }
    #[derive(Default)]
    struct Boundary(Vec<String>);
    impl<'ast> Visit<'ast> for Boundary {
        fn visit_field(&mut self, field: &'ast syn::Field) {
            if field.ident.as_ref().is_some_and(|id| inode_name(&id.to_string()))
                && integer(&field.ty, "u32")
            {
                self.0.push("u32 inode field".into());
            }
            visit::visit_field(self, field);
        }
        fn visit_pat_type(&mut self, pat: &'ast syn::PatType) {
            if matches!(&*pat.pat, syn::Pat::Ident(id) if inode_name(&id.ident.to_string()))
                && integer(&pat.ty, "u32")
            {
                self.0.push("u32 inode parameter".into());
            }
            visit::visit_pat_type(self, pat);
        }
        fn visit_expr_cast(&mut self, cast: &'ast syn::ExprCast) {
            if integer(&cast.ty, "u32") {
                let inode = match &*cast.expr {
                    syn::Expr::Path(path) => path.path.segments.last().is_some_and(|id| inode_name(&id.ident.to_string())),
                    syn::Expr::Field(field) => matches!(&field.member, syn::Member::Named(id) if inode_name(&id.to_string()) || id == "inode"),
                    _ => false,
                };
                if inode { self.0.push("unchecked inode narrowing".into()); }
            }
            visit::visit_expr_cast(self, cast);
        }
    }
    fn walk(directory: &Path, violations: &mut Vec<String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.ends_with("fs/ext2") { continue; }
            if path.is_dir() { walk(&path, violations); }
            else if path.extension().is_some_and(|ext| ext == "rs") {
                let mut boundary = Boundary::default();
                boundary.visit_file(&syn::parse_file(&fs::read_to_string(&path).unwrap()).unwrap());
                for error in boundary.0 { violations.push(format!("{}: {error}", path.display())); }
            }
        }
    }
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src");
    let source = fs::read_to_string(kernel.join("fs/namei.rs")).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    let target = parsed.items.iter().find_map(|item| match item {
        syn::Item::Enum(item) if item.ident == "Target" => Some(item),
        _ => None,
    }).unwrap();
    let mut violations = Vec::new();
    for (variant, field) in [("Inode", "ino"), ("Absent", "parent")] {
        let ty = &target.variants.iter().find(|v| v.ident == variant).unwrap()
            .fields.iter().find(|f| f.ident.as_ref().is_some_and(|id| id == field)).unwrap().ty;
        if !integer(ty, "u64") { violations.push(format!("Target::{variant}.{field} must be u64")); }
    }
    walk(&kernel, &mut violations);
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn home_prefix_helpers_stay_in_ext2() {
    fn walk(directory: &Path, violations: &mut Vec<String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.ends_with("fs/ext2") { continue; }
            if path.is_dir() { walk(&path, violations); }
            else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = fs::read_to_string(&path).unwrap();
                for (line, text) in source.lines().enumerate() {
                    if text.contains("strip_home_prefix") || text.contains("is_home_path") {
                        violations.push(format!("{}:{}: {text}", path.display(), line + 1));
                    }
                }
            }
        }
    }
    let mut violations = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src"), &mut violations);
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn namei_enters_mounts_only_through_the_table() {
    use syn::visit::{self, Visit};
    #[derive(Default)]
    struct Names(Vec<String>);
    impl<'ast> Visit<'ast> for Names {
        fn visit_arm(&mut self, arm: &'ast syn::Arm) {
            struct Pattern<'a>(&'a mut Vec<String>);
            impl<'ast> Visit<'ast> for Pattern<'_> {
                fn visit_lit_byte_str(&mut self, literal: &'ast syn::LitByteStr) {
                    if matches!(literal.value().as_slice(), b"home" | b"dev" | b"proc") {
                        self.0.push(format!("mount-name match arm: {:?}", literal.value()));
                    }
                }
            }
            Pattern(&mut self.0).visit_pat(&arm.pat);
            visit::visit_arm(self, arm);
        }
    }
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/fs/namei.rs")).unwrap();
    let mut names = Names::default();
    names.visit_file(&syn::parse_file(&source).unwrap());
    assert!(!source.contains("ufs::covering"), "namei calls the separate ufs cover lookup");
    assert!(names.0.is_empty(), "{}", names.0.join("\n"));
}

#[test]
fn open_file_descriptions_hold_vnodes() {
    use syn::visit::{self, Visit};

    fn structure<'a>(file: &'a syn::File, name: &str) -> &'a syn::ItemStruct {
        file.items.iter().find_map(|item| match item {
            syn::Item::Struct(item) if item.ident == name => Some(item),
            _ => None,
        }).unwrap_or_else(|| panic!("missing struct {name}"))
    }
    fn named(ty: &syn::Type, name: &str) -> bool {
        matches!(ty, syn::Type::Path(path) if path.path.segments.last().is_some_and(|p| p.ident == name))
    }
    #[derive(Default)]
    struct Handles(bool);
    impl<'ast> Visit<'ast> for Handles {
        fn visit_type_path(&mut self, path: &'ast syn::TypePath) {
            self.0 |= path.path.segments.iter().any(|p| p.ident == "FileHandle");
            visit::visit_type_path(self, path);
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src");
    let fd = syn::parse_file(&fs::read_to_string(root.join("ipc/fd.rs")).unwrap()).unwrap();
    let vfs = syn::parse_file(&fs::read_to_string(root.join("fs/vfs/file.rs")).unwrap()).unwrap();
    let open = structure(&vfs, "OpenFile");
    for name in ["RegularFile", "DirectoryFile"] {
        let description = structure(&fd, name);
        assert!(description.fields.iter().any(|f| named(&f.ty, "OpenFile")),
            "{name} must retain an OpenFile");
        let mut handles = Handles::default();
        handles.visit_item_struct(description);
        handles.visit_item_struct(open);
        assert!(!handles.0, "{name} must not hold a FileHandle field");
        assert!(open.fields.iter().any(|field| {
            let syn::Type::Path(path) = &field.ty else { return false; };
            let Some(arc) = path.path.segments.last() else { return false; };
            let syn::PathArguments::AngleBracketed(args) = &arc.arguments else { return false; };
            arc.ident == "Arc" && args.args.len() == 1
                && matches!(args.args.first(), Some(syn::GenericArgument::Type(ty)) if named(ty, "VNode"))
        }), "{name}'s OpenFile must hold Arc<VNode>");
    }
}

#[test]
fn open_file_read_syscalls_use_the_trait() {
    use syn::visit::Visit;

    #[derive(Default)]
    struct BackendNames(Vec<String>);
    impl<'ast> Visit<'ast> for BackendNames {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if matches!(ident.to_string().as_str(),
                "ext2" | "ext2_handle" | "FileHandle" | "Mount" | "read_inode"
                | "read_file_range" | "sync_range" | "root_fs_read" | "home_fs_read" | "home_mount_id"
                | "read_file_range_coherent" | "get_ext2_file_size_for_handle"
                | "load_ext2_inode_stat_for_handle" | "load_ext2_inode_stat_for_mount"
                | "update_read_atime")
            {
                self.0.push(ident.to_string());
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/syscall");
    let mut violations = Vec::new();
    for (file, functions) in [
        ("handlers.rs", ["sys_read", "sys_pread64"]),
        ("fs.rs", ["sys_lseek", "sys_fstat"]),
    ] {
        let parsed = syn::parse_file(&fs::read_to_string(root.join(file)).unwrap()).unwrap();
        for name in functions {
            let function = parsed.items.iter().find_map(|item| match item {
                syn::Item::Fn(function) if function.sig.ident == name => Some(function),
                _ => None,
            }).unwrap_or_else(|| panic!("missing {file}::{name}"));
            let mut names = BackendNames::default();
            names.visit_block(&function.block);
            for backend in names.0 {
                violations.push(format!("{file}::{name} names {backend}"));
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn open_file_write_syscalls_use_the_trait() {
    use syn::visit::Visit;

    #[derive(Default)]
    struct BackendNames(Vec<String>);
    impl<'ast> Visit<'ast> for BackendNames {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if matches!(ident.to_string().as_str(),
                "ext2" | "ext2_handle" | "FileHandle" | "Mount" | "read_inode"
                | "read_file_range" | "sync_range" | "root_fs_write" | "home_fs_write" | "home_mount_id"
                | "write_file_range_as" | "wait_for_inode")
            {
                self.0.push(ident.to_string());
            }
        }
    }

    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("kernel/src/syscall/handlers.rs")).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    let mut violations = Vec::new();
    // sys_write and write_vector share the write_with_limit implementation.
    for name in ["sys_write", "write_with_limit", "sys_pwrite64"] {
        let function = parsed.items.iter().find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        }).unwrap_or_else(|| panic!("missing handlers.rs::{name}"));
        let mut names = BackendNames::default();
        names.visit_block(&function.block);
        for backend in names.0 {
            violations.push(format!("handlers.rs::{name} names {backend}"));
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn directory_and_sync_syscalls_use_the_trait() {
    use syn::visit::Visit;

    #[derive(Default)]
    struct BackendNames(Vec<String>);
    impl<'ast> Visit<'ast> for BackendNames {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if matches!(ident.to_string().as_str(),
                "ext2" | "ext2_handle" | "FileHandle" | "Mount" | "read_inode"
                | "read_file_range" | "sync_range" | "read_mount" | "write_mount" | "raw_entries" | "read_directory"
                | "check_shrink" | "ext2_fd_info" | "resize_inode")
            {
                self.0.push(ident.to_string());
            }
        }
    }

    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("kernel/src/syscall/fs.rs")).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    let mut violations = Vec::new();
    for name in ["sys_getdents64", "sys_fsync", "sys_ftruncate"] {
        let function = parsed.items.iter().find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        }).unwrap_or_else(|| panic!("missing fs.rs::{name}"));
        let mut names = BackendNames::default();
        names.visit_block(&function.block);
        for backend in names.0 {
            violations.push(format!("fs.rs::{name} names {backend}"));
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn write_signals_only_for_rejected_fsize() {
    use syn::visit::{self, Visit};
    struct Signals { success: bool, bad: Vec<String> }
    impl<'ast> Visit<'ast> for Signals {
        fn visit_arm(&mut self, arm: &'ast syn::Arm) {
            let old = self.success;
            if matches!(&arm.pat, syn::Pat::TupleStruct(p) if p.path.is_ident("Ok")) {
                self.success = true;
            }
            visit::visit_arm(self, arm);
            self.success = old;
        }
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if matches!(&*call.func, syn::Expr::Path(p) if p.path.segments.last().unwrap().ident == "signal_fsize") && self.success {
                self.bad.push("successful writes must not signal SIGXFSZ".into());
            }
            visit::visit_expr_call(self, call);
        }
        fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
            if matches!(&*expr.right, syn::Expr::Path(p) if p.path.segments.last().unwrap().ident == "EFBIG") {
                self.bad.push("filesystem EFBIG must not imply SIGXFSZ".into());
            }
            visit::visit_expr_binary(self, expr);
        }
    }
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/syscall/handlers.rs")).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    let mut signals = Signals { success: false, bad: Vec::new() };
    for item in &parsed.items {
        if let syn::Item::Fn(f) = item {
            if f.sig.ident == "write_with_limit" || f.sig.ident == "sys_pwrite64" {
                signals.visit_block(&f.block);
            }
        }
    }
    assert!(signals.bad.is_empty(), "{}", signals.bad.join("\n"));
}

#[test]
fn getdents_collects_a_batch_in_one_backend_traversal() {
    use syn::visit::{self, Visit};
    #[derive(Default)]
    struct Traversal { loops: usize, calls: usize }
    impl<'ast> Visit<'ast> for Traversal {
        fn visit_expr_loop(&mut self, expr: &'ast syn::ExprLoop) {
            self.loops += 1;
            visit::visit_expr_loop(self, expr);
            self.loops -= 1;
        }
        fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
            if expr.method == "dirents" {
                self.calls += 1;
                assert_eq!(self.loops, 0, "getdents must not restart traversal per entry");
                let Some(syn::Expr::Reference(r)) = expr.args.last() else { panic!("missing sink") };
                let syn::Expr::Closure(c) = &*r.expr else { panic!("missing sink closure") };
                assert_eq!(c.inputs.len(), 2, "sink needs each entry's next cookie");
            }
            visit::visit_expr_method_call(self, expr);
        }
    }
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/syscall/fs.rs")).unwrap();
    let parsed = syn::parse_file(&source).unwrap();
    let f = parsed.items.iter().find_map(|i| match i {
        syn::Item::Fn(f) if f.sig.ident == "sys_getdents64" => Some(f), _ => None
    }).unwrap();
    let mut traversal = Traversal::default();
    traversal.visit_block(&f.block);
    assert_eq!(traversal.calls, 1);
}

#[test]
fn remove_syscalls_use_the_trait() {
    use syn::visit::Visit;

    #[derive(Default)]
    struct BackendNames(Vec<String>);
    impl<'ast> Visit<'ast> for BackendNames {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if matches!(ident.to_string().as_str(),
                "ext2" | "ext2_errno" | "Ext2Inode" | "read_inode"
                | "lookup_in_dir_bytes" | "unlink_in_dir" | "remove_directory_in"
                | "write_removal")
            {
                self.0.push(ident.to_string());
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/syscall");
    let parsed = syn::parse_file(&fs::read_to_string(root.join("fs.rs")).unwrap()).unwrap();
    let mut violations = Vec::new();
    for name in ["sys_unlink_from", "sys_rmdir_from", "sys_unlinkat", "remove_entry"] {
        let function = parsed.items.iter().find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        }).unwrap_or_else(|| panic!("missing fs.rs::{name}"));
        let mut names = BackendNames::default();
        names.visit_item_fn(function);
        for backend in names.0 {
            violations.push(format!("fs.rs::{name} names {backend}"));
        }
    }
    let policy = syn::parse_file(&fs::read_to_string(root.join("fifo_policy.rs")).unwrap()).unwrap();
    let mut names = BackendNames::default();
    names.visit_file(&policy);
    for backend in names.0 {
        violations.push(format!("fifo_policy.rs names {backend}"));
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn create_syscalls_use_the_trait() {
    use syn::visit::Visit;

    #[derive(Default)]
    struct BackendNames(Vec<String>);
    impl<'ast> Visit<'ast> for BackendNames {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            let name = ident.to_string();
            if matches!(name.as_str(),
                "ext2" | "ext2_errno" | "read_inode" | "lookup_in_dir_bytes" | "create_node")
                || name.starts_with("create_directory") || name.starts_with("create_symlink")
            {
                self.0.push(name);
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/syscall");
    let mut violations = Vec::new();
    for (file, functions) in [
        ("fs.rs", &["sys_mkdir_from", "sys_symlink_from"][..]),
        ("fifo.rs", &["mknodat_path"][..]),
    ] {
        let parsed = syn::parse_file(&fs::read_to_string(root.join(file)).unwrap()).unwrap();
        for name in functions {
            let function = parsed.items.iter().find_map(|item| match item {
                syn::Item::Fn(function) if function.sig.ident == *name => Some(function),
                _ => None,
            }).unwrap_or_else(|| panic!("missing {file}::{name}"));
            let mut names = BackendNames::default();
            names.visit_item_fn(function);
            for backend in names.0 {
                violations.push(format!("{file}::{name} names {backend}"));
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
