//! Repository hygiene oracle: no tracked symlink may name an absolute path,
//! and the forked-Rust escape hatch must stay documented (#678).
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

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    std::fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn git(args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root())
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
fn tracked_symlinks() -> Vec<(String, String)> {
    let index = git(&["ls-files", "-s"]);
    assert!(
        !index.trim().is_empty(),
        "git index listing is empty - the oracle would be vacuous"
    );

    index
        .lines()
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
            Some((path.to_owned(), git(&["cat-file", "blob", object])))
        })
        .collect()
}

/// A symlink target is portable when it does not begin at the filesystem root.
fn target_is_absolute(target: &str) -> bool {
    target.starts_with('/')
}

#[test]
fn tracked_symlinks_name_no_absolute_path() {
    let offenders: Vec<String> = tracked_symlinks()
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
