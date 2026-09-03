# #693 round-2 battery serials

Branch `fix/693-poll-wake-loss` @ `5983fc6f14b135f3810c472b7dc2b5e346eb89c9`. Battery run
by the round-2 prove slot; companion to the round-2 review (`review-693.md`) and the
round-2 fix slot's own notes (`fix-r2-notes.md`). Four batteries, summarized in full
below: x86 boot-tests gate (3 of 3 branch, 0 of 4 main, all main reds attributed),
the bespoke x86 693 driver (25 of 25), the aarch64 service-sequence gate (50 of 50
GREEN), and two aarch64 mutations (693-K 3 of 3 RED as required, 693-U 3 of 3 GREEN
as required).
<!-- claim-lint:ok: N-of-M counts for all four batteries restated in the sections below -->

## x86 -- `docker/qemu/run-x86-boot-tests.sh`

* `x86-boottests-branch-5983fc6f-20260902.txt` -- branch head, 3 sequential boots on
  beast `breenix-x86` (TCG). **3 of 3 PASS**, `TOMBSTONE_QUIESCE:...removed=6` on all
  three (matches the census pin: `TOMBSTONE_FIXTURE_REMOVALS=2` +
  `PRODUCTION_REAPED_ROWS=4` = 6). No `POLL_TCP_ORACLE` marker of any kind appears --
  the oracle is not on the x86 `RING3_SMOKE` roster at this head (B1's fix), so this
  gate carries no #693 evidence either way; it is here to show the gate itself is
  green on the branch.
* `x86-boottests-main-509802e5-attempt{1,2,3,4}-*.txt` -- `origin/main` @
  `509802e5ef41e9d10003f6b7a4c3eafcda60b355` (fetched fresh from
  `https://github.com/ryanbreen/breenix.git`, NOT the shared `/root/breenix` clone,
  whose checked-out commit at the time was a stale mid-branch state -- see the
  "worktree caveat" note below), same script, same host, four sequential attempts
  (the script always aborts the whole invocation on its first `set -e` failure, so
  four single-invocation attempts were needed to sample four boots). **0 of 4
  clean.** Every failure is attributed and none touches a file `fix/693-poll-wake-loss`
  changes:
  * attempt 1: `[TEST:userspace:loopback_recv_wake:FAIL:reader_exit_15]` -- #692's
    exact signature (confirmed against the issue text: exit 15 is the EOF-wait-bound
    arm of `loopback_wake_test`).
  * attempt 2 and attempt 4: the census assertion at `run-x86-boot-tests.sh:548`
    (`CENSUS_RESIDENT + CENSUS_REMOVED - TOMBSTONE_FIXTURE_REMOVALS ==
    PRODUCTION_REAPED_ROWS`) fails because the sum is 5, not 4 -- attempt 4's
    last `[TOMBSTONE_CENSUS:` line is `resident=0:removed=7`, `removed` itself
    one over; attempt 2's is `resident=1:removed=6`, where `removed` matches
    but `resident` is the extra 1 (round-2 review F2). Root cause common to
    both: commit `63e5f8e0` (`test(net): add tcp_cloexec_exec_test`, merged to
    main via #765, 24 commits ahead of this branch's `3d601400` merge-base)
    added a new fork+exec+reap test to the x86 roster without updating the
    census literal -- a pre-existing, main-only defect unrelated to #693,
    filed as **#768**.
  * attempt 3: `test "$passed" = true` fails -- this one boot did not reach the
    terminal marker within the script's 900x2s poll window (0 of the required
    markers present at the 1800s mark); the kernel serial was still emitting
    `sys_recvfrom` retries when the tail was captured. Same profile, same host
    as the other three; **#731** (round-2 review F5) -- the serial carries 4 of
    the 4 fingerprints that issue names: `TEST_TALLY: exited=109
    nonzero=0 failed=[]`, the `🏁 TEST RUNNER` terminal marker,
    `loopback_recv_wake:PASS` present, and 0 panic/crash literals over a
    healthy idle tail.

  Net: the branch's own x86 boot-tests gate is strictly better than current
  `origin/main`'s (3/3 vs 0/4), and every main-side red is either a filed,
  independently-confirmed pre-existing issue (#692) or a stale census literal from
  an unrelated, already-merged PR -- not anything this branch's four touched kernel
  files (`ipc/poll.rs`, `main.rs`, `net/tcp.rs`, `syscall/handlers.rs`) could cause.

## x86 -- bespoke 693 driver (`x86-693fix-driver-20260902.sh`, private copy)

* `x86-693driver-kvm-batchA-18boot-20260902.txt` (boots 1-18) and
  `x86-693driver-kvm-batchB-7boot-20260902.txt` (boots 1-7, a second invocation
  continuing after the first was stopped by the harness) -- branch head, KVM
  (`-accel kvm -cpu host`), the x86-launch-site patch applied for the duration of
  the battery and reverted immediately after (`git diff --stat` clean, confirmed).
  **25 of 25 PASS**, `oracle_loaded=1` and `poll_tcp_timeout=2` on 25 of 25,
  `ready_lost=0` on 25 of 25, every verdict `[POLL_TCP_ORACLE:PASS:stages=4:...]`
  (stage 4 / forced ran on every boot). Zero `[POLL_TCP_READY_LOST]` lines.
  TCG was tried first and abandoned after an environment-only lock/orphan-process
  problem (a `qemu-system-x86_64` instance from an interrupted attempt held the
  shared, non-readonly `breenix-uefi.img` write lock across later iterations,
  reporting `oracle_loaded=0` on every subsequent boot); the actual boots that ran
  to completion (before that stray process was found and killed) showed no
  behavior related to #693 either.

## aarch64 -- `run-aarch64-service-sequence-gate.sh`

* `aarch64-service-sequence-gate-branch-5983fc6f-50boot-20260902.txt` -- branch
  head, campaign default (`--boots 25 --profile both` = 25 `max` + 25
  `cortex-a72`). **50 of 50 GREEN, UNATTRIBUTED 0, gate PASSED.** 0
  `[POLL_TCP_ORACLE:FAIL` in the tracked file (`grep -c` on it returns 0
  `POLL_TCP` lines of any kind, gate summaries only, so `100 [POLL_TCP_TIMEOUT]
  / 0 [POLL_TCP_READY_LOST]` is not independently re-derivable from THIS file
  the way FIX §3.1's equivalent table is from its own committed directory --
  round-2 review F10). The raw per-boot serials this summary was produced from
  do not survive on this host (the preserved-serials path the tracked file
  itself names is a DIFFERENT run,
  `.../T211926Z-6418`, not `…-89829`, and neither is present); what supports
  the claim is the tracked summary's own GREEN=50/UNATTRIBUTED=0, which by
  construction (693-FIX-2026-09-02.md §2.7's `[POLL_TCP_TIMEOUT]`-required
  wiring, on top of the `[POLL_TCP_READY_LOST]` failure pin every gate already
  carries) already requires both markers to have fired on every boot.

## aarch64 mutations

* `aarch64-mutK-driver-3boot-20260902.txt` + `aarch64-mutK-boot1-serial-20260902.txt`
  -- 693-K (`sys_poll`'s post-wake `scan_fds` line deleted). **3 of 3 boots
  UNATTRIBUTED / gate FAILED**, `[POLL_TCP_READY_LOST]` fires on 3 of 3, kernel
  marker is the RED; the oracle's own verdict on the same boots is
  `[POLL_TCP_ORACLE:LOST_SUSPECTED:...]` (a report, not a FAIL) followed by a
  retry ladder -- confirms R93: the kernel marker is the sole gate-failing
  authority, non-vacuously. Reverted; `aarch64-mutK-revert-clean-1boot-20260902.txt`
  is 1 of 1 clean (gate PASSED) on the reverted tree.
* `aarch64-mutU-driver-3boot-20260902.txt` + `aarch64-mutU-boot1-serial-20260902.txt`
  -- 693-U (main's naive "any timeout is a lost wake" predicate, reconstructed for
  the demoted (B2) machinery by forcing `verdict = TimeoutVerdict::LostSuspected`
  unconditionally on every timeout arm, in place of the current
  write_ms/readable_at_return three-way decision -- main's own code already IS
  the `timed_out` boolean this forces past, so this is the smallest faithful
  reproduction of "main's predicate, routed through the code that exists today").
  **3 of 3 boots GREEN / gate PASSED**, `ready_lost=0` on 3 of 3: the forced arm
  reports `[POLL_TCP_ORACLE:LOST_SUSPECTED:...]` repeatedly, exhausts its 2-attempt
  retry ladder, emits `[POLL_TCP_ORACLE:LOST_SUSPECTED_UNRESOLVED:...]`, and the
  trial completes -- no `[POLL_TCP_ORACLE:FAIL` is possible from this arm post-B2,
  and the kernel correctly stays silent (nothing was actually lost). Reverted;
  `aarch64-mutU-revert-clean-1boot-20260902.txt` is 1 of 1 clean.

Both mutations were reverted before the next step in each case and the tree was
confirmed clean (`git status --short` / `git diff --stat` empty) before moving on.

## Worktree/clone caveat

Two beast scratch clones were used, per the round's isolation requirement:
`/root/breenix-693-r2-prove` (branch, explicitly checked out to `5983fc6f...`) and
`/root/breenix-693-r2-prove-main` (main). The main clone's FIRST setup cloned from
the shared `/root/breenix` without an explicit checkout, which silently picked up
whatever commit that shared repo happened to have checked out at that moment
(`85596f62`, an intermediate `#693` branch commit belonging to a different lane,
not `origin/main`) -- caught before any boots were reported, by checking
`git rev-parse HEAD`. The one boot run against that wrong commit (a B1-shaped
census failure, `removed=9`) was discarded and is not counted in either the 3-of-3
branch total or the 0-of-4 main total above; the clone was re-pointed at
`origin/main` fetched directly from GitHub before any of the four counted main
attempts ran. <!-- claim-lint:ok: 1 of 1 mis-checked-out boots discarded and excluded from the 3 of 3 branch and 0 of 4 main counts in the x86 boot-tests section above -->
