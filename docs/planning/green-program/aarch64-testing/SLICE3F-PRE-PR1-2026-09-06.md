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
`kernel/src/task/scheduler.rs` (+52/-1 in that stat, per
`git diff --numstat a0ec6cf8 a9d4bd3e -- kernel/src`), `kernel/src/main.rs`,
`kernel/src/main_aarch64.rs`, `kernel/src/interrupts/context_switch.rs` and
four other kernel files (8 `kernel/src` files touched in total). 14 of 14
`kernel/src` citations in the brief were
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
| probe pin clear | `:4849` | `:4900` |
| `census_before` tuple | `:4799-4805` | `:4850-4856` |
| `census_after` tuple | `:4828-4834` | `:4879-4885` |
| `census_clean` | `:4859` | `:4910` |
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
* The exclusivity prose in `emit_pin_guard_oracle` is rewritten, with its own
  new `claim-lint:ok` pair; the counter's doc gains a new paragraph, with a
  second new `claim-lint:ok` pair appended below it -- the pre-existing pair
  on that doc (`spawn_on_cpu`'s writer count) is untouched by this diff. The
  single-caller precondition is stated where the cumulative `refused=` is
  read.

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
(PINNED_MIGRATION_REFUSED.fetch_sub())\"] ...
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
| x86 `boot_tests,testing,external_test_bins` | beast `<x86-build-environment>`, `<isolated-checkout-42>` | 0, 0 warnings |
| x86 `testing,external_test_bins` | same | 0, 0 warnings |
| x86 no features | same | 0, 0 warnings |

The beast clone was made from the pushed branch at
`1c9f0f533fcef53d204e257dbdda764f5eb553b3`, with `rust-fork` symlinked to
`<rust-fork-checkout>` and the userspace ELFs and fonts copied from
`<canonical-checkout>`. On this host the aarch64 kernel build needs
`userspace/programs/aarch64/*.elf` and `target/ext2-aarch64.img`, which a fresh
worktree does not have; both were copied from the primary checkout at
`<local-checkout>`. Neither is tracked, and `git status --short`
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
note were repaired by rewording, not by annotation. The counts on the two new
claim-linted sentences this PR adds in `scheduler.rs` -- the counter's new
doc paragraph and the rewritten exclusivity prose in `emit_pin_guard_oracle`
-- were re-derived by grep in this round. The pre-existing claim-lint pair on
the counter's doc (`spawn_on_cpu` as the field's writer) is untouched by this
diff and was not re-verified here.

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

## 8. Review fix round

Six review findings against this round (F2, F3, F6, F8, F11, F12), each
minor or nit severity, closed together as one fix round.

* **F3** -- `kernel/src/main_aarch64.rs`'s comment above the two call sites
  (`emit_pin_guard_oracle()` then `emit_pinned_placement_census()`) still said
  the ordering mattered because "the probe subtracts its own contribution",
  which is the mechanism section 2 of this round deleted. Reworded to name the
  mechanism that replaced it: `count_pinned_migration_refusal` routes the
  probe's own tid to `PIN_GUARD_ORACLE_REFUSED`, so any refusal it attributes
  to a different thread during that window still needs to reach the census
  before this line prints it.
* **F12** -- the non-`boot_tests` definition of `count_pinned_migration_refusal`
  (the plain `fetch_add` arm the `#[cfg(not(all(target_arch = "aarch64",
  feature = "boot_tests")))]` selects: both x86 profiles plus the aarch64
  non-`boot_tests` profile) had no `#[inline]`, unlike its sibling one-line
  wrapper functions elsewhere in this file (5 of 5 checked -- `:231`, `:241`,
  `:5863`, `:5876`, `:6352` before this round -- carry the attribute). Added
  `#[inline]` to match that pattern. This is cost-of-shape only, as F12 itself
  said: no logging, lock, allocation or I/O is on the path either way.
* **F2** -- section 2 and section 6 of this note overstated what the diff did
  to the `PINNED_MIGRATION_REFUSED` doc comment. The diff *adds* a new
  paragraph and a new `claim-lint:ok` pair to that doc; it does not touch the
  pre-existing pair on the same doc (`spawn_on_cpu` as the field's 1 mutating
  writer), which the diff at `kernel/src/task/scheduler.rs:444-445` (a9d4bd3e
  numbering) leaves byte-identical. Reworded both sections to say what
  changed (a new paragraph, a new pair, appended) and what did not (the
  existing pair, not re-verified in this round). Doc-only; no source line
  changed for this finding, and no re-run was needed for it beyond the
  no-source-diff check itself.
* **F8** -- three numeric slips in section 1's baseline reporting, each
  checked directly against the immutable `a9d4bd3e` blob (unaffected by
  anything in this fix round, since `a9d4bd3e` predates it):
  `git diff --numstat a0ec6cf8 a9d4bd3e -- kernel/src` reports
  `52	1	kernel/src/task/scheduler.rs`, not the `+53/-0` this note first said;
  the same numstat lists 8 `kernel/src` files touched in total, so the four
  named plus "five other kernel files" should have read four; and in
  `git show a9d4bd3e:kernel/src/task/scheduler.rs`, `census_after`'s tuple runs
  `:4879-4885` (not `:4880-4886`, and `:4886` is a blank line),
  `census_clean` is at `:4910` (not `:4909`, which is `on_home,`), and the
  probe pin clear (`probe.cpu_affinity = None;`
  (claim-lint:ok: "None" is Rust's `Option` variant, not the universal
  quantifier, kernel/src/task/scheduler.rs)) is at `:4900` (not `:4899`, which
  is the preceding `let mut probe = ...` line). All three corrected in
  section 1's tables; `probe pin stamp` (`:4846`) and `census_before`
  (`:4850-4856`) were checked in the same pass and are already correct, so
  neither was touched.
* **F11** -- `validate_census_counters_have_no_decrementing_writer` in
  `tests/loopback_pump_structure.rs` built its offender message as
  `format!("{path}:{line} ({counter}.{method})")` with `method` drawn from a
  `lowering` array whose entries already carried their own opening
  paren (`"fetch_sub("`, etc.), so the rendered text had one more `(` than
  `)` -- observed verbatim as `...(PINNED_MIGRATION_REFUSED.fetch_sub()` in
  both `01` and `03`'s stored evidence. Changed `lowering` to bare method
  names and moved the opening paren into the two call sites that use it (the
  `needle` search and the offender message), so the search behaviour is
  unchanged (`needle` is byte-identical either way) and the message now reads
  `...(PINNED_MIGRATION_REFUSED.fetch_sub())` with balanced parens.
* **F6** -- `01-ratchet-red-on-unmodified-kernel.txt` and
  `03-leg-i-reinserted-decrement-red.txt` were captured before the mid-round
  fixture repair recorded in section 2's deviations, so their two rename-rule
  panics cited `tests/loopback_pump_structure.rs:3826` and `:3815` where the
  committed file (both before and after this fix round -- the repair touched
  no line count) reads `:3836` and `:3825`. Rather than disclose the mismatch
  and leave it standing, both files are regenerated in this round against the
  exact committed ratchet, so the mismatch is closed rather than annotated:
  - `01`: `kernel/src/task/scheduler.rs` checked out at `a9d4bd3e` (the
    unmodified baseline), `tests/loopback_pump_structure.rs` at this round's
    final committed bytes, `scripts/run-structure-tests.sh
    loopback_pump_structure` -> exit 101, `test result: FAILED. 110 passed;
    3 failed`, same three tests as before
    (`census_counters_have_no_decrementing_writer`,
    `census_rules_track_a_consistent_rename_of_every_counter`,
    `oracle_snapshots_cover_the_census`), now citing `:3836` and `:3825` and
    (from the F11 fix, captured in the same run) `fetch_sub())` with the
    closing paren.
  - `03`: `kernel/src/task/scheduler.rs` at this round's fixed bytes, with a
    `PINNED_MIGRATION_REFUSED.fetch_sub(0, Ordering::Relaxed);` reinserted
    directly after the `fetch_add` in `count_pinned_migration_refusal`'s
    `boot_tests` arm (the same site and shape leg (i) used originally) ->
    exit 101, `test result: FAILED. 111 passed; 2 failed`, naming
    `scheduler.rs:571` -- the same line leg (i) named the first time, because
    F12's `#[inline]` addition sits in the second, non-`boot_tests` definition
    of `count_pinned_migration_refusal`, physically below the `boot_tests` arm
    this mutation is inserted into, so the line numbers inside that first arm
    are unmoved by F12. Reverted; `diff` against the pre-mutation file showed
    0 residual bytes changed, and a re-run returned to `113 passed;
    0 failed`.

  Section 3's inline RED-text quote (the `fetch_sub()` excerpt) is also
  corrected to `fetch_sub())`, taken from the regenerated `01`. Files `02`,
  `04`, `05` and `06` were not regenerated: `04`/`05`/`06`'s line citations
  (`:3836`/`:3830`, `:3866`) already matched the committed file before this
  round (checked directly against `grep -n` on the committed bytes) and this
  round's edits do not change `tests/loopback_pump_structure.rs`'s total line
  count (the `lowering` array stays 5 entries across 7 lines; only string
  contents moved), so 0 lines in those three files' citations moved. `02` was
  re-run (`113 passed; 0 failed`, matching what it already said) but not
  overwritten: the check above found 0 lines in it to correct.

### Re-run after the fix

| Suite / gate | Exit | Result |
|---|---|---|
| `loopback_pump_structure`, HEAD-with-fixes | 0 | `113 passed; 0 failed` (twice: once before the leg-i mutation, once after reverting it) |
| `loopback_pump_structure`, `scheduler.rs`@`a9d4bd3e` (RED baseline, regenerated) | 101 | `110 passed; 3 failed`, same 3 tests, new citations |
| `loopback_pump_structure`, leg (i) reinserted (regenerated) | 101 | `111 passed; 2 failed`, `scheduler.rs:571` |
| aarch64 `boot_tests` build, `aarch64-breenix-kernel.json`, this host | 0 | 1 future-incompat notice from upstream `core`, no warning/error naming a file in this repo |
| `docker/qemu/run-aarch64-boot-test-strict.sh 3` | 0 | 3/3 boots, `refused=3:census_clean=1:verdict=PASS`, `migration_refused=0` in the census line -- `serials/3f-pre/14-review-fix-round-strict-gate-3boots.txt` |

The strict gate was re-run because this fix round edits `kernel/src` bytes
(the `main_aarch64.rs` comment and the `scheduler.rs` `#[inline]`), even
though neither is a behavioural change; `pgrep -fl qemu-system` before this
gate run showed 0 processes running, and after it showed 0 processes left by
this round (no process was killed by name).

### What this fix round does NOT claim

* That `04`, `05` or `06` were re-verified byte-for-byte in this round beyond
  the `grep -n` check against the committed file described above -- they were
  not re-run.
* That the strict gate's `#[inline]` addition changes emitted code on any
  profile. It was not measured; the gate confirms the boot still passes with
  the byte present, not that codegen is identical either way.
* That F2's untouched claim-lint pair (`spawn_on_cpu` as the field's 1
  mutating writer) is itself correct. It was read, not re-derived, in this
  round; whether `run_pin_guard_oracle`'s direct `cpu_affinity` writes bear on
  that pair's scope is not evaluated here.

### Claim discipline, this fix round

```
claim-lint: python3 scripts/claim-lint.py                             -> exit 1 (6 findings, universal-claim, in this section)
claim-lint: python3 scripts/claim-lint.py                             -> exit 0 (after rewording those 6)
claim-lint: python3 scripts/claim-lint.py                             -> exit 1 (1 finding, universal-claim, in this very paragraph)
claim-lint: python3 scripts/claim-lint.py                             -> exit 0 (after annotating that one)
claim-lint: python3 scripts/claim-lint.py --commit-msg <round-fix msg> -> exit 0
```

5 of the 6 first-pass findings were repaired by rewording. The 6th, against
the `probe.cpu_affinity = None;` code-span quote in the F8 bullet, is
annotated `claim-lint:ok` per the same idiom this note already uses for
Rust's `cfg(all(...))` in section 2 and section 7 -- the flagged word names a
Rust `Option` variant, not an empirical universal claim. Describing that
annotation in this paragraph then tripped a 7th finding against the word
`None` appearing here too (self-referential, the same shape section 6 of the
original round note describes for its own claim-lint passes), closed with a
second copy of the same annotation.
(claim-lint:ok: "None" is Rust's `Option` variant, kernel/src/task/scheduler.rs)

## 9. Landing re-smoke, at the merged head

`git fetch origin && git merge origin/main --no-edit` merged `origin/main`
(`5bfc7077af7dfde2e0aa81189088cc838584dea3`) into this branch's tip
(`ce56e44d0311e88c4c3381800c05392dfb817f4a`) as merge commit
`2653a3739`, auto-resolved with 0 conflicts and 0 kernel/ path collisions.
`git diff --stat` from the merge-base (`a9d4bd3e`) shows both sides touched
`kernel/src/main_aarch64.rs` and `kernel/src/task/scheduler.rs` (main's side:
PR #894's timer/wake-dispatch work and PR #896's failure-capture PR-7
lockup-report changes; this branch's side: the pin-guard oracle attribution
fix), and `git diff --name-only --diff-filter=U` after the merge is empty --
`grep -n '^<<<<<<<\|^=======\|^>>>>>>>'` over both files returns 0 matches.

Main also touched three aarch64 gate scripts since the merge-base
(`run-aarch64-boot-test-strict.sh`, `run-aarch64-prod-profile-boot-test.sh`,
`run-aarch64-service-sequence-gate.sh`), each adding one call to the new
`scripts/check-aarch64-lockup-no-alloc.sh` guard (failure-capture PR-7). That
call sits inside each script's real-boot branch, after the
`SCORE_ONLY_SERIAL` early-exit the fixture-replay tests use
(`run-aarch64-boot-test-strict.sh:314-316`) -- `tests/loopback_pump_structure.rs`'s
`both_aarch64_gates_fail_on_a_pinned_placement_refusal` and
`the_gates_score_the_pin_guard_oracle_in_opposite_directions`, plus
`tests/ttbr0_shadow_reconciliation_structure.rs`'s own copy of the same
scoring-only replay, invoke these two gates with `BREENIX_STRICT_SCORE_ONLY`
/ `BREENIX_PROD_SCORE_ONLY` set, which takes that early-exit path instead of
reaching the new guard call -- confirmed by reading both scripts' control
flow (the guard call sits after the `SCORE_ONLY_SERIAL` branch's own
early-return, at a line number strictly greater than it in both files). So
R182 does not fire: no scorer requirement
reaches the replayed `slice3e` fixtures, and no fixture was re-recorded. This
was confirmed by running the suites, not only by reading the branch: both are
green at the merged head (below).

### 9.1 Structure suites

`scripts/run-structure-tests.sh <stem>`, once per discovered
`tests/*_structure.rs` file (the same discovery loop
`docker/qemu/lib/gate-structure-preflight.sh` uses): **51/51 green**, 0
failures. `tests/loopback_pump_structure.rs`: `113 passed; 0 failed`.
`tests/ttbr0_shadow_reconciliation_structure.rs`: `32 passed; 0 failed`.
`tests/teardown_structure.rs`: `92 passed; 0 failed`.

### 9.2 aarch64 strict gate, `docker/qemu/run-aarch64-boot-test-strict.sh` (default 20 boots)

**Run 1** (host load 8-10 from a concurrent lane's gate on this shared Mac):
exit 1, 18/20. `[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=260:pinned=120]`.
Two failures:

* Boot 9: `[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=14:armed=0:acquired=1:holder_cpu=2:pm_busy_probe=0:calls=0:eagain=0:first_errno=18446744073709551615:first_wait_us=0:hold_safety=1:hold_done=1:joined=1:FAIL:hold_safety_release]`
  -- byte-identical `hold_safety_release` arm to the one already filed as
  **#836** (`hold_safety=1`, same field shape). Serial preserved:
  `docs/planning/green-program/aarch64-testing/serials/3f-pre-land/15-landing-strict-run1-boot9-fcntl-pm-contention-oracle-fail.txt`.
* Boot 10: "Ring-span self-check marker missing" -- the raw serial shows the
  `[RING_SPAN:...]` line byte-interleaved with another unlocked writer
  (`[L[RING_SPAN:cpu=O0:span_ms=1300G:writes=480:dropGped=0:...]`), the exact
  mechanism **#847** documents (unlocked multi-byte serial writers on an SMP
  boot can interleave on the shared UART; fails safe by rejecting a good
  boot). Serial preserved:
  `docs/planning/green-program/aarch64-testing/serials/3f-pre-land/16-landing-strict-run1-boot10-ring-span-corruption-fail.txt`.

Neither failure's marker or mechanism involves `scheduler.rs`, `main_aarch64.rs`,
the pin-guard oracle, or the pinned-placement census -- the two files this
branch's own diff touches. Both signatures are pre-existing and already filed
(#836 opened 2026-09-06T06:27:59Z, #847 opened 2026-09-06T07:58:16Z, both
before this landing round).

**Run 2**, same merged-head kernel binary, lower host load (4-10, no
concurrent gate for most of the run): exit 0, **20/20**, `Success rate: 100%`.
`[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=260:pinned=120]`.
`PIN_GUARD_ORACLE` and the pinned-placement census, sampled from boots 1, 5,
10, 15 and 20's serial (byte-identical across all five):

```
[PIN_GUARD_ORACLE:aarch64:home=1:here=0:reclaim=1:requeue=1:previous=1:on_home=3:refused=3:census_clean=1:verdict=PASS]
[PINNED_HOME_CPU_UNAVAILABLE:count=0:publish_discarded=0:hold_pen_migrated=0:delivered=0:migration_refused=0:stack_home_conflict=0]
```

Same shape as section 5's original gate run: `refused=3` sourced from
`PIN_GUARD_ORACLE_REFUSED`, `census_clean=1` covering 6 fields,
`migration_refused=0` in the census. Full logs for both runs:
`docs/planning/green-program/aarch64-testing/serials/3f-pre-land/01-strict-gate-run1-2of20-failed.txt`,
`.../02-strict-gate-run2-20of20-passed.txt`.

### 9.3 aarch64 production-profile gate, `docker/qemu/run-aarch64-prod-profile-boot-test.sh`

One run (the brief's command takes no iteration argument): exit 0, PASS.
`[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=260:pinned=120]`.
`PASS: production profile reached bsshd with the futex oracle seam absent`.

```
Observed pin-guard oracle line count (must be 0 in this profile): 0
Observed: [PINNED_HOME_CPU_UNAVAILABLE:count=0:publish_discarded=0:hold_pen_migrated=0:delivered=0:migration_refused=0:stack_home_conflict=0]
```

Same as section 5's original run: 0 pin-guard oracle lines in this
no-`boot_tests` profile, and `migration_refused=0` in the census. Full log:
`docs/planning/green-program/aarch64-testing/serials/3f-pre-land/03-prod-profile-gate-run1-passed.txt`.

### 9.4 x86, beast, `docker/qemu/run-x86-boot-tests.sh 1`

One run on beast (the x86 build environment, clone `<isolated-checkout-42>`,
reset to this branch's merge commit `2653a3739`): exit 0, no `FAIL` marker
anywhere in the log (`grep -c 'TEST:.*:FAIL\|^FAIL\|:FAIL\]'` returns 0).
`[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=260:pinned=120]`
at the top of the log; `x86 frame-custody gate run 1: PASS` at the bottom,
with the script's five `FAIL` arms (`BOOT_TESTS:FAIL`/panic,
`CENSUS_WIDEN_ORACLE:FAIL`, `TEST:network:*:FAIL`, `TEST:userspace:*:FAIL`,
`CREATION_LOCK_ORDER:VIOLATION`) each guarded by a `false` that would have
aborted the script under `set -e` before that PASS line printed. Wall-clock
was long (roughly 20 minutes from `git reset --hard` to completion) because
beast's host load averaged 20-27 from other lanes' concurrent builds/gates
during this run (`breenix-slot1` had its own x86 boot-tests QEMU running
throughout) and `accel=tcg` (no hardware virtualization in the container) is
inherently slower than the Mac's HVF-accelerated aarch64 boots -- the boot
itself ran to completion once it acquired the shared `x86-qemu.lock`, it was
not stalled. Full log:
`docs/planning/green-program/aarch64-testing/serials/3f-pre-land/04-x86-boot-tests-run1-passed.txt`.

### 9.5 What this section does NOT claim

* That #836 or #847 are fixed, or that this round investigated either beyond
  matching the marker text and fields against the filed issue. Both stay open.
* That the strict gate's 18/20 run-1 result and 20/20 run-2 result together
  prove the gate's failure rate. Two runs of 20 is not a rate measurement; the
  claim here is narrower -- both run-1 failures decode to signatures already
  filed as pre-existing before this round started, and a same-binary re-run
  reached 20/20.
* That the aarch64 service-sequence gate ran. The brief's landing checklist
  names the strict gate, the production-profile gate, the structure suites and
  the x86 gate; it does not name `run-aarch64-service-sequence-gate.sh`, and
  this section does not claim it was run.
* That `origin/main` stayed at `5bfc7077a` for the rest of this round. A
  `git fetch` partway through beast setup showed `origin/main` had already
  advanced to `a012bcfbd` from other lanes landing concurrently; this branch's
  merge and re-smoke are against `5bfc7077a`, and this branch was not
  re-merged against the newer head.
