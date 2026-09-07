//! Behavioral oracle for #890's compile cache. Previously each structure
//! suite was recompiled on gate calls without reuse. Current timing:
//! docs/planning/green-program/gates/
//! GATE-TOOLING-PREFLIGHT-CACHE-890-V2-2026-09-06.md.
//!
//! Tests cover transitive source hashing, checkout identity in keys and
//! default directories, nonempty/executable/test-list binary validation,
//! and mkdir locking with atomic binary-then-key publication. Hits still
//! execute tests. The original five behavioral tests remain unchanged.
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
//! sandbox (a `cache/` subdirectory), or TMPDIR is sandboxed for default
//! directory checks, so no test run here
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
    run_sandboxed_with_value(sandbox, if no_cache { Some("1") } else { None })
}

/// Runs the sandboxed script with an arbitrary bypass value, or unset.
/// Returns (exit success, combined stdout+stderr).
fn run_sandboxed_with_value(sandbox: &Path, no_cache: Option<&str>) -> (bool, String) {
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
    if let Some(value) = no_cache {
        cmd.env("BREENIX_STRUCTURE_NO_CACHE", value);
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

/// BREENIX_STRUCTURE_NO_CACHE=0 must retain a warm-cache hit without compiling.
#[test]
fn zero_value_env_var_does_not_bypass_a_warm_cache() {
    let sandbox = unique_sandbox("zero");
    write_fixture(&sandbox, "v1");

    let (ok1, out1) = run_sandboxed(&sandbox, false);
    assert!(ok1, "warm-up run 1 must succeed: {out1}");
    let (ok2, out2) = run_sandboxed(&sandbox, false);
    assert!(ok2, "warm-up run 2 must succeed: {out2}");
    assert!(
        out2.contains("cache: hit"),
        "cache must be warm before the zero-value check, got: {out2}"
    );

    let (ok3, out3) = run_sandboxed_with_value(&sandbox, Some("0"));
    assert!(ok3, "zero-value run must succeed: {out3}");
    assert!(
        out3.contains("cache: hit"),
        "BREENIX_STRUCTURE_NO_CACHE=0 must report a cache hit, got: {out3}"
    );
    assert!(
        !out3.contains("cache: bypass"),
        "BREENIX_STRUCTURE_NO_CACHE=0 must not bypass a warm cache, got: {out3}"
    );
    assert!(
        !out3.contains("== compiling"),
        "a zero-value cache hit must not recompile, got: {out3}"
    );

    fs::remove_dir_all(&sandbox).ok();
}

#[test]
fn a_change_to_an_included_path_module_invalidates_the_cache_and_recompiles() {
    let sandbox = unique_sandbox("transitive");
    let top = sandbox.join("tests/cache_fixture.rs");
    fs::write(&top, "#[path = \"helper.rs\"] mod helper;\n#[test] fn placeholder() { assert!(helper::VALUE > 0); }\n").unwrap();
    let original = fs::read(&top).unwrap();
    let helper = sandbox.join("tests/helper.rs");
    fs::write(&helper, "pub const VALUE: u8 = 1;\n").unwrap();
    let (ok, out) = run_sandboxed(&sandbox, false);
    assert!(ok && out.contains("cache: miss"), "{out}");
    let (ok, out) = run_sandboxed(&sandbox, false);
    assert!(ok && out.contains("cache: hit"), "{out}");
    fs::write(&helper, "pub const VALUE: u8 = 2;\n").unwrap();
    assert_eq!(original, fs::read(&top).unwrap());
    let (ok, out) = run_sandboxed(&sandbox, false);
    assert!(ok && out.contains("cache: miss") && out.contains("== compiling"), "{out}");

    // A missing dependency must reach compilation as a miss, where rustc
    // reports the real missing input; the stale binary must not execute.
    fs::remove_file(&helper).unwrap();
    let (ok, out) = run_sandboxed(&sandbox, false);
    assert!(!ok && out.contains("cache: miss") && out.contains("== compiling"), "{out}");
    assert!(!out.contains("== running"), "{out}");
    assert!(!sandbox.join("cache/cache_fixture.lock").exists());
    fs::remove_dir_all(sandbox).unwrap();
}

fn fixture_command(sandbox: &Path, cache: &Path) -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(sandbox.join("scripts/run-structure-tests.sh"))
        .arg(FIXTURE_STEM).arg("placeholder")
        .env("BREENIX_STRUCTURE_CACHE", cache)
        .env_remove("BREENIX_STRUCTURE_NO_CACHE")
        .current_dir(repo_root());
    cmd
}

fn successful_output(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{text}");
    text
}

#[test]
fn two_checkouts_sharing_one_cache_directory_never_reuse_each_others_binary() {
    let a = fs::canonicalize(unique_sandbox("checkout-a")).unwrap();
    let b = fs::canonicalize(unique_sandbox("checkout-b")).unwrap();
    for dir in [&a, &b] {
        fs::write(dir.join("tests/cache_fixture.rs"), "#[test] fn placeholder() { println!(\"MANIFEST={}\", env!(\"CARGO_MANIFEST_DIR\")); }\n").unwrap();
    }
    let cache = a.join("cache");
    for dir in [&a, &b, &a] {
        let out = successful_output(&mut fixture_command(dir, &cache));
        assert!(out.contains("cache: miss") && out.contains("== compiling"), "{out}");
        let manifest = format!("MANIFEST={}", fs::canonicalize(dir).unwrap().display());
        assert_eq!(out.lines().filter(|l| l.starts_with("MANIFEST=")).collect::<Vec<_>>(), vec![manifest.as_str()], "{out}");
    }
    // Both checkouts use the SAME TMPDIR with no override. Observe the
    // directories actually created, then verify each retains its own hit.
    let temp = a.join("default-tmp");
    fs::create_dir(&temp).unwrap();
    for dir in [&a, &b] {
        let out = successful_output(fixture_command(dir, &cache)
            .env_remove("BREENIX_STRUCTURE_CACHE").env("TMPDIR", &temp));
        assert!(out.contains("cache: miss"), "{out}");
    }
    let entries: Vec<_> = fs::read_dir(temp.join("breenix-structure-cache")).unwrap()
        .map(|e| e.unwrap().path()).collect();
    assert_eq!(entries.len(), 2, "expected two checkout directories: {entries:?}");
    for entry in entries {
        assert!(entry.join(FIXTURE_STEM).is_file());
        assert_eq!(entry.file_name().unwrap().to_str().unwrap().len(), 64);
    }
    for dir in [&a, &b] {
        let out = successful_output(fixture_command(dir, &cache)
            .env_remove("BREENIX_STRUCTURE_CACHE").env("TMPDIR", &temp));
        assert!(out.contains("cache: hit") && !out.contains("== compiling"), "{out}");
    }
    fs::remove_dir_all(a).unwrap();
    fs::remove_dir_all(b).unwrap();
}

#[test]
fn a_zero_byte_cached_binary_is_treated_as_a_miss_and_recompiled() {
    let sandbox = unique_sandbox("empty");
    write_fixture(&sandbox, "v1");
    let (ok, out) = run_sandboxed(&sandbox, false);
    assert!(ok, "{out}");
    let binary = sandbox.join("cache").join(FIXTURE_STEM);
    // Also cover a nonempty executable shell no-op: exit 0 is insufficient.
    for payload in [b"".as_slice(), b"#!/bin/sh\nexit 0\n".as_slice()] {
        fs::write(&binary, payload).unwrap();
        let (ok, out) = run_sandboxed(&sandbox, false);
        assert!(ok && out.contains("cache: miss") && out.to_lowercase().contains("corrupt"), "{out}");
        assert!(out.contains("== compiling") && out.contains("test result: ok. 1 passed"), "{out}");
    }
    fs::remove_dir_all(sandbox).unwrap();
}

#[test]
fn two_concurrent_invocations_on_a_cold_cache_both_succeed_and_produce_one_valid_binary() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let sandbox = unique_sandbox("concurrent");
    write_fixture(&sandbox, "v1");
    let cache = sandbox.join("cache");
    let wrappers = sandbox.join("bin");
    fs::create_dir(&wrappers).unwrap();
    let rustc = successful_output(Command::new("bash").args(["-c", "command -v rustc"]));
    let wrapper = wrappers.join("rustc");
    // Delegate to real rustc, recording output paths and widening the cold
    // miss window. Version queries are unchanged. This also directly pins
    // staging rather than hoping a torn binary happens during the race.
    fs::write(&wrapper, r##"#!/bin/bash
if [[ "$1" != "--version" ]]; then
    args=("$@")
    for ((i=0; i<$#; i++)); do
        if [[ "${args[i]}" == "-o" ]]; then
            printf '%s\n' "${args[i+1]}" >> "$COMPILE_LOG"
        fi
    done
    sleep 1
fi
exec "$REAL_RUSTC" "$@"
"##).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let log = sandbox.join("compiles");
    let mut cmd = fixture_command(&sandbox, &cache);
    cmd.env("REAL_RUSTC", rustc.trim()).env("COMPILE_LOG", &log)
        .env("PATH", format!("{}:{}", wrappers.display(), std::env::var("PATH").unwrap()))
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    let first = cmd.spawn().unwrap();
    let second = cmd.spawn().unwrap();
    for child in [first, second] {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "stdout={} stderr={}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
    }
    let binary = cache.join(FIXTURE_STEM);
    let metadata = fs::metadata(&binary).unwrap();
    assert!(metadata.len() > 0 && metadata.permissions().mode() & 0o111 != 0);
    let listing = successful_output(Command::new(&binary).arg("--list"));
    assert!(listing.contains(": test"), "{listing}");
    assert!(successful_output(&mut Command::new(&binary)).contains("1 passed"));
    let compiles = fs::read_to_string(log).unwrap();
    assert_eq!(compiles.lines().count(), 1, "waiter must reuse the published entry: {compiles}");
    assert!(compiles.trim().starts_with(&format!("{}.tmp.", binary.display())), "compile must stage privately: {compiles}");
    assert!(cache.join("cache_fixture.key").is_file());
    assert!(!cache.join("cache_fixture.lock").exists());
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 2, "no temporary artifacts remain");
    fs::remove_dir_all(sandbox).unwrap();
}

#[test]
fn nested_include_forms_resolve_from_their_own_files_and_ignore_escaped_literals() {
    let sandbox = unique_sandbox("nested");
    fs::create_dir(sandbox.join("tests/nested")).unwrap();
    fs::write(sandbox.join("tests/cache_fixture.rs"), r##"#[path = "helper.rs"] mod helper;
#[test] fn placeholder() {
    assert!(helper::VALUE > 0);
    assert!("text".find("#[path = \"").is_none());
    assert!("text".find("include!(\"fake.rs\")").is_none());
}
"##).unwrap();
    fs::write(sandbox.join("tests/helper.rs"), "include!(\"nested/value.rs\");\n").unwrap();
    fs::write(sandbox.join("tests/nested/value.rs"), "pub const VALUE: usize = include_str!(\"data.txt\").len() + include_bytes!(\"bytes.txt\").len();\n").unwrap();
    // The data scan reaches itself through a canonical alias. It must
    // terminate and hash this file once, even though this is text data.
    fs::write(sandbox.join("tests/nested/data.txt"), "include_str!(\"../nested/data.txt\")\n").unwrap();
    fs::write(sandbox.join("tests/nested/bytes.txt"), "abc").unwrap();
    let (ok, out) = run_sandboxed(&sandbox, false);
    assert!(ok && out.contains("cache: miss"), "{out}");
    fs::write(sandbox.join("tests/fake.rs"), "not a dependency").unwrap();
    let (ok, out) = run_sandboxed(&sandbox, false);
    assert!(ok && out.contains("cache: hit"), "escaped literals are not dependencies: {out}");
    for leaf in ["data.txt", "bytes.txt"] {
        let path = sandbox.join("tests/nested").join(leaf);
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(b'!');
        fs::write(path, bytes).unwrap();
        let (ok, out) = run_sandboxed(&sandbox, false);
        assert!(ok && out.contains("cache: miss") && out.contains("== compiling"), "{leaf}: {out}");
        let (ok, out) = run_sandboxed(&sandbox, false);
        assert!(ok && out.contains("cache: hit"), "{out}");
    }
    fs::remove_dir_all(sandbox).unwrap();
}
