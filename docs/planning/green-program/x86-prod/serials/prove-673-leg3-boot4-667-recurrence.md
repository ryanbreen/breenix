# #673 prove-slot (fix round 2) leg 3 — a genuine #667 recurrence, full serial preserved

Branch: fix/673-x86-prod-init @ c3070765f106250b5760047e5062dec756007a1c
Captured: 2026-08-30, beast (`breenix-x86` Incus container), `docker/qemu/run-x86-boot-tests.sh 21`,
boot 4 of 21 (unmodified kernel, `boot_tests,testing,external_test_bins` profile).

## What happened

Boot 4 reached genuine, complete success: every userspace test exited 0
(`TEST_TALLY: exited=104 nonzero=0 failed=[]`), and the kernel printed
`🏁 TEST RUNNER: All tests passed - you can exit QEMU now 🏁`. The gate's own
poll loop (`docker/qemu/run-x86-boot-tests.sh:277-341`) requires that marker
**and** roughly 30 other markers as one big AND-chain before setting
`passed=true`. Three of them never appeared in the full 900-second poll
window, so the loop ran out the clock, `passed` stayed `false`, and the gate
aborted at `test "$passed" = true` (`run-x86-boot-tests.sh:378`) even though
the boot itself was healthy in every way the log shows.

The three missing markers, verified by direct grep against the full,
untruncated combined serial (both `serial_kernel.txt` and `serial_user.txt`,
19,063 lines total, checked one marker at a time against the exact patterns
in the gate script):

- `[TOMBSTONE_QUIESCE:` — never appears
- `[KSTACK_QUIESCE_LEAK:baseline_outstanding=...:outstanding=...:leaked=0]` — never appears
- `[RECLAIM_DRAIN:` — never appears

Every one of the other ~28 required markers (including the sibling
`[TOMBSTONE_CENSUS:...]`, which *did* print) is present and matches. After
`TEST RUNNER: All tests passed`, the kernel serial shows nothing but repeated
idle-loop churn (`Next thread from queue: 1, cpu: 0` / `Idle thread 1 is alone,
continuing (no switch needed)`) for the rest of the capture — a genuinely
silent, healthy-looking idle, not a crash, panic, or fault.

## This is issue #667, recurring — with the full capture #667 itself asked for

`gh issue view 667` — "x86: post-test settle-census stall — TOMBSTONE_QUIESCE/
RECLAIM_DRAIN never emit after all tests pass, kernel serial goes silent" — is
this exact shape. #667 was **closed as RETRACTED / not-confirmable**: its own
evidence didn't hold up (one preserved log turned out to be #636's signature,
another was a truncated 200-line tail, and a follow-up fresh capture showed
the markers printing normally). #667's own closing text says:

> Recommendation. If a genuine settle-census stall recurs, refile with a full
> (untruncated) serial capture; do not rely on the observations above.

This capture is exactly that: full and untruncated (both serial files copied
in their entirety, not tailed), with every one of the ~30 AND-chained markers
checked individually against the gate script's own patterns rather than
inferred from a truncated excerpt.

## Not a match for this round's pre-registered signature list

The prove brief's known-signature list was #630 (livelock), #700, #692,
#636-family. None match:

- **#630**: "USERSPACE TEST COMPLETE was absent... boot did not finish...
  last kernel line mid-syscall" — closed, tied to a specific pre-#609 commit
  on a different script (`run-boot-parallel.sh`), never reproduced in 30
  further control rounds. This boot's `USERSPACE TEST COMPLETE` line **is**
  present; the boot finished its work completely.
- **#700**: `clonevm_exec_test`'s post-exec futex timeout returns the wrong
  errno and the process exits nonzero. This boot's `TEST_TALLY` shows
  `nonzero=0` — no process failed.
- **#692**: `loopback_wake_test_child` fails under *concurrent* loopback-TCP
  load from a second simultaneous gate run. This battery ran strictly
  sequentially (one QEMU boot at a time); no concurrency was present.
- **#636-family**: `loopback_recv_wake` reader-EOF (`reader_exit_15`,
  ~9.5s stall then a FAIL tally). This boot's
  `[TEST:userspace:loopback_recv_wake:PASS]` is present and the boot never
  produced a tally at all (it never got that far in the poll chain) —
  different shape.

So per the "preserve any unattributed serial in-repo and report verbatim —
never rerun-to-green" instruction, this specific boot was originally reported
as **UNATTRIBUTED to this round's pre-registered list**, noting plainly that
it looked like the same defect class as the previously-retracted #667. That
report additionally claimed, in the filed #716 issue, that branch-causation
was excluded because "#673 touches `kernel/src/main.rs` ... and
`docker/qemu/run-x86-prod-profile-boot-test.sh` ... code #673 does not
touch." **That claim was false and has been corrected (#673 fix round 3,
review finding BB1).** `fix/673-x86-prod-init` also touches
`kernel/src/per_cpu.rs` (`can_schedule()`'s blocked-state admission),
`kernel/src/interrupts/context_switch.rs` — the file that contains
`idle_loop()` *and* the `x86_settled_tombstone_census()` call site whose
absence this issue is about — widening `check_need_resched_and_switch()`'s
blocked-state admission from four variants to six
(`is_blocked() || Terminated`) in every x86 profile including `boot_tests`,
plus `kernel/src/task/scheduler.rs`, `kernel/src/task/thread.rs`, and
`kernel/src/time_test.rs`. Those are exactly the knobs that decide when the
scheduler switches into idle — the mechanism this stall hinges on — so a
diff-only argument could never have excluded branch causation.

## Round 3: branch-causation excluded by measurement, not by the diff

Round 3 ran the identical `run-x86-boot-tests.sh` battery against unmodified
`origin/main` (`caaa264f`, no `fix/673-x86-prod-init` changes present at
all), 30 attempted boots total, and hit this exact signature (`TEST_TALLY`
clean + `TEST RUNNER: All tests passed` + zero `TOMBSTONE_QUIESCE:` /
`KSTACK_QUIESCE_LEAK:` / `RECLAIM_DRAIN:` occurrences + silent idle to the
900s poll bound) **2 times out of 30 attempted boots (~6.7%)**, both
verified against the exact signature by reading the full serial rather than
pattern-matching loosely. One of the two is preserved in full, in-repo,
alongside this file:
`docs/planning/green-program/x86-prod/serials/r3-baseline-main-caaa264f-boot1-716-recurrence-serial_kernel.txt`
(18,309 lines) and the matching `..._serial_user.txt` (899 lines); the
second occurrence (invocation 2, boot 25 of that battery) was verified by
the same full-serial method but not additionally duplicated in-repo to avoid
doubling an already-large preservation.

Compared against the rate this same signature hit **on the branch itself**
in round 2's own leg 3 (`fix/673-x86-prod-init` @ `c3070765`,
`run-x86-boot-tests.sh`, 20 attempted boots, 2 of them this shape — **~10%**,
serials preserved alongside this file as
`prove-673-leg3-boot4-667-recurrence-serial_kernel.txt` /
`..._serial_user.txt`), the two rates are comparable (6.7% on main, 10% on
the branch, both small-sample), which is what actually settles
branch-causation: this is a **pre-existing, baseline-measured stall**, not
one introduced by this branch's diff. Full baseline methodology and tally:
`fix2-prove.md`'s round-3 addendum and the round-3 baseline note
(`r3-baseline.md`, held with the round's other scratch notes).

This boot — and the one refiled as #716 — is therefore reported as
**ATTRIBUTED to #716 (pre-existing, confirmed via the round-3
`origin/main` baseline above)**, not UNATTRIBUTED. It is very likely the
same defect class as the previously-retracted #667, now with the full
evidence #667's own recommendation asked for, plus a baseline rate #667
itself never had.

## Rate context (this round only, not a new measurement campaign)

1 occurrence in 4 completed boots before the script aborted (this round's
own sequential, single-boot-at-a-time battery, no concurrency). The prove
slot did not run a dedicated rate-measurement campaign for this — a proper
rate estimate (the kind #667's RCA would need) is out of scope for a prove
pass; round 3's baseline above is the first such measurement, taken on
`origin/main` specifically to settle attribution, not as a full RCA.

## Filed forward

Re-opened/refiled as **#716** with this evidence attached, since #667's own
closing text explicitly asked for exactly this artifact if the stall
recurred. #716's own body has been corrected to match this file (#673 fix
round 3, BB1).
