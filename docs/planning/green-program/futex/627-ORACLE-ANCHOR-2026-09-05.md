# #627 — the futex handoff oracle's stage3_elapsed_ok anchor

<!-- claim-lint:ok: #627 -- every command and its exit code is cited in the
     "Proofs" section below. -->
R180 (ruling, binding on this round): #627 is an oracle-origin defect, not a
kernel timeout defect. This document records the defect, the fix, the two
false sentences it replaces, and the proof this round ran.

## The defect

`FUTEX_HANDOFF_ORACLE_PATTERN` requires `stage3_elapsed_ok=1` on the strict
gate. Issue #627 recorded two live specimens (2 of 2 preserved failing
serials) where that bit read `0` on a wait that was never actually short:
`stage3_elapsed_ms=49` and `=46` against a 50ms request, `stage3_ret=ETIMEDOUT`,
`rescues=0` (the backstop did not end the wait). claim-lint:ok: #627, 2 of 2
specimens cited above by their exact fields.

Three clock reads of the same monotonic counter, in program order:

* **B** — `kernel/src/syscall/futex.rs`'s deadline base, read once at syscall
  entry (`get_monotonic_time_ns()`), used to compute
  `deadline = B + relative_ns`. `ETIMEDOUT` is returned only once a later
  read of this same clock is `>= deadline`.
* **A** — the read `record_arm` used to take, inside
  `kernel/src/syscall/futex_oracle.rs`, called *after* `B` and after a
  process-manager lookup (`current_thread_group_id`, a global spin mutex
  taken with interrupts masked, scanning the process table) that this call
  site performs to resolve the caller's thread group.
* **E** — the read `elapsed_since_arm` takes at report time, after the
  `ETIMEDOUT` arbitration read that compared against `deadline`.

By program order, `B <= A <= E`, and the arbitration read that produces
`ETIMEDOUT` only fires once the clock is `>= deadline`, so `E >= B +
relative_ns` always. The oracle's *old* report computed `elapsed = E - A`,
which is therefore `>= relative_ns - (A - B)` — a strict subinterval of the
wait the kernel actually honoured, short by exactly the process-manager
lookup's cost (`A - B`), a window with no upper bound in code. The kernel's
own producer-side early-return budget is `0` — the deadline arithmetic is
exact (no rounding: the clock is a live counter, not a tick), and the wake
side only ever fires at or after the deadline
(`kernel/src/task/scheduler.rs`'s heap breaks on `wake_time > now_ns`). The
defect is in where the oracle's own elapsed measurement was anchored, not in
the wait the kernel performed. claim-lint:ok: #627 -- provable by
construction from program order (`B <= A <= E`), not by boot sampling; see
`validate_futex_oracle_record_arm_anchor` in `tests/teardown_structure.rs`.

## Why the kernel is not at fault

`futex_wait`'s deadline arithmetic (`kernel/src/syscall/futex.rs:89-107`) is
not tick-quantised on this path: the aarch64 clock is `CNTVCT_EL0`, a live
counter read directly (not a tick counter), and the wake side only returns
`ETIMEDOUT` after re-reading that same counter and finding it `>=` the
deadline. This round's own diff (commit `futex: anchor the oracle's stage3
elapsed measurement to the deadline's own base_ns`) touches that arithmetic
in 0 lines; the 50ms request (`STAGE3_REQUEST_NS`), the 1s backstop
(`BACKSTOP_NS`), and the `stage3_elapsed_ok` predicate's threshold are the
3 of 3 values a `git diff` of that commit shows byte-identical to before this
round.

## The fix

Observer-side only, per R180. `record_arm` (`kernel/src/syscall/futex_oracle.rs`)
now takes `base_ns: u64` — the same clock read `futex.rs` used to compute the
deadline — and stores it as the stage's `ARM_NS` anchor. `elapsed_since_arm`
is unchanged; it still just subtracts `ARM_NS` from a fresh read, so it now
measures `E - B` instead of `E - A`, and `E - B >= relative_ns` holds by
construction (not by luck). `record_arm`'s own clock read is kept, but only
to compute the existing `1s` backstop return value — that computation is
untouched, so no futex deadline arithmetic changed. The gap the anchor used
to silently absorb (`A - B`) is now reported as its own field,
`arm_delay_us`, so it stays visible instead of disappearing.

`kernel/src/syscall/futex.rs`'s `futex_wait` captures the deadline's own
clock read into `deadline_base_ns` (gated `#[cfg(feature = "boot_tests")]`,
compiled out of a production kernel) and passes it to `record_arm` at the
existing call site. If no timeout was supplied (`deadline_base_ns` is
`None`, unreachable on this branch — see below), the call falls back to
reading the clock itself, matching `record_arm`'s pre-#627 shape.
claim-lint:ok: #627, the production-profile boot proof below is x1 with the
oracle marker count pinned at 0 (`docker/qemu/run-aarch64-prod-profile-boot-test.sh`'s
own `KERNEL_ORACLE_COUNT` check).

## The two false sentences, and the sweep for others

Two paragraphs stated the old (false) claim about what `stage3_elapsed_ok=1`
proves — `kernel/src/syscall/futex_oracle.rs:249-251`'s comment above the
predicate ("This bit proves only that the wait did not return before its
requested timeout") and `docker/qemu/run-aarch64-boot-test-strict.sh:26`'s
gate header ("stage3_elapsed_ok=1 proves no early timeout return"). A repo
grep for the same claim (`proves no early timeout return`, `did not return
before its requested`, `the wait the kernel performed`) found it repeated
verbatim in three more gate scripts that carry the identical comment block:
`docker/qemu/run-aarch64-service-sequence-gate.sh`,
`docker/qemu/run-aarch64-full-test.sh`, and `docker/qemu/run-x86-boot-tests.sh`.
All five are rewritten in this round (`git log` commit
`docs(gates): rewrite the futex oracle's false stage3_elapsed_ok sentence`);
a post-rewrite grep for both phrases returns 0 matches. claim-lint:ok: #627,
0 of 5 sites still carry either phrase after this round's rewrite commit.

## Marker shape

New field `arm_delay_us`, inserted between `stage3_elapsed_ms` and `rescues`:

```
[FUTEX_HANDOFF_ORACLE:aarch64:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=50:arm_delay_us=30:rescues=0:queue_residual=0:balance=0]
```

All four gate scripts' `FUTEX_HANDOFF_ORACLE_PATTERN` (a non-anchored
substring `grep -qE`) widened to `:arm_delay_us=[0-9]+:` between the two
existing fields; the predicate itself (`stage3_elapsed_ok=1`, no tolerance)
is unchanged. The two committed serial fixtures
`docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-boot1-serial.txt`
and `02-prod-boot1-serial.txt` — the green baselines
`loopback_pump_structure::both_aarch64_gates_fail_on_a_pinned_placement_refusal`
and `ttbr0_shadow_reconciliation_structure::both_aarch64_gates_fail_on_an_untagged_publish`
read — predate this field and stopped matching the widened pattern; both are
re-recorded in this round from fresh live boots at this branch's HEAD, per
the same standing landing step `SLICE3D-2026-09-05.md` used for this same
fixture pair when PR #819 widened the pattern for a different oracle. Each
new fixture was confirmed `PASS` through the gate's own
`BREENIX_STRICT_SCORE_ONLY` / `BREENIX_PROD_SCORE_ONLY` scoring-only entry
point before being committed (section "Proofs" below).

## Structure test

`tests/teardown_structure.rs` gained
`validate_futex_oracle_record_arm_anchor`: it checks `record_arm`'s signature
carries `base_ns: u64`, that each stage's `ARM_NS` store uses `base_ns` (not
a fresh internal read), that the backstop return is still computed from
`record_arm`'s own clock read (proving the futex deadline arithmetic did not
move), and that `futex_wait`'s call site passes an explicit `base_ns`
argument. It is registered in `current_teardown_bypass_surface_is_exact`
alongside the existing oracle marker/gate-pin check, and exercised in
`deliberately_broken_variants_fail_the_ratchet`: reverting
`STAGE3_ARM_NS`'s store to the pre-fix `started_at` shape reddens it (the
mutation leg's `report_vacuity` call panics if the validator does not
reject the mutation; the suite is green with the leg in place — see
"Proofs").

## What is NOT claimed

* This round does not claim the anchor gap (`arm_delay_us`) is bounded. It
  is not — the RCA that filed #627 traced it to a process-manager lookup
  under a global spin mutex with interrupts masked, and this round's own
  20-boot sample (below) shows it ranging from single-digit to 71
  microseconds on an idle host; a loaded host could push it further, exactly
  as the two original #627 specimens (1-4ms) demonstrate. That is fine here
  because `arm_delay_us` no longer feeds `stage3_elapsed_ok`; it is reported,
  not bounded.
* This round does not open or close the two RCA "open question" hypotheses
  for what *else* could make `ETIMEDOUT` genuinely early: a re-based
  `BASE_TIMESTAMP` moving the deadline backwards, or a second `ETIMEDOUT` arm
  that skips the deadline comparison. Neither specimen on #627 matches
  either shape (both carry `rescues=0` and a single `stage3_ret=ETIMEDOUT`
  arm), and this round changes no code that would touch them. They stay open
  on #627.
* This round does not touch `STAGE3_REQUEST_NS`, `BACKSTOP_NS`, the
  `stage3_elapsed_ok` predicate, its threshold, or any other futex deadline
  arithmetic. `record_arm`'s backstop return value is computed identically to
  before this round (from its own clock read, not from `base_ns`) — only the
  value *stored* as the stage's reporting anchor changed.
* This round does not claim the x86 leg needed the fix to pass — x86's
  stage3 wait already overruns its 50ms request by roughly 18x on this
  round's own measurement (`stage3_elapsed_ms=892`, see "Proofs"), so a few
  hundred microseconds of anchor gap could not have pushed
  `stage3_elapsed_ok` to `0` there. The x86 marker gained the same
  `arm_delay_us` field regardless, since `record_arm` is shared between both
  architectures.

## Proofs

### aarch64 build

```
$ touch kernel/src/main_aarch64.rs
$ cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64
   Finished `release` profile [optimized] target(s) in 7.50s
-> exit 0; 1 warning line, the pre-existing toolchain `core`
   future-incompat note (unrelated to this branch's files)

$ ./scripts/check-kernel-no-neon.sh
PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0)
```

### aarch64 strict gate, 20 boots, one QEMU at a time

Run in isolation from other concurrent worktrees' gates on this Mac (a
private `OUTPUT_DIR`, not `/tmp/breenix_aarch64_strict_N`, used only to keep
this round's own evidence from being overwritten by another session's
concurrently-running gate at the same default path — the committed gate
script's `OUTPUT_DIR` is untouched by this round):

```
=========================================
RESULTS
=========================================
Total iterations: 20
Successes: 20
Failures: 0
Success rate: 100%
Duration: 230s

=========================================
PASS: 20/20 boots succeeded
=========================================
```

All 20 boots: `stage3_elapsed_ok=1`. `stage3_elapsed_ms` ranged 50-51;
`arm_delay_us` ranged 5-71 (microseconds). Full 20-line capture:
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/proof/futex-markers-20boots.txt`
(scratch, not in-repo).

### aarch64 production profile, x1

```
PASS: production profile reached bsshd with the futex oracle seam absent
```

### Fixture re-record, score-only replay against the committed gate scripts

```
$ BREENIX_STRICT_SCORE_ONLY=docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-boot1-serial.txt \
    bash docker/qemu/run-aarch64-boot-test-strict.sh
SCORE: PASS - docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-boot1-serial.txt

$ BREENIX_PROD_SCORE_ONLY=docs/planning/green-program/aarch64-testing/serials/slice3d/02-prod-boot1-serial.txt \
    bash docker/qemu/run-aarch64-prod-profile-boot-test.sh
PASS: production profile reached bsshd with the futex oracle seam absent
```

### x86, beast, own clone

```
$ cargo build --release --features testing,external_test_bins --bin qemu-uefi
    Finished `release` profile [optimized] target(s) in 2m 38s
-> exit 0; 0 warning/error lines (grep -iE '^(warning|error)' matched nothing)

$ ./docker/qemu/run-x86-boot-tests.sh 1
x86 frame-custody gate run 1: PASS
```

x86's futex marker, same boot, gained the identical `arm_delay_us` field
(the field is not architecture-specific — `record_arm` is shared code):

```
[FUTEX_HANDOFF_ORACLE:x86:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=892:arm_delay_us=145:rescues=0:queue_residual=0:balance=0]
```

Beast clone (`/root/breenix-p627`) and its gate tmp dir removed at the end of
this round.

### Structure test suites

31 of 31 `tests/*_structure.rs` files (`teardown_structure.rs` included) run
via `cargo test --test <name>`, filtered to just those targets:

```
green/total: 32 / 32   cases: 601
```

32 of 32 targets green: the 31 `tests/*_structure.rs` files plus
`x86_gate_verdict_test.rs`, the other target this round's gate-pattern edits
touch. `deliberately_broken_variants_fail_the_ratchet` includes the new
mutation leg (`record_arm base_ns anchor`) reported by name in its
`--nocapture` output, confirming it fired and was rejected.

### Claim discipline

```
claim-lint: scripts/claim-lint.py                              -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg1>           -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg2>           -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg3>           -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg4>           -> exit 0
claim-lint: scripts/test_claim_lint.py                          -> exit 0
```

This round adds no new rule to `claim-lint.py` and no new fixture to
`test_claim_lint.py`; the run above is the tool's own pre-existing
harness-and-corpus suite, unrelated to this branch's files, confirming this
round's edits did not perturb it.
