# SLICE3F-PRE PR 1 -- retiring the census subtraction, and attributing the pin-guard oracle's own refusals

Round record for PR 1 of the amended slice-3f prerequisite plan
(`PLAN-SLICE3F-PRE.md` section 2 "PR 1" and section 3, as corrected by
`PLAN-SLICE3F-PRE-EVAL.md` section 5). Scope is PR 1 alone: 0 gate scripts are
edited, 0 production pins are stamped, and 0 items from PR 2, PR 4a, PR 4b or
PR 5 are started.

## 1. Baseline, and the drift since the brief was written

| Fact | Value |
|---|---|
| Branch | `sched/3f-pre-oracle-attribution` |
| Base | `a9d4bd3ea04b88b7c4d697e6b894310ed39994d0` (`origin/main`) |
| Brief's assumed HEAD | `a0ec6cf8473d02c5029fc9ab44403c7f68cedde3` |
| `git merge-base --is-ancestor a0ec6cf8 HEAD` | true |
| `git status --short` at branch creation | empty |

`git diff --stat a0ec6cf8 a9d4bd3e` touches 64 files, and unlike the state the
eval measured it **does** touch `kernel/src`: PR #888 and PR #894 changed
`kernel/src/task/scheduler.rs` (+53/-0 in that stat), `kernel/src/main.rs`,
`kernel/src/main_aarch64.rs`, `kernel/src/interrupts/context_switch.rs` and
five other kernel files. 14 of 14 `kernel/src` citations in the brief were
re-derived by grep at `a9d4bd3e` before the first edit, and the ones that moved
are tabulated below. The drift, brief line vs
line at this head:

| What | Brief (at `a0ec6cf8`) | At `a9d4bd3e` |
|---|---|---|
| the `fetch_sub` on the census counter | `scheduler.rs:565` | `scheduler.rs:574` |
| `refused_before` snapshot | `:554` | `:563` |
| delta computation | `:562-564` | `:571-573` |
| the guard's refusal `fetch_add` | `:4707` | `:4758` |
| `run_pin_guard_oracle` | not cited by line | `:4819` |
| probe pin stamp | `:4794-4795` | `:4846` |
| probe pin clear | `:4849` | `:4899` |
| `census_before` tuple | `:4799-4805` | `:4850-4856` |
| `census_after` tuple | `:4828-4834` | `:4880-4886` |
| `census_clean` | `:4859` | `:4909` |
| `PINNED_MIGRATION_REFUSED` doc + claim-lint pair | `:437-438` | `:444-445` |
| oracle exclusivity prose | `:543-553` | `:553-562` |
| `emit_pin_guard_oracle` callers | `main_aarch64.rs:1365`, `main.rs:740` | `main_aarch64.rs:1365`, `main.rs:749` |
| prod census literal | eval's `prod:166` | `run-aarch64-prod-profile-boot-test.sh:171` |
| prod `PIN_GUARD_ORACLE` 0-count assertion | eval's `prod:786` | `run-aarch64-prod-profile-boot-test.sh:796` |

Unchanged from the eval's corrected values: the strict census literal at
`run-aarch64-boot-test-strict.sh:268`, the #528 NEON guard at
`run-aarch64-boot-test-strict.sh:316-325`, the host-wide QEMU lock sourced at
`run-aarch64-boot-test-strict.sh:26-30`, and `scripts/run-structure-tests.sh`
compiling at `:58` and running the binary at `:62`/`:64`.

`rg -n 'fetch_sub' kernel/src/task/scheduler.rs` at `a9d4bd3e` returns exactly
one hit, `:574`. `rg -n 'PINNED_MIGRATION_REFUSED' kernel/src/task/scheduler.rs`
returns `:446`, `:505`, `:563`, `:571`, `:574`, `:4758` plus doc lines.

## 2. The change

One commit, `1c9f0f533fcef53d204e257dbdda764f5eb553b3`, carrying the kernel
change and its ratchet together -- required, not stylistic: since PR #889 the
four boot gates run `gate_structure_preflight` before building or booting, so a
ratchet that is red on unmodified source would redden 4 of the 4 boot gates if
it landed on its own.

`kernel/src/task/scheduler.rs`:

* `PIN_GUARD_ORACLE_PROBE_TID` and `PIN_GUARD_ORACLE_REFUSED`, both
  `cfg(all(target_arch = "aarch64", feature = "boot_tests"))`, beside
  `PIN_GUARD_ORACLE_HELD`.
  (claim-lint:ok: "all" is Rust's `cfg(all(...))` combinator syntax, kernel/src/task/scheduler.rs)
* `count_pinned_migration_refusal`, two cfg-exclusive definitions. The
  `boot_tests` aarch64 one routes a refusal of the published probe tid into
  `PIN_GUARD_ORACLE_REFUSED` and any other refusal into
  `PINNED_MIGRATION_REFUSED`; the other is the plain increment. The guard's
  refusal arm calls it in place of the bare `fetch_add`.
* `run_pin_guard_oracle` stores the probe tid beside the pin stamp and stores 0
  beside the pin clear, both inside the one masked scheduler-lock window.
* The `refused_before` load, the `saturating_sub` delta and the `fetch_sub` are
  deleted; `refused` reads `PIN_GUARD_ORACLE_REFUSED`.
* `PINNED_MIGRATION_REFUSED` is added to both snapshot tuples, so
  `census_clean` covers 6 of the 6 fields the census emits rather than 5 of 6.
* The exclusivity prose and the `claim-lint:ok` pair on the counter's doc are
  rewritten, and the single-caller precondition is stated where the cumulative
  `refused=` is read.

`tests/loopback_pump_structure.rs` gains three rules and six mutation tests.
The counter set is discovered from the format arguments of
`emit_pinned_placement_census`, per the #549 and #551 rule -- no literal name
list.

Deviations from the brief, with reasons:

* **The refusal site got a helper rather than an inline `cfg` block.** The brief
  says "replace the single unconditional `fetch_add` with" the attributed
  choice. Two cfg-exclusive `fn` definitions were used instead of an inline
  `#[cfg]` block so that the call site reads identically in both profiles and
  the non-`boot_tests` build has no `_`-prefixed local. Same semantics, same
  cost on the refusal arm.
* **The eval's H7 addition was implemented as a third rule**, not folded into
  the census rules: `pin_guard_oracle_has_one_call_site_per_architecture`
  requires each call site of `emit_pin_guard_oracle` to be in one of the two
  architecture entry points and requires exactly one per entry point.
* **Two mutation fixtures were made rename-proof mid-round.** As first written,
  `census_decrement_validator_rejects_a_reintroduced_fetch_sub` and
  `..._rejects_a_load_subtract_store` injected the literal name
  `PINNED_MIGRATION_REFUSED`, so leg (ii) -- a consistent tree-wide rename --
  failed on those two fixtures rather than on the rule. Both now build the
  injected code from the first discovered counter. Leg (ii) was re-run after the
  repair; the pre-repair run is recorded below as what it was.
* **The secondary behavioural leg of PLAN section 3.3 was NOT run.** It is
  specified to run on PR 4's branch, where a real pinned daemon exists. 0
  changes in PR 1's scope produce a second pinned thread.

## 3. Ratchet: red at the baseline, green on the branch

The structure-test script was invoked directly, not through a boot gate, so
**no boot gate ran red in this round** -- the only boot gates run here are in
section 5, after the fix.

| Run | Command | Exit | Evidence |
|---|---|---|---|
| RED, unmodified kernel + new rules | `scripts/run-structure-tests.sh loopback_pump_structure` | 101 | `serials/3f-pre/01-ratchet-red-on-unmodified-kernel.txt` |
| GREEN, after the fix | same | 0 | `serials/3f-pre/02-ratchet-green-after-fix.txt` |

The RED text, from `01-ratchet-red-on-unmodified-kernel.txt`:

```
no pinned-placement census counter is written downward: "a pinned-placement
census counter is written downward at [\"kernel/src/task/scheduler.rs:574
(PINNED_MIGRATION_REFUSED.fetch_sub()\"] ...
```

```
both oracle snapshots load every counter the census emits:
"PINNED_MIGRATION_REFUSED is emitted by the pinned-placement census but absent
from the oracle's census_before snapshot ..."
```

`test result: FAILED. 110 passed; 3 failed` at the baseline;
`test result: ok. 113 passed; 0 failed` on the branch.

`pin_guard_oracle_has_one_call_site_per_architecture` passed at the baseline
too -- it pins a property the tree already had, which is the point of it.

## 4. Anti-vacuity legs

Each leg was applied to the tree, run, recorded, and reverted before the next;
the fix was re-applied between legs, and `git status --short` plus a re-run of
the suite confirmed the tree back at 113 passed / 0 failed afterwards.

| Leg | Mutation | Exit | Evidence |
|---|---|---|---|
| (i) | a `fetch_sub` of a census counter re-inserted in `count_pinned_migration_refusal` | 101 (RED, names `scheduler.rs:571`) | `serials/3f-pre/03-leg-i-reinserted-decrement-red.txt` |
| (ii) | all six census counters renamed consistently across `kernel/src` | 0 (GREEN) | `serials/3f-pre/04-leg-ii-consistent-rename-green.txt` |
| (iii) | one counter dropped from `census_before` only | 101 (RED, names `PINNED_HOME_CPU_UNAVAILABLE`) | `serials/3f-pre/05-leg-iii-census-before-dropped-red.txt` |
| H7 | a second call site of `emit_pin_guard_oracle` in `main_aarch64.rs` | 101 (RED) | `serials/3f-pre/06-leg-h7-second-call-site-red.txt` |

Leg (ii) on its first run was exit 101 with the two rules themselves green and
two *fixture* tests red, because those fixtures named a counter literally. That
is recorded here rather than discarded; the fixtures were repaired and the leg
re-run to the exit 0 above.

The in-suite mutation tests (`census_decrement_validator_rejects_a_reintroduced_fetch_sub`,
`..._rejects_a_load_subtract_store`,
`oracle_snapshot_validator_rejects_a_counter_dropped_from_census_before`,
`census_rules_track_a_consistent_rename_of_every_counter`,
`pin_guard_oracle_call_site_validator_rejects_a_second_entry_point_caller`,
`..._rejects_a_caller_outside_the_entry_points`) are 6 of the 113 passing tests
in the green run, so the legs are enforced on a gate run rather than only here.

Other suites the brief names: `teardown_structure` 90 passed / exit 0,
`serial_line_atomicity_structure` 9 passed / exit 0,
`critical_path_logging_census_structure` 10 passed / exit 0 -- the last of
these still fixes the `serial_println!` counts inside the two
`emit_pin_guard_oracle` arms at 3 and 1. The strict gate's own preflight then
ran every suite in the tree: `[GATE_PREFLIGHT:structure_suites=50/50:critical_path_lines=259:pinned=120]`.

The scoring-only tests `both_aarch64_gates_fail_on_a_pinned_placement_refusal`
and `the_gates_score_the_pin_guard_oracle_in_opposite_directions` pass against
their existing `slice3e` fixtures, unmodified. No gate script changed, so R182
does not fire and no fixture was re-recorded.

## 5. Builds and gates

Builds, 5 of 5 at exit 0 with 0 crate warnings and 0 errors. Each aarch64 build
log carries one line from the toolchain -- a future-incompatibility notice about
the upstream `core v0.0.0` in the rustup toolchain -- which names no file in
this repository and is not produced by this change.

| Profile | Where | Exit |
|---|---|---|
| aarch64 `boot_tests`, `aarch64-breenix-kernel.json` | this host | 0 |
| aarch64 `testing`, `aarch64-breenix-kernel.json` | this host | 0 |
| x86 `boot_tests,testing,external_test_bins` | beast `breenix-x86`, `/root/breenix-3fpre1` | 0, 0 warnings |
| x86 `testing,external_test_bins` | same | 0, 0 warnings |
| x86 no features | same | 0, 0 warnings |

The beast clone was made from the pushed branch at
`1c9f0f533fcef53d204e257dbdda764f5eb553b3`, with `rust-fork` symlinked to
`/root/breenix/rust-fork-real` and the userspace ELFs and fonts copied from
`/root/breenix`. On this host the aarch64 kernel build needs
`userspace/programs/aarch64/*.elf` and `target/ext2-aarch64.img`, which a fresh
worktree does not have; both were copied from the primary checkout at
`/Users/wrb/fun/code/breenix`. Neither is tracked, and `git status --short`
after the copies showed only the two edited source files.

Gates, 4 of 4 invocations run on this host behind the host-wide QEMU lock,
sequentially:

| Gate | Result | Evidence |
|---|---|---|
| `docker/qemu/run-aarch64-boot-test-strict.sh 3` | exit 0, 3/3 | `serials/3f-pre/07-strict-gate-3boots.txt` |
| `docker/qemu/run-aarch64-prod-profile-boot-test.sh` run 1 | exit 0 | `serials/3f-pre/11-prod-gate-run1.txt` |
| run 2 | exit 0 | `serials/3f-pre/12-prod-gate-run2.txt` |
| run 3 | exit 0 | `serials/3f-pre/13-prod-gate-run3.txt` |

Strict, per-boot serials preserved at `serials/3f-pre/08-strict-boot1-serial.txt`,
`09-strict-boot2-serial.txt`, `10-strict-boot3-serial.txt`. Each of the three
prints the census literal the gate scores at
`run-aarch64-boot-test-strict.sh:268`:

```
[PINNED_HOME_CPU_UNAVAILABLE:count=0:publish_discarded=0:hold_pen_migrated=0:delivered=0:migration_refused=0:stack_home_conflict=0]
```

and the oracle line, byte-identical in 3 of the 3 boots:

```
[PIN_GUARD_ORACLE:aarch64:home=1:here=0:reclaim=1:requeue=1:previous=1:on_home=3:refused=3:census_clean=1:verdict=PASS]
```

`refused=3` is now sourced from `PIN_GUARD_ORACLE_REFUSED` and `census_clean=1`
now covers 6 fields. `grep -c 'PINNED_HOME_CPU_UNAVAILABLE:first:'` returns 0
on each of the three serials.

Production, each of the three runs, from the gate logs above:

```
Observed pin-guard oracle line count (must be 0 in this profile): 0
Observed: [PINNED_HOME_CPU_UNAVAILABLE:count=0:publish_discarded=0:hold_pen_migrated=0:delivered=0:migration_refused=0:stack_home_conflict=0]
```

The census literal the production gate scores is at
`run-aarch64-prod-profile-boot-test.sh:171`; the oracle 0-count assertion is at
`:796`.

0 reds were seen in these six boots, so 0 attributions to a pre-adjudicated
signature were needed and the R52 UNATTRIBUTED rule was not reached.

The gates were re-run from scratch after a wording-only repair to comments in
both edited files (section 6), so the results above are from the source that
was committed, not from an earlier byte-set.

Cleanup: `pgrep -f qemu-system` after the last gate showed 2 processes, both
belonging to another lane's worktree (`scratchpad/battery/wt-a9d4bd3e`); no
process of this round's was left running, and no process was killed by name. No
Parallels VM was started.

## 6. Claim discipline

Seven invocations, in the order they were run:

```
claim-lint: python3 scripts/claim-lint.py                                   -> exit 1 (5 findings, universal-claim, in this round's new source prose)
claim-lint: python3 scripts/claim-lint.py                                   -> exit 0 (after rewording those 5)
claim-lint: python3 scripts/claim-lint.py --commit-msg <kernel+ratchet msg> -> exit 1 (1 finding, universal-claim)
claim-lint: python3 scripts/claim-lint.py --commit-msg <kernel+ratchet msg> -> exit 0 (after rewording)
claim-lint: python3 scripts/claim-lint.py                                   -> exit 1 (11 findings, universal-claim, in this note)
claim-lint: python3 scripts/claim-lint.py                                   -> exit 0 (after rewording this note)
claim-lint: python3 scripts/claim-lint.py --commit-msg <round-note msg>     -> exit 0
```

The 5 source findings, the 1 commit-message finding and the 11 findings in this
note were repaired by rewording, not by annotation. The counts on the two
claim-linted sentences this PR edits in `scheduler.rs` were re-derived by grep
in this round.

## 7. What is NOT claimed

* That a production `per_cpu_worker` pin is landable. It is not; PR 2 must land
  first, and the eval's S2 correction to PR 2 is still owed to PR 2's own brief.
* That #562 moved.
* That a subtraction of a real refusal was ever observed on a boot. The exposure
  is derived from the two snapshot loads standing outside the lock and from the
  probe driving the general reclaim, not from a failing serial.
* That one instruction changes in the shipped no-feature build. The attributed
  path is entirely `cfg(all(target_arch = "aarch64", feature = "boot_tests"))`,
  and the no-feature x86 build was run to confirm it compiles, not to measure a
  binary diff.
  (claim-lint:ok: "all" is Rust's `cfg(all(...))` combinator syntax, kernel/src/task/scheduler.rs)
* That the ratchet detects each possible mis-attribution. It detects the class
  the defect belongs to -- a gate counter with a decrementing writer, and a snapshot
  narrower than the census -- which is the class the #549 and #551 rule polices.
* That six boots are a soak. They are the sizes the brief set, and 0 sentences
  here state a rate.
* That an x86 boot gate was run. Only the three x86 builds were run, which is
  what the brief's step 5 asks for; the brief's gate step names the two aarch64
  gates only.
