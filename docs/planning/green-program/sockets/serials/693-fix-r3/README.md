# #693 round-3 battery serials

Branch `fix/693-poll-wake-loss` @ `23d9a10010357a457cdac230576409564fc2efc3` (round-2 review's minors-fix
commit; matches `origin/fix/693-poll-wake-loss` at dispatch time). Run by the
round-3 prove slot per the round-2 review's F1 disposition -- F1 (the blocker,
the second gate-failing userspace FAIL naming a lost wake, `late_woken_by_clock`)
was demoted in the same round the review found it; this slot re-runs the campaign
default aarch64 gate plus mutation 693-K against those demoted bytes. On 3 of
the 3 mutation-K boots below, `[POLL_TCP_READY_LOST]` (kernel) fails the gate
and 0 of the 3 boots' oracle output contains `[POLL_TCP_ORACLE:FAIL` -- R93
authority confirmed non-vacuously for the timeout arm (`lost_suspected`) via
693-K. The `woken_by_clock` demotion is a different claim: mutation 693-K
reaches only the timeout-return arm, not the POLLIN-return arm
`woken_by_clock_suspected` sits on, and no battery in this round or FIX
section 3 exercises that arm (FIX section 2.6 discloses this). Its demotion is
verified by construction instead -- 0 `fail("late_woken_by_clock"` call sites
and the literal itself absent from live source (section "Literal grep" below)
-- not by a battery.
<!-- claim-lint:ok: 3 of 3 mutation-K boots, 0 of 3 oracle FAILs -- see the
     Mutation 693-K section below and the committed driver/boot serials in
     this directory; the woken_by_clock arm's unexercised status is round-3
     review finding F2 -->

<!-- claim-lint:ok: N-of-M counts below restated verbatim from the committed
     driver/gate output files in this directory -->

## aarch64 service-sequence gate -- campaign default, both profiles

`aarch64-693fixr3-service-sequence-gate-branch-23d9a100-50boot-20260902.txt` --
`docker/qemu/run-aarch64-service-sequence-gate.sh --boots 25 --profile both`
(25 boots x 2 profiles = 50), unmutated branch head. **49 of 50 GREEN,
UNATTRIBUTED = 0.** The 1 red is `cortex-a72` boot 7:
`690 -- clonevm_exec_test stalled at 'second stage'...` -- the pre-existing,
open, ATTRIBUTED #690 defect (also the one red FIX doc section 5 discloses at
section 3.1's earlier bytes), not anything this branch touches. `profile max`
gate: 25/25 GREEN, PASSED. `profile cortex-a72` gate: 24/25 GREEN, 1x #690,
FAILED (by the gate's own #690-is-gate-failing design, matching the "ATTRIBUTED
and gate-failing exactly as the UNATTRIBUTED verdict it replaces" wording in the
script's own output).

Marker census over the 50 of 50 per-boot serials this run wrote to its
`OUTPUT_DIR` (the gate script's own stdout prints census tables, not raw
marker counts, so the counts below are `grep -c` totals over those 50 files,
not re-derivable from the committed gate-output file above alone):
<!-- claim-lint:ok: 50 of 50 per-boot serials grepped; counts in the table
     immediately below -->

| marker | count | expected |
|---|---|---|
| `[POLL_TCP_TIMEOUT]` | 100 | 2/boot x 50 boots |
| `[POLL_TCP_READY_LOST]` | 0 | 0 |
| `[POLL_TCP_ORACLE:FAIL` | 0 | 0 |

100/0/0 matches FIX section 3.1's earlier-bytes table exactly and is the same
reading M5/F10 of the round-2 review asked a later round to reproduce
independently: both markers present on every boot, no lost-wake FAIL of either
kind.

## Mutation 693-K -- `sys_poll`'s post-wake `scan_fds` re-scan deleted

Applied by hand at `kernel/src/syscall/handlers.rs:4156`
(`ready_count = scan_fds(&mut pollfds, &fd_snapshots);`, the blocking loop's
tail re-check, per FIX doc section 4's own description of the mutation) --
no committed apply/revert script exists for 693-K (unlike the x86 launch-site
patch), so this describes the one-line hand edit directly. Built, ran, reverted;
`git status`/`git diff` clean on `kernel/` at the end.

`aarch64-mutK-driver-3boot-20260903.txt` --
`run-aarch64-service-sequence-gate.sh --boots 3 --profile max` on the mutated
kernel. **3 of 3 boots UNATTRIBUTED, gate FAILED, `[POLL_TCP_READY_LOST]` fires
on 3 of 3** (`fd=4 timeout_ms=5000 publish_after_entry_us=8{1,7}xxx
before_deadline_us=49128xx rx_len=23 revents=0x0000`). Exit code 1.

`aarch64-mutK-boot1-serial-20260903.txt` -- boot 1's full serial. The kernel
marker (`:724`) and the oracle's own read of the same event
(`:725-726`, `[POLL_TCP_ORACLE:LOSTWAKE_PROBE:...rescan_ready=1
rescan_revents=0x0001 nbread_n=23]` then `:729-730`,
`[POLL_TCP_ORACLE:LOST_SUSPECTED:...attempt=1...]`) sit a few lines apart: the
kernel carries the FAIL, the oracle reports and does not fail. Confirmed across
all 3 boots: `grep -c '\[POLL_TCP_ORACLE:FAIL'` over the 3 preserved serials is
0 (0/0/0); `grep -c '\[POLL_TCP_READY_LOST\]'` is 1/1/1 (3 total);
`grep -c '\[POLL_TCP_ORACLE:LOST_SUSPECTED'` is 2/2/2 (6 total), and every one
of those 6 lines is `attempt=1` -- each `LOST_SUSPECTED` decision is printed
as an identical duplicate pair (the same standing pattern the round-2 evidence
shows at `693-fix-r2/aarch64-mutK-boot1-serial-20260902.txt:703-704`, also two
identical `attempt=1` lines), not two retry attempts; the boot's own trial
resolved on the first decision and the gate's per-boot classification already
fired on the kernel marker by then.

This is R93 exercised non-vacuously against the round-2-demoted bytes, by a
slot that did not write the demotion: on 3 of 3 boots the kernel-side detector
is what fails the gate, and on 0 of those 3 boots does the userspace detector
-- running the same demoted code the round-2 review asked a later round to
exercise -- emit anything but a report.

`aarch64-mutK-revert-clean-1boot-20260903.txt` -- 1 boot on the reverted
(rebuilt, zero-warning) kernel. **1 of 1 GREEN, gate PASSED.** Confirms the
mutation, not some other state, produced the 3 reds above.

## Literal grep -- `late_woken_by_clock`

`grep -rn "late_woken_by_clock" .` (whole tree, excluding only `target/` and
`.git/`; this section's own directory is excluded from the count below since
this README is written after the grep and would otherwise report on itself) at
branch head `23d9a10010357a457cdac230576409564fc2efc3` returns **21 hits across 9 files**, in three
groups:
<!-- claim-lint:ok: re-run at round-4 fix time as
     `grep -rn "late_woken_by_clock" --exclude-dir=target --exclude-dir=.git . | grep -v 693-fix-r3 | wc -l` = 21
     and `| cut -d: -f1 | sort -u | wc -l` = 9; round-3 review finding F3 -->

**Prose (11 of the 21 hits, 2 files) -- describes the arm's history, 0 of the
11 is code.**
* `docs/planning/green-program/sockets/693-FIX-2026-09-02.md` -- 7 of the 11,
  at `:164`, `:242`, `:281`, `:449`, `:457`, `:461`, `:462`. 7 of 7 are in
  sections 2.6, 3 and 5, describing the pre-demotion arm, the round-2
  demotion, or citing the preserved specimen file below by its own name.
* `docs/planning/green-program/sockets/693-RCA-2026-09-02.md` -- 4 of the 11,
  at `:42`, `:66`, `:179`, `:332`. The original RCA's table column and prose,
  describing what the arm *was* before the demotion this RCA itself
  recommended reversing (F8 of the round-2 review).

**Preserved pre-fix specimen serials (9 of the 21 hits, 6 files) -- historical
captured output, not live code.**
`docs/planning/green-program/sockets/serials/693-rca/` holds raw serial/driver
captures from the pre-fix x86 boots that originally produced this arm's
`FAIL`, predating both the fix and the round-2 demotion, 2 hits each in
`docs/planning/green-program/sockets/serials/693-rca/x86-693rca-tcg3-boot18-late_woken_by_clock-user-20260902.txt`
(the same specimen FIX section 5 cites),
`docs/planning/green-program/sockets/serials/693-rca/x86-693rca-tcg-boot8-late_woken_by_clock-user-20260902.txt`
and
`docs/planning/green-program/sockets/serials/693-rca/x86-693rca-kvm2-boot46-late_woken_by_clock-user-20260902.txt`
(6 of the 9), plus 1 hit each in
`docs/planning/green-program/sockets/serials/693-rca/x86-693rca-tcg3-driver-output-20260902.txt`,
`docs/planning/green-program/sockets/serials/693-rca/x86-693rca-tcg-driver-output-20260902.txt`
and
`docs/planning/green-program/sockets/serials/693-rca/x86-693rca-kvm2-driver-output-20260902.txt`
(the remaining 3 of 9). 9 of these 9 hits are inside a literal
`[POLL_TCP_ORACLE:FAIL:late_woken_by_clock:...]` line -- but each is a
**captured boot transcript from before this branch's fix existed**, preserved
as evidence per this arc's own preserve-failing-serials rule, not a line any
build on this branch's HEAD can produce: the pre-fix oracle source that
emitted it is not what `userspace/programs/src/poll_tcp_oracle.rs` compiles
today.

**Live source (1 hit, 1 file) -- a doc comment, not a `fail()` call.**
`userspace/programs/src/poll_tcp_oracle.rs:227` cites the specimen's filename
above inside the doc comment on `LATE_MAX_WOKEN_BY_CLOCK_ATTEMPTS`, the bound
governing the demoted arm's retry ladder. Confirmed
separately: `grep -n 'fail("late_woken_by_clock"' userspace/programs/src/poll_tcp_oracle.rs`
returns 0 lines, and `grep -n "woken_by_clock" userspace/programs/src/poll_tcp_oracle.rs`
shows the only *other* live-code sites are the `woken_by_clock_seen` counter and
the `LATE_MAX_WOKEN_BY_CLOCK_ATTEMPTS` bound -- the renamed, demoted arm. No
hit anywhere in the tree's `.rs` files is inside a `fail()` call.
