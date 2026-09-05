# The native ARM64 boot-test gate now refuses a wrong-profile kernel fast (health-811)

Base: `main` at `9b3dd4af9dc53d2950688f8094a26351703892c`.

## What was red

`docker/qemu/run-aarch64-boot-test-native.sh` scores a boot only through
`run_single_test`'s final checks, and one of those checks is unconditional:

```bash
if ! grep -qF -x "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    echo "FAIL: init-group refusal oracle counter marker missing"
    return 1
fi
```

<!-- claim-lint:ok: structural claim about #[cfg(feature = "boot_tests")]
     gating, resolved by direct citation --
     kernel/src/tracing/providers/teardown.rs:5125-5126
     (init_group_refusal_oracle_test), teardown.rs:1422-1423
     (emit_init_group_walk), and kernel/src/test_framework/mod.rs:60-66
     (registry+executor module gate). A build without --features boot_tests
     does not compile this code, so it cannot emit these two markers; this is
     the same reasoning docker/qemu/run-aarch64-boot-test-native.sh's own
     guard comment cites at the same three locations. -->
`INIT_GROUP_REFUSAL_ORACLE_LITERAL` is the serial line
`init_group_refusal_oracle_test()` prints
(`kernel/src/tracing/providers/teardown.rs:5125`), and that function is
`#[cfg(feature = "boot_tests")]`. The gate also requires the
`[INIT_GROUP_WALK:...]` marker, emitted by `emit_init_group_walk()`
(`kernel/src/tracing/providers/teardown.rs:1422`), which carries the same
`#[cfg(feature = "boot_tests")]` gate. A kernel built without
`--features boot_tests` — the shipped production profile — cannot print
either line, because `kernel/src/test_framework/mod.rs:60-66` gates the whole
`registry`/`executor` module tree, where these functions are registered and
invoked, on that same feature. Since the marker can not be present in such a
kernel's serial output under any boot outcome, a boot of it deterministically
fails `run_single_test` on `"init-group refusal oracle counter marker
missing"` (or, earlier in the same run, `"exec first commit not observed"`
if the boot is slow enough that the 24s liveness window elapses first) — this
branch did not run the pre-fix script against a production kernel to count
the failed attempts directly, since the fix lands before the retry loop is
reached, but the deduction above is what the pre-fix script's own
`MAX_RETRIES=5` retry loop and its generic
`ARM64 BOOT TEST: FAILED (after 5 attempts)` banner would have produced.
Nothing in that banner says the kernel was built in the wrong profile; it
reads exactly like a boot regression.

`docker/qemu/run-aarch64-boot-test-strict.sh` had already been fixed for
this class of failure (its own `require_boot_tests_kernel()`, added ahead of
this change) but the native script had not.

## The fix

### (a) `require_boot_tests_kernel()` guard

Added the same function name and body shape as the strict gate's guard —
same for-loop census of boot_tests-only marker prefixes, same
`missing="$missing $marker"` accumulation, same
`if [ -n "$missing" ]; then ... exit 1; fi` structure — inserted right after
the existing `check-kernel-no-neon.sh` preflight and before the ext2-disk
check, called once at top level as `require_boot_tests_kernel "$KERNEL"`.

The one deliberate difference from the strict gate's copy: this gate's own
verdict convention is the `ARM64 BOOT TEST: PASSED` / `ARM64 BOOT TEST:
FAILED` banner printed at the bottom of the script on the retry-loop path.
**Review R157 correction:** the first version of this change routed only
the new missing-marker arm through that banner, leaving this script's two
pre-existing preflight rejections (no kernel found, no ext2 disk found)
exiting bare — the same defect this whole change exists to remove, on a
different arm. Fixed in the same commit as this correction: 3 of 3
preflight arms now print the banner (each with its own parenthetical)
immediately before their `exit 1`, so every preflight rejection in this
script flows through the verdict shape this script already reports through
on its ordinary retry-exhausted path, not just the marker-census one.
<!-- claim-lint:ok: resolving citation for "the single path every preflight
rejection ... is required to go through" -- that requirement is the ratchet
`x86_production_profile_gate_verdict_discipline_holds` in
tests/teardown_structure.rs, described in
docs/planning/green-program/gates/GATE-PREFLIGHT-VERDICT-802-2026-09-05.md.
This branch adds no equivalent ratchet for the aarch64 native gate -- the
banner-before-`exit 1` on all three preflight arms here is a maintained
convention this document records, not an invariant any test enforces, and
the parallel drawn below is to that x86 ratchet's underlying PRINCIPLE, not
a claim that this script would itself satisfy that ratchet's structural
checks. --> The same principle — route a preflight rejection through the
script's one verdict-reporting mechanism rather than a bare `exit` — is
what `docker/qemu/run-x86-prod-profile-boot-test.sh`'s `report_gate_failure`
machinery enforces for that gate, in that mechanism's own idiom: a single
`report_gate_failure ...; exit "$exit_code"` call site, an ERR trap, and a
`reached` flag, all checked by
`tests/teardown_structure.rs::validate_x86_prod_profile_harness`
(`docs/planning/green-program/gates/GATE-PREFLIGHT-VERDICT-802-2026-09-05.md`).
This native script implements the analogous idea independently, with its
own bespoke banner-before-`exit 1` convention, no ERR trap, and no `reached`
flag, and would NOT itself satisfy that x86-specific structural test — it
rejects any `exit` statement other than `exit "$exit_code"`, and every
preflight arm in this script exits via a bare `exit 1`. A downstream
harness grepping for the `ARM64 BOOT TEST: FAILED` banner text gets a
labeled verdict on all three preflight rejections, not an unlabeled early
exit that reads as a script crash.

The census reuses the strict gate's exact seven marker prefixes
(`[SCHED_STRAND_ORACLE:`, `[STRAND_INJECT_ORACLE:`, `[CENSUS_WIDEN_ORACLE:`,
`[FUTEX_HANDOFF_ORACLE:`, `[CTX596_ORACLE:`, `[TOMBSTONE_JOIN_ORACLE:`,
`[BOOT_TESTS:`) rather than a native-specific pair. <!-- claim-lint:ok:
checkable by grep -- `grep -c '<marker>' docker/qemu/run-aarch64-boot-test-native.sh`
returns 1 for each of the six ORACLE markers (their sole appearance is inside
this guard's own `for marker in` line), and 2 for `[BOOT_TESTS:` (the guard
line, plus one existing `run_single_test` check at
`grep -qF '[BOOT_TESTS:FAIL' "$OUTPUT_DIR/serial.txt"` a few lines below). -->
Six of the seven — every ORACLE marker in the list — are general
boot_tests-only profile markers this gate's own `run_single_test` does not
otherwise score; `[BOOT_TESTS:` is the exception, since `run_single_test`
already checks for a `[BOOT_TESTS:FAIL` line (a check that is a harmless
no-op against a kernel built without the feature, since such a kernel never
emits any `[BOOT_TESTS:...]` line at all). The six-marker redundancy is
reused here so the guard stays a robust profile detector — a single marker
regressing to a different profile cannot quietly disarm it — instead of a
narrower check that a smaller future refactor could accidentally shrink
toward vacuousness. The gate's actual pinned boot_tests-only markers,
`INIT_GROUP_REFUSAL_ORACLE_LITERAL` and `INIT_GROUP_WALK`, are named in the
guard's failure message so the operator sees the concrete reason this gate
specifically needs the feature, not just an abstract marker list.

### (b) CLAUDE.md

Two minimal wording changes in the "Test Scripts" / "Standard Workflow"
sections, no other edits:

- The **ARM64:** bullet list now says the native and strict scripts each
  need a `--features boot_tests` kernel, and adds the previously-unlisted
  `run-aarch64-prod-profile-boot-test.sh` as the production-profile script
  (it builds its own no-features kernel internally, so it needs no build
  line of its own).
- The `cargo build ... kernel-aarch64` command that precedes
  `./docker/qemu/run-aarch64-boot-test-native.sh` in "Standard Workflow" now
  includes `--features boot_tests`, with a one-line comment saying which
  script needs the feature and which one (the prod-profile gate) does not.

Before this change, CLAUDE.md's own documented workflow built a
no-features kernel and then ran the gate that requires boot_tests markers —
the exact mismatch this whole change is about, reproduced by a reader
following CLAUDE.md verbatim.

### (c) Anti-vacuity: extend the existing structure-test census

`tests/strand_handoff_structure.rs::boot_tests_gates_refuse_a_wrong_profile_kernel`
already censused the service-sequence, strict, and full-test gates for this
guard shape (function body non-empty, `for marker in '...' ...; do` line
with at least `MIN_BOOT_TESTS_PROFILE_MARKERS` (6) bracketed markers,
`grep -aqF` inspection, single `missing="$missing $marker"` accumulation,
single `exit 1` in the missing-marker arm, single top-level invocation,
no-NEON preflight ordered first). Added a `NATIVE_GATE_PATH` constant and
one entry, `("native gate", NATIVE_GATE_PATH)`, to that test's gate list.
<!-- claim-lint:ok: narrowed per review R157 finding N2 -- the test iterates
a literal four-path list (SERVICE_SEQUENCE_GATE_PATH, STRICT_GATE_PATH,
FULL_TEST_PATH, NATIVE_GATE_PATH; see the top of
tests/strand_handoff_structure.rs), not a directory scan, so it censuses
only those four scripts. A brand-new fifth aarch64 gate script is not
covered by this list at all and would ship with no guard and no red test to
say so. docker/qemu/run-aarch64-arma609-arm.sh already demonstrates the
gap: it defines and calls its own `require_boot_tests_kernel()` with a
5-marker census (below `MIN_BOOT_TESTS_PROFILE_MARKERS`), sits outside this
test's list, and `grep -rn arma609 tests/*.rs` finds no reference to it
anywhere in the test tree -- deleting its guard reddens no test. This repo
does have a directory-discovery shape elsewhere
(`discover_aarch64_oracle_gates()` in tests/block_request_lifetime_structure.rs,
which enumerates `docker/qemu/run-aarch64-*.sh` by content), but porting it
here naively would also sweep in run-aarch64-prod-profile-boot-test.sh
(name matches, and it references the same kernel path at its own line 207),
which must NOT carry this guard -- so the literal-list mechanism is
defensible; only the claim about what it covers was wrong. --> This census
covers the four gate scripts the test already names, not a directory-wide
discovery: those four scripts cannot silently lose this guard shape, but a
new fifth script is invisible to this list until someone adds it.

### (d) Review R157: the top-of-script build hint, and one honesty fix

**N1 (major).** The script's own "No ARM64 kernel found" arm — the very
first preflight check, at the top of the file, before
`require_boot_tests_kernel()` is even defined — still told the operator to
build a kernel WITHOUT `--features boot_tests`: the exact kernel the guard
60 lines below then refuses. 3 of 3 peer boot_tests-requiring aarch64
gates (service-sequence, strict, full-test) already carried the feature in
that same arm; this script was the one exception. Fixed by adding
`--features boot_tests` to that hint, with a one-line comment pointing at
`require_boot_tests_kernel()` as the reason.

The pre-existing ratchet,
`strict_gate_build_hint_enables_boot_tests`, only ever checked the strict
gate's build hint, so it could not have caught this. Renamed to
`boot_tests_gate_build_hints_enable_boot_tests` and generalized to loop over
both the strict and native gates. The first pass at the generalized
assertion (`.any(build_hints, contains "--features boot_tests")`) was
itself vacuous against exactly this bug: the native gate has a *second*
`cargo build` echo line, inside `require_boot_tests_kernel()`'s own
missing-marker arm, which already carried `--features boot_tests` before
this round — so "at least one hint has the feature" was already true even
with the top-of-script hint broken. Strengthened to `.all(...)`, which
requires every build-hint line in the gate to carry the feature.
Mutation-proven both ways in this round: reverting only the top-of-script
hint to drop `--features boot_tests` (leaving the guard's own hint
untouched) reddens `boot_tests_gate_build_hints_enable_boot_tests` with
"native gate every build hint must enable --features boot_tests"; restoring
it returns the suite to green (38/38 in
`tests/strand_handoff_structure.rs`, plus `tests/exec_lock_order_structure.rs`
44/44 and `tests/teardown_structure.rs` 83/83, all of which also read this
script's text and were otherwise unaffected).

**N3 (minor).** The in-script comment ahead of `require_boot_tests_kernel()`
had claimed the ratchet proved the banner echoes print "by construction,"
citing the same `exit 1`-count check the ratchet actually performs. The
ratchet proves the arm has exactly one `exit 1` line; it does not inspect
the echo lines above it, so deleting the three banner echoes alone (proven
by mutation, then reverted) leaves the ratchet green. Reworded the comment
to state only what the ratchet checks, and to say plainly that the banner's
presence is a maintained convention, not a ratcheted one.

## Mutation proof the added census line is load-bearing

With the fix applied and `require_boot_tests_kernel "$KERNEL"` temporarily
deleted from a scratch copy of the native script (not committed — restored
before this branch's tests were re-run), `cargo test --test
strand_handoff_structure boot_tests_gates_refuse_a_wrong_profile_kernel`
failed:

```
thread 'boot_tests_gates_refuse_a_wrong_profile_kernel' panicked at tests/strand_handoff_structure.rs:1755:9:
assertion `left == right` failed: native gate must invoke the boot-tests profile guard exactly once at top level
  left: 0
 right: 1
```

Restoring the file and re-running the same test returned `test result: ok. 1
passed`.

## Proofs

### Fail-fast on the production (no-features) kernel

Built with `cargo build --release --target aarch64-breenix-kernel.json -Z
build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel
--bin kernel-aarch64` (no `--features`), verified with
`scripts/check-kernel-no-neon.sh` (`PASS: 0 FP/SIMD load/store
instructions`), then run once against
`docker/qemu/run-aarch64-boot-test-native.sh`:

```
Error: .../target/aarch64-breenix-kernel/release/kernel-aarch64 was not built with --features boot_tests.
  Missing boot_tests-only marker literal(s): [SCHED_STRAND_ORACLE: [STRAND_INJECT_ORACLE: [CENSUS_WIDEN_ORACLE: [FUTEX_HANDOFF_ORACLE: [CTX596_ORACLE: [TOMBSTONE_JOIN_ORACLE: [BOOT_TESTS:
  This gate pins INIT_GROUP_REFUSAL_ORACLE_LITERAL and the INIT_GROUP_WALK
  marker, both boot_tests-only, so every boot below would fail on
  'marker missing' after 5 retries -- not a kernel red.
  Rebuild with:
    cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64

=========================================
ARM64 BOOT TEST: FAILED (kernel is not a boot_tests build)
=========================================
```

Exit status 1, wall time 2.08s (`time` on the run). `pgrep -fl
qemu-system-aarch64 | wc -l` reported 0 immediately before this run and 0
immediately after it, so this run itself launched no QEMU process; this
branch did not run the pre-fix script against this same production kernel to
directly count how many `qemu-system-aarch64` processes its 5-attempt retry
loop would have launched (see "What was red" above for the deduction that it
would be 5, one per `MAX_RETRIES` attempt, each on a 30s `timeout`).

### Pass on the boot_tests kernel

Rebuilt with `--features boot_tests` added (userspace ELFs built first via
`userspace/programs/build.sh --arch aarch64`, since the boot_tests registry
embeds `simple_exit.elf` at compile time), re-verified with the same
no-NEON guard, ext2 image created with `scripts/create_ext2_disk.sh --arch
aarch64`, then run once:

```
Attempt 1/5...
SUCCESS

=========================================
ARM64 BOOT TEST: PASSED
=========================================
```

Exit status 0, passed on the first attempt (no retry needed).

### Pass on the production-profile gate

`docker/qemu/run-aarch64-prod-profile-boot-test.sh`, which builds its own
no-features kernel and boots it, run once:

```
PASS: production profile reached bsshd with the futex oracle seam absent
Observed: [FUTEX_HANDOFF_ORACLE_DRIVER:seam_absent:probe=-110]
Observed: [init] futex_handoff_oracle exited pid=6 code=0
Observed: bsshd: listening on 0.0.0.0:2222
Observed kernel oracle marker count: 0
...
Observed: [TTBR0_ASID_CENSUS:untagged=0:tagged=23431:kernel=26892:cleared=49512]
Observed crash marker count: 0
```

Exit status 0.

### Structure-test suites

Every file in `tests/*_structure.rs` (30 files at this head) run
individually with `cargo test --test <file>`: 30 of 30 green, 562 test
cases total (`aarch64_testing_profile_structure` 2,
`block_request_lifetime_structure` 12, `context_restore_structure` 97,
`coreproof_component_h_structure` 5, `coreproof_coverage_structure` 4,
`coreproof_mutation_register_structure` 5, `coreproof_sites_structure` 4,
`degenerate_transfer_fd_validation_structure` 4,
`dispatch_path_lock_free_structure` 4, `dispatch_strand_census_structure` 7,
`dma_and_log_sink_structure` 4, `entry_point_df_structure` 5,
`exec_lock_order_structure` 44, `exit_tally_structure` 6,
`ext2_lock_structure` 36, `fork_lock_order_structure` 10,
`green_program_envelope_structure` 14, `loopback_pump_structure` 72,
`masked_binary_load_structure` 4, `mmap_floor_structure` 9,
`net_lock_structure` 19, `poll_tcp_gate_wiring_structure` 3,
`preempt_bracket_structure` 8, `serial_line_atomicity_structure` 9,
`signal_eintr_predicate_structure` 2, `strand_handoff_structure` 38,
`syscall_return_register_structure` 6, `teardown_structure` 83,
`ttbr0_shadow_reconciliation_structure` 32, `tty_oracle_structure` 14),
0 failed.

### Claim-lint

```
claim-lint: scripts/claim-lint.py -> exit 0
```

plus a `--files`/`--commit-msg` run recorded in the round notes for this
branch's own diff and commit message.

## What is NOT claimed

- This is a gate/tooling fix, not a kernel change. `kernel/` was not
  touched; no kernel behavior changed.
- The seven marker prefixes the guard's census reuses are general
  boot_tests-profile detectors, not a claim that `run-aarch64-boot-test-native.sh`
  itself scores `SCHED_STRAND_ORACLE`, `FUTEX_HANDOFF_ORACLE`,
  `CENSUS_WIDEN_ORACLE`, `CTX596_ORACLE`, or `TOMBSTONE_JOIN_ORACLE` — it
  does not. The two markers this gate does score and require are
  `INIT_GROUP_REFUSAL_ORACLE_LITERAL` and `INIT_GROUP_WALK`, named
  separately in the guard's own failure message.
- The three boot proofs above are each a single observation (one fail-fast
  run, one pass on `boot_tests`, one pass on the production-profile gate),
  not a multi-boot soak. The native gate's own `MAX_RETRIES=5` retry
  mechanism and the strict gate's iteration count remain the tools for
  measuring flake rate; this change does not add or remove either.
- This change does not touch `docker/qemu/run-aarch64-boot-test-strict.sh`,
  `run-aarch64-service-sequence-gate.sh`, `run-aarch64-full-test.sh`, or any
  x86 gate script. Their preflight/verdict shapes are unchanged.
- x86 boot gates and the `run-x86-prod-profile-boot-test.sh` verdict-
  discipline ratchet (`x86_production_profile_gate_verdict_discipline_holds`)
  were read for the pattern this fix follows but were neither run nor
  modified as part of this change.
- CLAUDE.md changes are limited to the two lines described in (b) above; no
  other section of CLAUDE.md was reviewed or edited as part of this task.
