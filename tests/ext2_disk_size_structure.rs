//! #850 ext2 disk payload-vs-size structural ratchet.
//!
//! `scripts/create_ext2_disk.sh --arch aarch64 --size 8` used to fail
//! partway through the Docker copy step with a bare `cp: write error: No
//! space left on device`, after the 8MB image had already been created,
//! formatted, and partially populated. The requested size was smaller than
//! the real userspace-binary + font payload (measured ~62MB on this
//! branch) that each image receives regardless of `--size`. #850's fix:
//! the script now computes that payload from the real `*.elf`/`*.ttf`
//! globs on disk and refuses up front -- before touching Docker/mke2fs at
//! all -- when the requested (or default) size cannot fit it, printing the
//! shortfall rather than truncating an image mid-copy. The one caller that
//! carried its own stale hardcoded `--size 8`
//! (`docker/qemu/run-aarch64-userspace.sh`, unchanged since long before
//! the aarch64 binary set grew past it) now passes no `--size` at all,
//! matching the other 12 of 13 real call sites in this tree.
//!
//! Two properties, each checked against the real files, not merely
//! asserted:
//!
//! 1. Shape (`payload_guard_structure_is_present_and_load_bearing`):
//!    create_ext2_disk.sh's own text computes a payload size from the real
//!    binary/font globs, derives a required minimum from it (not a flat
//!    hardcoded number), compares that against the requested `SIZE_MB`,
//!    and exits with a failing status -- naming the shortfall -- on the
//!    branch where it cannot fit. Each of the three pieces is proven load-bearing by
//!    mutating the real source and confirming only the matching predicate
//!    reddens, the other two staying green.
//! 2. Census (`no_caller_passes_a_size_flag`): each of the 13 real
//!    invocations of create_ext2_disk.sh across `docker/` (recursive),
//!    `scripts/` (recursive, excluding the script's own file), and
//!    `run.sh` passes no `--size` flag at all -- the exact caller-side
//!    half of #850's fix, so
//!    a future caller that reintroduces a hardcoded `--size` override (the
//!    shape that caused #850) reddens by name instead of silently drifting
//!    stale again.
//!
//! Deliberately narrower than "the caller-side line contains the
//! `create_ext2_disk.sh` substring": an `echo "...create_ext2_disk.sh..."`
//! help/usage string contains the token too without calling it, and the
//! script's own header-comment usage examples contain it (one of them
//! together with `--size`, since `--size` is a real, documented flag) as
//! documentation, not a call site. `is_create_ext2_disk_invocation` below
//! matches only the two invocation idioms real call sites in this tree
//! use.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

const CREATE_EXT2_DISK_SCRIPT: &str = "scripts/create_ext2_disk.sh";

fn shell_scripts_below(relative: &str) -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read script directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "sh") {
                let relative = path
                    .strip_prefix(root)
                    .expect("script below repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, fs::read_to_string(&path).expect("read shell script")));
            }
        }
    }
    let root = repo_root();
    let mut scripts = Vec::new();
    visit(&root, &root.join(relative), &mut scripts);
    scripts.sort_by(|left, right| left.0.cmp(&right.0));
    scripts
}

/// The full caller census: each `.sh` file under `docker/` (recursive) and
/// `scripts/` (recursive), plus `run.sh` itself checked as a single named
/// file since it is not a directory. Mirrors the walk shape used by
/// `qemu_host_lock_structure.rs`'s own census.
fn all_shell_scripts() -> Vec<(String, String)> {
    let mut scripts = shell_scripts_below("docker");
    scripts.extend(shell_scripts_below("scripts"));
    scripts.push(("run.sh".to_string(), repo_text("run.sh")));
    scripts.sort_by(|left, right| left.0.cmp(&right.0));
    scripts
}

/// A real invocation of create_ext2_disk.sh: a non-comment line whose
/// trimmed text, after stripping an optional leading `if ! ` or `if `,
/// starts with one of the two invocation idioms real call sites in this
/// tree use -- a relative `./scripts/create_ext2_disk.sh` (callers
/// already running from the repo root) or a quoted
/// `"$BREENIX_ROOT/scripts/create_ext2_disk.sh"` absolute path (callers
/// that are not). An `echo "...create_ext2_disk.sh..."` help string and a
/// `#`-led comment both contain the bare token without being a call, and
/// neither of those two shapes starts this way.
fn is_create_ext2_disk_invocation(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.starts_with('#') {
        return false;
    }
    let after_if = trimmed
        .strip_prefix("if ! ")
        .or_else(|| trimmed.strip_prefix("if "))
        .unwrap_or(trimmed);
    after_if.starts_with("./scripts/create_ext2_disk.sh")
        || after_if.starts_with("\"$BREENIX_ROOT/scripts/create_ext2_disk.sh\"")
}

fn invocation_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|line| is_create_ext2_disk_invocation(line)).collect()
}

#[test]
fn no_caller_passes_a_size_flag() {
    let scripts = all_shell_scripts();

    let mut all_invocations: Vec<(String, String)> = Vec::new();
    for (path, text) in &scripts {
        if path == CREATE_EXT2_DISK_SCRIPT {
            // The script's own usage/doc comments mention itself (one
            // example together with --size, since --size is a real,
            // documented flag); it is not its own caller.
            continue;
        }
        for line in invocation_lines(text) {
            all_invocations.push((path.clone(), line.trim().to_string()));
        }
    }

    // Anti-vacuity floor: measured at 13 real call sites on this branch
    // (9 distinct files under docker/qemu/ with one call each, plus
    // run.sh's own 4). Not a closed list -- a new caller only raises this;
    // a drop below 13 means the invocation predicate stopped matching a
    // real call shape, not that callers went away.
    assert!(
        all_invocations.len() >= 13,
        "only {} real create_ext2_disk.sh invocation(s) found across docker/, scripts/, \
         and run.sh; expected at least 13 -- the invocation-line predicate may have stopped \
         matching a real call shape:\n{:#?}",
        all_invocations.len(),
        all_invocations
    );

    let violations: Vec<String> = all_invocations
        .iter()
        .filter(|(_, line)| line.contains("--size"))
        .map(|(path, line)| format!("{path}: {line:?} passes --size (#850)"))
        .collect();
    assert!(
        violations.is_empty(),
        "a caller passes its own --size instead of relying on the script's payload-derived \
         default; this is exactly the shape that caused #850 (a hardcoded size drifting stale \
         as the real binary/font payload grows):\n{}",
        violations.join("\n")
    );
}

/// ANTI-VACUITY: the invocation predicate fires on each real shape this
/// census actually contains and not on the shapes deliberately excluded
/// (a comment, two echo/usage strings, and the script's own doc example
/// that happens to combine the token with `--size`). Also proves, by
/// mutating the real (now-fixed) caller file, that the whole-suite rule
/// above would catch #850's actual bug: reinstating
/// `run-aarch64-userspace.sh`'s original `--size 8` reddens
/// `no_caller_passes_a_size_flag` by name, not merely a synthetic string.
#[test]
fn invocation_predicate_is_not_vacuous() {
    for line in [
        "        ./scripts/create_ext2_disk.sh >/dev/null",
        "if ! ./scripts/create_ext2_disk.sh > /tmp/gate-ext2-disk.log 2>&1; then",
        "    \"$BREENIX_ROOT/scripts/create_ext2_disk.sh\" --arch aarch64",
        "        \"$BREENIX_ROOT/scripts/create_ext2_disk.sh\"",
        "./scripts/create_ext2_disk.sh",
    ] {
        assert!(
            is_create_ext2_disk_invocation(line),
            "must detect a real create_ext2_disk.sh invocation: {line:?}"
        );
    }

    for line in [
        "# #721 K9: create_ext2_disk.sh installs *.elf by glob with no manifest check (its",
        "echo \"Build it with: userspace/programs/build.sh --arch aarch64 && scripts/create_ext2_disk.sh --arch aarch64\"",
        "echo \"  cargo run -p xtask -- create-test-disk && ./scripts/create_ext2_disk.sh\"",
        "#   ./scripts/create_ext2_disk.sh --arch aarch64 --size 128",
    ] {
        assert!(
            !is_create_ext2_disk_invocation(line),
            "must NOT count a comment or echo/usage string as a real invocation: {line:?}"
        );
    }

    // Reproduce #850's actual bug against the real fixed file: put its
    // own original hardcoded call back and confirm the whole-suite rule
    // reddens by name.
    let real_caller = repo_text("docker/qemu/run-aarch64-userspace.sh");
    assert!(
        invocation_lines(&real_caller).iter().all(|line| !line.contains("--size")),
        "sanity: the real caller must be clean (no --size) before mutation"
    );
    let fixed_line = "    \"$BREENIX_ROOT/scripts/create_ext2_disk.sh\" --arch aarch64\n";
    assert!(
        real_caller.contains(fixed_line),
        "the reconstructed call line must match the real file, or this mutation proves nothing"
    );
    let mutated = real_caller.replacen(
        fixed_line,
        "    \"$BREENIX_ROOT/scripts/create_ext2_disk.sh\" --arch aarch64 --size 8\n",
        1,
    );
    assert_ne!(mutated, real_caller, "mutation must apply");
    let mutated_invocations = invocation_lines(&mutated);
    assert!(
        mutated_invocations.iter().any(|line| line.contains("--size 8")),
        "reddening: the mutated text must show the reinstated --size 8 as a real invocation, \
         reproducing #850's original bug shape"
    );
}

// --- Property 1: the script's own payload-vs-size guard --------------------

fn script_text() -> String {
    repo_text(CREATE_EXT2_DISK_SCRIPT)
}

/// The guard computes a payload total from the real binary/font globs into
/// `payload_bytes`, accumulating each match -- anchored to the specific
/// `for f in ...` loop and accumulation line #850 added, not the
/// pre-existing, unrelated `*.elf`/`*.ttf` copy loops further down the
/// same file (those use `elf_file`/`font_file` as loop variables and don't
/// accumulate a running total).
fn computes_payload_from_globs(text: &str) -> bool {
    text.contains(r#"for f in "$USERSPACE_DIR"/*.elf "$PROJECT_ROOT/fonts"/*.ttf; do"#)
        && text.contains("payload_bytes=$(( payload_bytes +")
}

/// The guard derives its minimum (`required_bytes`/`required_mb`)
/// arithmetically from `payload_bytes` -- not a flat hardcoded number --
/// and compares it against the requested `SIZE_MB`.
fn compares_against_size_mb(text: &str) -> bool {
    text.contains("required_bytes=$(( payload_bytes") && text.contains("if (( SIZE_MB < required_mb ))")
}

/// The substring of the script's own text spanning the `SIZE_MB <
/// required_mb` guard, from its `if` to its closing `fi`. Absent when the
/// guard's `if` line itself is missing.
fn shortfall_guard_block(text: &str) -> Option<&str> {
    let start = text.find("if (( SIZE_MB < required_mb ))")?;
    let after_start = &text[start..];
    let end = after_start.find("\nfi\n")?;
    Some(&after_start[..end])
}

/// The guard's failing branch names the shortfall and actually exits
/// with a failing status -- checked within the guard block specifically (not
/// "`exit 1` appears somewhere in the file", which would be vacuously true
/// against the script's several unrelated `exit 1`s: bad-usage, missing
/// Docker, missing mke2fs, failed image creation).
fn exits_on_shortfall(text: &str) -> bool {
    match shortfall_guard_block(text) {
        Some(block) => block.contains("shortfall") && block.contains("exit 1"),
        None => false,
    }
}

#[test]
fn payload_guard_structure_is_present_and_load_bearing() {
    let text = script_text();
    assert!(
        computes_payload_from_globs(&text),
        "create_ext2_disk.sh must compute a payload total from the real *.elf/*.ttf globs (#850)"
    );
    assert!(
        compares_against_size_mb(&text),
        "create_ext2_disk.sh must derive a required minimum from the payload and compare it \
         against the requested SIZE_MB (#850)"
    );
    assert!(
        exits_on_shortfall(&text),
        "create_ext2_disk.sh must exit non-zero, naming the shortfall, when the requested \
         size cannot fit the payload, rather than silently truncating (#850)"
    );

    // ANTI-VACUITY mutation 1: delete only the glob-accumulation loop.
    // computes_payload_from_globs must redden; the other two predicates,
    // which don't depend on that loop's own lines, must stay green.
    let loop_text = "payload_bytes=0\n\
for f in \"$USERSPACE_DIR\"/*.elf \"$PROJECT_ROOT/fonts\"/*.ttf; do\n    \
[[ -f \"$f\" ]] || continue\n    \
payload_bytes=$(( payload_bytes + $(file_size_bytes \"$f\") ))\ndone\n";
    assert!(
        text.contains(loop_text),
        "the reconstructed payload loop must match the real file, or this mutation proves nothing"
    );
    let without_loop = text.replacen(loop_text, "payload_bytes=0\n", 1);
    assert_ne!(without_loop, text, "mutation 1 must apply");
    assert!(
        !computes_payload_from_globs(&without_loop),
        "reddening: removing the glob-accumulation loop must fail computes_payload_from_globs"
    );
    assert!(
        compares_against_size_mb(&without_loop),
        "removing only the glob loop must not touch the separate SIZE_MB comparison"
    );
    assert!(
        exits_on_shortfall(&without_loop),
        "removing only the glob loop must not touch the separate shortfall-exit branch"
    );

    // ANTI-VACUITY mutation 2: delete only the trailing `exit 1` from the
    // guard's failing branch, leaving the `if` condition and each echo
    // (including the one naming the shortfall) intact -- this is exactly
    // the "silently continue instead of refusing" regression #850's own
    // fix rules out. exits_on_shortfall must redden; the other two
    // predicates, which don't depend on this one line, must stay green.
    let exit_in_context = "    echo \"  use the 256MB default.\" >&2\n    exit 1\nfi\n";
    assert!(
        text.contains(exit_in_context),
        "the reconstructed exit line must match the real file, or this mutation proves nothing"
    );
    let without_exit = text.replacen(exit_in_context, "    echo \"  use the 256MB default.\" >&2\nfi\n", 1);
    assert_ne!(without_exit, text, "mutation 2 must apply");
    assert!(
        computes_payload_from_globs(&without_exit),
        "removing only the trailing exit must not touch the separate glob-loop check"
    );
    assert!(
        compares_against_size_mb(&without_exit),
        "removing only the trailing exit must not touch the separate SIZE_MB comparison"
    );
    assert!(
        !exits_on_shortfall(&without_exit),
        "reddening: removing the guard's `exit 1` (leaving the shortfall message as dead-end \
         advice) must fail exits_on_shortfall -- this is the silent-continue shape #850 rules out"
    );

    // ANTI-VACUITY mutation 3: delete the whole guard block (the `if`
    // through its `fi`). compares_against_size_mb and exits_on_shortfall
    // must both redden; computes_payload_from_globs, which doesn't depend
    // on this block, must stay green.
    let guard_block = "if (( SIZE_MB < required_mb )); then\n    \
echo \"\"\n    \
echo \"ERROR: requested size ${SIZE_MB}MB is too small for the ${payload_mb_display}MB payload.\" >&2\n    \
echo \"  Needs at least ${required_mb}MB (shortfall: $((required_mb - SIZE_MB))MB).\" >&2\n    \
echo \"  Refusing to build a disk image that would fail partway through the copy\" >&2\n    \
echo \"  step with ENOSPC. Pass --size ${required_mb} or larger, or omit --size to\" >&2\n    \
echo \"  use the 256MB default.\" >&2\n    \
exit 1\nfi\n";
    assert!(
        text.contains(guard_block),
        "the reconstructed guard block must match the real file, or this mutation proves nothing"
    );
    let without_guard = text.replacen(guard_block, "", 1);
    assert_ne!(without_guard, text, "mutation 3 must apply");
    assert!(
        computes_payload_from_globs(&without_guard),
        "removing the guard block must not touch the separate payload-computation loop"
    );
    assert!(
        !compares_against_size_mb(&without_guard),
        "reddening: removing the guard block must fail compares_against_size_mb"
    );
    assert!(
        !exits_on_shortfall(&without_guard),
        "reddening: removing the guard block must fail exits_on_shortfall"
    );
}
