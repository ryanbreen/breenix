# Slice 3e — captured runs

The 20 artifacts behind
`docs/planning/green-program/aarch64-testing/SLICE3E-2026-09-05.md`, recorded on
`sched/562-slice3e-pin-guard`. `01` through `14` are round 1, at head
`fc302093`; `15` through `20` are round 2, at head `9136c85e`, and section 11 of
that document is the table they evidence. 18 of the 18 `.txt` files here are raw
captures; the
`-text` attribute in `.gitattributes` keeps the CR bytes the guest console emits
byte-exact, for the same reason the slice 3a and slice 3d trees carry it.

## Provenance

**aarch64, on this Mac.** `01-strict-boot1-serial.txt` and
`02-prod-boot1-serial.txt` are boot 1 of the strict gate's 20-iteration default
run and the production gate's single boot, both at the branch head `fc302093`,
with `BUILD_ID` `006a9c95a5088d` and `006a9c998f1fdd` respectively. Both were
taken after the last `.rs` and `.sh` edit on the branch — `kernel/build.rs`
stamps `BREENIX_BUILD_ID` from `SystemTime::now()`, so `shasum` is not a stable
identifier for these bytes and the `BUILD_ID` line is the tie instead. They are
the **fixtures** two replay tests score:
`tests/loopback_pump_structure.rs::both_aarch64_gates_fail_on_a_pinned_placement_refusal`,
`...::the_gates_score_the_pin_guard_oracle_in_opposite_directions` and
`tests/ttbr0_shadow_reconciliation_structure.rs::both_aarch64_gates_fail_on_an_untagged_publish`,
each of which replays them through a gate's scoring-only mode. They replace the
slice 3d captures those tests read before, which carry the 4-field
`PINNED_HOME_CPU_UNAVAILABLE` census line this slice widened to 6 fields — the
standing "widen the line, re-record the fixture, update both scorers" step.

`13-strict-x20.txt` and `14-prod-gate.txt` are the two gate runs' own stdout at
the same head.

**The red-on-main capture.** `03-red-on-main-oracle-serial.txt` is one
`docker/qemu/run-aarch64-boot-test-strict.sh 1` boot of a worktree at
`origin/main` `55689b42` carrying `03b-red-on-main-oracle-probe.patch` and
nothing else (225 added lines across 3 files: the probe, its builder and its
call site). `BUILD_ID` `006a9c8c1c1074`. It is the reading the slice's oracle is
red on:

```
[PIN_GUARD_ORACLE:aarch64:home=1:here=0:reclaim=0:requeue=0:previous=0:on_home=0:refused=0:census_clean=1:verdict=FAIL]
```

The patch differs from the branch's own probe in 2 places, both commented in the
patch: `main` has no `PINNED_MIGRATION_REFUSED` counter to snapshot, so
`refused` is a literal 0 there, and `main`'s census tuple has 4 fields rather
than 6. `origin/main` advanced from the branch's base `e9b0a4f6` to `55689b42`
during the round; `git diff --stat e9b0a4f6..55689b42` lists 7 files, 0 of them
under `kernel/`, `docker/` or `tests/`.

**Mutation captures.** `04`, `05` and `06` are the failing boots of the three
kernel mutations in section 5 of the round doc, each applied singly to
`kernel/src/task/scheduler.rs` and reverted from a pristine copy kept outside
the tree. `05` is the one that matters most: the guard declines immediately —
behaviourally what `main` does at these sites — the source ratchets stay green
at 95 of 95, and the oracle reads exactly the red-on-main line.

**The testing-profile red.** `07-testing-profile-red-562-serial.txt` is
`docker/qemu/run-aarch64-testing-profile-boot-test.sh 1` at the branch head. It
is red for #562's own panic (`ksoftirqd should have processed deferred
softirqs`), which this slice does not move and which section 3f of
`SLICE3-PLAN-2026-09-05.md` is what removes. Reported, not scored against this
branch.

**x86, on beast.** `08` through `12` are from the `breenix-x86` Incus container,
in this round's own clone `/root/breenix-slice3e` with
`BREENIX_GATE_TMP=/root/breenix-slice3e-tmp`, at the branch head. `10`, `11` and
`12` are the three build profiles' tails; the boot-tests build of the first pass
carried 1 warning on 2 of 2 kernel-lib compilations -- the oracle's queue
sentinel had 0 readers on that architecture -- which commit `fc302093` fixed by
scoping the constant to aarch64, and `10` is the re-run at that head.

| file | what it is |
|---|---|
| `01-strict-boot1-serial.txt` | strict gate boot 1 at `fc302093`, the green fixture 3 replay tests score |
| `02-prod-boot1-serial.txt` | production gate boot at `fc302093`, the second green fixture |
| `03-red-on-main-oracle-serial.txt` | `origin/main` `55689b42` plus the oracle probe: `verdict=FAIL` |
| `03b-red-on-main-oracle-probe.patch` | the patch that produced it, verbatim |
| `04-mutation-unconditional-move-serial.txt` | the guard's refusing answers turned into declines: `on_home=0`, `refused=3` |
| `05-mutation-guard-declines-serial.txt` | the guard declines at its first statement: `on_home=0`, `refused=0`, source ratchets green |
| `06-mutation-one-site-unguarded-serial.txt` | the guard call deleted at site 9: `reclaim=1 requeue=1 previous=0 on_home=2` |
| `07-testing-profile-red-562-serial.txt` | the testing-profile gate's red, #562's panic, report-only |
| `08-x86-prod-gate.txt` | x86 production-profile gate on beast: `PASS` |
| `09-x86-boot-tests-gate.txt` | x86 boot-tests gate on beast: `PASS`, `exited=110`, `stranded=0` |
| `10-x86-build-boot-tests.txt` | x86 build, `--features boot_tests,testing,external_test_bins`: exit 0, 0 warning/error lines |
| `11-x86-build-testing.txt` | x86 build, `--features testing,external_test_bins`: exit 0, 0 lines |
| `12-x86-build-zero-feature.txt` | x86 build, no features: exit 0, 0 lines |
| `13-strict-x20.txt` | the strict gate's own stdout: `PASS: 20/20 boots succeeded` |
| `14-prod-gate.txt` | the production gate's own stdout, with the 6-field census and the absent oracle |
| `15-r2-strict-x1.txt` | round 2's strict gate stdout at `9136c85e`: `PASS: 1/1 boots succeeded` |
| `16-r2-prod-gate.txt` | round 2's production gate stdout at `9136c85e`: `PASS` |
| `17-r2-strict-boot1-serial.txt` | the boot behind `15`, `BUILD_ID` `006a9ca4e213dd`: `verdict=PASS`, all-zero census |
| `18-r2-x86-boot-tests-gate.txt` | round 2's x86 boot-tests gate on beast: `PASS`, `exited=110`, `stranded=0` |
| `19-r2-x86-dispatch-no-alloc.txt` | `scripts/check-x86-dispatch-no-alloc.sh` at `9136c85e`: `PASS`, 3 symbols, 19 edges — the receipt round 1 left as "see below" |
| `20-r2-x86-build-boot-tests.txt` | round 2's x86 boot-tests build tail: exit 0, 0 warning/error lines |

**Round 2, `9136c85e`.** `15`, `16` and `17` are from this Mac; `18`, `19` and
`20` are from the `breenix-x86` container on beast, in round 2's own clone
`/root/breenix-slice3e` with `BREENIX_GATE_TMP=/root/breenix-slice3e-tmp`. `17`
carries `BUILD_ID` `006a9ca4e213dd` and `16`'s boot carries `006a9ca4fe0397`;
neither replaces `01` or `02`, which stay the fixtures the 3 replay tests score,
because this round changed 0 fields of the census line those tests compare.

**`01` re-recorded landing `capture/ftc-pr2-tick-sampling`.** Landing that
branch merged `docker/qemu/run-aarch64-boot-test-strict.sh`'s
`score_serial` with this slice's `PIN_GUARD_ORACLE` check, adding a
`RING_SPAN` sampling-ratio check the other branch's own tick-sampling PR
(`docs/planning/green-program/failure-capture/PR-2-2026-09-05.md`) contributed
-- the standing "widen the scorer, re-record the fixture" step. `01` did not
carry `RING_SPAN` (it predates that PR's kernel change), so the anti-vacuity
leg in `tests/loopback_pump_structure.rs::the_gates_score_the_pin_guard_oracle_in_opposite_directions`
and the green leg in
`tests/ttbr0_shadow_reconciliation_structure.rs::both_aarch64_gates_fail_on_an_untagged_publish`
would both score the unmodified `01` FAIL at the merged head
("Ring-span self-check marker missing"). `01` is replaced with a fresh strict
boot 1 at the merge commit, `BUILD_ID` `006a9cbfd61727`, carrying both
`[PIN_GUARD_ORACLE:aarch64:home=1:here=0:reclaim=1:requeue=1:previous=1:on_home=3:refused=3:census_clean=1:verdict=PASS]`
and `[RING_SPAN:cpu=0:span_ms=1498:writes=520:dropped=0:ticks_total=3979:tick_events=62]`
(ratio 64.2, well clear of `RING_SPAN_RATIO_FLOOR=10`); re-scored PASS via
`BREENIX_STRICT_SCORE_ONLY` against the merged gate, and both replay tests
above pass green against it. `02` is untouched -- the prod-profile scorer
requires both markers ABSENT, which the existing `02` already satisfies (0
occurrences of either), so no scorer requirement newly applies to it.
`03` (the red-on-main pin-guard capture) is also untouched: the tests' only
read of it splices its `PIN_GUARD_ORACLE` line into a copy of `01`, and does
not read a `RING_SPAN` line from `03` at all, so `03` does not need one.
