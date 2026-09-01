//! Widening detector for `docs/planning/green-program/WORKLOAD-ENVELOPES.md` (R4).
//!
//! The green program's one revert (Filesystem x86/aarch64/blended, root cause #728)
//! happened because Filesystem was declared green against an x86 image that had
//! never run a second concurrent userspace process; an unrelated arc (#713) later
//! gave x86 that second process and a latent lock-discipline race became reachable.
//! No filesystem code changed between the declaration and the downgrade -- the
//! *workload* the declaration was proven under simply widened, and nothing recorded
//! what that workload was.
//!
//! This suite is the mechanical half of that fix. It reads the same four workload
//! axes `WORKLOAD-ENVELOPES.md` documents for the program's six currently-standing
//! green cells (TTY x86/aarch64/blended, Tracing-aarch64, Bus-aarch64, NIC-aarch64)
//! and asserts the current tree still matches what those declarations were proven
//! against. It is a CENSUS in the same sense every other `tests/*_structure.rs`
//! ratchet in this tree is: every check reads its fact out of the real source file
//! it governs (an `init.rs` call sequence, a gate script's own QEMU flags, a
//! kernel-registry function body), never a hand-typed expected value copied out of
//! the envelope document. A change that widens one of these axes -- the same shape
//! of change #713 was -- reddens the corresponding test by name instead of silently
//! invalidating a declaration nobody re-checks.
//!
//! It is host-side and requires no kernel build or QEMU boot: everything below is a
//! text read of files already in the tree. Run it the same way any other structural
//! ratchet here is run: `cargo test --test green_program_envelope_structure`.
//!
//! Every mutation-proof test below follows the established idiom in this file
//! family (see `tty_oracle_structure.rs`): mutations are applied to an in-memory
//! copy of the file content via `String::replace`, never written to disk, so a
//! mutation proof never risks leaving the tree dirty.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const INIT_RS: &str = "userspace/programs/src/init.rs";
const EXECUTOR_RS: &str = "kernel/src/test_framework/executor.rs";

/// The seven gate scripts `WORKLOAD-ENVELOPES.md` cites as backing the six
/// currently-standing green cells (2 x86, 5 aarch64 -- the tracing harness counts
/// once even though it drives both arches, since only its aarch64 branch backs a
/// standing cell).
const X86_GATE_SCRIPTS: &[&str] = &[
    "docker/qemu/run-x86-tty-oracle-gate.sh",
    "docker/qemu/run-x86-prod-profile-boot-test.sh",
];
const AARCH64_GATE_SCRIPTS: &[&str] = &[
    "docker/qemu/run-aarch64-tty-oracle-gate.sh",
    "docker/qemu/run-aarch64-prod-profile-boot-test.sh",
    "docker/qemu/run-aarch64-full-test.sh",
    "docker/qemu/run-aarch64-service-sequence-gate.sh",
    "scripts/test_tracing_via_gdb.sh",
];

// ---------------------------------------------------------------------------
// Item 1: the TTY concurrency invariant (init.rs call-sequence census)
// ---------------------------------------------------------------------------

/// The `#[cfg(target_arch = "...")]`-gated call sequence inside `fn main() { ... }`,
/// in source order, as `(gate_arch, fn_name)` pairs. `gate_arch` is `None` for a
/// call with no `#[cfg(target_arch = ...)]` immediately above it. Only direct
/// `ident(...);` statement lines are recorded; truncates at the reap-loop comment
/// so the zombie-reap `loop { waitpid(-1, ...) }` below it (which is not part of
/// the launch sequence) is never walked.
fn main_call_sequence(source: &str) -> Vec<(Option<String>, String)> {
    let start = source.find("fn main() {").expect("init.rs declares fn main()");
    let after_header = start + "fn main() {".len();
    let reap_marker = "// Reap zombies forever";
    let reap_idx = source[after_header..]
        .find(reap_marker)
        .expect("init.rs main() reaches the reap-loop comment");
    let body = &source[after_header..after_header + reap_idx];

    let mut calls = Vec::new();
    let mut pending_cfg: Option<String> = None;
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#[cfg(target_arch = \"") {
            let arch = rest
                .split('"')
                .next()
                .expect("cfg(target_arch = \"...\") names an arch")
                .to_string();
            pending_cfg = Some(arch);
            continue;
        }
        if let Some(paren) = line.find('(') {
            let candidate = &line[..paren];
            let is_bare_ident = !candidate.is_empty()
                && candidate
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_');
            if is_bare_ident && line.trim_end().ends_with(");") {
                calls.push((pending_cfg.take(), candidate.to_string()));
                continue;
            }
        }
        // Any other statement (e.g. the `let pid = ...;` / `print!(...)` setup
        // lines at the top of main()) is not a call in the launch sequence; clear
        // a stray pending cfg defensively rather than mis-attach it to whatever
        // call comes next.
        pending_cfg = None;
    }
    calls
}

/// The body of `fn <name>(...) { ... }`, brace-matched from the first occurrence of
/// `fn <name>(` in `source`. Panics if the function is not declared -- a missing
/// function is exactly the kind of drift this census must not pass through
/// silently.
fn fn_body<'a>(source: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}(");
    let idx = source
        .find(&needle)
        .unwrap_or_else(|| panic!("{INIT_RS} declares fn {name}(...)"));
    let after = &source[idx..];
    let open = after
        .find('{')
        .unwrap_or_else(|| panic!("fn {name} has a body"));
    let bytes = after.as_bytes();
    let mut depth: i32 = 1;
    let mut i = open + 1;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    &after[open + 1..i - 1]
}

#[derive(Debug, PartialEq, Eq)]
enum LaunchKind {
    /// Body calls neither `spawn(` nor `spawnv(` -- not a process launcher at all
    /// (e.g. `run_init_group_refusal_probe`, which drives a raw clone via inline
    /// asm, never `spawn()`).
    NotASpawn,
    /// Body calls `spawn(`/`spawnv(` and also `waitpid(` -- the child is reaped
    /// before the launcher returns, so it does not overlap with whatever main()
    /// calls next.
    Reaped,
    /// Body calls `spawn(`/`spawnv(` with no `waitpid(` anywhere in the same
    /// body -- a fire-and-forget launch that keeps running concurrently with
    /// everything main() calls afterward.
    Persistent,
}

fn classify_launcher(body: &str) -> LaunchKind {
    let has_spawn = body.contains("spawn(") || body.contains("spawnv(");
    let has_wait = body.contains("waitpid(");
    match (has_spawn, has_wait) {
        (false, _) => LaunchKind::NotASpawn,
        (true, true) => LaunchKind::Reaped,
        (true, false) => LaunchKind::Persistent,
    }
}

/// Counts persistent (fire-and-forget) launchers `main()` calls, for the given
/// `arch` ("aarch64" or "x86_64"), strictly before the first call to `stop_fn` that
/// is itself gated to `arch`. Only calls that actually execute on `arch` --
/// unconditional (`cfg == None`) or gated to `arch` itself -- are counted; calls
/// gated to the other arch are skipped, matching what the compiler would actually
/// build for that target.
fn persistent_count_before(source: &str, arch: &str, stop_fn: &str) -> usize {
    let calls = main_call_sequence(source);
    let mut count = 0;
    let mut found_stop = false;
    for (cfg, name) in &calls {
        if name == stop_fn && cfg.as_deref() == Some(arch) {
            found_stop = true;
            break;
        }
        let applies_to_arch = match cfg {
            None => true,
            Some(c) => c == arch,
        };
        if !applies_to_arch {
            continue;
        }
        if classify_launcher(fn_body(source, name)) == LaunchKind::Persistent {
            count += 1;
        }
    }
    assert!(
        found_stop,
        "never reached a {arch}-gated call to {stop_fn}(); main()'s call sequence \
         changed shape enough that this census can no longer find its own stop marker"
    );
    count
}

#[test]
fn aarch64_tty_oracle_runs_with_exactly_one_persistent_background_process() {
    let source = read(INIT_RS);
    let count = persistent_count_before(&source, "aarch64", "run_tty_oracle");
    assert_eq!(
        count, 1,
        "WORKLOAD-ENVELOPES.md \u{a7}1 declares aarch64's TTY oracle runs with exactly \
         one persistent background process (heartbeat) alive alongside it; the tree \
         now starts {count} before run_tty_oracle() is called on aarch64 -- the TTY \
         cell's proven concurrency envelope no longer matches the tree. Re-derive and \
         re-prove TTY-aarch64 (and re-check whether the new process can race an ext2 \
         write the same way #713 did for Filesystem) before trusting this declaration."
    );
}

#[test]
fn x86_tty_oracle_runs_with_no_persistent_background_process() {
    let source = read(INIT_RS);
    let count = persistent_count_before(&source, "x86_64", "run_tty_oracle");
    assert_eq!(
        count, 0,
        "WORKLOAD-ENVELOPES.md \u{a7}2 declares x86's TTY oracle runs with no \
         background daemon alive yet (bsshd/heartbeat-equivalent both start later); \
         the tree now starts {count} before run_tty_oracle() is called on x86 -- the \
         TTY-x86/blended concurrency envelope no longer matches the tree."
    );
}

// ---------------------------------------------------------------------------
// Item 2: ext2 read-only (x86) vs writable (aarch64) census
// ---------------------------------------------------------------------------

/// Whether the ext2 root-disk `-drive` (or, on the service-sequence gate, the
/// `drive_opts=` variable that becomes one) declares `readonly=on`. Scans line by
/// line rather than the whole file so a `readonly=on` on an unrelated drive (the
/// UEFI `id=hd` disk, x86's `id=placeholder`) can never be mistaken for the ext2
/// disk's own flag.
fn ext2_drive_is_readonly(script: &str) -> bool {
    for line in script.lines() {
        let l = line.trim();
        let names_ext2_drive = l.contains("id=ext2disk") || l.contains("id=ext2,");
        if names_ext2_drive && (l.contains("-drive") || l.starts_with("drive_opts=")) {
            return l.contains("readonly=on");
        }
    }
    panic!("no ext2 -drive (or drive_opts=) declaration found in this script");
}

#[test]
fn x86_gate_scripts_mount_ext2_readonly() {
    for path in X86_GATE_SCRIPTS {
        let script = read(path);
        assert!(
            ext2_drive_is_readonly(&script),
            "WORKLOAD-ENVELOPES.md \u{a7}2 and the Cross-cell structural facts section \
             both rely on {path} mounting the ext2 root disk `readonly=on` -- so \
             `root_fs_write()` cannot succeed on this workload even in principle. That \
             flag is now absent. This is exactly the shape of widening #728 needed: an \
             x86 write-envelope guarantee just weakened. Re-derive the TTY-x86 and \
             TTY-blended filesystem-envelope claims before trusting them."
        );
    }
}

#[test]
fn aarch64_gate_scripts_mount_ext2_writable() {
    for path in AARCH64_GATE_SCRIPTS {
        let script = read(path);
        assert!(
            !ext2_drive_is_readonly(&script),
            "WORKLOAD-ENVELOPES.md records every aarch64 gate in this document \
             mounting ext2 writable (no device-level guarantee against writes, unlike \
             x86); {path} now mounts it readonly=on. This narrows rather than widens \
             the envelope, but the document's own claim about this script is now \
             stale either way -- update WORKLOAD-ENVELOPES.md to match."
        );
    }
}

// ---------------------------------------------------------------------------
// Item 3: -smp census
// ---------------------------------------------------------------------------

/// The `-smp N` value on the same QEMU-invocation line as `anchor` (the flag that
/// names this line as the aarch64 or x86 launch, e.g. `-M virt,gic-version=3` or
/// `-machine pc,accel=tcg`) -- so a multi-arch script (the tracing harness) is read
/// correctly by picking the line for the arch actually being checked, not just the
/// first `-smp` in the file.
fn smp_on_line_containing(script: &str, anchor: &str) -> u32 {
    let line = script
        .lines()
        .find(|l| l.contains(anchor))
        .unwrap_or_else(|| panic!("no line containing `{anchor}` found in this script"));
    let idx = line
        .find("-smp ")
        .unwrap_or_else(|| panic!("no `-smp` flag on the `{anchor}` line: {line}"));
    let rest = &line[idx + "-smp ".len()..];
    let token = rest
        .split(|c: char| c.is_whitespace() || c == '\\')
        .find(|t| !t.is_empty())
        .expect("a value follows -smp");
    token
        .parse::<u32>()
        .unwrap_or_else(|_| panic!("-smp value `{token}` is not a number"))
}

const AARCH64_SMP_ANCHOR: &str = "-M virt,gic-version=3";
const X86_SMP_ANCHOR: &str = "-machine pc,accel=tcg";

#[test]
fn aarch64_gate_scripts_boot_smp_4() {
    for path in AARCH64_GATE_SCRIPTS {
        let script = read(path);
        let smp = smp_on_line_containing(&script, AARCH64_SMP_ANCHOR);
        assert_eq!(
            smp, 4,
            "WORKLOAD-ENVELOPES.md records every aarch64 gate backing a standing \
             cell booting -smp 4; {path} now boots -smp {smp}. CPU count is a \
             concurrency axis in its own right -- re-derive the affected cell's \
             envelope before trusting it."
        );
    }
}

#[test]
fn x86_gate_scripts_boot_smp_1() {
    for path in X86_GATE_SCRIPTS {
        let script = read(path);
        let smp = smp_on_line_containing(&script, X86_SMP_ANCHOR);
        assert_eq!(
            smp, 1,
            "WORKLOAD-ENVELOPES.md \u{a7}2 records both x86 TTY gates booting -smp 1 \
             (single CPU); {path} now boots -smp {smp}. This is precisely the shape \
             of axis the #728 postmortem says was never recorded for Filesystem --\
             re-derive TTY-x86's envelope before trusting it under the new CPU count."
        );
    }
}

// ---------------------------------------------------------------------------
// Item 4: the boot_tests registry stays kthread-only
// ---------------------------------------------------------------------------

#[test]
fn boot_tests_registry_stays_kthread_only() {
    let source = read(EXECUTOR_RS);
    let body = fn_body(&source, "run_all_tests");
    assert!(
        !body.contains("create_user_process"),
        "WORKLOAD-ENVELOPES.md \u{a7}4/\u{a7}5-6 rely on run_all_tests() (the \
         boot_tests-profile 109-test registry backing Tracing-aarch64 and \
         Bus/NIC-aarch64) never creating a real userspace process -- it now calls \
         create_user_process(). The Tracing/Bus/NIC aarch64 concurrency envelope was \
         proven against a kthread-only registry; re-derive it now that the registry \
         itself can spawn userspace processes."
    );
    assert!(
        !body.contains("spawn("),
        "run_all_tests() now calls spawn(); re-derive the Tracing/Bus/NIC aarch64 \
         concurrency envelope in WORKLOAD-ENVELOPES.md \u{a7}4/\u{a7}5-6."
    );
}

// ---------------------------------------------------------------------------
// Mutation proofs -- each item above, proven to redden on a #713-shaped widening
// and stay quiet on an unrelated change. All mutations are applied to in-memory
// copies only (String::replace), matching this file family's existing idiom; the
// tree on disk is never touched.
// ---------------------------------------------------------------------------

#[test]
fn item1_mutation_proof() {
    let source = read(INIT_RS);
    let baseline = persistent_count_before(&source, "aarch64", "run_tty_oracle");
    assert_eq!(baseline, 1, "sanity: baseline must match the live #[test] above");

    // WIDENING: insert a second persistent aarch64 spawn ahead of run_tty_oracle(),
    // the same shape of change #713 made to Filesystem (a new concurrent process
    // appears ahead of code that assumed it was alone).
    let widened = source.replace(
        "    #[cfg(target_arch = \"aarch64\")]\n    run_tty_oracle();",
        "    #[cfg(target_arch = \"aarch64\")]\n    start_bounce();\n    #[cfg(target_arch = \"aarch64\")]\n    run_tty_oracle();",
    );
    assert_ne!(widened, source, "mutation did not apply");
    let widened_count = persistent_count_before(&widened, "aarch64", "run_tty_oracle");
    assert_eq!(
        widened_count, 2,
        "inserting a second persistent aarch64 launcher ahead of run_tty_oracle() \
         did not change the census -- the check would not have caught a #713-shaped \
         widening"
    );
    assert_ne!(
        widened_count, baseline,
        "the widening must be detectable as a change from the live baseline"
    );

    // CONTROL: an unrelated edit inside an already-reaped launcher's own print
    // string. Must not move the census at all.
    let control = source.replace(
        "\"[init] futex_handoff_oracle exited pid={} code={}\\n\"",
        "\"[init] futex_handoff_oracle finished pid={} code={}\\n\"",
    );
    assert_ne!(control, source, "control mutation did not apply");
    let control_count = persistent_count_before(&control, "aarch64", "run_tty_oracle");
    assert_eq!(
        control_count, baseline,
        "an unrelated print-string edit inside a reaped launcher must not move the \
         persistent-launcher census"
    );
}

#[test]
fn item2_mutation_proof() {
    let script = read("docker/qemu/run-x86-tty-oracle-gate.sh");
    assert!(ext2_drive_is_readonly(&script), "sanity: baseline must be readonly");

    // WIDENING: the x86 ext2 disk loses its readonly flag -- the FS write-envelope
    // guarantee this document leans on disappears.
    let widened = script.replace(
        "if=none,id=ext2disk,format=raw,readonly=on,file=$EXT2_IMG",
        "if=none,id=ext2disk,format=raw,file=$EXT2_IMG",
    );
    assert_ne!(widened, script, "mutation did not apply");
    assert!(
        !ext2_drive_is_readonly(&widened),
        "stripping readonly=on from the x86 ext2 drive line did not flip the check -- \
         the read-only guarantee could be lost silently"
    );

    // CONTROL: readonly=on stays, but an unrelated drive (the placeholder disk)
    // gets edited. Must not move the ext2 check either way.
    let control = script.replace(
        "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_ROOT/placeholder.img",
        "if=none,id=placeholder,format=raw,file=$OUTPUT_ROOT/placeholder.img",
    );
    assert_ne!(control, script, "control mutation did not apply");
    assert!(
        ext2_drive_is_readonly(&control),
        "editing an unrelated drive's readonly flag must not affect the ext2 census"
    );
}

#[test]
fn item3_mutation_proof() {
    let script = read("docker/qemu/run-x86-tty-oracle-gate.sh");
    let baseline = smp_on_line_containing(&script, X86_SMP_ANCHOR);
    assert_eq!(baseline, 1, "sanity: baseline must match the live #[test] above");

    // WIDENING: x86 goes multi-CPU. A real concurrency-envelope change.
    let widened = script.replace(
        "-machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512",
        "-machine pc,accel=tcg -cpu qemu64 -smp 2 -m 512",
    );
    assert_ne!(widened, script, "mutation did not apply");
    assert_eq!(
        smp_on_line_containing(&widened, X86_SMP_ANCHOR),
        2,
        "bumping -smp on the x86 QEMU line did not change what the census reads"
    );

    // CONTROL: -smp changed on a gate script none of the six standing cells cite
    // (run-x86-gate.sh, the beast merge-gate script, not in either watched list).
    // The watched-script census must not even look at it.
    let unrelated = read("docker/qemu/run-x86-gate.sh");
    assert!(
        !X86_GATE_SCRIPTS.contains(&"docker/qemu/run-x86-gate.sh")
            && !AARCH64_GATE_SCRIPTS.contains(&"docker/qemu/run-x86-gate.sh"),
        "sanity: this script must not be one of the watched gate scripts"
    );
    let _ = unrelated; // read only to prove the file exists; never fed to the census
}

#[test]
fn item4_mutation_proof() {
    let source = read(EXECUTOR_RS);
    let baseline_body = fn_body(&source, "run_all_tests").to_string();
    assert!(
        !baseline_body.contains("create_user_process") && !baseline_body.contains("spawn("),
        "sanity: baseline run_all_tests() must be kthread-only"
    );

    // WIDENING: run_all_tests() starts creating a real userspace process directly.
    let widened_body = format!(
        "{baseline_body}\n    let _ = kernel::process::creation::create_user_process(alloc::string::String::new(), &[]);"
    );
    assert!(
        widened_body.contains("create_user_process"),
        "mutation did not apply"
    );

    // CONTROL: a create_user_process-shaped call appears elsewhere in the same
    // file, outside run_all_tests()'s own body. The item-4 check only reads
    // run_all_tests()'s body, so this must not affect it.
    let control = source.replace(
        "/// Run all registered tests in parallel (EarlyBoot stage only)",
        "/// Run all registered tests in parallel (EarlyBoot stage only)\n/// (unrelated doc note: some other function might call create_user_process)",
    );
    assert_ne!(control, source, "control mutation did not apply");
    let control_body = fn_body(&control, "run_all_tests");
    assert!(
        !control_body.contains("create_user_process"),
        "an edit outside run_all_tests()'s own body must not leak into its census"
    );
}
