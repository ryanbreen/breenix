//! Census-shaped ratchet for R157/F3 (#796 review round, finding F3).
//!
//! `fcntl_pm_contention_oracle` measures whether an `fcntl()` issued while a
//! peer CPU holds `PROCESS_MANAGER` *waits* for the lock instead of reporting
//! `EAGAIN`. Two fields carry that reading, and before this file existed 1 of
//! the 2 was defended anywhere:
//! claim-lint:ok: 2 of 2 fields are named in the two bullets below; the oracle's
//! red-on-main / green-on-branch runs are in the #796 doc's STEP 3.
//!
//! * `eagain=0` -- the property under test. Pinned by the strict gate.
//! * `first_wait_us` -- the anti-vacuity floor. Without it, a boot whose peer
//!   failed to arm, or armed and released early, prints `eagain=0` and scores
//!   green having measured an *uncontended* call.
//!
//! The review found the floor unratcheted and unpinned: the strict gate's
//! pattern accepted `first_wait_us=[0-9]+`, i.e. `first_wait_us=0`, so the sole
//! authority making "the call actually waited" true was the in-kernel conjunct
//! `first_wait_us >= FCNTL_PM_MIN_WAIT_US` in the oracle's own `passed`
//! predicate. Deleting that one conjunct left the gate and 30 of the 30
//! structure suites then in the tree green -- a grep for fcntl over tests/
//! returned 0 files.
//! claim-lint:ok: R157 finding F3, reproduced as mutation M1 in the #796 doc's
//! STEP 3 anti-vacuity block.
//!
//! This file closes both halves, and closes them by CENSUS rather than by
//! naming today's script (the campaign's standing rule after #549/#551/#527-r1):
//!
//! 1. `oracle_pass_predicate_carries_a_wait_floor_conjunct` derives the
//!    `passed` predicate that feeds the emitted verdict from the registry
//!    source and requires it to compare `first_wait_us` against a floor of at
//!    least `MIN_DEFENSIBLE_FLOOR_US`. Deleting the conjunct, or zeroing the
//!    constant it names, reddens it.
//! 2. `every_gate_pinning_the_aarch64_pass_verdict_selfchecks_its_pattern`
//!    derives the set of scripts under docker/qemu/ and scripts/ that pin the
//!    oracle's aarch64 `PASS` verdict, and requires each one to carry the
//!    `FCNTL_PM_WAIT_SELFCHECK` block. A gate added later is swept into the
//!    census automatically.
//!
//! The division of labour matters. This file does not evaluate anybody's
//! regular expression: whether a pattern actually rejects `first_wait_us=0` is
//! decided at gate time by the gate's own matcher, in the self-check block,
//! which runs before the pattern is used to score any boot. So a loosened
//! pattern fails the gate that uses it rather than a ratchet that models it,
//! and this file's job is only to make sure no gate can quietly drop the
//! self-check.
//!
//! Two more ratchets were added after the 2026-09-05 health run, where the
//! oracle reddened the strict gate on 2 of 40 boots with
//! `attempts=3:armed=0:...:calls=0:FAIL` under the verdict text "fcntl reported
//! a contended process-manager lock to userspace" -- a sentence the same line
//! contradicted, because `calls=0` says no `fcntl` was issued at all:
//! claim-lint:ok: 2 of 40 boots, serials preserved at
//! docs/planning/green-program/syscalls/serials/819-oracle-arming/
//!
//! 3. `oracle_arming_is_a_rendezvous_not_an_attempt_count` reads the emitted
//!    marker's format string and requires it to carry `arm_wait_us=` and not
//!    `attempts=`, and requires the driver's rendezvous deadline to be at least
//!    `MIN_RENDEZVOUS_DEADLINE_US`. Re-adding a retry counter to the line, or
//!    shrinking the deadline back to the half-second the old arm-wait used,
//!    reddens it.
//! 4. `verdict_arms_are_distinct_and_only_one_describes_a_syscall_result`
//!    derives the oracle's arm enum and the message each arm reports, requires
//!    each variant to have one, requires the messages to be distinct, and
//!    requires each arm other than the EAGAIN one to avoid saying something
//!    reached userspace. Restoring the old single verdict text reddens it.
//!
//! What neither of them reaches: they read the shape of the reporting, not the
//! behaviour. That an arming failure really does report `arming_timeout` rather
//! than the EAGAIN arm is decided by the boot, not by this file.
//!
//! Host-side only: a text read of the tree, no kernel build and no QEMU boot.
//! Run: `cargo test --test fcntl_pm_contention_gate_structure`.

use std::fs;
use std::path::{Path, PathBuf};

/// The oracle's aarch64 verdict prefix. A gate script mentioning this string
/// and `PASS` is treating the oracle's green verdict as gate-relevant.
const ORACLE_AARCH64_PREFIX: &str = "FCNTL_PM_CONTENTION_ORACLE:aarch64";
/// A floor below this would not distinguish a contended call from an
/// uncontended one on this hardware: the uncontended calls recorded on
/// origin/main returned in 2-21 us.
const MIN_DEFENSIBLE_FLOOR_US: u64 = 1_000;
/// The smallest rendezvous deadline this ratchet accepts. The driver has to
/// outwait the sum of what the holder can spend before it exits -- 500 ms of
/// acquire retries, 250 ms of safety hold and the 8 ms overlap, about 758 ms --
/// so that a driver-side timeout means the holder thread did not run, rather
/// than one deadline racing the other.
const MIN_RENDEZVOUS_DEADLINE_US: u64 = 2_000_000;
/// The token a gate script carries when it proves, at gate time and in its own
/// matcher, that its pinned pattern rejects `first_wait_us=0` and accepts a
/// real wait. Keyed on rather than reproduced here, so this ratchet asserts the
/// self-check exists and the gate itself asserts that it works.
const GATE_SELFCHECK_TOKEN: &str = "FCNTL_PM_WAIT_SELFCHECK";

/// The oracle's driver function. The marker is emitted from inside it, so it
/// anchors the region the derivations below read.
const ORACLE_DRIVER_FN: &str = "fn run_fcntl_pm_contention_oracle";
/// The constant holding the driver's rendezvous deadline.
const RENDEZVOUS_DEADLINE_CONST: &str = "FCNTL_PM_ARM_WAIT_US";
/// The oracle's verdict-arm enum, its declaration, and the two functions that
/// have to answer for each variant of it.
const ARM_ENUM_NAME: &str = "FcntlPmArm";
const ARM_ENUM: &str = "enum FcntlPmArm";
const ARM_RESULT_FN: &str = "fn result(self) -> TestResult";
const ARM_TAG_FN: &str = "fn tag(self) -> &'static str";
/// The one variant that reports a passing verdict.
const PASS_VARIANT: &str = "Pass";
const REGISTRY: &str = "kernel/src/test_framework/registry.rs";
const GATE_ROOTS: [&str; 2] = ["docker/qemu", "scripts"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    String::from_utf8_lossy(
        &fs::read(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e)),
    )
    .into_owned()
}

/// Each regular file under `root`, recursively.
fn discover_files(root: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("read entry in {}: {}", root.display(), e))
            .path();
        if path.is_dir() {
            discover_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

/// The oracle's aarch64 body: from the declaration of the driver function to
/// the marker it emits. Derived from the emit site rather than pinned to a
/// line, so moving the oracle's internals around does not redden this.
fn oracle_body(source: &str) -> &str {
    let emit = source
        .find(ORACLE_AARCH64_PREFIX)
        .expect("the registry no longer emits the oracle aarch64 marker");
    let start = source[..emit]
        .rfind(ORACLE_DRIVER_FN)
        .expect("the oracle marker is not emitted from the oracle driver function");
    &source[start..emit]
}

/// The `let` binding inside the oracle body that compares `first_wait_us`
/// against a floor. Returns its name, the floor's right-hand side as written,
/// and the offset in `body` just past the binding's terminating `;`.
fn wait_floor_binding(body: &str) -> (String, String, usize) {
    let needle = "first_wait_us >=";
    let hits: Vec<usize> = body.match_indices(needle).map(|(at, _)| at).collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly 1 `{}` comparison in the oracle body, found {}. \
         With none, nothing makes the measured call's wait a condition of the \
         verdict; with several, this derivation cannot say which one decides it",
        needle,
        hits.len()
    );
    let at = hits[0];

    let rhs: String = body[at + needle.len()..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    assert!(
        !rhs.is_empty(),
        "the first_wait_us floor has no right-hand side"
    );

    let let_at = body[..at]
        .rfind("let ")
        .expect("the first_wait_us floor is not part of a `let` binding");
    let after_let = &body[let_at + "let ".len()..at];
    let name: String = after_let
        .split('=')
        .next()
        .expect("split always yields one element")
        .trim()
        .trim_start_matches("mut ")
        .trim()
        .to_string();
    assert!(
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "the first_wait_us floor's binding has no plain name, derived {:?}",
        name
    );

    let end = at + body[at..]
        .find(';')
        .expect("unterminated binding around the first_wait_us floor")
        + 1;
    (name, rhs, end)
}

/// The `u64` value of a `const NAME: ... = <literal>;` declaration.
fn const_u64(source: &str, name: &str) -> u64 {
    let needle = format!("const {}:", name);
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("const {} is not declared in {}", name, REGISTRY));
    let tail = &source[start..];
    let eq = start + tail.find('=').expect("const without an initializer");
    let semi = eq + source[eq..].find(';').expect("const without a terminator");
    source[eq + 1..semi]
        .trim()
        .replace('_', "")
        .parse::<u64>()
        .unwrap_or_else(|e| panic!("const {} is not a plain u64 literal: {}", name, e))
}

/// The braced block that follows `needle`, braces included.
fn block_after<'a>(source: &'a str, needle: &str) -> &'a str {
    let at = source
        .find(needle)
        .unwrap_or_else(|| panic!("{} is not present in {}", needle, REGISTRY));
    let open = at + source[at..].find('{').unwrap_or_else(|| {
        panic!("{} is not followed by a block in {}", needle, REGISTRY)
    });
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open..open + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced block after {} in {}", needle, REGISTRY);
}

/// The registry source, read once per test.
fn registry_source() -> String {
    let path = repo_root().join(REGISTRY);
    read(&path)
}

#[test]
fn oracle_pass_predicate_carries_a_wait_floor_conjunct() {
    let registry = registry_source();
    let body = oracle_body(&registry);
    let (name, rhs, end) = wait_floor_binding(body);

    let floor = match rhs.replace('_', "").parse::<u64>() {
        Ok(literal) => literal,
        Err(_) => const_u64(&registry, &rhs),
    };
    assert!(
        floor >= MIN_DEFENSIBLE_FLOOR_US,
        "the first_wait_us floor is {} us, below the {} us that separates a contended \
         call from the 2-21 us an uncontended one took on origin/main",
        floor,
        MIN_DEFENSIBLE_FLOOR_US
    );

    assert!(
        body[end..].contains(&name),
        "`{}` carries the first_wait_us floor but is never read again before the \
         verdict is emitted, so the floor decides nothing",
        name
    );
}

/// Scripts under the gate roots that pin the oracle's aarch64 PASS verdict.
/// Derived rather than listed, so a gate added later is swept in without anyone
/// editing this file.
fn gates_pinning_the_aarch64_pass_verdict() -> Vec<(PathBuf, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    for gate_root in GATE_ROOTS {
        discover_files(&root.join(gate_root), &mut files);
    }
    files.sort();
    let mut gates = Vec::new();
    for path in files {
        let text = read(&path);
        if text.contains(ORACLE_AARCH64_PREFIX) && text.contains("PASS") {
            gates.push((path, text));
        }
    }
    gates
}

#[test]
fn every_gate_pinning_the_aarch64_pass_verdict_selfchecks_its_pattern() {
    let gates = gates_pinning_the_aarch64_pass_verdict();
    assert!(
        !gates.is_empty(),
        "no script under {:?} pins the oracle aarch64 PASS verdict -- either the \
         oracle lost its only gate or its marker was renamed, and this ratchet would \
         have passed vacuously",
        GATE_ROOTS
    );

    for (path, text) in &gates {
        assert!(
            text.contains(GATE_SELFCHECK_TOKEN),
            "{} pins the oracle aarch64 PASS verdict but carries no {} block, so \
             nothing proves its pattern can tell a real wait from a zero wait",
            path.display(),
            GATE_SELFCHECK_TOKEN
        );
    }
}

/// The oracle's emitted marker format string: the marker prefix through the end
/// of the string literal it sits in.
fn marker_format(source: &str) -> &str {
    let at = source
        .find(ORACLE_AARCH64_PREFIX)
        .expect("the registry no longer emits the oracle aarch64 marker");
    let tail = &source[at..];
    let end = tail
        .find('"')
        .expect("the oracle marker literal is unterminated");
    &tail[..end]
}

#[test]
fn oracle_arming_is_a_rendezvous_not_an_attempt_count() {
    let registry = registry_source();
    let marker = marker_format(&registry);

    assert!(
        marker.contains("arm_wait_us="),
        "the oracle marker does not report how long the driver waited for the peer to \
         publish its hold, so a boot that armed instantly and one that never armed at \
         all print the same evidence. Marker was:\n{}",
        marker
    );
    assert!(
        !marker.contains("attempts="),
        "the oracle marker counts arming attempts again. Arming is a rendezvous with a \
         deadline: a retry count means the hold closes on something other than the \
         driver's request, which is the shape that reported armed=0:calls=0 on 2 of 40 \
         boots. Marker was:\n{}",
        marker
    );

    // Census over the oracle's own constants rather than one remembered name:
    // any constant counting arming attempts is the shape this keeps out.
    for (at, _) in registry.match_indices("const FCNTL_PM_") {
        let name: String = registry[at + "const ".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        assert!(
            !name.contains("ATTEMPT"),
            "{} counts arming attempts; the arming rendezvous has no retry loop to count",
            name
        );
    }

    let deadline = const_u64(&registry, RENDEZVOUS_DEADLINE_CONST);
    assert!(
        deadline >= MIN_RENDEZVOUS_DEADLINE_US,
        "the driver's rendezvous deadline is {} us, under the {} us this ratchet \
         requires: below it the driver can time out while the holder is still inside \
         its own acquire and safety deadlines, and the reported arm becomes a race \
         between two timeouts",
        deadline,
        MIN_RENDEZVOUS_DEADLINE_US
    );
}

/// The variants declared by the oracle's arm enum, in declaration order.
fn arm_variants(source: &str) -> Vec<String> {
    let block = block_after(source, ARM_ENUM);
    let mut variants = Vec::new();
    for line in block.lines() {
        let line = line.trim_end();
        let trimmed = line.trim_start();
        if !trimmed.ends_with(',') {
            continue;
        }
        let ident: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !ident.is_empty() && ident.len() + 1 == trimmed.len() {
            variants.push(ident);
        }
    }
    variants
}

/// The `Variant => <body>` arms of a match on `scrutinee`, in source order.
fn match_arms(block: &str, scrutinee: &str) -> Vec<(String, String)> {
    let pattern = format!("{}::", scrutinee);
    let mut arms = Vec::new();
    let mut rest = block;
    while let Some(at) = rest.find(&pattern) {
        let after = &rest[at + pattern.len()..];
        let variant: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let Some(arrow) = after.find("=>") else {
            break;
        };
        let body_start = arrow + "=>".len();
        let end = after[body_start..]
            .find(&pattern)
            .map(|next| body_start + next)
            .unwrap_or(after.len());
        arms.push((variant, after[body_start..end].to_string()));
        rest = &after[end..];
    }
    arms
}

/// The first string literal in a match arm's body.
fn first_string_literal(arm: &str) -> Option<String> {
    let open = arm.find('"')?;
    let end = arm[open + 1..].find('"')? + open + 1;
    Some(arm[open + 1..end].to_string())
}

#[test]
fn verdict_arms_are_distinct_and_only_one_describes_a_syscall_result() {
    let registry = registry_source();
    let variants = arm_variants(&registry);
    assert!(
        variants.len() >= 3,
        "the oracle declares {} verdict arms; the health run's finding was one arm \
         answering for arming failures and syscall verdicts alike",
        variants.len()
    );

    let result_arms = match_arms(block_after(&registry, ARM_RESULT_FN), ARM_ENUM_NAME);
    let tag_arms = match_arms(block_after(&registry, ARM_TAG_FN), ARM_ENUM_NAME);

    // Census: a variant added later is swept in here rather than on the day
    // somebody remembers to widen a list.
    for variant in &variants {
        assert!(
            result_arms.iter().any(|(name, _)| name == variant),
            "verdict arm {} reports no message, so the boot-test line would say nothing \
             about what it measured",
            variant
        );
        assert!(
            tag_arms.iter().any(|(name, _)| name == variant),
            "verdict arm {} has no marker tag, so the serial cannot name it",
            variant
        );
    }

    let mut messages: Vec<(String, String)> = Vec::new();
    for (variant, arm) in &result_arms {
        if variant == PASS_VARIANT {
            assert!(
                arm.contains("TestResult::Pass"),
                "the {} arm does not report a pass",
                PASS_VARIANT
            );
            continue;
        }
        assert!(
            arm.contains("TestResult::Fail"),
            "verdict arm {} reports neither a pass nor a failure",
            variant
        );
        let message = first_string_literal(arm)
            .unwrap_or_else(|| panic!("verdict arm {} reports a failure with no text", variant));
        messages.push((variant.clone(), message));
    }

    for (index, (variant, message)) in messages.iter().enumerate() {
        for (other_variant, other_message) in messages.iter().skip(index + 1) {
            assert_ne!(
                message, other_message,
                "verdict arms {} and {} report the same text, so the serial cannot tell \
                 which of them failed",
                variant, other_variant
            );
        }
    }

    let syscall_claims: Vec<&String> = messages
        .iter()
        .filter(|(_, message)| message.contains("EAGAIN"))
        .map(|(variant, _)| variant)
        .collect();
    assert_eq!(
        syscall_claims.len(),
        1,
        "exactly 1 verdict arm may report the EAGAIN the oracle exists to catch; {} do: \
         {:?}",
        syscall_claims.len(),
        syscall_claims
    );

    for (variant, message) in &messages {
        if message.contains("EAGAIN") {
            continue;
        }
        assert!(
            !message.contains("to userspace"),
            "verdict arm {} says something reached userspace: {:?}. The arming arms fail \
             before any call is issued -- the line they print carries calls=0 -- and \
             reporting them as a syscall verdict is the 2026-09-05 regression",
            variant,
            message
        );
    }

    let mut tags: Vec<(String, String)> = Vec::new();
    for (variant, arm) in &tag_arms {
        let tag = first_string_literal(arm)
            .unwrap_or_else(|| panic!("verdict arm {} has an empty marker tag", variant));
        if variant == PASS_VARIANT {
            assert_eq!(
                tag, "PASS",
                "the {} arm's marker tag is not the gate's PASS token",
                PASS_VARIANT
            );
        } else {
            assert!(
                tag.starts_with("FAIL"),
                "verdict arm {}'s marker tag {:?} is not a failing token, so the gate's \
                 FAIL scan would not see it",
                variant,
                tag
            );
        }
        tags.push((variant.clone(), tag));
    }

    for (index, (variant, tag)) in tags.iter().enumerate() {
        for (other_variant, other_tag) in tags.iter().skip(index + 1) {
            assert_ne!(
                tag, other_tag,
                "verdict arms {} and {} print the same marker tag",
                variant, other_variant
            );
        }
    }
}
