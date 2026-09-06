//! #877 ratchet: `docker/qemu/lib/gate-boot-facts.sh` must stay safe under
//! `set -euo pipefail` for the inputs its two x86 callers can hand it,
//! including a PID whose process has already exited.
//!
//! #877's own report: `gbf_resolve_qemu_pid`'s `child="$(pgrep ... | head
//! -1)"` assignment had no `|| true`, even though the function's own
//! `comm=` assignment two lines above it already carried one for the
//! identical `set -o pipefail` + "no match" hazard. #865's own landing
//! review generalized that hazard as "a `ps -o ...` pipeline whose first
//! stage can fail while a later stage still exits 0" and, under that
//! generalization, fixed 4 of 5 command-substitution assignments in this
//! file carrying the shape (the census below covers the 5th too); it did
//! not recognize `pgrep -P ... -x ... | head -1` -- a different command,
//! the same hazard -- as the one it missed. That gap was closed on `main`
//! in a separate landing commit for #821 (`3446eb16`, discovered the same
//! way #865's own 4 were: a gate aborting mid-boot on a genuinely-passing
//! boot) before this ratchet was written. This file re-demonstrates that
//! fix on this machine (see the pipefail-survival test below) and locks
//! the shape in place with a census plus its own mutation test, rather
//! than re-applying the fix itself. Both x86 gates
//! (`run-x86-boot-tests.sh`, `run-x86-prod-profile-boot-test.sh`) run under
//! `set -euo pipefail`; the two aarch64 callers run under plain `set -e`
//! with no `pipefail`, so this defect was x86-only and did not show up in
//! any aarch64 gate run, however many.
//!
//! Two properties, checked against the real file, not merely asserted:
//!
//! 1. A shape census: each of the library's 5 `var="$(...)"` command-
//!    substitution assignments carries a `|| true` (or equivalent)
//!    fallback immediately after it. This is the general shape of the
//!    #877 defect,
//!    not a check pinned to the one line that happened to be missing it --
//!    a future helper added to this file with the same bare-assignment
//!    shape reddens this test by name before it ever reaches beast.
//! 2. A real `bash -euo pipefail` execution of the sourced library,
//!    against a PID this process cannot be (a `u32::MAX`-adjacent value),
//!    on each of the (up to 2) `bash` binaries this Mac has among the
//!    candidates checked (`/bin/bash`, the system-shipped 3.2, and
//!    whatever `bash` resolves to on PATH) -- exit 0, with each helper's
//!    own documented fallback value.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

const GATE_BOOT_FACTS_LIB: &str = "docker/qemu/lib/gate-boot-facts.sh";

/// First byte offset of `needle` in `haystack`, or `None`. Plain
/// byte-window search -- this file is pure ASCII shell, so byte offsets
/// and `str` char-boundary offsets coincide throughout.
/// claim-lint:ok: verified via `LC_ALL=C grep -nP '[^\x00-\x7F]'
/// docker/qemu/lib/gate-boot-facts.sh`, which reported 0 matches.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Each `IDENT="$(...)"` command-substitution assignment in
/// `gate-boot-facts.sh` whose `$(...)` is NOT immediately followed by
/// `|| true` -- the fallback this file's own comments document as
/// load-bearing under `set -o pipefail`, since both real x86 callers of
/// this file (`run-x86-boot-tests.sh`, `run-x86-prod-profile-boot-test.sh`)
/// use `set -euo pipefail`, under which a bare failing assignment (no
/// `|| true`) aborts the whole script via the `set -e` ERR trap before any
/// of this file's own `if`/`case` recovery logic runs.
///
/// Backslash-newline continuations are joined first (one real assignment
/// in this file, in `gbf_last_heartbeat_uptime_ms`, wraps its `$(...)`
/// across two source lines), and comment lines are skipped -- this file's
/// own header comments quote the generic shape `v="$(pipeline)"` as prose,
/// which is not a real assignment.
fn assignments_missing_pipefail_fallback(lib_text: &str) -> Vec<String> {
    let joined = lib_text.replace("\\\n", " ");
    let marker = b"=\"$(";
    let mut missing = Vec::new();

    for line in joined.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let bytes = line.as_bytes();
        let mut scan_from = 0usize;
        while let Some(rel) = find_bytes(&bytes[scan_from..], marker) {
            let marker_pos = scan_from + rel;

            // Identifier immediately before the `=`.
            let mut ident_start = marker_pos;
            while ident_start > 0 {
                let c = bytes[ident_start - 1];
                if c.is_ascii_alphanumeric() || c == b'_' {
                    ident_start -= 1;
                } else {
                    break;
                }
            }
            let ident = line[ident_start..marker_pos].to_string();
            assert!(
                !ident.is_empty(),
                "found `=\"$(` with no identifier before it in {GATE_BOOT_FACTS_LIB} \
                 (line: {line:?}) -- census logic assumption broken, fix the test"
            );

            // Walk parens from just past the already-matched `(` (depth
            // starts at 1) to find its close. 0 of the file's 5 real
            // `$(...)` bodies contain a nested literal `(` or `)` --
            // verified against the source this test reads, not assumed --
            // so a plain depth count (ignoring quoting) is exact here.
            let mut depth = 1i32;
            let mut idx = marker_pos + marker.len();
            let mut close_paren = None;
            while idx < bytes.len() {
                match bytes[idx] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            close_paren = Some(idx);
                            break;
                        }
                    }
                    _ => {}
                }
                idx += 1;
            }
            let close_paren = close_paren.unwrap_or_else(|| {
                panic!(
                    "unbalanced $(...) for assignment `{ident}` in {GATE_BOOT_FACTS_LIB} \
                     (line: {line:?})"
                )
            });
            assert_eq!(
                bytes.get(close_paren + 1),
                Some(&b'"'),
                "assignment `{ident}` in {GATE_BOOT_FACTS_LIB} does not close with \
                 `)\"` as every real assignment in this file does (line: {line:?})"
            );

            let rest = line[close_paren + 2..].trim_start();
            if !rest.starts_with("|| true") {
                missing.push(ident);
            }
            scan_from = close_paren + 1;
        }
    }
    missing
}

#[test]
fn every_command_substitution_assignment_has_a_pipefail_fallback() {
    let lib_text = repo_text(GATE_BOOT_FACTS_LIB);
    let missing = assignments_missing_pipefail_fallback(&lib_text);
    assert!(
        missing.is_empty(),
        "{GATE_BOOT_FACTS_LIB} has command-substitution assignment(s) with no \
         `|| true` fallback: {missing:?} -- under the `set -euo pipefail` both \
         x86 gates run under, a bare failing assignment here aborts the whole \
         gate on a genuinely-passing boot (#877)"
    );
}

/// ANTI-VACUITY: the census above must actually redden, by name, when a
/// real `|| true` is removed from the real file's own text (in memory) --
/// on two different assignments, so the test is not accidentally locked to
/// only the one line #877 itself was missing.
#[test]
fn pipefail_fallback_census_is_not_vacuous() {
    let lib_text = repo_text(GATE_BOOT_FACTS_LIB);
    assert!(
        assignments_missing_pipefail_fallback(&lib_text).is_empty(),
        "sanity: the real library must be clean before mutation"
    );

    // Mutation 1: the exact line #877 reported -- `child="$(pgrep ... |
    // head -1)"` in gbf_resolve_qemu_pid, fixed on `main` by 3446eb16
    // before this ratchet was written (see the module doc above).
    let child_line = "child=\"$(pgrep -P \"$wrapper_pid\" -x \"$qemu_bin\" 2>/dev/null | head -1)\" || true";
    assert!(
        lib_text.contains(child_line),
        "the reconstructed `child=` assignment line must match the real file \
         exactly, or this mutation applies to the wrong text"
    );
    let child_mutated = lib_text.replacen(
        child_line,
        "child=\"$(pgrep -P \"$wrapper_pid\" -x \"$qemu_bin\" 2>/dev/null | head -1)\"",
        1,
    );
    assert_ne!(child_mutated, lib_text, "mutation must apply");
    assert_eq!(
        assignments_missing_pipefail_fallback(&child_mutated),
        vec!["child".to_string()],
        "removing `child`'s `|| true` must redden specifically on `child`, \
         not some other assignment"
    );

    // Mutation 2: a different helper's assignment (`raw=` in
    // gbf_qemu_cpu_seconds) -- proves the census is not hardcoded to only
    // the `child` line #877 happened to be missing.
    let raw_line = "raw=\"$(ps -o time= -p \"$pid\" 2>/dev/null | tr -d ' ')\" || true";
    assert!(
        lib_text.contains(raw_line),
        "the reconstructed `raw=` assignment line must match the real file \
         exactly, or this mutation applies to the wrong text"
    );
    let raw_mutated = lib_text.replacen(
        raw_line,
        "raw=\"$(ps -o time= -p \"$pid\" 2>/dev/null | tr -d ' ')\"",
        1,
    );
    assert_ne!(raw_mutated, lib_text, "mutation must apply");
    assert_eq!(
        assignments_missing_pipefail_fallback(&raw_mutated),
        vec!["raw".to_string()],
        "removing `raw`'s `|| true` must redden specifically on `raw`"
    );
}

/// A `bash -euo pipefail` process, sourcing the real library and printing
/// each helper's result for a PID/file value chosen so this process
/// cannot collide with a real one, one line per helper so a failure names
/// which helper broke.
fn run_helpers_under_pipefail(bash_bin: &str, lib_path: &PathBuf) -> std::process::Output {
    // A PID this process cannot legitimately collide with: kernel PID
    // space on both macOS and Linux tops out well below u32::MAX, and this
    // value is reused verbatim for the "already-gone" serial-file path
    // check's own directory component so both take the identical
    // no-such-entity path through the OS.
    let bogus_pid = "4294960001";
    let script = format!(
        "source {lib}\n\
         printf 'PID=%s\\n' \"$(gbf_resolve_qemu_pid {pid} qemu-system-x86_64)\"\n\
         printf 'LOAD=%s\\n' \"$(gbf_load_1m)\"\n\
         printf 'CPU=%s\\n' \"$(gbf_qemu_cpu_seconds {pid})\"\n\
         printf 'HB=%s\\n' \"$(gbf_last_heartbeat_uptime_ms /nonexistent/{pid}/serial.txt)\"\n",
        lib = shell_quote(&lib_path.to_string_lossy()),
        pid = bogus_pid,
    );
    Command::new(bash_bin)
        .args(["-euo", "pipefail", "-c", &script])
        .output()
        .unwrap_or_else(|error| panic!("spawn {bash_bin} -euo pipefail -c ...: {error}"))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `bash` binaries this Mac has, out of 2 candidates checked:
/// `/bin/bash` (the oldest bash this library must run under -- Apple has
/// not updated it since bash 3.2's GPLv2 relicensing) and whatever `bash`
/// resolves to on `PATH` (a newer Homebrew bash in this environment).
/// A candidate whose binary does not exist on this machine is skipped
/// rather than failing, so this test degrades gracefully on a Mac without
/// Homebrew bash installed rather than making a false claim about a
/// binary this run did not exercise.
fn available_bash_binaries() -> Vec<String> {
    let candidates = ["/bin/bash", "bash"];
    candidates
        .iter()
        .filter(|candidate| {
            Command::new(candidate)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn gbf_helpers_survive_pipefail_with_a_nonexistent_pid_on_every_bash_here() {
    let lib_path = repo_root().join(GATE_BOOT_FACTS_LIB);
    let bash_binaries = available_bash_binaries();
    assert!(
        !bash_binaries.is_empty(),
        "no bash binary found on this machine (checked /bin/bash and PATH `bash`) \
         -- cannot prove the #877 fix without at least one"
    );

    for bash_bin in &bash_binaries {
        let output = run_helpers_under_pipefail(bash_bin, &lib_path);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "gate-boot-facts.sh helpers must survive `{bash_bin} -euo pipefail` with \
             a nonexistent pid (#877); exit status: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status.code()
        );
        assert!(
            stdout.contains("PID=4294960001"),
            "gbf_resolve_qemu_pid must fall back to the wrapper pid itself when no \
             matching process/child exists ({bash_bin}); stdout:\n{stdout}"
        );
        assert!(
            stdout.contains("CPU=NA"),
            "gbf_qemu_cpu_seconds must report its documented \"NA\" fallback for a \
             pid with no `ps` entry ({bash_bin}); stdout:\n{stdout}"
        );
        assert!(
            stdout.contains("HB=NA"),
            "gbf_last_heartbeat_uptime_ms must report its documented \"NA\" fallback \
             for a serial file that does not exist ({bash_bin}); stdout:\n{stdout}"
        );
        assert!(
            stdout.contains("LOAD="),
            "gbf_load_1m must print a LOAD= line regardless of the bogus pid used \
             elsewhere in this script ({bash_bin}); stdout:\n{stdout}"
        );
    }
}

#[test]
fn gate_boot_facts_lib_has_no_syntax_errors() {
    let lib_path = repo_root().join(GATE_BOOT_FACTS_LIB);
    for bash_bin in available_bash_binaries() {
        let output = Command::new(&bash_bin)
            .args(["-n", &lib_path.to_string_lossy()])
            .output()
            .unwrap_or_else(|error| panic!("spawn {bash_bin} -n {GATE_BOOT_FACTS_LIB}: {error}"));
        assert!(
            output.status.success(),
            "{bash_bin} -n {GATE_BOOT_FACTS_LIB} must succeed; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
