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


## User-directed acceptance at b26abb03: 79/81

The user explicitly directed acceptance before blind review, without another ruling
or prior review. This run used clean `b26abb03752f2f0876bbb2110cc267917343e913`;
fresh `origin/main` was already an ancestor. The mechanism investigation and u11
added no runtime repair after `403f5501`. This acceptance does not establish that
ledger line 167's unresolved mechanisms were fixed.

Vigil `20261111T165116Z-qemu-68510` records ARM64 QEMU, standalone window-manager,
watch false, display false, dirty 0 and launcher exit 1. The launcher command was
`boot-interactive.sh --mode suite --suite window-manager --gpu-gl --no-display --no-build`.
It used HVF and headless SPICE GL, without a debugger. The prediction precedes the
boot in the ARM ledger. Start uptime loads were 5.48 / 6.32 / 10.23; end loads were
7.77 / 6.94 / 8.79. No lane builds, installers or heavy work ran during the boot.

[Producer records](serials/a3-acceptance-b26abb03.txt) retain 81/81 CASE lines,
startup stamps and performance values as JSON-encoded source lines, preserving
coalesced diagnostics. The full raw serial remains in Vigil; its SHA-256 is
`22c35eb03f702b83c2c36cd2e1adb51b9f2edba327ef2353ea47d2b123b31358`.
The unchanged suite-verdict selection API was invoked once against that raw file;
the selected manifest ids exactly equal the 81 ids of `gt710-step-21-7.json`.
It refuses these two failed cases:

```text
SUITE window-manager CASE recovery/supervisor-restarts FAIL ms=9639 msg=bwm composite of client 83 commit 4: no acknowledgment within 2000 ms; last readback error=Some("SCANOUT_READBACK: EAGAIN, reason 0"); last completed=None; [ini
SUITE window-manager CASE performance/frame-floor FAIL ms=8951 msg=one client did not receive 60 completed composites per second
```

All 81 CASE records are complete. DONE is line 3026 (79 passed, 2 failed, 0 skipped),
MEMORY reclaimed is line 3030 (84635 to 84625 pages, PASS with allowance 48), and
standalone READY is line 3036. The generic scorer refuses exactly the 12 permitted
planned-empty categories, with no additional selection refusal. The launcher
accepts ANGLE/Metal with 983 attestation records. Those conditions do not excuse
the two failed cases. Performance is 6/7, including passing typing, pointer,
present, idle CPU, animation latency and clipboard latency.

### Recovery deadlines and source interpretation

| Case | Deadline | Measured evidence | Headroom acceptance |
| --- | ---: | --- | --- |
| restart-adopts | 500 ms | PASS, case total 6370 ms; no exported kill, reap-return or exit-stage records | Unresolved: actual reap interval and the required 125 ms headroom cannot be calculated |
| supervisor-restarts | 2000 ms | Client 83 commit 4 wait fails; replacement PID 728 reaches frame-buffers-ready at 2558.114 ms since exec, with no first-composite stamp | Failed; no passing margin or required 500 ms headroom established |

The source of the reap evidence gap is unchanged: `Child::stop` in
`wm_suite/desktop.rs` exports requested/kill-return times and exit stages only
inside its error handler. A passing total is not a reap interval. No synthetic
margin is inferred from its PASS, especially because `until_from` evaluates its
predicate before the deadline check.

The replacement and original timelines are distinct. Original PID 717 reaches
first composite at 3419.482 ms. Replacement PID 728 records:

| Phase | Milliseconds since replacement exec |
| --- | ---: |
| Five target allocations complete | 307.605 |
| Context ready | 1333.583 |
| Background ready | 1641.331 |
| Font planning begins | 2483.322 |
| Font planning returns | 2490.609 |
| Font submission returns | 2521.932 |
| Fonts ready | 2523.903 |
| Chrome ready | 2531.132 |
| Initial windows ready | 2552.984 |
| Initial frame check begins | 2554.115 |
| Frame buffers ready | 2558.114 |
| Checks ready / first composite | absent |

`bwm.rs` joins the font worker before chrome and the initial whole-frame check;
that check precedes initial client composition. The interval from startup-font-begin
at 7.762 ms to font-plan-begin is 2475.560 ms. That interval includes config loading,
font reading/parsing, cache creation, logging and scheduling before coverage
planning. These stamps do not isolate those costs. This capture's slow interval
is before planning, rather than the previous failure's long font-submit interval.
The EAGAIN readback gate in `kernel/src/syscall/window.rs` still combines busy and
claim-baseline refusal; GPU fallback is another possible source, as the mechanism
report explains. These diagnostics do not establish a unique causal refusal or demonstrate
a mechanism repair. Exact wait-start and
first-composite times are not exported. The observed exec-to-phase values above
are not substituted for the 2000 ms wait measurement.

### Frame-floor failure

The producer reports window 120, measurement bounds 392042417416 to 397042485000 ns,
1635 presentation intervals, span 4933235000 ns, aggregate 331 Hz and maximum
internal gap 8462291 ns. `wm_suite/rate.rs::measure` also requires the first and
last receipts to be within one 60 Hz period of the measurement endpoints. The
window lasts 5000067584 ns, so the combined endpoint gaps are 66832584 ns. Their
sum exceeds two periods (33333333.333 ns), so at least one endpoint violates
the unchanged bound even though the interior aggregate rate passes. The capture
does not export the two endpoint gaps individually. This is failed frame-floor
evidence, not grounds to weaken or remove its endpoint checks. #2763 tracks ARM
pacing parity; #2872 retains the recovery issue.

This acceptance is failed and A.3 remains unresolved. No passing pending board
cell is set. There is no repeat ARM boot. u11 and this acceptance change no
runtime code, so the brief's conditional new R240 window-manager run is not
triggered; the earlier native `20261111T150757Z-metal` 80/81 remains failed evidence.
No R240 access, tool deployment or persistent install occurs in this acceptance.
The report/excerpt update requires no runtime rebuild or suite rerun. PR #2874
remains draft and unmerged for the subsequent review.
