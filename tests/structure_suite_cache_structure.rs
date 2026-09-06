//! Behavioral oracle for #890: `scripts/run-structure-tests.sh`'s compile
//! cache. Filed from PR #889's review measurements (~106s Mac / ~6m20s
//! beast, uncached, per gate call -- docs/planning/green-program/gates/
//! GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md, "Isolated preflight
//! time cost"). This suite runs the real script end to end against a
//! throwaway fixture and reads its own stdout, not the cache
//! implementation's internals -- so a rewrite that keeps the observable
//! contract (miss on first sight, hit on an unchanged source, miss again on
//! a changed byte, bypass ignores a warm cache, a corrupted cached binary
//! is treated as a miss) still passes this file untouched.
//!
//! # Isolation: a sandboxed REPO_ROOT, not this repository
//!
//! `scripts/run-structure-tests.sh` derives `REPO_ROOT` from its own script
//! path (`dirname "${BASH_SOURCE[0]}"/..`), not from the caller's cwd or any
//! env var. Each test below builds a private sandbox directory with its own
//! `scripts/` and `tests/` subdirectories, copies the REAL
//! `scripts/run-structure-tests.sh` (the file this repository ships, read
//! fresh on each test run -- not a duplicated copy of its logic) into the
//! sandbox's `scripts/`, and writes a trivial fixture `<stem>.rs` into the
//! sandbox's `tests/`. Invoking the sandboxed copy makes REPO_ROOT resolve
//! to the sandbox, so SOURCE resolves to the sandbox's fixture -- no file
//! under this repository's own `tests/` is read, written, or mutated by
//! these tests. `BREENIX_STRUCTURE_CACHE` is pinned inside the same
//! sandbox (a `cache/` subdirectory) for each test, so no test run here
//! reads or writes the real `${TMPDIR}/breenix-structure-cache` a
//! concurrent gate invocation might be using at the same time.
//!
//! Run: `cargo test --test structure_suite_cache_structure` or `scripts/
//! run-structure-tests.sh structure_suite_cache_structure`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Stem of the throwaway fixture structure-test file each sandbox gets.
/// Chosen not to collide with a real `tests/*_structure.rs` file --
/// the sandbox's own `tests/` directory is unrelated to this repository's,
/// but keeping the name distinctive documents intent.
const FIXTURE_STEM: &str = "cache_fixture";

fn unique_sandbox(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "structure-cache-sandbox-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(dir.join("scripts")).expect("create sandbox scripts dir");
    fs::create_dir_all(dir.join("tests")).expect("create sandbox tests dir");
    fs::create_dir_all(dir.join("cache")).expect("create sandbox cache dir");
    fs::copy(
        repo_root().join("scripts/run-structure-tests.sh"),
        dir.join("scripts/run-structure-tests.sh"),
    )
    .expect("copy the real scripts/run-structure-tests.sh into the sandbox");
    dir
}

/// A trivial, always-passing structure-test fixture. `marker` is embedded
/// as a comment so two calls with different markers differ by at least one
/// source byte while both still compile and pass -- the "touch + change one
/// byte" fixture the stale-cache test needs.
fn write_fixture(sandbox: &Path, marker: &str) {
    let content = format!(
        "// marker: {marker}\n#[test]\nfn placeholder() {{ assert!(true); }}\n"
    );
    fs::write(
        sandbox.join("tests").join(format!("{FIXTURE_STEM}.rs")),
        content,
    )
    .expect("write fixture structure-test source");
}

/// Runs the sandboxed `scripts/run-structure-tests.sh` against the fixture,
/// with `BREENIX_STRUCTURE_CACHE` pinned inside the sandbox and
/// `BREENIX_STRUCTURE_NO_CACHE` set only when `no_cache` is true. Returns
/// (exit success, combined stdout+stderr).
fn run_sandboxed(sandbox: &Path, no_cache: bool) -> (bool, String) {
    let mut cmd = Command::new("bash");
    cmd.arg(sandbox.join("scripts/run-structure-tests.sh"))
        .arg(FIXTURE_STEM)
        .env("BREENIX_STRUCTURE_CACHE", sandbox.join("cache"))
        .env_remove("BREENIX_STRUCTURE_NO_CACHE")
        // NOT `sandbox`: `scripts/run-structure-tests.sh` computes REPO_ROOT
        // from its OWN script path (`dirname "${BASH_SOURCE[0]}"/..`), which
        // resolves to the sandbox regardless of cwd, so SOURCE still
        // resolves to the sandbox's fixture either way. cwd matters for a
        // DIFFERENT reason: on a host where `rustc` is a rustup shim with no
        // global default toolchain configured (e.g. this repository's own
        // beast Incus container), rustup picks a toolchain by walking UP
        // FROM THE CURRENT DIRECTORY looking for `rust-toolchain.toml` --
        // not from the invoked script's path. Running with cwd = the
        // sandbox (which has no such file) makes rustup refuse to run at
        // all ("could not choose a version of rustc to run"), a failure
        // this test would misreport as this repo's own cache logic being
        // broken. Each real caller of this script (the four boot gates,
        // and a person running it by hand) invokes it from within this
        // repository, where `rust-toolchain.toml` above is found -- so
        // pinning cwd here to the real repo root matches actual usage
        // instead of deviating from it.
        .current_dir(repo_root());
    if no_cache {
        cmd.env("BREENIX_STRUCTURE_NO_CACHE", "1");
    }
    let output = cmd
        .output()
        .expect("failed to spawn the sandboxed run-structure-tests.sh");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

#[test]
fn identical_source_reuses_the_compiled_binary_on_the_second_call() {
    let sandbox = unique_sandbox("reuse");
    write_fixture(&sandbox, "v1");

    let (ok1, out1) = run_sandboxed(&sandbox, false);
    assert!(ok1, "first run against an empty cache must succeed: {out1}");
    assert!(
        out1.contains("cache: miss"),
        "first run against an empty cache must report a miss, got: {out1}"
    );
    assert!(
        out1.contains("== compiling"),
        "a cache miss must actually compile, got: {out1}"
    );

    let (ok2, out2) = run_sandboxed(&sandbox, false);
    assert!(ok2, "second run (unchanged source) must succeed: {out2}");
    assert!(
        out2.contains("cache: hit"),
        "second run with an unchanged source, rustc, and flags must reuse \
         the compiled binary, got: {out2}"
    );
    assert!(
        !out2.contains("== compiling"),
        "a cache hit must not recompile, got: {out2}"
    );

    fs::remove_dir_all(&sandbox).ok();
}

#[test]
fn a_changed_source_byte_invalidates_the_cache_and_recompiles() {
    let sandbox = unique_sandbox("stale");
    write_fixture(&sandbox, "v1");

    let (ok1, out1) = run_sandboxed(&sandbox, false);
    assert!(ok1, "first run must succeed: {out1}");
    assert!(out1.contains("cache: miss"), "first run must miss: {out1}");

    let (ok_hit, out_hit) = run_sandboxed(&sandbox, false);
    assert!(ok_hit, "warm-up second run must succeed: {out_hit}");
    assert!(
        out_hit.contains("cache: hit"),
        "cache must be warm before the staleness check: {out_hit}"
    );

    // Touch + change one byte: same fixture stem and test name, one
    // different character in the marker comment.
    write_fixture(&sandbox, "v2");

    let (ok3, out3) = run_sandboxed(&sandbox, false);
    assert!(ok3, "run against the changed source must succeed: {out3}");
    assert!(
        out3.contains("cache: miss"),
        "a changed source byte must invalidate the cache, got: {out3}"
    );
    assert!(
        out3.contains("== compiling"),
        "a stale-source miss must actually recompile, not silently reuse \
         the old binary, got: {out3}"
    );

    fs::remove_dir_all(&sandbox).ok();
}

#[test]
fn no_cache_env_var_bypasses_a_warm_cache() {
    let sandbox = unique_sandbox("bypass");
    write_fixture(&sandbox, "v1");

    let (ok1, _out1) = run_sandboxed(&sandbox, false);
    assert!(ok1, "warm-up run 1 must succeed");
    let (ok2, out2) = run_sandboxed(&sandbox, false);
    assert!(ok2, "warm-up run 2 must succeed: {out2}");
    assert!(
        out2.contains("cache: hit"),
        "cache must be warm before the bypass check, got: {out2}"
    );

    let cached_binary = sandbox.join("cache").join(FIXTURE_STEM);
    let before = fs::read(&cached_binary).expect("read the warm cached binary");

    let (ok3, out3) = run_sandboxed(&sandbox, true);
    assert!(ok3, "bypass run must succeed: {out3}");
    assert!(
        out3.contains("cache: bypass"),
        "BREENIX_STRUCTURE_NO_CACHE=1 must report a bypass even with a warm \
         cache present, got: {out3}"
    );
    assert!(
        out3.contains("== compiling"),
        "a bypass run must actually recompile rather than silently reuse \
         the cache, got: {out3}"
    );

    let after = fs::read(&cached_binary).expect("read the cached binary after the bypass run");
    assert_eq!(
        before, after,
        "a bypass run must not touch the warm cache it bypassed"
    );

    fs::remove_dir_all(&sandbox).ok();
}

#[test]
fn a_corrupted_cached_binary_is_treated_as_a_miss_and_recompiled() {
    let sandbox = unique_sandbox("corrupt");
    write_fixture(&sandbox, "v1");

    let (ok1, out1) = run_sandboxed(&sandbox, false);
    assert!(ok1, "warm-up run must succeed: {out1}");

    let cached_binary = sandbox.join("cache").join(FIXTURE_STEM);
    assert!(
        cached_binary.exists(),
        "expected a cached binary at {}",
        cached_binary.display()
    );
    // Overwrite the executable's bytes with plain text; fs::write preserves
    // the existing execute permission bit, so the sanity check inside
    // scripts/run-structure-tests.sh must fail on content, not on mode.
    fs::write(&cached_binary, b"not an executable\n").expect("corrupt the cached binary");

    let (ok2, out2) = run_sandboxed(&sandbox, false);
    assert!(
        ok2,
        "a run against a corrupted cache must still succeed by recompiling: {out2}"
    );
    assert!(
        out2.contains("cache: miss") && out2.to_lowercase().contains("corrupt"),
        "a corrupted cached binary must be detected as a corrupt-binary \
         miss specifically, not silently reused or misdiagnosed as a \
         plain stale key, got: {out2}"
    );
    assert!(
        out2.contains("== compiling"),
        "a corrupt-binary miss must actually recompile, got: {out2}"
    );

    fs::remove_dir_all(&sandbox).ok();
}
