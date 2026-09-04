# Aarch64 testing-profile — round-6 independent verification — 2026-09-04

Independent verification pass over `fix/562-761-aarch64-testing-profile`
round 6, run from a fresh worktree on this Mac. Scope: (1) a documentation
fix (R143), (2) confirmation of the round-6 review's three non-blocking
one-liners, (3) four boot batteries -- the aarch64 `testing` profile (T),
the strict boot-test gate (S), the production-profile gate (P), and a
5-boot `main` baseline for the #562 assertion (M).

**Partial-scope statement.** This round does not run the x86 gate, does
not re-derive the helper-census diff quoted in the round-6 FIX brief (it
was already re-run at `95de851e` per that round's own commits), and does
not attempt to fix the regression the S and P batteries surfaced -- that
needs the project's GDB-based kernel-debugging workflow against
`context_switch.rs` and the virtio-blk wait path, which is out of scope for
a verification pass. It is filed (#786) and evidenced here, not repaired.

## R143: what the one-recogniser ratchets cannot police

One sentence was added to each of three headers in `tests/teardown_structure.rs`:
the doc comment above `validate_one_call_recogniser` (which
`call_recognition_is_spelled_in_exactly_one_place` exercises), the header of
`validate_kthread_joins_are_kthread_bodies_or_tests`, and the header of
`validate_sleep_guards_consult_the_idle_refusal`. Each now says that a
rewrite of the test file introducing a second call recogniser -- one that
does not itself spell an open-parenthesis comparison, so
`validate_one_call_recogniser`'s own scan cannot see it -- or one that
quietly exempts a site by name, is outside what these ratchets can police:
that is a code-review question about the test file, not something any of
the three catches by running. This is Sol's N22 shape from the round-6
review, disclosed rather than left implicit.

No behavior change: `cargo test --test teardown_structure` at the R143
commit (`88199ba4`) reports `test result: ok. 103 passed; 0 failed; 0
ignored; 0 measured; 0 filtered out`, the same count as the round-6 head.
The edit is in `tests/teardown_structure.rs`, committed as `88199ba4` and
pushed to `origin/fix/562-761-aarch64-testing-profile`
(`git log --oneline -1 origin/fix/562-761-aarch64-testing-profile` after
the push shows `88199ba4`).

## The round-6 review's three non-blocking one-liners

3/3 arrived already marked CLOSED in this round's brief, with the
round-6 FIX commits (`0c8f137f`, `5da3d329`) as their evidence. Independent
corroboration this round, at the R143 head `88199ba4`:

* **N16** (the `kthread_join` census read the join itself, not just calls to
  it, across a comment) -- `cargo test --test teardown_structure
  the_kthread_join_census_reads_the_join_itself_written_across_a_comment --
  --exact` reports `test result: ok. 1 passed; 0 failed`.
* **N20** (the sleep-guard census read a predicate called across a comment)
  -- `cargo test --test teardown_structure
  the_sleep_guard_census_reads_a_predicate_called_across_a_comment --
  --exact` reports `test result: ok. 1 passed; 0 failed`.
* **N18** (batch serial counts match the committed catalog) -- this round's
  own T battery independently reproduces the claim from a from-scratch
  fixture: `userspace/programs/build.sh --arch aarch64` +
  `scripts/create_ext2_disk.sh --arch aarch64` in this worktree, then 25
  boots, print `Loaded 73/78 test binaries (0 failed, 5 not found)` in
  25/25 (`grep -lc "Loaded 73/78 test binaries (0 failed, 5 not found)"
  T-battery/boot*/serial.txt | wc -l` -> `25`), matching the round-6 FIX
  brief's own accounting of the fixture's musl gap.

Both cited tests are part of the 103/103 pass in the R143 run above, so this
is not a separate build. `taken = [N16, N20, N18]`, `left = []`.

## Battery T: aarch64 `testing` profile, 25 boots

Fresh build: `cargo build --release --features testing --target
aarch64-breenix-kernel.json -Z build-std=core,alloc -Z
build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
0 warnings/errors beyond the toolchain's `core v0.0.0` notice, then
`scripts/check-kernel-no-neon.sh` -> `PASS: 0 FP/SIMD load/store
instructions`. 25 boots, `-cpu max`, 3 QEMUs at a time, 45 s each, a fresh
copy of the fixture per boot. Raw serials committed under
`serials/prove/T-specimens/` (3 of the 25, see below); the full set of 25
lives only in the run's own temp directory (not committed -- 25 files at
~7.5k lines each was judged too large for this doc's evidence budget
relative to the 3 specimens plus the grep-reproducible summary below).

| Signal | Result | Command |
|---|---|---|
| Marker reached | 25/25 | `grep -lc "Test processes loaded - will run via timer interrupts" boot*/serial.txt \| wc -l` |
| `Loaded 73/78 (0 failed, 5 not found)` | 25/25 | `grep -lc "Loaded 73/78 test binaries (0 failed, 5 not found)" boot*/serial.txt \| wc -l` |
| musl tally (`BTRT_READY`) observed | 0/25 (expected) | `grep -lc "BTRT_READY" boot*/serial.txt \| wc -l` |
| Lockup (`SOFT LOCKUP DETECTED`) within the 45 s window | 23/25 | `grep -lc "SOFT LOCKUP DETECTED" boot*/serial.txt \| wc -l` |
| Of those 23, >=1 `EXT2_LOCK_SPIN_STALL` before the lockup line | 23/23 | per-boot `awk` comparison of stall line numbers against the lockup line number |
| Stall lines appearing after a lockup line | 0/25 | same comparison, reverse direction |
| `softirq_tests.rs:228` panics | 0/25 | `grep -lc "softirq_tests.rs:228" boot*/serial.txt \| wc -l` |

The 5 not-found names are the vendored musl C programs this from-scratch
worktree fixture cannot build (`hello_musl`, `env_musl_test`,
`uname_musl_test`, `rlimit_musl_test`, `identity_musl_test`) -- the same
disclosed narrowing the round-6 FIX brief itself names, reproduced fresh
here rather than restated.

**The 2 boots that did not lock up within the window (boot22, boot25).**
Neither is a red under the stated criterion (a lockup without the #728
signature, or any other red): neither locked up at all inside 45 s, and
neither shows a fault distinguishable from the 23 that did. `boot22`
(2519 lines vs ~7500-7700 for the 23 that locked up) was killed mid-boot
still running fork/exec tests, already showing 2
`EXT2_LOCK_SPIN_STALL lock=ROOT_EXT2_write` lines -- the same signature,
in progress, just not yet accumulated into a soft-lockup print. `boot25`
(2407 lines) shows 0 stall lines and was likewise still executing the test
catalog. Both contain routine `thread '<unnamed>' panicked at src/*_test.rs`
and `[DATA_ABORT]` lines from the userspace test suite's own deliberate
fault-injection tests (`fcntl_test.rs`, `nonblock_test.rs`, `pipe2_test.rs`
and similar) -- present in EVERY boot regardless of lockup status
(`for i in 1..25: grep -ac "panicked at" boot$i/serial.txt` ranges 0-4
across all 25, uncorrelated with the lockup column), so this is baseline
test-suite noise, not a distinguishing signal for the 2 non-lockup boots.
Committed as `serials/prove/T-specimens/T-boot22-no-lockup-in-window.txt`
and `T-boot25-no-lockup-in-window.txt`.

**T criterion: MET.** 25/25 marker + expected loaded line; every observed
lockup (23/23) carries the #728 signature; 0 unattributed lockups; 0
`softirq_tests.rs:228` panics.

## Battery S: strict boot-test gate, 25 boots — RED, UNATTRIBUTED, filed #786

`./docker/qemu/run-aarch64-boot-test-strict.sh 25` against a fresh
`--features boot_tests` build (0 warnings beyond the toolchain notice,
`check-kernel-no-neon.sh` PASS). **0/25 (0%) passed.**

| Failure class | Count | Example |
|---|---|---|
| `block_wedge_oracle` FAIL (3 distinct messages) | 12/25 | `[TEST:filesystem:block_wedge_oracle:FAIL:block wedge oracle second lock was not sleep-permitted]` |
| CPU exception -- the *same* `INSTRUCTION_ABORT` every time | 9/25 | `FAR=0x40004530 ELR=0x40004530 ESR=0x8200000e IFSC=0xe TTBR0=0x40200000 from_el0=1`, bit-identical in 9/9 |
| `census_widen_oracle` FAIL | 1/25 | `[TEST:process:census_widen_oracle:FAIL:census widening mutation oracle failed]` |
| Futex handoff oracle marker missing/failed | 1/25 | -- |
| Userspace not detected | 1/25 | 109/109 boot_tests suite completed (`BOOT_TESTS:PASS`) before the poll window closed on userspace liveness |
| Exec smoke did not complete | 1/25 | -- |

Full run log: `serials/prove/S-battery-run-log.txt`. All 25 failing serials:
`serials/prove/S-battery-all25/` (25 files). None of these six classes is
the pre-adjudicated set (#555 softirq, #576 EL1 NULL-PC `ELR=0,FAR=0`,
#626, #586 starved wake-loss, #609 network kthread) -- #576's own address is
zero; this branch's is a fixed, different, nonzero address.

**Controlled comparison against `main`, same host, same day.** A separate
worktree at `origin/main` @ `78179c56` (5 commits ahead of the round-6
merge-base, all x86-only `bus-x86-enum-gate` work per
`git log --oneline 78179c56 -5`), built from scratch the same way, passes
**15/15 (100%)** across two runs (`serials/prove/S-main-baseline-run-log.txt`,
the logged 12 of them; an earlier 3-boot check preceded the log and is not
separately committed). 0 of the six failure classes above reproduce on
main.

**S criterion: FAILED / UNATTRIBUTED.** 25/25 reds, none matching the
pre-adjudicated set, and a same-day same-host A/B against `main` (15/15
clean) rules out host contention as the explanation: the failures are
content-specific (a named oracle failing with a fixed message; a fault at
a byte-identical address 9/9 times), which is not what CPU-contention noise
looks like. Filed as
[#786](https://github.com/ryanbreen/breenix/issues/786), with the RCA
hypothesis that #761's relocation of CPU 0's boot sequence off a
preemption-pinned context and onto a schedulable, contended kernel thread
changed the timing assumptions `block_wedge_oracle` and neighboring
oracles depend on. Not fixed here -- see the partial-scope statement above.

## Battery P: production-profile gate, 25 boots — degraded, likely same root cause

25 sequential invocations of `./docker/qemu/run-aarch64-prod-profile-boot-test.sh`
(each invocation builds its own no-`--features` kernel and boots once).
**13/25 (52%) passed.**

| Failure class | Count |
|---|---|
| `FAIL: seam-absent timeout marker count must be exactly one` | 10/25 (9 of the 10 show 0 occurrences of the marker; 1, boot 8, shows it once but reading `probe=-3` instead of the expected `probe=-110`) |
| `FAIL: TTY oracle marker missing` | 1/25 |
| `FAIL: Poll TCP oracle marker missing` | 1/25 |

Full run log: `serials/prove/P-battery-run-log.txt`. All 25 serials:
`serials/prove/P-battery-all25/`. 0 crash markers in any of the 12
failures -- each is a boot that progressed (198-909 lines) without reaching
a marker the script's 120 s poll window expects.

**Controlled comparison against `main`, same host, same day.** The same
`main` @ `78179c56` worktree, 8 sequential invocations of the same script:
**8/8 (100%)** passed (`serials/prove/P-main-baseline-run-log.txt`,
`P-main-baseline/`).

**P criterion: DEGRADED, UNATTRIBUTED.** 12/25 reds, none a crash marker,
none matching the pre-adjudicated set (which is boot_tests/strict-gate
scoped and does not name a prod-profile signature), and a same-day
same-host A/B against `main` (8/8 clean) again rules out generic host
contention as the sole explanation. This is a softer, timeout-shaped
symptom than S's -- consistent with, but not independently proven to share,
S's root cause: CPU 0's boot-sequence relocation is not gated behind
`--features boot_tests`, so a timing change there would show up on the
production profile too, just as a missed deadline rather than a named
oracle failure. Filed as a follow-up comment on
[#786](https://github.com/ryanbreen/breenix/issues/786#issuecomment-5544203787)
rather than a new issue, pending whatever RCA #786 gets.

## Battery M: `main` @ `78179c56`, `--features testing`, 5 boots — confirms the pre-fix #562 defect

Fresh build in a separate worktree at `origin/main` @ `78179c56` (the
5-commits-ahead x86-only merge base), `--features testing`. 3+2 boots
(3 concurrent, then 2), 45 s each.

`kernel/src/task/softirq_tests.rs:228` at this commit is
`assert!(ksoftirqd_did_work, "ksoftirqd should have processed deferred
softirqs (tid={:?})", ksoftirqd_tid);` -- Test 7's daemon-identity check,
the exact assertion the #562 RCA describes as failing pre-fix (a different
line number, and a different assertion, than the round-6 head's own
`softirq_tests.rs:228`, which after the fix's refactor is Test 8's
`is_initialized` check; `awk 'NR==225,NR==233'` against each file shows the
divergence).

**4/5 boots panicked at that assertion within the 45 s window**
(`grep -a "softirq_tests.rs:228" boot{2,3,4,5}/serial.txt` each print
`panicked at kernel/src/task/softirq_tests.rs:228:5:` followed by
`ksoftirqd should have processed deferred softirqs (tid=Some(N))`).
`boot1` (232 lines, the shortest of the 5, part of the first 3-way
concurrent batch) was still completing SMP bring-up
(`[smp] 4 CPUs online` is its last line) when killed at 45 s -- it never
reached the softirq self-test at all, so it is a timing artifact of that
boot's slot in a 3-way batch, not a different outcome. Re-run alone
(no concurrent boots, same kernel, same fixture, 45 s), it panics at the
same assertion at 453 lines
(`serials/prove/M-battery/M-boot1-retry-panic.txt`). **5/5 confirmed** once
each boot gets an uncontended 45 s.

Committed: `serials/prove/M-battery/M-boot2-panic.txt` (a from-the-batch
specimen), `M-boot1-original-cutoff.txt` (the inconclusive original),
`M-boot1-retry-panic.txt` (the isolated re-run that resolves it).

## Overall verdict

R143 and the three one-liners are clean. The T battery meets its stated
criterion (25/25 marker+loaded, 23/23 lockups attributed to #728, 0
unattributed, 0 `softirq_tests.rs:228` panics). The M battery confirms
`main` @ `78179c56` still carries the pre-fix #562 defect this branch
repairs (5/5 panics given uncontended time).

**The S and P batteries are the news this round surfaces: this branch, as
of the R143 head `88199ba4`, fails the strict boot-test gate 0/25 and the
production-profile gate 13/25, against a same-day same-host `main` baseline
of 15/15 and 8/8 respectively.** Both are UNATTRIBUTED against the given
pre-adjudicated set. This is a blocking finding for the branch, not a
clean bill of health -- filed as
[#786](https://github.com/ryanbreen/breenix/issues/786) with full evidence,
not fixed in this round per the partial-scope statement above.
