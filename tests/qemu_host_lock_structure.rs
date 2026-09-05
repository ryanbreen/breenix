//! #826/#834/R181 host-wide aarch64 QEMU lock structural ratchet.
//!
//! R181's own measurement: 4-6 concurrent qemu-system-aarch64 processes on
//! this Mac ran the guest clock at 37-53% of wall-clock, which then falsely
//! reds the strict gate's ~18s poll ceiling (#826) even on a healthy guest.
//! `docker/qemu/lib/qemu-host-lock.sh` is the fix: a shared helper each
//! aarch64-launching script sources and routes its `qemu-system-aarch64`
//! invocation(s) through, so "at most one aarch64 QEMU boot alive on this
//! host at a time" holds mechanically.
//!
//! This file's one property: each `.sh` script under `docker/` (recursive)
//! or `scripts/` (recursive), plus `run.sh` itself, that contains a real
//! `qemu-system-aarch64` LAUNCH line -- not merely a mention of the token,
//! which the lock helper's own `pgrep -f 'qemu-system-aarch64'` line and
//! this file's own doc comments both are -- also sources the helper and
//! calls `qemu_host_lock_acquire` somewhere in its text.
//!
//! #826/R181's own ratchet scoped this census to `docker/qemu/*.sh` only,
//! by its own wording ("a shared helper ... sourced by each aarch64 gate
//! script"). #834 found six more real launchers under `scripts/`, 0 of them
//! wired, plus a seventh the original scan could not reach at all:
//! `docker/qemu-aarch64/run-arm64-boot.sh` sits in a directory sibling to
//! `docker/qemu/`, so `shell_scripts_below("docker/qemu")` (a recursive walk
//! *rooted* at that one directory) cannot see it regardless of how the
//! `launches`/`sources`/`acquires` predicates below are written -- widening
//! the walk's root to `docker` closes that reach gap the same motion that
//! adds `scripts/` closes the `scripts/`-directory gap. `run.sh`'s own
//! native (non-Parallels, non-VMware) aarch64 launch is the third and last
//! site #834's own grep found outside the original scope; it is checked as
//! a single named file since it is not itself a directory root.
//!
//! Deliberately NOT covered by this file, disclosed rather than silently
//! excluded (see `docs/planning/green-program/gates/
//! HOST-QEMU-LOCK-SCRIPTS-834-2026-09-05.md` for the reasoning): the three
//! `.py` launchers under `scripts/`/`docker/qemu/` (no bash to `source`) and
//! `docker/qemu/run-aarch64-test.exp` (a Tcl/expect script, unreferenced by
//! any caller in this repo). `shell_scripts_below`'s `.sh`-extension filter
//! excludes each of these four by construction, so this ratchet's `>= 28` floor below
//! cannot regress by silently absorbing one of them into "covered."
//!
//! Census-shaped, not a closed file list (the #549/#551/#527-r1 lesson this
//! campaign has learned three times already): `launches_qemu_aarch64`
//! re-derives "this script starts a real qemu-system-aarch64 process" from
//! each script's own text on each run, so a new aarch64 launcher anywhere
//! under `docker/`, `scripts/`, or `run.sh` that forgets to route through
//! the lock is caught automatically, not only the 28 total (20 from #826/
//! R181 plus #834's 8: 6 under `scripts/`, `docker/qemu-aarch64/
//! run-arm64-boot.sh`, and `run.sh`) wired as of this file's own last edit.

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

/// The full launch-site census this ratchet polices: each `.sh` file under
/// `docker/` (recursive -- reaches both `docker/qemu/` and the sibling
/// `docker/qemu-aarch64/`) and `scripts/` (recursive), plus `run.sh` itself
/// checked as a single named file since it is not a directory.
fn all_shell_scripts() -> Vec<(String, String)> {
    let mut scripts = shell_scripts_below("docker");
    scripts.extend(shell_scripts_below("scripts"));
    scripts.push(("run.sh".to_string(), repo_text("run.sh")));
    scripts.sort_by(|left, right| left.0.cmp(&right.0));
    scripts
}

/// A real `qemu-system-aarch64` launch: a non-comment line that, once any
/// trailing `\` line-continuation and the whitespace around it are
/// stripped, ends with the bare token. Each real invocation in this tree
/// (`timeout N qemu-system-aarch64 \`, `nice -n 19 qemu-system-aarch64 \`,
/// a bare `qemu-system-aarch64 \`, the `qemu-system-aarch64 \` line that
/// follows a `docker run ... <image> \` chain, or a plain
/// `QEMU_BIN=qemu-system-aarch64` assignment feeding an indirect `"$QEMU_BIN"
/// \` invocation further down the same file) is shaped exactly this way.
/// This is deliberately narrower than "the file contains the substring":
/// the lock helper's own `pgrep -f 'qemu-system-aarch64'` line contains the
/// token too but is a *search*, not a launch, and does not end with a
/// continuation -- the predicate below excludes it without needing a
/// path-based exemption.
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
/// function name (each call site this campaign added is
/// `    qemu_host_lock_acquire` alone on its line, at some indentation).
/// This excludes the helper's own `qemu_host_lock_acquire() {` definition
/// line and any comment mentioning the function by name.
fn calls_qemu_host_lock_acquire(script: &str) -> bool {
    script.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "qemu_host_lock_acquire"
    })
}

#[test]
fn every_aarch64_qemu_launch_script_sources_and_acquires_the_host_lock() {
    let scripts = all_shell_scripts();

    let launching: Vec<&(String, String)> = scripts
        .iter()
        .filter(|(_, text)| launches_qemu_aarch64(text))
        .collect();

    // Anti-vacuity floor: measured at 28 on this branch (20 from #826/R181's
    // original docker/qemu/ census, plus #834's 8: six under scripts/, the
    // docker/qemu-aarch64/ sibling-directory script, and run.sh). The
    // census this counts is reproduced in the ratchet's own sibling test
    // below. Not a closed list -- a future aarch64 launcher only needs to
    // raise this count, not edit it down.
    assert!(
        launching.len() >= 28,
        "only {} script(s) under docker/, scripts/, or run.sh launch qemu-system-aarch64; expected at least 28",
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
            violations.push(format!("{path}: launches qemu-system-aarch64 but does not {missing} (#826/#834/R181)"));
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
/// -- not just on a synthetic string. Covers both halves of #834's widened
/// census: a `docker/qemu/` script (the original #826/R181 scope) and a
/// `scripts/` script (the gap #834 closes), so the widening is proven, not
/// merely asserted by the doc comment above.
#[test]
fn qemu_host_lock_predicates_are_not_vacuous() {
    // Positive: each real continuation shape this campaign used is detected.
    for line in [
        "    timeout 20 qemu-system-aarch64 \\",
        "timeout \"$BOOT_SECONDS\" qemu-system-aarch64 \\",
        "        nice -n 19 qemu-system-aarch64 \\",
        "qemu-system-aarch64 \\",
        "    qemu-system-aarch64 \\",
        "    QEMU_BIN=qemu-system-aarch64",
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

    // ANTI-VACUITY mutation, leg 1 (docker/qemu/, #826/R181's original
    // scope): strip the real qemu_host_lock_acquire call from
    // run-aarch64-boot-test-strict.sh (the gate #826's own health battery
    // ran) and confirm the whole-suite rule reddens by name.
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
    let mutated_strict = real_strict.replacen(acquire_line, "", 1);
    assert_ne!(mutated_strict, real_strict, "mutation must apply");
    assert!(
        sources_qemu_host_lock(&mutated_strict),
        "the mutation must leave the source line intact -- this proves the acquire check \
         specifically, not the source check"
    );
    assert!(
        !calls_qemu_host_lock_acquire(&mutated_strict),
        "reddening: the mutated text must no longer show an acquire call"
    );
    assert!(
        launches_qemu_aarch64(&mutated_strict),
        "the mutation must not have touched the launch line itself"
    );

    // ANTI-VACUITY mutation, leg 2 (scripts/, #834's widened scope): the
    // same proof against a real script under scripts/ -- a bypass here is
    // exactly the shape #834 disclosed (six scripts/ launchers, 0 wired)
    // and the whole-suite rule above must catch it the same way it catches
    // a docker/qemu/ bypass, not merely by extending the doc comment.
    let real_boot_test = repo_text("scripts/run-arm64-boot-test.sh");
    assert!(
        launches_qemu_aarch64(&real_boot_test),
        "sanity: scripts/run-arm64-boot-test.sh must still launch qemu-system-aarch64"
    );
    assert!(
        sources_qemu_host_lock(&real_boot_test) && calls_qemu_host_lock_acquire(&real_boot_test),
        "sanity: scripts/run-arm64-boot-test.sh must be clean before mutation"
    );
    let scripts_acquire_line = "qemu_host_lock_acquire\n";
    assert!(
        real_boot_test.contains(scripts_acquire_line),
        "the reconstructed acquire line must match the real scripts/ file, or this mutation \
         proves nothing"
    );
    let mutated_boot_test = real_boot_test.replacen(scripts_acquire_line, "", 1);
    assert_ne!(mutated_boot_test, real_boot_test, "mutation must apply");
    assert!(
        sources_qemu_host_lock(&mutated_boot_test),
        "the mutation must leave the source line intact -- this proves the acquire check \
         specifically, not the source check"
    );
    assert!(
        !calls_qemu_host_lock_acquire(&mutated_boot_test),
        "reddening: the mutated text must no longer show an acquire call"
    );
    assert!(
        launches_qemu_aarch64(&mutated_boot_test),
        "the mutation must not have touched the launch line itself"
    );

    // ANTI-VACUITY reach check: docker/qemu-aarch64/run-arm64-boot.sh sits
    // in a directory sibling to docker/qemu/, unreachable by a walk rooted
    // at "docker/qemu" no matter how the predicates above are written.
    // Confirms the root widened to "docker" actually reaches it, rather
    // than the file merely existing on disk unchecked.
    let census = all_shell_scripts();
    assert!(
        census
            .iter()
            .any(|(path, _)| path == "docker/qemu-aarch64/run-arm64-boot.sh"),
        "the docker-rooted walk must reach the docker/qemu-aarch64/ sibling directory, \
         not only docker/qemu/ -- a docker/qemu-only root would silently exempt it"
    );
    assert!(
        census.iter().any(|(path, _)| path == "run.sh"),
        "the census must include run.sh itself, not only files under docker/ or scripts/"
    );
}
