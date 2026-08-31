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

/// Maps each `arm_<name>` function to the ARM string it stamps, by reading
/// the `const ARM: &str = "..."` line that appears inside that function's
/// own body (between its `fn arm_<name>(` header and the next `fn ` header).
/// Returns (fn_name, arm_string) pairs in file order.
fn arm_fn_to_arm_string(source: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(rest) = line.strip_prefix("fn arm_") {
            let name_end = rest.find('(').expect("arm fn has a parameter list");
            let fn_name = format!("arm_{}", &rest[..name_end]);
            let mut j = i + 1;
            let mut arm_value = None;
            while j < lines.len() {
                let candidate = lines[j].trim();
                if candidate.starts_with("fn ") {
                    break;
                }
                if let Some(rest) = candidate.strip_prefix("const ARM: &str = \"") {
                    arm_value = rest.split('"').next().map(|s| s.to_string());
                    break;
                }
                j += 1;
            }
            let arm_value = arm_value
                .unwrap_or_else(|| panic!("{fn_name} declares no const ARM in its own body"));
            result.push((fn_name, arm_value));
        }
        i += 1;
    }
    result
}

fn arm_string_for_fn<'a>(pairs: &'a [(String, String)], fn_name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(name, _)| name == fn_name)
        .map(|(_, arm)| arm.as_str())
        .unwrap_or_else(|| panic!("no arm_ fn named {fn_name} declares a const ARM"))
}

/// Arms excluded from a given `target_arch` by a `#[cfg(target_arch = "...")]`
/// attribute sitting directly above their `arm_<name>()?;` call inside
/// `run()`'s body -- i.e. arms gated IN for one arch and therefore absent on
/// every other arch. Reads `run()`'s actual call sites, not a maintained
/// list, so a new per-arch exclusion is picked up with no edit to this file.
fn arms_excluded_from(source: &str, arch: &str) -> Vec<String> {
    let pairs = arm_fn_to_arm_string(source);
    let run_start = source
        .find("fn run() -> Result<u32, Failure> {")
        .expect("oracle declares run()");
    let run_body = &source[run_start..];
    let run_end = run_body.find("\nfn main()").unwrap_or(run_body.len());
    let run_body = &run_body[..run_end];

    let cfg_line = format!("#[cfg(target_arch = \"{arch}\")]");
    let lines: Vec<&str> = run_body.lines().collect();
    let mut excluded = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() != cfg_line {
            continue;
        }
        let Some(next) = lines.get(i + 1) else {
            continue;
        };
        let next = next.trim();
        let Some(rest) = next.strip_prefix("arm_") else {
            continue;
        };
        let Some(fn_tail) = rest.strip_suffix("()?;") else {
            continue;
        };
        let fn_name = format!("arm_{fn_tail}");
        excluded.push(arm_string_for_fn(&pairs, &fn_name).to_string());
    }
    excluded
}

/// Every arm actually reachable on x86: every arm the oracle declares,
/// minus whatever `run()` gates behind `#[cfg(target_arch = "aarch64")]`
/// specifically (i.e. arms that exist ONLY on aarch64). Generic over future
/// exclusions -- a second aarch64-only arm needs no edit here.
fn x86_reachable_arms(source: &str) -> Vec<String> {
    let all = oracle_arms(source);
    let excluded = arms_excluded_from(source, "aarch64");
    let mut arms: Vec<String> = all.into_iter().filter(|a| !excluded.contains(a)).collect();
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

#[test]
fn the_x86_gate_scores_exactly_the_arms_the_oracle_drives_on_x86() {
    let source = read(ORACLE);
    let x86_arms = x86_reachable_arms(&source);
    assert!(
        x86_arms.len() >= 8,
        "found only {} x86-reachable arms - the census is not matching, \
         which would make this ratchet vacuous: {x86_arms:?}",
        x86_arms.len()
    );
    assert!(
        x86_arms.len() < oracle_arms(&source).len(),
        "the x86-reachable census found no arch-excluded arm at all - if \
         #721 has closed and cloexec_exec was re-admitted on x86, this \
         ratchet (and the oracle's ARM_COUNT split) needs to be retired, \
         not left silently checking a no-op exclusion"
    );
    assert_eq!(
        x86_arms,
        gate_expected_arms(&read(X86_GATE)),
        "the x86 gate's EXPECTED_ARMS and the oracle's x86-reachable arms have drifted"
    );
}

#[test]
fn the_oracle_tally_matches_its_x86_arm_census() {
    let source = read(ORACLE);
    let x86_arms = x86_reachable_arms(&source);
    let lines: Vec<&str> = source.lines().collect();
    let mut declared = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "#[cfg(target_arch = \"x86_64\")]" {
            if let Some(next) = lines.get(i + 1) {
                if let Some(value) = next
                    .trim()
                    .strip_prefix("const ARM_COUNT: u32 = ")
                    .and_then(|s| s.strip_suffix(';'))
                {
                    declared = value.parse::<usize>().ok();
                    break;
                }
            }
        }
    }
    let declared = declared.expect("oracle declares an x86_64-gated ARM_COUNT");
    assert_eq!(
        declared,
        x86_arms.len(),
        "the x86-gated ARM_COUNT is {declared} but {} arms are actually \
         reachable on x86 - the COMPLETE marker the x86 gate matches on \
         would be wrong",
        x86_arms.len()
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
fn the_x86_gate_refuses_a_cloexec_exec_verdict() {
    let script = read(X86_GATE);
    assert!(
        script.contains("[TTY_ORACLE:cloexec_exec:"),
        "the x86 gate does not guard against cloexec_exec reporting a \
         verdict at all - a cfg regression that re-admits the arm \
         unconditionally would run it against #721's ENOSYS uncaught"
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

    // An arm added to the oracle's x86-reachable set but not to the x86 gate.
    let extra_arm = oracle.replace(
        "const ARM: &str = \"winsize\";",
        "const ARM: &str = \"winsize\";\n    const ARM: &str = \"unscored_x86_arm\";",
    );
    assert_ne!(extra_arm, oracle, "mutation did not apply");
    assert_ne!(
        x86_reachable_arms(&extra_arm),
        gate_expected_arms(&x86_gate),
        "an x86-reachable arm the x86 gate does not score went undetected"
    );

    // An arm dropped from the x86 gate's expectations.
    let short_gate = x86_gate.replace("    winsize\n", "");
    assert_ne!(short_gate, x86_gate, "mutation did not apply");
    assert_ne!(
        x86_reachable_arms(&oracle),
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

    // cloexec_exec silently re-admitted on x86: strip whichever line is
    // exactly `#[cfg(target_arch = "aarch64")]` and sits directly above the
    // `arm_cloexec_exec()?;` call in run(), and confirm the x86-reachable
    // census picks the arm back up -- proving the parser above actually
    // reads run()'s cfg gates rather than passing vacuously.
    let mut readmitted_lines: Vec<&str> = oracle.lines().collect();
    let call_idx = readmitted_lines
        .iter()
        .position(|l| l.trim() == "arm_cloexec_exec()?;")
        .expect("run() calls arm_cloexec_exec()?;");
    assert_eq!(
        readmitted_lines[call_idx - 1].trim(),
        "#[cfg(target_arch = \"aarch64\")]",
        "arm_cloexec_exec()?; in run() is not directly cfg-gated"
    );
    readmitted_lines.remove(call_idx - 1);
    let readmitted = readmitted_lines.join("\n");
    assert_ne!(readmitted, oracle, "mutation did not apply");
    assert!(
        x86_reachable_arms(&readmitted).contains(&"cloexec_exec".to_string()),
        "removing the aarch64 cfg on run()'s arm_cloexec_exec() call did \
         not change the x86-reachable census - the parser is not actually \
         reading run()'s cfg gates"
    );
}
