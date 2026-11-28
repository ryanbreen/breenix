//! Repository hygiene oracle: tracked symlinks and Rust module includes must
//! resolve to files, and tracked symlinks must use relative paths (#678).
//!
//! `rust-fork` was committed as a symlink to
//! `/Users/wrb/fun/code/breenix-parallels/rust-fork`. An absolute path into one
//! developer's home directory resolves on exactly one machine, so every other
//! checkout - the beast x86 VM included - got a dangling entry,
//! `userspace/programs/build.sh` reported `forked Rust library not found`, and
//! `kernel/build.rs` panicked. The fix is to keep the symlink out of version
//! control and document `BREENIX_RUST_FORK_LIBRARY` instead.
//!
//! The rule below is a shape rule over git's own index, not a list of known
//! offenders: any newly committed absolute symlink is caught the moment it is
//! written.

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use std::path::{Path, PathBuf};
use std::process::Command;
use syn::{punctuated::Punctuated, visit::Visit, Expr, ItemMod, Lit, Meta, Token};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    std::fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn git(args: &[&str]) -> String {
    git_at(&repo_root(), args)
}

fn git_at(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output is UTF-8")
}

/// The git file mode of a symbolic link.
const SYMLINK_MODE: &str = "120000";

/// Every tracked symlink, as `(path, link target)`.
fn tracked_symlinks(root: &Path) -> Vec<(String, String)> {
    let index = git_at(root, &["ls-files", "-s", "-z"]);
    assert!(
        !index.trim().is_empty(),
        "git index listing is empty - the oracle would be vacuous"
    );

    index
        .split('\0')
        .filter_map(|line| {
            // "<mode> <object> <stage>\t<path>"
            let (meta, path) = line.split_once('\t')?;
            let mut fields = meta.split_whitespace();
            let mode = fields.next()?;
            let object = fields.next()?;
            if mode != SYMLINK_MODE {
                return None;
            }
            // A symlink blob's contents are its target.
            Some((path.to_owned(), git_at(root, &["cat-file", "blob", object])))
        })
        .collect()
}

fn collect_paths(meta: &Meta, paths: &mut Vec<String>) {
    if meta.path().is_ident("path") {
        let Meta::NameValue(value) = meta else {
            panic!("expected a path attribute with a string value");
        };
        let Expr::Lit(value) = &value.value else {
            panic!("expected a literal module path");
        };
        let Lit::Str(path) = &value.lit else {
            panic!("expected a string module path");
        };
        paths.push(path.value());
    } else if let Meta::List(list) = meta {
        if list.path.is_ident("cfg_attr") {
            let args = list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .expect("parse cfg_attr on a module");
            for attr in args.iter().skip(1) {
                collect_paths(attr, paths);
            }
        }
    }
}

fn has_path_attribute(tokens: TokenStream) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Group(group) if group.delimiter() == Delimiter::Bracket => {
            let tokens = group.stream();
            matches!(tokens.into_iter().next(), Some(TokenTree::Ident(name))
                if name == "path" || name == "cfg_attr" && group.stream().to_string().contains("path"))
                || has_path_attribute(group.stream())
        }
        TokenTree::Group(group) => has_path_attribute(group.stream()),
        _ => false,
    })
}

struct ModulePaths<'a> {
    source: &'a str,
    dirs: Vec<PathBuf>,
    nested_dir: Option<PathBuf>,
    missing: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for ModulePaths<'_> {
    fn visit_item_mod(&mut self, module: &'ast ItemMod) {
        let mut paths = Vec::new();
        for attr in &module.attrs {
            collect_paths(&attr.meta, &mut paths);
        }
        if module.content.is_some() {
            let mut dirs = Vec::new();
            for dir in &self.dirs {
                dirs.extend(paths.iter().map(|path| dir.join(path)));
                if !module.attrs.iter().any(|attr| attr.path().is_ident("path")) {
                    let name = module.ident.to_string();
                    dirs.push(dir.join(name.trim_start_matches("r#")));
                    if let Some(nested) = &self.nested_dir {
                        dirs.push(nested.join(name.trim_start_matches("r#")));
                    }
                }
            }
            let mut nested = ModulePaths {
                source: self.source,
                dirs,
                nested_dir: None,
                missing: self.missing,
            };
            syn::visit::visit_item_mod(&mut nested, module);
        } else {
            for path in paths {
                if !self.dirs.iter().any(|dir| dir.join(&path).is_file()) {
                    self.missing.push(format!(
                        "{}: mod {} has #[path = {path:?}] with no file",
                        self.source, module.ident
                    ));
                }
            }
        }
    }
}

// These templates are written to a build directory after their paths have
// been substituted or their modules generated by the owning host checks.
const GENERATED_SOURCES: &[&str] = &[
    "scripts/metal/sha512-crypt-linux.rs",
    "tests/fixtures/arena_call_timing.rs",
    "tests/fixtures/breenfs_wait_retry.rs",
    "tests/fixtures/breengel_inflight_screen.rs",
    "tests/fixtures/bterm_render_host.rs",
    "tests/fixtures/bterm_screen_selection_host.rs",
    "tests/fixtures/bwm_spawn.rs",
    "tests/fixtures/completion_killable.rs",
    "tests/fixtures/gt710_constant_retirement.rs",
    "tests/fixtures/gt710_contexts_orchestration.rs",
    "tests/fixtures/gt710_contexts_submission.rs",
    "tests/fixtures/gt710_core_producer.rs",
    "tests/fixtures/gt710_display_crc.rs",
    "tests/fixtures/gt710_display_evidence.rs",
    "tests/fixtures/gt710_frame_retirement.rs",
    "tests/fixtures/gt710_gr_probe_unwind.rs",
    "tests/fixtures/gt710_gr_two_channel.rs",
    "tests/fixtures/gt710_observation_path.rs",
    "tests/fixtures/gt710_output_perturb.rs",
    "tests/fixtures/gt710_panic_transport.rs",
    "tests/fixtures/gt710_retained_acceptance.rs",
    "tests/fixtures/gt710_source_alias.rs",
    "tests/fixtures/gt710_stream_overflow.rs",
    "tests/fixtures/gt710_trap_ack_capture.rs",
    "tests/fixtures/gt710_window_front.rs",
    "tests/fixtures/gt710_window_lifetime.rs",
    "tests/fixtures/gt710_window_residency.rs",
    "tests/fixtures/guskit_async_file.rs",
    "tests/fixtures/panic_capture_complete.rs",
    "tests/fixtures/wm_native_input.rs",
    "tools/regular-read-host-tests/tests/fixtures/shared_offset.rs",
];

fn source_reference_errors(root: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    for (name, target) in tracked_symlinks(root) {
        let link = root.join(&name);
        if !link.is_symlink() || !link.is_file() || !link.parent().unwrap().join(&target).is_file()
        {
            missing.push(format!("{name}: tracked symlink -> {target:?} has no file"));
        }
    }

    let sources = git_at(
        root,
        &[
            "ls-files",
            "-z",
            "--",
            "*.rs",
            // Scan Breenix source only, per the clean-room rule.
            ":(exclude)vendor/**",
            ":(exclude)libs/libc/**",
        ],
    );
    for source in sources.split('\0').filter(|source| !source.is_empty()) {
        if GENERATED_SOURCES.contains(&source) {
            continue;
        }
        let file = root.join(source);
        if file.is_symlink() && !file.is_file() {
            continue; // Already reported by the symlink check.
        }
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|_| panic!("read repository file {source}"));
        if !text.contains("path") {
            continue;
        }
        let parsed = match syn::parse_file(&text) {
            Ok(parsed) => parsed,
            // Some checked-in test fragments are tokens inserted into a
            // generated file. A fragment with a path needs a resolution context.
            Err(_) if text.parse().is_ok_and(|tokens| !has_path_attribute(tokens)) => continue,
            Err(error) => {
                missing.push(format!("{source}: cannot check module paths: {error}"));
                continue;
            }
        };
        let parent = file.parent().unwrap();
        // Inside an unannotated inline module, a file loaded by a plain
        // `mod name;` can add its stem to the base directory. A crate root
        // or a file loaded by an explicit path uses its containing directory.
        let nested_dir =
            (file.file_name().unwrap() != "mod.rs").then(|| parent.join(file.file_stem().unwrap()));
        ModulePaths {
            source,
            dirs: vec![parent.to_path_buf()],
            nested_dir,
            missing: &mut missing,
        }
        .visit_file(&parsed);
    }
    missing
}

#[test]
fn tracked_source_references_resolve_to_files() {
    let missing = source_reference_errors(&repo_root());
    assert!(
        missing.is_empty(),
        "repository source references with no target file:\n{}",
        missing.join("\n")
    );
}

struct SourceFixture(PathBuf);

impl SourceFixture {
    fn new(name: &str, sources: &[(&str, &str)]) -> Self {
        let target = repo_root().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let root = target.join(format!("hygiene-{name}-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let fixture = Self(root);
        git_at(&fixture.0, &["init", "--quiet"]);
        for (path, source) in sources {
            let file = fixture.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, source).unwrap();
        }
        git_at(&fixture.0, &["add", "."]);
        fixture
    }

    fn errors(&self) -> Vec<String> {
        source_reference_errors(&self.0)
    }
}

impl Drop for SourceFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn source_fixture_checks_plain_modules_in_both_file_layouts() {
    let fixture = SourceFixture::new(
        "plain",
        &[
            ("src/lib.rs", "mod flat; mod directory;"),
            ("src/flat.rs", "mod missing;"),
            ("src/directory/mod.rs", ""),
        ],
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("src/flat.rs: mod missing"), "{errors:?}");
}

#[test]
fn source_fixture_checks_inline_path_without_external_children() {
    let fixture = SourceFixture::new(
        "inline",
        &[("src/lib.rs", r#"#[path = "absent"] mod inline {}"#)],
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("src/lib.rs: mod inline"), "{errors:?}");
}

#[test]
fn source_fixture_rejects_paths_found_only_in_the_other_context() {
    let fixture = SourceFixture::new(
        "context",
        &[
            (
                "src/lib.rs",
                r#"mod normal; #[path = "explicit.rs"] mod alias;"#,
            ),
            (
                "src/normal.rs",
                r#"mod inline { #[path = "normal_only.rs"] mod wrong; }"#,
            ),
            ("src/inline/normal_only.rs", ""),
            (
                "src/explicit.rs",
                r#"mod inline { #[path = "wrong.rs"] mod wrong; }"#,
            ),
            ("src/explicit/inline/wrong.rs", ""),
        ],
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors
        .iter()
        .any(|e| e.contains("src/normal.rs: mod wrong")));
    assert!(errors
        .iter()
        .any(|e| e.contains("src/explicit.rs: mod wrong")));
}

#[test]
fn source_fixture_accepts_a_tracked_directory_symlink() {
    let fixture = SourceFixture::new("symlink", &[("src/lib.rs", "")]);
    std::os::unix::fs::symlink("src", fixture.0.join("alias")).unwrap();
    git_at(&fixture.0, &["add", "alias"]);
    assert_eq!(fixture.errors(), Vec::<String>::new());
}

/// A symlink target is portable when it does not begin at the filesystem root.
fn target_is_absolute(target: &str) -> bool {
    target.starts_with('/')
}

#[test]
fn tracked_symlinks_name_no_absolute_path() {
    let offenders: Vec<String> = tracked_symlinks(&repo_root())
        .into_iter()
        .filter(|(_, target)| target_is_absolute(target))
        .map(|(path, target)| format!("{path} -> {target}"))
        .collect();

    assert!(
        offenders.is_empty(),
        "tracked symlinks name absolute paths, which dangle in every other \
         checkout (#678):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn absolute_symlink_predicate_rejects_the_committed_rust_fork_shape() {
    // The exact entry that was tracked before #678 was fixed.
    assert!(target_is_absolute(
        "/Users/wrb/fun/code/breenix-parallels/rust-fork"
    ));
    assert!(!target_is_absolute("../breenix-parallels/rust-fork"));
    assert!(!target_is_absolute("rust-fork-real"));
}

#[test]
fn rust_fork_is_untracked_and_ignored() {
    let tracked = git(&["ls-files", "--", "rust-fork"]);
    assert!(
        tracked.trim().is_empty(),
        "rust-fork is tracked again: {tracked}"
    );

    let ignored = Command::new("git")
        .args(["check-ignore", "-q", "rust-fork"])
        .current_dir(repo_root())
        .status()
        .expect("run git check-ignore");
    assert!(
        ignored.success(),
        "rust-fork is not git-ignored, so a local symlink shows up as untracked noise"
    );
}

#[test]
fn the_forked_rust_library_override_is_documented() {
    const ENV: &str = "BREENIX_RUST_FORK_LIBRARY";
    let readme = repo_text("README.md");
    assert!(
        readme.contains(ENV),
        "README does not document {ENV}, the only supported way to point a \
         checkout at the forked Rust library"
    );
    assert!(
        readme.contains("rust-fork"),
        "README does not explain the rust-fork fallback path"
    );
}
