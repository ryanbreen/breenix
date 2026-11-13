use crate::{
    ext2::errno::{classified, errno},
    fs::errno::Errno,
    syscall::errno::*,
};
use std::{collections::BTreeSet, path::Path};
use syn::visit::{self, Visit};

#[test]
fn nospace_is_enospc() {
    for message in [
        "No free blocks available",
        "No free inodes available",
        "No space in directory",
    ] {
        assert_eq!(errno(message), Errno(ENOSPC), "{message}");
    }
}

#[test]
fn nonempty_is_enotempty() {
    assert_eq!(errno("Directory not empty"), Errno(ENOTEMPTY));
}

#[test]
fn readonly_is_erofs() {
    assert_eq!(errno("Read-only filesystem"), Errno(EROFS));
}

#[test]
fn long_name_is_enametoolong() {
    assert_eq!(errno("Symlink target too long"), Errno(ENAMETOOLONG));
}

#[test]
fn corruption_and_device_errors_are_eio() {
    for message in ["Corrupt directory entry", "Failed to read inode"] {
        assert_eq!(errno(message), Errno(EIO), "{message}");
    }
    assert_eq!(errno("unrecognized ext2 failure"), Errno(EIO));
    assert_eq!(classified("unrecognized ext2 failure"), None);
}

#[derive(Default)]
struct ErrorLiterals(BTreeSet<String>);

impl<'ast> Visit<'ast> for ErrorLiterals {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            if path.path.is_ident("Err") {
                for arg in &call.args {
                    Strings(&mut self.0).visit_expr(arg);
                }
            }
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if ["ok_or", "ok_or_else", "map_err"]
            .iter()
            .any(|name| call.method == name)
        {
            for arg in &call.args {
                Strings(&mut self.0).visit_expr(arg);
            }
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        // The two mount errors cross the boundary through named constants.
        if item.ident.to_string().starts_with("EXT2_") {
            Strings(&mut self.0).visit_expr(&item.expr);
        }
    }
}

struct Strings<'a>(&'a mut BTreeSet<String>);
impl<'ast> Visit<'ast> for Strings<'_> {
    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        self.0.insert(literal.value());
    }
}

fn scan(directory: &Path, errors: &mut ErrorLiterals) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, errors);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && path.file_name().unwrap() != "errno.rs"
        {
            let source = std::fs::read_to_string(&path).unwrap();
            errors.visit_file(&syn::parse_file(&source).unwrap());
        }
    }
}

#[test]
fn every_ext2_error_literal_is_classified() {
    let mut errors = ErrorLiterals::default();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/src/fs/ext2"),
        &mut errors,
    );
    assert!(!errors.0.is_empty(), "ext2 source scan found no errors");
    let mut missing = Vec::new();
    for message in errors.0 {
        let message = Box::leak(message.into_boxed_str());
        if let Some(expected) = classified(message) {
            assert_eq!(errno(message), expected, "{message}");
        } else {
            missing.push(message);
        }
    }
    assert!(missing.is_empty(), "unclassified ext2 errors: {missing:#?}");
}
