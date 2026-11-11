# A.3 continuation: corrected ARM acceptance still fails 80/81

Later continuation: [ARM reap-path repair and final-head result](a3-reap.md).
The later boot passes restart-adopts but fails supervisor-restarts, still 80/81.

Latest runtime head `7dc1a45c5110d1dc82436ce34dcd605d64fdfbc0`, Vigil
`20261111T154755Z-qemu-92251`: **80 passed, 1 failed, 0 skipped, 81 total**.
A.3 remains blocked. The sole failure is the unchanged 500 ms reap deadline in
`recovery/restart-adopts`. The supervisor replacement reaches first composite at
1.311134 s, below the 1.6 s target. Performance is 7/7.
[Corrected producer records](serials/a3-acceptance-7dc1a45c.txt) retain the result
and both supervisor startup timelines; the complete raw capture remains in Vigil.
The sections below preserve the preceding regression and native result.

## First continuation acceptance

Runtime head `71c03a245b40c0babb2cb14b40804cf42ceaeb24`.
Vigil `20261111T145221Z-qemu-11150`, standalone window-manager, HVF,
headless SPICE GL (`--no-display`), clean checkout, exit 1.
The first continuation acceptance reports **67 passed, 14 failed, 0 skipped, 81 total**.
This regresses from the preceding 79/81; A.3 remains blocked and the PR must not merge.
[Retained producer results](serials/a3-acceptance-71c03a24.txt) include every CASE line.
The full, unfiltered serial and renderer attestation remain in the Vigil record.

## Source changes and their limits

Check commit `dcd2ec8f` precedes implementation `71c03a24`.
The corrected branch-owned preparation check follows G2 and
`docs/gui/gpu-abi.md:138-143,246-248` and `docs/gui/gpu.md` section 1.4.1:
context warm-up is asynchronous and a draw must not wait for unrelated pipelines.
The old preparation waits for the full catalogue even when SOLID and COPY are ready;
the new QEMU preparation waits for those two initial compositor pipelines, leaving
remaining compilation on the existing worker. The model fails on the old barrier
and passes with this change. This does **not** validate interactive warm-up scheduling:
the new full boot's input and initial-client failures remain unresolved.

The readback check forces a pending kill after a ready band. The old loop proceeds
through the entire readback without entering the wait's signal path. The repair
checks signals before finishing a lane and after reading it, before sending its
complement or successor. The existing error path abandons outstanding transfers,
retaining DMA holds until completion. The model fails on the old loop and passes
with the repair, including the outstanding-lane lifetime assertion.
`recovery/restart-adopts` passes in the new boot; the preceding capture lacks teardown
phase attribution, so that result alone does not establish which teardown stage caused
its previous 500 ms miss.

The supervisor case exports its existing startup stamps after success, but this boot
fails before the replacement is killed or started. No replacement margin is measured.
The earlier 3.173426 s stamp in capture `20261111T142945Z-qemu-31433` belongs to original
PID 716, not replacement PID 728. The latter reached fonts-ready at 2.053970 s and did
not report first-composite before failure. Earlier reports conflated those timelines.

## Exact failing cases

- `stacking/all-minimized`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `focus/keys-to-focused`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `focus/text-exact`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `focus/releases`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `focus/ctrl-c`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `focus/navigation-keys`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `focus/cycle-next`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `pointer/scroll-under-pointer`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `pointer/press-release-order`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `lifecycle/close-request`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `recovery/stack-restored`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `recovery/supervisor-restarts`: init desktop's initial client reached scanout: no acknowledgment within 2000 ms; windows=[(82, 0)]; last readback error=None; last completed=Some("context=342 s <!-- claim-lint:ok: docs/planning/arm64-catchup/serials/a3-acceptance-71c03a24.txt retains this literal CASE message; None is captured text. -->
- `liveness/hung-client`: positive click delivery to target client: positive click delivery to target client: no acknowledgment within 1000 ms
- `performance/frame-floor`: one client did not receive 60 completed composites per second

Twelve cases fail the positive-click wait. The supervisor case fails while initial
window 82 still has commit 0; its failure diagnostic retains completed context 342,
serial 487, clients `[(82, 0)]`. It does not exercise replacement startup.
Frame-floor reports aggregate 428 Hz and 2133 presentation intervals over
4974687750 ns, but fails its separate per-client 60-completions/s assertion.
Neither assertion nor any deadline changed.

## Remaining acceptance records

The retained producer file has 81/81 complete CASE records. MEMORY reclaimed=PASS, free pages 84635 to 84625,
allowance 48. Standalone READY follows DONE and MEMORY. The launcher accepts the
ANGLE/Metal attestation with 978 records. Performance typing, pointer, present,
idle CPU, animation latency and clipboard latency pass; frame-floor fails.

The selection is exactly the 81 ids in `docs/gui/gt710-step-21-7.json`.
The unchanged suite-verdict selection path was invoked once and refuses:

```
a SUITE line has none of the known shapes: SUITE window-manager HOSTPOINTER focus/ctrl-c seq=409 x=84 y=164 w
```

The generic scorer returns the exact 12 planned-empty category refusal permitted
by coordinator ledger line 143. That exception does not excuse either the malformed
HOSTPOINTER record or the failed cases. No serial reconstruction or filtering was
used to obtain a passing verdict. Issue #2411 tracks serial corruption, #2421 the
click acknowledgment path, and #2872 the recovery work; #2761/#2763 cover pacing.

## Validation and handoff

- The two scoped host checks compile with `-Dwarnings`: old-red/new-green, one
  behavior run per changed code state. No other suites were rerun.
- ARM production kernel: build completes with only
  G35(2)'s existing pinned-core future-incompatibility advisory. Kernel FP/SIMD guard passes.
- ARM suite-window-manager: scoped release build completes without diagnostics.
- The updated suite ELF was installed and the disposable disk rebuilt;
  userspace-manifest source/disk check succeeds.
- x86 UEFI artifact and changed suite: release builds complete without diagnostics.
- A background x86 build briefly overlapped late liveness and was interrupted before
  performance began; it resumed only after QEMU stopped. These host conditions are
  disclosed and this capture is not claimed as passing evidence.
- `claim-lint: scripts/claim-lint.py` -> exit 0 for 73 changed files, with 137
  pre-existing findings outside changed hunks.
- `claim-lint: scripts/claim-lint.py --base HEAD` -> exit 0.
- `claim-lint: scripts/claim-lint.py --commit-msg /tmp/a3-cont-check-msg.txt` -> exit 0.
- `claim-lint: scripts/claim-lint.py --commit-msg /tmp/a3-cont-fix-msg.txt` -> exit 0.
- `git diff --check` -> exit 0 before the runtime commit.

No repeat ARM boot, board update or merge followed this failed ARM acceptance.
The required native window-manager run is recorded below. The subsequent correction
is awaiting its one new ARM acceptance under coordinator ledger line 154. The ledger contains the
full blocker and prediction. Continue from source and retained captures; do not
repeat this boot to look for a better result.


## Native shared-path result

Native boot `20261111T150757Z-metal` at runtime and tools commit `71c03a24`
completed 80/81, exit 1. The selected suite-verdict path refuses the failing case:

```text
SUITE window-manager CASE recovery/stack-restored FAIL ms=6444 msg=bwm composite of client 81 commit 1: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [bwm
SUITE window-manager DONE passed=80 failed=1 skipped=0 total=81
SUITE window-manager MEMORY reclaimed=PASS free_before=2019292 free_after=2019280 allowance=48
SUITE_SEQUENCE READY window-manager
```

The remote build completed with exit 0 (userspace 523 s, kernel 150 s,
stub 23 s). The branch tools lock was released immediately after the run ended;
branch tools remain deployed. No persistent installation was requested. This is
failed shared-path evidence, not acceptance of the step or other native suites.

## Coordinator-directed correction

Coordinator ledger line 154 directs fixing or reverting the regression and one
new ARM acceptance. The partial-catalogue interpretation above was insufficient:
public CONTEXT_CREATE's asynchronous contract does not require admitting the
interactive compositor before its reserved catalogue has finished. Returning
after SOLID/COPY leaves 14 pipelines to warm in the event loop. `warm_next`
uses QEMU's eager policy, so this moves shader compilation into the same GPU
queue used by the first client interactions. The capture's twelve missed clicks
precede the lane's brief late-liveness build; host contention does not excuse them.

Check-first commit `2b953c82` models partial then complete warm-up and fails when
preparation returns at its second check instead of waiting for the third. The
source reversal `625936bc` restores full reserved-catalogue admission, retaining
`b27c7dc4`/`a29a067d` target overlap and the ready-band cancellation repair.

The UART failure is separate. Both retained prefixes are cut immediately before
an ordinary exit record, followed by replay of the entire HOSTPOINTER line.
`uart_owner::wait` permits stalled-owner revocation after 100 ms;
`uart_owner::write` replays a whole line but cannot remove the abandoned prefix.
`6e82505d` uses non-revoking acquisition for ordinary terminal output and normal
IRQ-enabled logs, with failed tries restoring the caller's interrupt state.
Raw and emergency output keep bounded takeover. Its deterministic stalled-owner
model fails on the old wire bytes and passes only for exactly the original
request followed by the exit line; production policy tests also keep masked
terminal fallback order and distinguish emergency output. These three correction
checks compile with -Dwarnings and pass after the source fixes.

The correction kernel builds without new compiler warnings, apart from G35(2)'s
existing pinned-core advisory, and its FP/SIMD guard passes. An additional
soft-lockup allocation guard fails with two allocating targets and five unresolved
formatting calls. A separate clean origin/main `60e692be` production kernel build
and the unchanged guard reproduce the same class through
`capture::emit -> Scheduler::try_liveness_snapshot -> panic_bounds_check -> panic
handler -> normal output -> scheduler allocation`. This is recorded under #2420;
neither its producer nor its guard was changed to obtain a pass. The comparison
logs remain in the task's temporary evidence directory.

`claim-lint: scripts/claim-lint.py --base HEAD` -> exit 0 for correction hunks.
Whole-file lint also reports 36 existing prose findings outside those hunks;
these are not chased here. The correction commit messages each passed
`--commit-msg`, and `git diff --check` passes. The lane performs no builds,
installers or other heavy work during the next acceptance and records its start
and end load averages, as line 154 requires.

## Corrected-head acceptance

The one correction boot runs standalone window-manager through
`boot-interactive.sh --mode suite --suite window-manager --gpu-gl --no-display --no-build`.
Vigil records ARM64 QEMU, display false, clean checkout, runtime `7dc1a45c`, exit 1.
The retained raw serial was scored once with the unchanged suite-verdict selection
path; its 81 manifest ids exactly match `docs/gui/gt710-step-21-7.json`.

```text
SUITE window-manager CASE recovery/restart-adopts FAIL ms=3481 msg=reap owned process: no acknowledgment within 500 ms
SUITE window-manager DONE passed=80 failed=1 skipped=0 total=81
SUITE window-manager MEMORY reclaimed=PASS free_before=84635 free_after=84625 allowance=48
SUITE_SEQUENCE READY window-manager
```

The selection refuses this failed CASE. It reports no malformed HOSTPOINTER
record. The generic verdict's refusal is exactly the 12 planned-empty categories
allowed by coordinator line 143; that exception does not excuse the failed CASE.
81/81 CASE lines are complete. DONE precedes MEMORY, which precedes standalone
READY. The launcher's renderer check accepts ANGLE/Metal with 968 records.

`recovery/restart-adopts` calls `content`, freezes its surviving client, then calls
`Child::stop` on the old compositor before constructing the replacement desktop.
The failing acknowledgment is `Child::status`/`waitpid(WNOHANG)` in that stop.
This capture therefore fails before replacement startup. It contains termination
output for old compositor PID 665, but that output does not establish completion of
reaping within 500 ms. The ready-band cancellation model exercises an interruptible
readback loop; it does not establish which remaining exit/teardown stage consumes
this deadline. That cause remains unresolved in #2872. No threshold was changed.

`recovery/supervisor-restarts` passes. Original PID 718 records first composite at
3.692832 s; replacement PID 731 records its own exec and first composite at
1.311134 s. The replacement's five targets are allocated by 0.026663 s, reserved
context is ready at 0.973158 s, fonts at 1.124130 s, and checks at 1.287849 s.
The target-allocation overlap is retained and the replacement has the requested
margin. These are separate process timelines, not the original compositor's stamp
attributed to its replacement.

Every focus, pointer and lifecycle case passes, including the twelve positive-click
failures in the preceding run. Frame-floor and the other six performance cases pass.
Start uptime loads were 6.46 / 6.39 / 9.03; end loads were 6.62 / 6.56 / 8.09.
The lane ran no builds, installers or other heavy work during this acceptance.

No repeat ARM acceptance follows this failure. The correction after the native
`71c03a24` run changes ARM-only UART modules and QEMU-only preparation, so coordinator
line 154 requires no further native boot. The native 80/81 remains failed evidence.
The ARM board was read and retains older entries; no passing pending cell was set.
PR #2874 stays draft and unmerged. The ledger and #2872 retain the concrete blocker;
#2411 retains serial ordering evidence and #2420 the separately reproduced main guard
failure. This report update changes no runtime code and requires no further boot.
