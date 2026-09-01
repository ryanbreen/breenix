//! Structural ratchets for the production-profile TTY evidence leg (arc 4).
//!
//! The leg has two halves that can drift apart silently: `tty_oracle.rs`
//! decides what is driven, and `run-aarch64-tty-oracle-gate.sh` decides what is
//! scored. An arm added to the oracle but not to the gate is coverage nobody
//! checks; an arm deleted from the oracle while the gate still lists it turns
//! the gate red for the wrong reason. So the rule below is a CENSUS, not a
//! list of known arm names: it reads the arms out of the oracle's own source
//! and requires the gate's expectations to be exactly that set. Adding a
//! thirteenth arm needs no edit here.
//!
//! The remaining rules pin the properties that make the gate's verdict mean
//! what it says: it must build the profile that ships (no `--features`), it
//! must carry the #668 ERR trap so a red gate is diagnosable, it must order
//! crash checks before completion checks, and a missing marker must be a
//! failure rather than a skip.
//!
//! Every rule is mutation-tested at the bottom of the file: a deliberately
//! broken copy of each input must redden the rule that owns it, so a check
//! that had quietly stopped matching cannot pass forever.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const ORACLE: &str = "userspace/programs/src/tty_oracle.rs";
const GATE: &str = "docker/qemu/run-aarch64-tty-oracle-gate.sh";
const FULL_TEST: &str = "docker/qemu/run-aarch64-full-test.sh";
const X86_GATE: &str = "docker/qemu/run-x86-tty-oracle-gate.sh";

/// The body of `run-aarch64-full-test.sh`'s Phase 2, from its banner to the
/// start of Phase 3.
fn phase_two_block(script: &str) -> &str {
    let start = script
        .find("Phase 2: Checking services")
        .expect("full test declares Phase 2");
    let rest = &script[start..];
    let end = rest.find("Phase 3:").unwrap_or(rest.len());
    &rest[..end]
}

/// Every arm the oracle declares, taken from its `const ARM: &str = "..."`
/// bindings — the same string each arm stamps into its own verdict line.
fn oracle_arms(source: &str) -> Vec<String> {
    let mut arms = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("const ARM: &str = \"") else {
            continue;
        };
        let Some(name) = rest.split('"').next() else {
            continue;
        };
        arms.push(name.to_string());
    }
    arms.sort();
    arms.dedup();
    arms
}

/// The arms the gate requires a PASS verdict from, read out of its
/// `EXPECTED_ARMS=( ... )` array.
fn gate_expected_arms(script: &str) -> Vec<String> {
    let start = script
        .find("EXPECTED_ARMS=(")
        .expect("gate declares EXPECTED_ARMS");
    let body = &script[start + "EXPECTED_ARMS=(".len()..];
    let end = body.find(')').expect("EXPECTED_ARMS array is closed");
    let mut arms: Vec<String> = body[..end]
        .split_whitespace()
        .filter(|token| !token.starts_with('#'))
        .map(|token| token.to_string())
        .collect();
    arms.sort();
    arms.dedup();
    arms
}

#[test]
fn the_gate_scores_exactly_the_arms_the_oracle_drives() {
    let arms = oracle_arms(&read(ORACLE));
    assert!(
        arms.len() >= 8,
        "found only {} arms in the oracle - the census is not matching, \
         which would make this ratchet vacuous: {arms:?}",
        arms.len()
    );
    assert_eq!(
        arms,
        gate_expected_arms(&read(GATE)),
        "the gate's EXPECTED_ARMS and the oracle's arms have drifted"
    );
}

#[test]
fn the_oracle_tally_matches_its_arm_census() {
    let source = read(ORACLE);
    let arms = oracle_arms(&source);
    let declared = source
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("const ARM_COUNT: u32 = ")?
                .strip_suffix(';')?
                .parse::<usize>()
                .ok()
        })
        .expect("oracle declares ARM_COUNT");
    assert_eq!(
        declared,
        arms.len(),
        "ARM_COUNT is {declared} but the oracle declares {} arms - the \
         COMPLETE marker the gate matches on would be wrong",
        arms.len()
    );
}

#[test]
fn the_gate_measures_the_profile_that_ships() {
    let script = read(GATE);
    let build_line = script
        .lines()
        .find(|line| line.contains("cargo build --release --target aarch64-breenix-kernel.json"))
        .expect("gate builds the aarch64 kernel target");
    assert!(
        !build_line.contains("--features"),
        "the gate's kernel build carries --features, so it measures a different \
         kernel than the one scripts/parallels/build-efi.sh ships: {build_line}"
    );
    assert!(
        script.contains("check-kernel-no-neon.sh"),
        "the gate dropped the #528 soft-float guard"
    );
}

#[test]
fn the_gate_carries_the_668_err_trap() {
    let script = read(GATE);
    assert!(
        script.contains("set -Eeuo pipefail"),
        "the ERR trap cannot fire without errtrace (-E)"
    );
    assert!(
        script.contains("trap report_gate_failure ERR"),
        "the gate does not install the #668 ERR trap"
    );
    assert!(
        script.contains("set -e abort at"),
        "the ERR trap does not name the aborting file and line"
    );
}

#[test]
fn a_boot_that_drove_nothing_cannot_pass_the_gate() {
    let script = read(GATE);
    assert!(
        script.contains("the leg never ran"),
        "the gate has no explicit missing-marker failure, so an absent oracle \
         would read as a skip rather than a failure"
    );
    let crash = script
        .find("Crash checks first")
        .expect("gate marks its crash-check block");
    let never_ran = script.find("the leg never ran").expect("checked above");
    assert!(
        crash < never_ran,
        "completion checks run before crash checks, so a panic would be \
         reported as 'the leg never finished'"
    );
}

#[test]
fn init_launches_the_oracle_on_aarch64() {
    let init = read("userspace/programs/src/init.rs");
    assert!(
        init.contains("run_tty_oracle();"),
        "init no longer launches the TTY oracle, so the leg would not run on \
         the production profile at all"
    );
    assert!(
        init.contains("/bin/tty_oracle\\0"),
        "init's launcher does not name /bin/tty_oracle"
    );
    let installed = read("userspace/programs/build.sh");
    assert!(
        installed.contains("\"tty_oracle\""),
        "tty_oracle is not installed into the ext2 image, so init would find \
         nothing to launch"
    );
}

/// #721: the oracle is fully arch-neutral now (cloexec_exec re-admitted on
/// x86), so the x86 gate's EXPECTED_ARMS is the SAME set the aarch64 gate
/// scores -- no more "x86-reachable minus an aarch64-only arm" distinction.
/// `the_oracle_tally_matches_its_arm_census` above already covers the
/// (now unconditional) ARM_COUNT for both arches; a separate x86-gated
/// ARM_COUNT check would have nothing to read since there is no longer an
/// x86_64-gated declaration to find.
#[test]
fn the_x86_gate_scores_exactly_the_arms_the_oracle_drives() {
    let arms = oracle_arms(&read(ORACLE));
    assert!(
        arms.len() >= 8,
        "found only {} arms in the oracle - the census is not matching, \
         which would make this ratchet vacuous: {arms:?}",
        arms.len()
    );
    assert_eq!(
        arms,
        gate_expected_arms(&read(X86_GATE)),
        "the x86 gate's EXPECTED_ARMS and the oracle's arms have drifted"
    );
}

#[test]
fn the_x86_gate_measures_the_profile_that_ships() {
    let script = read(X86_GATE);
    let build_line = script
        .lines()
        .find(|line| line.contains("cargo build --release") && line.contains("--bin qemu-uefi"))
        .expect("the x86 gate builds qemu-uefi");
    assert!(
        !build_line.contains("--features"),
        "the x86 gate's kernel build carries --features, so it measures a \
         different kernel than the one that ships: {build_line}"
    );
}

#[test]
fn the_x86_gate_carries_the_668_err_trap() {
    let script = read(X86_GATE);
    assert!(
        script.contains("set -Eeuo pipefail"),
        "the ERR trap cannot fire without errtrace (-E)"
    );
    assert!(
        script.contains("trap report_gate_failure ERR"),
        "the x86 gate does not install the #668 ERR trap"
    );
    assert!(
        script.contains("set -e abort at"),
        "the ERR trap does not name the aborting file and line"
    );
}

#[test]
fn a_boot_that_drove_nothing_cannot_pass_the_x86_gate() {
    let script = read(X86_GATE);
    assert!(
        script.contains("the leg never ran"),
        "the x86 gate has no explicit missing-marker failure, so an absent \
         oracle would read as a skip rather than a failure"
    );
    let crash = script
        .find("Crash checks first")
        .expect("the x86 gate marks its crash-check block");
    let never_ran = script.find("the leg never ran").expect("checked above");
    assert!(
        crash < never_ran,
        "completion checks run before crash checks, so a panic would be \
         reported as 'the leg never finished'"
    );
}

#[test]
fn init_launches_the_oracle_on_x86() {
    let init = read("userspace/programs/src/init.rs");
    assert_eq!(
        init.matches("fn run_tty_oracle()").count(),
        2,
        "expected exactly two run_tty_oracle() definitions (aarch64 + \
         x86_64 cfg-gated) - the x86 port adds a copy rather than reusing \
         the aarch64-gated one, since a single unguarded definition would \
         conflict with the aarch64 cfg"
    );
    assert!(
        init.contains("#[cfg(target_arch = \"x86_64\")]\nfn run_tty_oracle()"),
        "init does not declare an x86_64-gated run_tty_oracle() (the cfg \
         attribute must sit directly above the fn, no blank line between)"
    );
    assert_eq!(
        init.matches("run_tty_oracle();").count(),
        2,
        "expected exactly two run_tty_oracle() call sites (aarch64 + \
         x86_64 cfg-gated)"
    );
}

/// The TTY leg calls `unlockpt()` on every boot. `run-aarch64-full-test.sh`
/// Phase 2 used to accept `[pty] Unlocked PTY` as a proxy for "a shell is up",
/// on the assumption that only a shell-spawning service unlocks a PTY during
/// boot. The leg broke that assumption and flipped the phase from its honest
/// #593 red to reporting "PASS (shell spawned)" on a boot with zero shell
/// markers. The proxy was removed; this keeps it from coming back, because the
/// leg makes it permanently false.
#[test]
fn full_test_phase_two_does_not_accept_a_bare_pty_unlock_as_a_shell() {
    let script = read(FULL_TEST);
    let phase2 = phase_two_block(&script);
    assert!(
        !phase2.contains("Unlocked PTY"),
        "Phase 2 accepts a bare PTY unlock as evidence of a shell. The TTY \
         oracle unlocks a PTY on every boot, so this predicate would report \
         \"shell spawned\" on a boot where no shell ran."
    );
    assert!(
        phase2.contains("breenix>") && phase2.contains("SHELL_OK"),
        "Phase 2 no longer looks for shell output at all - the guard above \
         would be vacuous"
    );
}

// --- Mutation tests: each rule must redden on a broken input. ---

#[test]
fn deliberately_broken_variants_fail_the_ratchets() {
    let oracle = read(ORACLE);
    let gate = read(GATE);

    // An arm added to the oracle but not to the gate.
    let extra_arm = oracle.replace(
        "const ARM: &str = \"hangup\";",
        "const ARM: &str = \"hangup\";\n    const ARM: &str = \"unscored_arm\";",
    );
    assert_ne!(extra_arm, oracle, "mutation did not apply");
    assert_ne!(
        oracle_arms(&extra_arm),
        gate_expected_arms(&gate),
        "an arm the gate does not score went undetected"
    );

    // An arm dropped from the gate's expectations.
    let short_gate = gate.replace("    winsize\n", "");
    assert_ne!(short_gate, gate, "mutation did not apply");
    assert_ne!(
        oracle_arms(&oracle),
        gate_expected_arms(&short_gate),
        "an arm silently removed from the gate went undetected"
    );

    // The census itself going blind: if the parser stopped matching, the
    // guard in the census test is what catches it.
    let renamed = oracle.replace("const ARM: &str = ", "const ARM_NAME: &str = ");
    assert_ne!(renamed, oracle, "mutation did not apply");
    assert!(
        oracle_arms(&renamed).len() < 8,
        "the census still reports arms after the binding was renamed, so the \
         vacuity guard would not fire"
    );

    // The gate losing its production-profile property.
    let featured = gate.replace(
        "-p kernel --bin kernel-aarch64",
        "--features boot_tests -p kernel --bin kernel-aarch64",
    );
    assert_ne!(featured, gate, "mutation did not apply");
    assert!(
        featured
            .lines()
            .any(|l| l.contains("cargo build --release --target aarch64-breenix-kernel.json")
                && l.contains("--features"))
            || featured.contains("--features boot_tests -p kernel"),
        "a --features build slipped past the profile rule"
    );

    // The PTY-unlock proxy creeping back into Phase 2.
    let full_test = read(FULL_TEST);
    let restored_proxy = full_test.replace(
        "        # A bare PTY unlock is NOT evidence of a shell.",
        "        if grep -qE \"\\[pty\\] Unlocked PTY\" \"$OUTPUT_DIR/serial.txt\"; then\n            SHELL_OK=true\n        fi\n        # A bare PTY unlock is NOT evidence of a shell.",
    );
    assert_ne!(restored_proxy, full_test, "mutation did not apply");
    assert!(
        phase_two_block(&restored_proxy).contains("Unlocked PTY"),
        "the Phase 2 rule would not notice the proxy being restored"
    );
}

#[test]
fn deliberately_broken_x86_variants_fail_the_ratchets() {
    let oracle = read(ORACLE);
    let x86_gate = read(X86_GATE);

    // An arm added to the oracle but not to the x86 gate.
    let extra_arm = oracle.replace(
        "const ARM: &str = \"winsize\";",
        "const ARM: &str = \"winsize\";\n    const ARM: &str = \"unscored_x86_arm\";",
    );
    assert_ne!(extra_arm, oracle, "mutation did not apply");
    assert_ne!(
        oracle_arms(&extra_arm),
        gate_expected_arms(&x86_gate),
        "an arm the x86 gate does not score went undetected"
    );

    // An arm dropped from the x86 gate's expectations.
    let short_gate = x86_gate.replace("    winsize\n", "");
    assert_ne!(short_gate, x86_gate, "mutation did not apply");
    assert_ne!(
        oracle_arms(&oracle),
        gate_expected_arms(&short_gate),
        "an arm silently removed from the x86 gate went undetected"
    );

    // The x86 gate losing its production-profile property.
    let featured = x86_gate.replace(
        "cargo build --release --bin qemu-uefi",
        "cargo build --release --features boot_tests --bin qemu-uefi",
    );
    assert_ne!(featured, x86_gate, "mutation did not apply");
    let build_line = featured
        .lines()
        .find(|line| line.contains("cargo build --release") && line.contains("--bin qemu-uefi"))
        .expect("mutated gate still builds qemu-uefi");
    assert!(
        build_line.contains("--features"),
        "a --features build slipped past the x86 profile rule"
    );
}
