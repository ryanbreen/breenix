//! Anti-vacuity for `scripts/check-aarch64-lockup-no-alloc.sh`.
//!
//! The guard is the authority on "no allocation is reachable from the AArch64
//! soft-lockup report". A guard with that standing has to be shown to FAIL
//! on the shapes it claims to catch, or a green result from it says little. This
//! suite builds tiny aarch64 ELFs whose call graphs are exactly those shapes
//! and runs the real script against each.
//!
//! # What each leg is for
//!
//! * DIRECT -- the root itself calls an allocator entry point.
//! * TWO HELPERS -- the allocation is two ordinary, innocently named frames
//!   down. This is the shape the depth-1 x86 guard cannot see, and the shape
//!   PR-7 actually removed (`process::try_dump_state` reaching
//!   `String::clone`).
//! * INDIRECT -- the callee is named only by a pointer in `.data`. A scan that
//!   read instruction text alone would see an anonymous jump and pass.
//! * TAIL CALL -- the transfer is a `b`, not a `bl`.
//! * MISSING ROOT -- no `dump_lockup_state` symbol. Checking no code must be a
//!   failure, not a pass.
//! * UNRESOLVED INDIRECT -- a `blr` through a register the decoder cannot
//!   resolve. A missing target is not a clean leaf.
//! * CLEAN CHAIN -- two helper frames and no allocation. Without this leg a
//!   guard that failed on everything would look perfect.
//!
//! # What these fixtures are NOT
//!
//! They exercise the ANALYSIS. They are not the PR's binary evidence: that is
//! the real linked kernel comparison, preserved under
//! `docs/planning/green-program/failure-capture/serials/pr7/` (green) and
//! `serials/pr7-red/binary-guard-red.txt` (the unfixed-main allocating
//! witness), and re-checked here without needing any toolchain.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const GUARD: &str = "scripts/check-aarch64-lockup-no-alloc.sh";
const RED_EVIDENCE: &str =
    "docs/planning/green-program/failure-capture/serials/pr7-red/binary-guard-red.txt";
const GREEN_EVIDENCE: &str =
    "docs/planning/green-program/failure-capture/serials/pr7/binary-guard-boottests.txt";

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let full = repo_path(rel);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

const FIXTURE_DIR: &str = "tests/fixtures/lockup-guard";

/// The committed ELF for one shape.
///
/// The fixtures are COMMITTED, not built at test time, and that is a
/// deliberate trade. Building them needs an assembler that can emit an aarch64
/// ELF, and the two hosts this repository's gates run on do not share one: the
/// Mac has `clang` and no `llvm-mc`, the beast x86 container has `llvm-mc` and
/// no `clang` or aarch64 cross toolchain. A suite that skipped its own legs on
/// whichever host was missing the tool would be vacuous exactly where nobody
/// was looking, and `scripts/run-structure-tests.sh` runs this file inside the
/// gate preflight on both.
///
/// So each of the 7 legs runs the guard against a committed ELF. The `.s`
/// beside each one is the source it was linked from, and
/// `the_committed_fixtures_match_their_sources` below rebuilds and re-scores
/// 7 of 7 on a host that CAN assemble -- which is what keeps the committed
/// binaries honest rather than merely present.
fn fixture_elf(name: &str) -> PathBuf {
    let path = repo_path(&format!("{FIXTURE_DIR}/{name}.aarch64-elf"));
    assert!(
        path.is_file(),
        "missing guard fixture {}; this suite has nothing to check",
        path.display()
    );
    path
}


/// Run the real guard against one ELF. Returns its status and its output.
fn run_guard(elf: &PathBuf) -> (bool, String) {
    let out = Command::new(repo_path(GUARD))
        .arg(elf)
        .output()
        .expect("the guard script must be executable");
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn assert_guard_rejects(name: &str, expected: &str) {
    let elf = fixture_elf(name);
    let (ok, text) = run_guard(&elf);
    assert!(!ok, "{name}: the guard PASSED a fixture it must reject:\n{text}");
    assert!(
        text.contains(expected),
        "{name}: the guard failed, but not for the stated reason. Expected \
         {expected:?} in:\n{text}"
    );
}

#[test]
fn the_guard_rejects_a_direct_allocation() {
    assert_guard_rejects(
        "direct",
        "allocating call target(s) reachable",
    );
}

#[test]
fn the_guard_rejects_an_allocation_behind_two_innocent_helpers() {
    assert_guard_rejects(
        "two-helpers",
        "dump_lockup_state -> format_the_banner -> widen_the_row -> __rust_alloc",
    );
}

#[test]
fn the_guard_rejects_an_allocation_reached_through_a_data_pointer() {
    assert_guard_rejects(
        "indirect",
        "dump_lockup_state -> __rust_alloc",
    );
}

#[test]
fn the_guard_rejects_an_allocation_reached_by_a_tail_call() {
    assert_guard_rejects(
        "tail-call",
        "dump_lockup_state -> write_the_tail -> __rust_realloc",
    );
}

#[test]
fn the_guard_fails_when_the_root_symbol_is_absent() {
    assert_guard_rejects(
        "missing-root",
        "no symbol whose name contains",
    );
}

#[test]
fn the_guard_fails_on_an_indirect_transfer_it_cannot_resolve() {
    assert_guard_rejects(
        "unresolved-indirect",
        "UNRESOLVED indirect",
    );
}

/// Without this leg, a guard that rejected each of its inputs would look perfect.
#[test]
fn the_guard_passes_a_clean_nonallocating_helper_chain() {
    let elf = fixture_elf("clean-chain");
    let (ok, text) = run_guard(&elf);
    assert!(ok, "the guard rejected a clean chain:\n{text}");
    assert!(
        text.contains("PASS: 0 allocation sinks reachable"),
        "the guard passed without saying what it checked:\n{text}"
    );
}

/// The PR's own binary evidence, re-read without any toolchain: the guard's
/// preserved output on the unfixed-main allocating witness and on the repaired
/// boot_tests kernel. The fixtures above prove the ANALYSIS rejects each shape;
/// these two files are what the analysis actually said about the real linked
/// kernels, and they are what the round note cites.
/// kernels. claim-lint:ok: the two files are the 1 red and 1 green real-kernel
/// result recorded in section 4 of
/// docs/planning/green-program/failure-capture/PR-7-2026-09-06.md
#[test]
fn the_preserved_binary_evidence_is_a_red_and_a_green_on_real_kernels() {
    let red = read(RED_EVIDENCE);
    assert!(
        red.contains("allocating call target(s) reachable from the root"),
        "the preserved red evidence does not show an allocation finding"
    );
    assert!(
        red.contains("process14try_dump_state"),
        "the preserved red evidence must name the allocating callee the repair \
         removed, not merely report some failure"
    );
    assert!(red.contains("sha256:"), "the red evidence must say which ELF it read");

    let green = read(GREEN_EVIDENCE);
    assert!(
        green.contains("PASS: 0 allocation sinks reachable"),
        "the preserved green evidence does not show a pass"
    );
    assert!(
        !green.contains("FAIL"),
        "the preserved green evidence carries a FAIL line"
    );
    assert!(green.contains("sha256:"), "the green evidence must say which ELF it read");
}

/// The 3 aarch64 gates have to RUN this guard, on the kernel each of them
/// selected. A guard with no caller is documentation.
#[test]
fn the_aarch64_gates_run_the_guard_on_their_own_kernel() {
    let gates = [
        "docker/qemu/run-aarch64-boot-test-strict.sh",
        "docker/qemu/run-aarch64-service-sequence-gate.sh",
        "docker/qemu/run-aarch64-prod-profile-boot-test.sh",
    ];
    for gate in gates {
        let body = read(gate);
        assert!(
            body.contains("check-aarch64-lockup-no-alloc.sh"),
            "{gate} does not run the soft-lockup allocation guard"
        );
        let wired = "check-aarch64-lockup-no-alloc.sh\" \"$KERNEL\"";
        assert!(
            body.contains(wired),
            "{gate} must hand the guard its OWN selected kernel"
        );
    }
}

/// The 7 shapes: name, entry symbol, and whether the guard must reject it.
const FIXTURES: [(&str, &str, bool); 7] = [
    ("direct", "dump_lockup_state", true),
    ("two-helpers", "dump_lockup_state", true),
    ("indirect", "dump_lockup_state", true),
    ("tail-call", "dump_lockup_state", true),
    ("missing-root", "some_other_function", true),
    ("unresolved-indirect", "dump_lockup_state", true),
    ("clean-chain", "dump_lockup_state", false),
];

/// An assembler that can emit an aarch64 ELF object, if this host has one.
/// `clang` targets any architecture; `llvm-mc` ships with the rustup
/// `llvm-tools` component on hosts that installed it.
fn aarch64_assembler() -> Option<(PathBuf, Vec<String>)> {
    if Command::new("clang").arg("--version").output().is_ok() {
        let args = ["-target", "aarch64-unknown-none-elf", "-c", "-o"];
        let owned = args.iter().map(|a| a.to_string()).collect();
        return Some((PathBuf::from("clang"), owned));
    }
    let mc = rust_lld().with_file_name("llvm-mc");
    if mc.is_file() {
        let args = ["--arch=aarch64", "--filetype=obj", "-o"];
        let owned = args.iter().map(|a| a.to_string()).collect();
        return Some((mc, owned));
    }
    None
}

/// Rebuild each committed fixture from the `.s` beside it and re-score it.
///
/// This is what keeps the committed binaries honest: on a host with an aarch64
/// assembler, 7 of 7 fixtures are re-derived from their own sources and the
/// guard's verdict on each rebuild must match its verdict on the committed ELF.
///
/// Stated narrowing: on a host with no such assembler this leg returns without
/// checking anything. The other 7 legs still run the guard against the committed
/// ELFs, so the suite is not vacuous there; what it cannot check on that host is
/// that each ELF and its `.s` still correspond.
#[test]
fn the_committed_fixtures_match_their_sources() {
    let Some((assembler, prefix)) = aarch64_assembler() else {
        return;
    };
    let dir = std::env::temp_dir().join("breenix-lockup-guard-rebuild");
    fs::create_dir_all(&dir).expect("temp dir");
    for (name, entry, must_reject) in FIXTURES {
        let source = repo_path(&format!("{FIXTURE_DIR}/{name}.s"));
        assert!(source.is_file(), "missing fixture source {}", source.display());
        let object = dir.join(format!("{name}.o"));
        let elf = dir.join(format!("{name}.aarch64-elf"));
        let cc = Command::new(&assembler)
            .args(&prefix)
            .arg(&object)
            .arg(&source)
            .output()
            .expect("assembler runs");
        assert!(
            cc.status.success(),
            "assembling {name} failed: {}",
            String::from_utf8_lossy(&cc.stderr)
        );
        let ld = Command::new(rust_lld())
            .args(["-flavor", "gnu", "-e", entry, "-o"])
            .arg(&elf)
            .arg(&object)
            .output()
            .expect("rust-lld runs");
        assert!(
            ld.status.success(),
            "linking {name} failed: {}",
            String::from_utf8_lossy(&ld.stderr)
        );
        let (rebuilt_ok, rebuilt_text) = run_guard(&elf);
        let (committed_ok, _) = run_guard(&fixture_elf(name));
        assert_eq!(
            rebuilt_ok, committed_ok,
            "{name}: the committed fixture and a rebuild from {name}.s score \
             differently, so the committed ELF no longer matches its source:\n{rebuilt_text}"
        );
        assert_eq!(
            rebuilt_ok, !must_reject,
            "{name}: rebuilt fixture scored the wrong way:\n{rebuilt_text}"
        );
    }
}

/// The `rust-lld` that ships with the active toolchain. Preferred over a PATH
/// `ld` because the host linker on macOS produces Mach-O, not ELF.
fn rust_lld() -> PathBuf {
    let out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc must be on PATH");
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let root = PathBuf::from(sysroot).join("lib/rustlib");
    let entries = fs::read_dir(&root).expect("rustlib must exist");
    for entry in entries {
        let candidate = entry.expect("readable entry").path().join("bin/rust-lld");
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("no rust-lld under {}", root.display());
}
