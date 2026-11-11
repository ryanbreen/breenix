# A.3 ARM reap-path continuation

The retained acceptance `20261111T154755Z-qemu-92251`, runtime `7dc1a45c`,
reports 80 passed and one failed case:

```text
SUITE window-manager CASE recovery/restart-adopts FAIL ms=3481 msg=reap owned process: no acknowledgment within 500 ms
```

This fails in `Child::stop` before replacement creation. The capture contains
PID 665's signal-exit line and ASID census, but no exit-phase timings. Those
lines precede zombie publication and cannot attribute the 500 ms interval.
The previous native result remains failed evidence, as recorded in
[a3-continuation.md](a3-continuation.md).

## Source defect and check-first repair

`sys_exit_aarch64` leaves the process TTBR0 and clears its return shadows before
calling `ProcessScheduler::handle_thread_exit`. The latter previously checked
whether a CPU still held the root. With no live root, it synchronously called
`release_process_leaves`, through `cleanup_cow_frames` to
`ProcessPageTable::release_mapped_leaves`, while holding PROCESS_MANAGER and before
`terminate_minimal` published the zombie. That walk audits the address space and
releases mapped frames without a per-pass budget. The compositor's exit could
therefore put address-space-sized work in front of its parent's wait result.
This is a code-derived latency defect; the old capture does not establish how
much time that particular walk consumed.

Check-first commit `c53289cc` adds `tests/arm_exit_deferral_test.py`. It compiles
the production receipt branch with `-Dwarnings`. A quiescent-root fixture with
32,768 modeled mapped leaves fails on the old branch: 32,768 leaves walked before
zombie publication, expected 0. The repair passes both quiescent and live-root
controls, preserving the leaf custody in one receipt and creating no second
receipt on repeated exit. This is an ordering/custody test, not a synthetic timing
measurement. It changes no check on main and needs no G2 correction.

ARM normal exit now hands current and superseded tables to the existing retirement
pipeline, whether or not the root is live. Its epoch, hardware, shadow, cached-root
and live-row proofs and bounded leaf/table drain are unchanged. The obsolete
forced-live injection and its guard are removed: the existing pairing fixture
now exercises the deferral directly, retaining its first/repeat and reclaim
assertions. x86 production exit behavior is unchanged.

Six ARM exit stages are retained in the existing bounded pipeline ring (17–22):
entry, windows released, before PM, zombie publication, after PM, GPU handles
released. Stage 20 binds the thread to its PID. A failed reap writes the kill
request, kill-return and failed-wait timestamps, process status and recent exit
stages through the existing evidence channel. Successful polls do not write these diagnostics.
The unchanged 500 ms assertion still decides the result. These diagnostics are
present in the production ARM build; they are not toggled between boots.

## Validation and acceptance scope

The new deterministic check fails before the repair and passes after it, one
behavior run per changed code state. The ARM production kernel and the scoped
suite-window-manager release build use four jobs under `taskpolicy -b`.
The kernel release build completes in 1m 46s with no compiler warnings or
errors; Cargo emits only G35(2)'s pinned-core future-incompatibility advisory.
The scoped suite release build completes in 29.27s without diagnostics.
The ARM FP/SIMD guard passes (0 forbidden loads/stores, 32 allowlisted).
The updated suite ELF is installed, the disposable ext2 disk is refreshed,
and the userspace manifest/source/disk check passes. No other suite runs.
`claim-lint --base HEAD` and `git diff --check` pass for the new hunks.

The new runtime branches and diagnostic producers are `cfg(aarch64)`; x86's
production receipt branch is unchanged. Removing the boot-test forcing guard
changes no production x86 code. No further R240 boot is required by coordinator
ledger line 154 for this correction. The prior native 80/81 is not a passing ARM
cell. The required new boot is one headless ARM64 HVF window-manager launch via
`boot-interactive.sh --mode suite --suite window-manager --gpu-gl --no-display`.
It must meet coordinator line 143's unchanged 81-case selection, memory, ordered
READY and ANGLE/Metal requirements. The result below remains failed acceptance.


## Single final-head result: 80/81, A.3 still blocked

Vigil `20261111T161134Z-qemu-52524`, runtime
`403f55016063bb8b961016dcbe847c29589bc040`, is a standalone suite launch on ARM64
QEMU, HVF, display false, watch false, dirty 0, exit 1. The
[producer excerpts](serials/a3-acceptance-403f5501.txt) retain the 81 CASE records
and the two supervisor process timelines. The raw serial was passed once to the
unchanged `suite-verdict.py` selection API, using exactly the categories whose
81 ids equal `docs/gui/gt710-step-21-7.json`. It refuses only this failed case:

```text
SUITE window-manager CASE recovery/supervisor-restarts FAIL ms=8857 msg=bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini
SUITE window-manager DONE passed=80 failed=1 skipped=0 total=81
SUITE window-manager MEMORY reclaimed=PASS free_before=84635 free_after=84625 allowance=48
SUITE_SEQUENCE READY window-manager
```

The generic verdict returns exactly the 12 planned-empty categories permitted
by coordinator line 143. The selection reports no malformed auxiliary record.
DONE is raw serial line 2700, MEMORY reclaimed line 2704, READY line 2710.
The launcher accepts ANGLE/Metal, 988 attestation records. Performance is 7/7;
`liveness/memory-bounded` also passes. Start uptime load averages are
4.44 / 9.76 / 9.98, end 5.15 / 9.88 / 10.36. The lane ran no builds, installers
or heavy work during the boot.

`recovery/restart-adopts` now passes, case total 6217 ms. That total includes
startup, content, kill and replacement; it is not the reap interval. The unchanged
500 ms reap check succeeds. No reap-failure diagnostics are emitted, so this run
does not measure how much the old synchronous walk cost. The deterministic
check establishes removal of that work from the pre-zombie path.

The sole remaining failure is the supervisor replacement's client-composite
wait. Original PID 723 reaches first composite at 3.719366 s. Replacement PID 736
has its own exec origin; it reaches five target allocations by 0.030214 s,
context ready at 1.051680 s, fonts-ready at 1.603704 s, chrome-ready at 1.872116 s,
and initial-windows-ready at 2.015147 s. Its frame pixels are ready at 2.057163 s;
there is no replacement checks-ready or first-composite stamp before failure.
Those are separate process timelines.

Compared with the previous replacement PID 731 at `7dc1a45c`:

| Phase | Previous passing replacement | This replacement |
| --- | ---: | ---: |
| Font planning interval | 425361 us | 895329 us |
| Font run.send interval | 38735 us | 674799 us |
| Fonts ready, since exec | 1124130 us | 1603704 us |
| Initial windows ready, since exec | 1143218 us | 2015147 us |
| First composite, since exec | 1311134 us | absent |

The source dependency is `bwm.rs`'s font worker preparing coverage, joined before
initial chrome and client composition. `CachedFont::run_mode` stamps planning
around lookup/rasterization, and `CachedFont::finish` stamps `run.send()` separately
from the subsequent fence wait (`libs/libgfx/src/ttf_font.rs`). The send interval
includes admission/submission work; these stamps do not identify shader compilation
or host contention as the cause. The capture does not isolate why that interval
grew, nor establish whether newly deferred reclamation overlapped it. That remains
source investigation under #2872, not an excuse to lengthen the 2 s bound or rerun.

A.3 is not proven. No second ARM acceptance, R240 boot, passing board cell or
merge follows this failure. The correction and check are pushed to
`arm-catchup/a-3` and `metal-arm-catchup/a-3`. PR #2874 remains draft and unmerged;
its description and #2872 are updated with this result. The R240's prior failed
80/81 remains recorded in the preceding continuation report.
