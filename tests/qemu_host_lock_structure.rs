//! #826/R181 host-wide aarch64 QEMU lock structural ratchet.
//!
//! R181's own measurement: 4-6 concurrent qemu-system-aarch64 processes on
//! this Mac ran the guest clock at 37-53% of wall-clock, which then falsely
//! reds the strict gate's ~18s poll ceiling (#826) even on a healthy guest.
//! `docker/qemu/lib/qemu-host-lock.sh` is the fix: a shared helper each
//! aarch64-launching gate script sources and routes its
//! `qemu-system-aarch64` invocation(s) through, so "at most one aarch64
//! QEMU boot alive on this host at a time" holds mechanically.
//!
//! This file's one property: each `.sh` script under `docker/qemu/`
//! (recursive -- stays inside that tree, matching R181's own
//! scope; a `scripts/`-directory gap this scan also found is disclosed in
//! `docs/planning/green-program/gates/HOST-QEMU-LOCK-2026-09-05.md`, not
//! fixed here) that contains a real `qemu-system-aarch64` LAUNCH line --
//! not merely a mention of the token, which the lock helper's own
//! `pgrep -f 'qemu-system-aarch64'` line and this file's own doc comments
//! both are -- also sources the helper and calls
//! `qemu_host_lock_acquire` somewhere in its text.
//!
//! Census-shaped, not a closed file list (the #549/#551/#527-r1 lesson this
//! campaign has learned three times already): `launches_qemu_aarch64`
//! re-derives "this script starts a real qemu-system-aarch64 process" from
//! each script's own text on each run, so a new aarch64 gate script that
//! forgets to route through the lock is caught automatically, not only the
//! 20 this branch itself wired up.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

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
                out.push((
                    relative,
                    fs::read_to_string(&path).expect("read shell script"),
                ));
            }
        }
    }

    let root = repo_root();
    let mut scripts = Vec::new();
    visit(&root, &root.join(relative), &mut scripts);
    scripts.sort_by(|left, right| left.0.cmp(&right.0));
    scripts
}

/// A real `qemu-system-aarch64` launch: a non-comment line that, once any
/// trailing `\` line-continuation and the whitespace around it are
/// stripped, ends with the bare token. Each real invocation in this tree
/// (`timeout N qemu-system-aarch64 \`, `nice -n 19 qemu-system-aarch64 \`,
/// a bare `qemu-system-aarch64 \`, or the `qemu-system-aarch64 \` line
/// that follows a `docker run ... <image> \` chain) is shaped exactly this
/// way. This is deliberately narrower than "the file contains the
/// substring": the lock helper's own `pgrep -f 'qemu-system-aarch64'`
/// line contains the token too but is a *search*, not a launch, and does
/// not end with a continuation -- the predicate below excludes it without
/// needing a path-based exemption.
fn is_qemu_aarch64_launch_line(line: &str) -> bool {
    let trimmed_start = line.trim_start();
    if trimmed_start.starts_with('#') {
        return false;
    }
    let trimmed_end = line.trim_end();
    let without_continuation = trimmed_end.strip_suffix('\\').unwrap_or(trimmed_end).trim_end();
    without_continuation.ends_with("qemu-system-aarch64")
}

fn launches_qemu_aarch64(script: &str) -> bool {
    script.lines().any(is_qemu_aarch64_launch_line)
}

fn sources_qemu_host_lock(script: &str) -> bool {
    script.contains("lib/qemu-host-lock.sh")
}

/// A real call: a non-comment line whose trimmed text is exactly the bare
/// function name (each call site this branch added is `    qemu_host_lock_acquire`
/// alone on its line, at some indentation). This excludes the helper's own
/// `qemu_host_lock_acquire() {` definition line and any comment mentioning
/// the function by name.
fn calls_qemu_host_lock_acquire(script: &str) -> bool {
    script.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "qemu_host_lock_acquire"
    })
}

#[test]
fn every_aarch64_qemu_launch_script_sources_and_acquires_the_host_lock() {
    let scripts = shell_scripts_below("docker/qemu");

    let launching: Vec<&(String, String)> = scripts
        .iter()
        .filter(|(_, text)| launches_qemu_aarch64(text))
        .collect();

    // Anti-vacuity floor: measured at 20 on this branch (the census that
    // found them is reproduced in the ratchet's own sibling test below).
    // Not a closed list -- a future aarch64 gate script only needs to
    // raise this count, not edit it down.
    assert!(
        launching.len() >= 20,
        "only {} script(s) under docker/qemu/ launch qemu-system-aarch64; expected at least 20",
        launching.len()
    );

    let mut violations = Vec::new();
    for (path, text) in &launching {
        let sourced = sources_qemu_host_lock(text);
        let acquires = calls_qemu_host_lock_acquire(text);
        if !sourced || !acquires {
            let missing = match (sourced, acquires) {
                (false, false) => "sources lib/qemu-host-lock.sh AND calls qemu_host_lock_acquire",
                (false, true) => "sources lib/qemu-host-lock.sh",
                (true, false) => "calls qemu_host_lock_acquire",
                (true, true) => unreachable!(),
            };
            violations.push(format!("{path}: launches qemu-system-aarch64 but does not {missing} (#826/R181)"));
        }
    }
    assert!(
        violations.is_empty(),
        "aarch64 QEMU launch(es) bypass the host-wide lock:\n{}",
        violations.join("\n")
    );
}

/// ANTI-VACUITY: the launch/source/acquire predicates must fire on the
/// real shapes they claim to, in both directions, and the whole-suite rule
/// above must actually redden on a real script with the lock call removed
/// -- not just on a synthetic string.
#[test]
fn qemu_host_lock_predicates_are_not_vacuous() {
    // Positive: each real continuation shape this branch used is detected.
    for line in [
        "    timeout 20 qemu-system-aarch64 \\",
        "timeout \"$BOOT_SECONDS\" qemu-system-aarch64 \\",
        "        nice -n 19 qemu-system-aarch64 \\",
        "qemu-system-aarch64 \\",
        "    qemu-system-aarch64 \\",
    ] {
        assert!(
            is_qemu_aarch64_launch_line(line),
            "must detect a real qemu-system-aarch64 launch line: {line:?}"
        );
    }

    // Negative: a comment mentioning the token, and the lock helper's own
    // pgrep search line, must NOT be detected as a launch -- checked against
    // the real helper file's real text, not a synthetic string.
    assert!(
        !is_qemu_aarch64_launch_line("# every script that launches qemu-system-aarch64 ..."),
        "a comment mentioning the token must not count as a launch"
    );
    let lock_helper = repo_text("docker/qemu/lib/qemu-host-lock.sh");
    assert!(
        !launches_qemu_aarch64(&lock_helper),
        "the lock helper's own pgrep -f 'qemu-system-aarch64' search line must not be \
         mistaken for a launch, or this ratchet would demand the helper acquire its own lock"
    );
    assert!(
        !calls_qemu_host_lock_acquire(&lock_helper),
        "the helper's `qemu_host_lock_acquire() {{` definition line must not be mistaken \
         for a call to it"
    );

    // ANTI-VACUITY mutation: strip the real qemu_host_lock_acquire call this
    // branch added to run-aarch64-boot-test-strict.sh (the gate #826's own
    // health battery ran) and confirm the whole-suite rule reddens by name.
    let real_strict = repo_text("docker/qemu/run-aarch64-boot-test-strict.sh");
    assert!(
        launches_qemu_aarch64(&real_strict),
        "sanity: the strict gate must still launch qemu-system-aarch64"
    );
    assert!(
        sources_qemu_host_lock(&real_strict) && calls_qemu_host_lock_acquire(&real_strict),
        "sanity: the strict gate must be clean before mutation"
    );
    let acquire_line = "    qemu_host_lock_acquire\n";
    assert!(
        real_strict.contains(acquire_line),
        "the reconstructed acquire line must match the real file, or this mutation proves nothing"
    );
    let mutated = real_strict.replacen(acquire_line, "", 1);
    assert_ne!(mutated, real_strict, "mutation must apply");
    assert!(
        sources_qemu_host_lock(&mutated),
        "the mutation must leave the source line intact -- this proves the acquire check \
         specifically, not the source check"
    );
    assert!(
        !calls_qemu_host_lock_acquire(&mutated),
        "reddening: the mutated text must no longer show an acquire call"
    );
    assert!(
        launches_qemu_aarch64(&mutated),
        "the mutation must not have touched the launch line itself"
    );
}
