# #728 ext2 lock-discipline fix — FIX ROUND 2 prove-round evidence

Branch `fix/728-ext2-lock-discipline`, landed bytes at `85d08733` (6 commits
on top of the reviewed `59a6ce16`; base `origin/main` @ `9bfcf0b5`). This
directory is the prove slot's in-repo evidence for round 2. Full narrative:
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/p728fix/fix2-prove.md`
(scratchpad, not committed).

**The central claim this round had to settle: does the fix repair #728 on
x86, the arch the bug was reported on? Verdict: NOT SETTLED by the direct
oracle harness. See `x86-oracle/` below — this is reported honestly per
R52, not waved through.**

## Leg 1 — the repro oracle (aarch64-oracle/, x86-oracle/)

### aarch64: settled, both directions, twice

Single-hunk revert: `git checkout f5b987f6 -- kernel/src/fs/ext2/mod.rs`
on top of `85d08733`, plus a one-function shim (`ext2_lock_parks() -> 0`,
since pre-fix code never parks) so the current, round-2-hardened harness
(`ext2_lock_race.rs`, unchanged by the revert) still compiles and can
score the reverted lock code with its own B4 instrumentation.
`revert-scope-confirmation.txt` confirms `15a66716` (the fix commit)
touches only `mod.rs` (344 insertions/9 deletions), zero lines in
`ext2_lock_race.rs`, relative to its direct parent `f5b987f6`.

- **aarch64 RED** (`red-gate-stdout.txt`, `red-serial.txt`):
  `EXT2_LOCK_SPIN_STALL lock=ROOT_EXT2_write elapsed_ns=~5e8` ×3, gate
  FAILs with the exact reason. Reproduced fresh this round against
  round 2's own harness (not re-quoting round 1).
- **aarch64 GREEN** (`green-gate-stdout.txt`, `green-serial.txt`):
  `[LOCKRACE:COMPLETE:pass=2:fail=0]`, `parks=63/67` (130 total, one of
  two runs this round), both filesystems `verdict=PASS`, gate PASSES,
  liveness confirmed genuinely after `COMPLETE`. Reran twice this round;
  both green (63+67=130 and 67+67=134 total parks).

### x86: NOT settled — two extended attempts, zero oracle output either direction

This is the honest finding, not a hand-wave. Two independent attempts
this round, using a dedicated isolated beast clone
(`/root/breenix-728-prove` for the fix bytes, `/root/breenix-728-prove-red`
for a fresh single-hunk revert — see `single-hunk-revert-diff.txt`, which
reinstates exactly the B1 defect the review found and this round's fix
commit `dc4cb536` removed: the `interrupts_enabled()` conjunct on x86,
which review B1 proved is unconditionally false at every x86 syscall
site):

- **Attempt 1** (`attempt1-green-gate-stdout.txt`,
  `attempt1-red-gate-stdout.txt`): `X86_BOOT_TIMEOUT` default (1800s).
  Both GREEN and RED hit the gate's own poll-bound timeout with **zero**
  `LOCKRACE` lines of any kind — `ext2 lock-race gate (x86): FAIL - the
  leg never reached its COMPLETE marker (hang with no stall/lockup
  signal caught, or it never ran)`. Both reached the leg (holder/
  contender kthreads 1194/1195 spawned) and were actively scheduling
  (`Switching from thread 1194 to thread 1195` and back) — not wedged,
  just extremely slow.
- **Attempt 2** (`attempt2-green-serial-snapshot-14749L.txt`,
  `attempt2-red-serial-snapshot-14749L.txt`): relaunched with
  `X86_BOOT_TIMEOUT=5400 X86_POLL_BOUND=2700` (90-minute budget) to give
  materially more room than either this round's own first attempt or the
  fix round's ~25-minute investigation. Snapshot taken at ~24 minutes
  into this second attempt (14749 lines each, both still mid-leg,
  actively scheduling, **zero** `LOCKRACE` output in either). The
  process was left running in the background past this snapshot; this
  directory captures the state as of the snapshot, not a final verdict
  from that run (see the prove.md narrative for exact timestamps).

**What makes this attributable, not just "unlucky":** the physical beast
host showed sustained load average 16-25 throughout both attempts
(`uptime`, checked repeatedly), with the same standing non-breenix
tenant process observed in the fix round's own B2 investigation (now at
**7 weeks** of accumulated CPU time, ~870-960% CPU) still dominant.
Within the leg itself, both GREEN and RED advanced at an identical,
consistent ~1 kernel-log line per 10-13 seconds of wall clock — matching
the fix round's own B2 measurement almost exactly. **RED reached the same
zero-signal state as GREEN this round** — under round 1's conditions, x86
RED reliably produced a stall marker within about a second of reaching the
leg (see `../728-prove/x86-oracle/red-serial-all.txt`).

**Correction (closure round, review finding B2): the inference drawn here
originally — that RED not reddening under heavier host contention was
itself "strong evidence the non-completion is a property of today's
shared-host contention, not of which lock code is running" — did not
follow from what was measured.** This round's RED reinstates only the
`interrupts_enabled()` conjunct, and `ext2_lock_race.rs`'s holder/
contender kthreads run with IF=1 regardless (`kthread_entry()` calls
`arch_enable_interrupts()`, `task/kthread.rs:366-371` — see the B1 section
above), so that conjunct's reinstatement does not change what this
specific harness observes on x86 either way; a control that cannot
distinguish its two settings supports no conclusion about *why* neither
setting completes. A later round (`../728-x86-recapture/README.md`)
measured the leg's line-advance pace directly against a real host-load
swing (2.28 to 24.21) and found it flat throughout, which weakens rather
than confirms the host-contention explanation this paragraph originally
leaned on. The correct standing statement is Q2 of the review this
correction responds to: x86's non-completion is unattributed, not
attributed to host contention.

**Verdict, stated plainly per this round's own instruction: a capture
with zero oracle output is NOT green. x86 GREEN is UNPROVEN by the direct
oracle harness this round — not disproven, not confirmed. Reported as
unattributed for the harness leg specifically**, consistent with (not
contradicting) the fix round's own B2 disclosure. See Leg 2 below for
independent, real-syscall x86 evidence that does bear on the same
question with a different instrument.

## Leg 2 — historical repro (historical-repro/)

`./docker/qemu/run-boot-parallel.sh 5`, twice (10 boots total) on beast
(x86, `-smp 1 accel=tcg` — script default, matches
`docs/planning/green-program/nic-bus/serials/728-live-repro/`'s exact
config and exercises real `sys_mkdir`/`sys_open`/`sys_read` syscall-path
acquisitions, the shape B1 in review.md specifically named as the one
this harness's kthread-based race leg does *not* exercise), at landed
fix bytes (`85d08733`), in a dedicated clone (`/root/breenix-728-prove-hist`).

**10/10 boots: zero occurrences of the #728 stall shape** (no silent
post-`sys_mkdir` hang, no wedge). 8/10 PASS cleanly; 2/10 (boot 4 of each
5-boot batch) FAIL on an unrelated, pre-existing, already-filed flake —
`clonevm_exec_test` exits nonzero (`TEST_TALLY: exited=20 nonzero=1
failed=[/usr/local/test/bin/clon:1]`), the same family as open issues
**#610** ("post-exec rendezvous is racy... false red") and **#700**
("x86: clonevm_exec_test's post-exec futex timeout does not return
ETIMEDOUT"). Confirmed by direct inspection of the failing boot's serial
(`kernel::task::process_task: Process ... exited with code 1`, no
filesystem/lock activity nearby) — this is a clean nonzero exit, not a
hang, and touches no ext2/lock code path.

**Statistical power, stated honestly:** 10 boots is a modest sample
against a bug whose only prior occurrence was a single incidental
observation with unknown base rate. If the true pre-fix stall rate were
even as high as 20%, P(zero stalls in 10 boots) ≈ 11%, so this alone
does not conclusively rule out a low-probability residual. Combined with
the aarch64 oracle's deterministic proof that the same arch-neutral
`ext2_acquire`/`ext2_acquire_write` code path parks and resolves
correctly (Leg 1), and the structural ratchet (Leg 5), it is one
supporting leg among several, not a standalone proof.

## Leg 3 — aarch64 batteries (aarch64-batteries/)

- `full-test-run1-610flake.txt`: FAILED — `clonevm_exec_test` Phase 1c
  (`parent wait was not woken by sibling`), attributed to the same
  pre-existing **#610**, not #728's diff surface.
- `full-test-run2-clean-109of109.txt`: rerun, no rebuild — **109/109
  PASS**, confirming #610's intermittent classification.
- `service-seq-run1-contaminated.txt`: max profile **25/25 GREEN,
  UNATTRIBUTED=0** (clean, unaffected). cortex-a72 profile: boots 1-9
  clean, boots 10-25 (16 boots) **UNATTRIBUTED — "#596 inline-save
  resume-point oracle never armed"**. Root-caused, not hand-waved: this
  prove slot's own concurrent `cargo test` invocation for Leg 5 (below)
  silently rebuilt `target/aarch64-breenix-kernel/release/kernel-aarch64`
  **without** the `boot_tests` feature partway through this run — the
  gate script's own guard rail explicitly warns "any 'cargo test' in
  this session rebuilds the kernel WITHOUT boot_tests and silently swaps
  this binary in a fraction of a second" — and boot 10 is exactly where
  the swap lands in the timeline. A second, unrelated contributor: a
  sibling agent's own x86 QEMU process (`wf_5f1f17f2-3af-2`, visible in
  `ps aux`, ~29-33 CPU-minutes at ~100%) was running concurrently on the
  same Mac throughout. Self-inflicted-methodology disclosure, not a
  #728 defect.
- `service-seq-cortex-a72-clean-rerun-24of25.txt`: clean rerun (fresh
  `--rebuild`, nothing else running locally) — **24/25 GREEN,
  UNATTRIBUTED=0**, 1/25 attributed to pre-existing **#690**
  (`clonevm_exec_test stalled at 'second stage'`, same family as #610/
  #700 above), explicitly logged by the gate itself as "ATTRIBUTED and
  gate-failing exactly as the UNATTRIBUTED verdict it replaces". This is
  the authoritative cortex-a72 result.
- **Combined authoritative service-sequence result: 49/50 GREEN,
  UNATTRIBUTED=0 across both profiles** (max 25/25 + cortex-a72 clean
  rerun 24/25), the one non-GREEN boot attributed to pre-existing #690.

## Leg 4 — x86 on beast

Covered by Leg 1 (oracle, not settled) and Leg 2 (historical repro,
10/10 no stall). No dedicated `run-x86-gate.sh` known-signature sweep
was run this round (out of the time budget once the two extended
oracle attempts and the aarch64/structural legs were covered); carried
forward as an open item for whoever picks up #728 next, same as round 1
disclosed.

## Leg 5 — host structural suites (host-structural/)

All **29** pure-static (no-QEMU) structural test files under `tests/`
(the 28 the review's own family plus this round's new
`ext2_lock_structure.rs`), enumerated fresh from disk this round (not
re-quoting fix2-notes.md's count), run individually via one
`cargo test --release --test <name> ...` invocation:
`all-29-suites.txt`. **0 failures across all 29**, including
`ext2_lock_structure` itself — **17/17 tests pass**, 7 positive
properties + 10 mutation-proof negatives, each individually confirmed to
reject the mutation it targets (`negative_x86_regains_interrupts_enabled_is_rejected`,
`negative_aarch64_loses_interrupts_enabled_is_rejected`,
`negative_raw_read_regression_is_rejected`, etc. — this is the direct
structural ratchet against B1 regressing silently again), plus every
other blocking-primitive-adjacent suite (`exec_lock_order_structure`,
`preempt_bracket_structure`, `net_lock_structure`,
`block_request_lifetime_structure`, `signal_eintr_predicate_structure`).

## Bottom line

- **aarch64: fully proven, both directions, unregressed.** Oracle
  red/green clean and reproduced twice; 109/109 + 49/50 (one pre-existing,
  attributed, unrelated flake) batteries clean; 29/29 structural suites
  including the new mutation-proven C9 ratchet.
- **x86: the central question is not settled by the direct oracle this
  round**, despite two extended attempts (a full 30-minute run plus a
  90-minute run, both ending in zero oracle output on both the fixed and
  the freshly-reverted code, under independently-confirmed severe, standing
  beast host contention). Correction (closure round, review finding B2):
  this line originally went on to claim RED's non-reddening was itself
  evidence the cause was host-driven rather than code-driven — see the
  correction under "x86: NOT settled" above for why that inference does
  not hold; it is removed here rather than repeated. **What does bear on
  x86**, from a different instrument:
  10/10 real-syscall boots (`sys_mkdir` included, the historical repro's
  own shape) produced zero occurrences of the #728 stall at the fix
  bytes — modest statistical power, honestly stated, but a real,
  independent, non-synthetic data point in the fix's favor. The fix is
  **not proven on x86 by the oracle this round**, and reporting
  otherwise would repeat exactly the review-B2 mistake this round was
  asked to correct.
