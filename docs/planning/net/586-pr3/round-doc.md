# Landing

Landing of `net/586-pr3-guest-execution-budget`, preserving implementation
commit `6199469281f6b3ba94a9a46dabd4832b54a7ad7a` as an ancestor.
Prior implementation evidence: [original round doc](../../586-pr3/round-doc.md).
The handoff named a different path and commit-message text than this checkout.
V-2 is addressed in the landing commit with explicit claim-lint checklist lines;
the original implementation commit is retained to permit the requested ancestry check.

## Preparation

- `python3 scripts/claim-lint.py --files commit c12ff1409b7e1e1f276ca4c6d5cd97f02973b69b` -> exit 0. The two arguments name missing files in this checkout; this invocation is not tree-validation evidence.
- `python3 scripts/claim-lint.py --commit-msg <(git log -1 --format=%B)` -> exit 0 for the original HEAD.
- `python3 scripts/claim-lint.py` -> exit 0; 22 files checked against `c12ff1409b7e`.
- Output for these invocations: `serials/v2-fix.txt`.
- R16: `git fetch origin` and `git merge origin/main --no-edit` -> exit 0; already up to date at `c12ff1409b7e1e1f276ca4c6d5cd97f02973b69b`.
- R182: `tests/breenix_structure.rs` is absent. Main did not advance; fixture re-record is not required by new scorer assertions. The branch's relevant suite is `tests/loopback_pump_structure.rs`.
- `bash scripts/run-structure-tests.sh` -> exit 0; 103 passed, 0 failed. Output: `serials/structure.txt`.

The landing uses the requested per-lane TMPDIR and BREENIX_GATE_TMP under
`scratchpad/n586`. No kernel rebuild is part of this landing. The earlier
pinned-toolchain warning remains tracked in issue #945; a warning-free kernel
build is not claimed. Rust fixture and shell comments clarify the distinction
between policy coverage, strict boot, and the separate contention probe.

- `bash -n docker/qemu/run-aarch64-starved-loop.sh` -> exit 0.
- `bash scripts/run-structure-tests.sh loopback_pump_structure` -> exit 0; 128 passed, 0 failed. Output: `serials/loopback.txt`.

## Strict boot result

- `bash docker/qemu/run-aarch64-boot-test-strict.sh 1` -> exit 0, PASS 1/1.
- Full structure preflight: 64/64 suites; critical-path census 260, pinned 120.
- The gate checked the existing kernel for NEON, lockup-path allocation, and boot_tests markers.
- Host-lock wait preceded the boot. The boot ended with `ended_by=scored_pass`.
- Gate output: `serials/strict.txt`; guest output: `serials/serial.txt`; host facts: `serials/gate_boot_facts.txt`.
- Both loopback wake-budget records have `verdict=ok` and `extensions=0`.
  This boot does not establish live recovery from host starvation.

## Landing claim checks

- `python3 scripts/claim-lint.py` -> exit 0 after the Rust and shell comment clarifications (26 files checked).
- Final precommit invocations and statuses are recorded below with their output files.

- `python3 scripts/claim-lint.py` -> exit 0; output `serials/claim-tree.txt`.
- `python3 scripts/claim-lint.py --commit-msg /tmp/n586-landing-message.txt` -> exit 0; output `serials/claim-message.txt`.
- `python3 scripts/claim-lint.py --commit-msg <(git log -1 --format=%B)` -> exit 0; output `serials/claim-head.txt` (implementation HEAD before landing commit).
- `git diff --check` -> exit 0.

The final tree and prepared-message checks are repeated after recording these
results, before committing. V-2's explicit checklist is in the landing message.
Issue #945 remains the follow-up for the earlier toolchain warning; the
optional contention probe and a warning-free kernel build are not claimed.
