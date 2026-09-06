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

/// Assemble and link one fixture, returning its ELF path.
///
/// A toolchain that cannot produce an aarch64 ELF is a FAILURE here, not a
/// skip: these legs are the only thing that shows the guard rejects anything,
/// and a suite that quietly passed without them would be the vacuity they
/// exist to prevent.
fn build_fixture(name: &str, entry: &str, asm: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("breenix-lockup-guard-fixtures");
    fs::create_dir_all(&dir).expect("temp dir");
    let source = dir.join(format!("{name}.s"));
    let object = dir.join(format!("{name}.o"));
    let elf = dir.join(format!("{name}.elf"));
    fs::write(&source, asm).expect("write fixture asm");
    let cc = Command::new("clang")
        .args(["-target", "aarch64-unknown-none-elf", "-c", "-o"])
        .arg(&object)
        .arg(&source)
        .output()
        .expect("clang must be available to build the guard fixtures");
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
        .expect("rust-lld must be available to link the guard fixtures");
    assert!(
        ld.status.success(),
        "linking {name} failed: {}",
        String::from_utf8_lossy(&ld.stderr)
    );
    elf
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

fn assert_guard_rejects(name: &str, entry: &str, asm: &str, expected: &str) {
    let elf = build_fixture(name, entry, asm);
    let (ok, text) = run_guard(&elf);
    assert!(!ok, "{name}: the guard PASSED a fixture it must reject:\n{text}");
    assert!(
        text.contains(expected),
        "{name}: the guard failed, but not for the stated reason. Expected \
         {expected:?} in:\n{text}"
    );
}

// The fixture call graphs, as assembly. Each one is the smallest program that
// has the shape its test names.

const ASM_DIRECT: &str = "
.global dump_lockup_state
dump_lockup_state:
    bl __rust_alloc
    ret
.global __rust_alloc
__rust_alloc:
    ret
";

const ASM_TWO_HELPERS: &str = "
.global dump_lockup_state
dump_lockup_state:
    bl format_the_banner
    ret
.global format_the_banner
format_the_banner:
    bl widen_the_row
    ret
.global widen_the_row
widen_the_row:
    bl __rust_alloc
    ret
.global __rust_alloc
__rust_alloc:
    ret
";

const ASM_INDIRECT: &str = "
.section .data
.align 3
alloc_slot:
    .quad __rust_alloc
.text
.global dump_lockup_state
dump_lockup_state:
    adrp x8, alloc_slot
    add x8, x8, :lo12:alloc_slot
    ldr x8, [x8]
    blr x8
    ret
.global __rust_alloc
__rust_alloc:
    ret
";

const ASM_TAIL_CALL: &str = "
.global dump_lockup_state
dump_lockup_state:
    b write_the_tail
.global write_the_tail
write_the_tail:
    bl __rust_realloc
    ret
.global __rust_realloc
__rust_realloc:
    ret
";

const ASM_MISSING_ROOT: &str = "
.global some_other_function
some_other_function:
    bl __rust_alloc
    ret
.global __rust_alloc
__rust_alloc:
    ret
";

const ASM_UNRESOLVED_INDIRECT: &str = "
.global dump_lockup_state
dump_lockup_state:
    mov x8, x0
    blr x8
    ret
";

const ASM_CLEAN_CHAIN: &str = "
.global dump_lockup_state
dump_lockup_state:
    bl print_the_banner
    ret
.global print_the_banner
print_the_banner:
    bl write_one_byte
    ret
.global write_one_byte
write_one_byte:
    ret
";

#[test]
fn the_guard_rejects_a_direct_allocation() {
    assert_guard_rejects(
        "direct",
        "dump_lockup_state",
        ASM_DIRECT,
        "allocating call target(s) reachable",
    );
}

#[test]
fn the_guard_rejects_an_allocation_behind_two_innocent_helpers() {
    assert_guard_rejects(
        "two-helpers",
        "dump_lockup_state",
        ASM_TWO_HELPERS,
        "dump_lockup_state -> format_the_banner -> widen_the_row -> __rust_alloc",
    );
}

#[test]
fn the_guard_rejects_an_allocation_reached_through_a_data_pointer() {
    assert_guard_rejects(
        "indirect",
        "dump_lockup_state",
        ASM_INDIRECT,
        "dump_lockup_state -> __rust_alloc",
    );
}

#[test]
fn the_guard_rejects_an_allocation_reached_by_a_tail_call() {
    assert_guard_rejects(
        "tail-call",
        "dump_lockup_state",
        ASM_TAIL_CALL,
        "dump_lockup_state -> write_the_tail -> __rust_realloc",
    );
}

#[test]
fn the_guard_fails_when_the_root_symbol_is_absent() {
    assert_guard_rejects(
        "missing-root",
        "some_other_function",
        ASM_MISSING_ROOT,
        "no symbol whose name contains",
    );
}

#[test]
fn the_guard_fails_on_an_indirect_transfer_it_cannot_resolve() {
    assert_guard_rejects(
        "unresolved-indirect",
        "dump_lockup_state",
        ASM_UNRESOLVED_INDIRECT,
        "UNRESOLVED indirect",
    );
}

/// Without this leg, a guard that rejected each of its inputs would look perfect.
#[test]
fn the_guard_passes_a_clean_nonallocating_helper_chain() {
    let elf = build_fixture("clean-chain", "dump_lockup_state", ASM_CLEAN_CHAIN);
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
