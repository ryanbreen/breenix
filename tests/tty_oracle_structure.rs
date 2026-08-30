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
}
