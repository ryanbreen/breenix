# A.3 animation latency: retained capture and stop

The ARM window-manager run `20261112T070425Z-qemu-31990` at `6dc20681`
remains failed: 80/81 cases, with animation input sample 39 taking
110.545333 ms against 100 ms. Its margin is **-10.545333 ms**. This analysis
narrows the delay but does not establish its mechanism or claim a latency fix.
Coordinator ledger lines 266 and 273 require a ruling after a failed run;
line 295 records the remaining question. No new boot is run in this unit.

## Measured path

The retained serial's first-slow input is `763619704375` ns. Window 127's
input serials 2687/2688 are delivered together; its next handling commit is
433 with processed watermark 2688. Composite 11716 presents that commit at
`763730249708` ns. The case uses that presentation timestamp, rather than
the later receipt-publication stamp. See `input_latency` in
`userspace/programs/src/wm_suite/cases.rs` and `paint` in
`userspace/programs/src/wm_client.rs`.

The [retained excerpt](serials/a3-animation-6dc20681.txt) prefixes records
with their original serial line numbers. Times below are milliseconds since
the injected input; durations subtract adjacent stamps, without rounding
intermediate values.

The original `serial.txt` has SHA-256
`3e96d1254031a73ae51610c9f75dc2f81137f126c031b9de6fe115de6400c8fa`.

| Boundary | Since input (ms) | Previous interval (ms) |
| --- | ---: | ---: |
| Device publication | 0.020791 | 0.020791 |
| Compositor input read | 0.931291 | 0.910500 |
| Window event delivery | 0.942166 | 0.010875 |
| Client event read | 16.969416 | 16.027250 |
| Client ACQUIRE begins | 16.971041 | 0.001625 |
| Client ACQUIRE ends | 88.869791 | **71.898750** |
| Client COMMIT begins | 103.652708 | 14.782917 |
| Client COMMIT ends | 103.690166 | 0.037458 |
| Scene admits commit 433 | 103.923083 | 0.232917 |
| Render completes | 109.850750 | 5.927667 |
| Presentation | 110.545333 | 0.694583 |
| Receipt publication | 110.550583 | 0.005250 |

The deadline has already expired when the client begins COMMIT. Its acquire
interval accounts for 65.0% of the measured latency. During it, composites
11711 through 11715 repeatedly present the previous commit, 432. This rules
out attributing the entire failed latency to a post-commit compositor stall;
it does not identify why acquire takes that long.

## What the code can and cannot decide

`kernel/src/window/ops.rs::acquire` places pipeline stages 5 and 6 around
the whole operation. Between them it can wait for pending publication or a
free buffer, replay the latest front, unseal the target, and change tree-page
protection. `replay` splits each damage rectangle into two half-copies.
`kernel/src/gpu/window_targets.rs::copy_from` waits for source readiness;
`execute` attaches the source, waits for context/resource dependencies, then
submits the copy and awaits completion. The virtual backend uses blocking
receipt waits. These are distinct possible contributors to the same 71.899 ms
bracket. Time spent runnable or descheduled can also stretch that bracket.

The capture does not separate those contributors:

- Accounting window 84 covers 759.499442–759.840633 s, ending before the
  slow input at 763.619704 s. Window 85 starts at 778.666114 s. The startup
  accounting therefore cannot identify this client's wait or CPU time.
- There are no exported GPU-core-command rows with submit timestamps in
  763.5–763.85 s. The per-case archive covers named recovery/click cases,
  rather than this animation case. The rolling command tail exported later
  does not recover the missing interval.
- The retained host sampling files cover recovery and click cases, rather
  than animation. The host serial sidecar records line receipt, not guest
  event time: the four first-slow lines share a receipt timestamp. This is
  not an exact host/guest clock anchor for the preceding acquire.

Host contention remains a possibility, not an attribution. There is no
forcing test for a specific latency mechanism yet: choosing a buffer wait,
GPU replay delay or scheduler delay as the fixture would assume the missing
answer. No threshold, expectation or case selection is changed.

## Other retained margins

These are measurements from the failed run, not acceptance at a new head.
Repeated exports of a wait are deduplicated by thread ID and begin time.

| Measurement | Observed | Bound | Margin |
| --- | ---: | ---: | ---: |
| Animation maximum | 110.545333 ms | 100 ms | **-10.545333 ms** |
| Animation median | 12.703 ms | 33 ms | 20.297 ms |
| Typing maximum / median | 16.364 / 10.452 ms | 50 / 20 ms | 33.636 / 9.548 ms |
| Pointer maximum / median | 12.528 / 4.961 ms | 50 / 20 ms | 37.472 / 15.039 ms |
| Present maximum / median | 4.533 / 1.391 ms | 33 / 16 ms | 28.467 / 14.609 ms |
| Clipboard maximum | 6.898 ms | 50 ms | 43.102 ms |
| Slowest of three recovery reaps | 5.420667 ms | 500 ms | 494.579333 ms |
| Supervisor replacement detection | 1107.749042 ms | 2000 ms | 892.250958 ms |
| Replacement exec to first composite | 451.513 ms | 2000 ms | 1548.487 ms |
| Slowest exported positive-click wait | 59.649125 ms | 1000 ms | 940.350875 ms |
| Slowest exported 1 s control wait (Alt+Tab) | 414.698083 ms | 1000 ms | 585.301917 ms |
| Frame floor | 546 Hz | minimum 60 Hz | 486 Hz above minimum |

The two other reaps are 5.333500 and 5.418583 ms. MEMORY is PASS,
84631 to 84621 free pages, retaining 10 of 48 allowed pages. READY follows
DONE and MEMORY, and the retained renderer report accepts ANGLE/Metal.
Those results do not override the failed animation case. No board cell is
updated, no native boot follows, and PR #2874 remains unmerged.

## Integration correction and validation

Fresh main `b26fbda8` assigns diagnostic stages 17–46 to window destruction,
metadata and frame records. This branch previously used 17–22 for exit
boundaries and 23 for destruction, colliding with those assignments.
Check-first commit `01062924` corrects this branch's earlier export check
under G2, against main's `docs/gui/breengel.md` section 17.1. Interleaved
metadata and frame rows impersonate the reaped process; the old production
exporter reports those unrelated rows and misses the intended exit records.

The merge resolution keeps main's stage assignments, uses 47–52 for the
six exit boundaries, and updates their consumer. The numeric labels change;
the exit operations, their order and the record count do not. This corrects
diagnostic identity, not the measured animation failure. Historical capture
excerpts retain their original stage IDs.

The scoped export checks fail first (2 failures of 4), then pass (4/4).
The merged frame diagnostic checks pass (2/2). Their Rust fixtures compile
with `-Dwarnings`. Logs are `/tmp/a3-animation-export-{red,green}.log` and
`/tmp/a3-animation-frame-check.log`.

With four build jobs under background scheduling, ARM production kernel,
x86 testing kernel/launcher, and ARM/x86 scoped `suite-window-manager`
release builds exit 0 in 67 s, 119 s, 42.04 s and 28.25 s respectively.
The ARM kernel repeats the pre-existing pinned-core future-compatibility
warning; the other three logs contain no warning or error lines. There are
no source warnings in those four builds. This is not a warning-free Cargo
transcript for the ARM kernel. Logs are `/tmp/a3-animation-arm-build.log`,
`/tmp/a3-animation-x86-build.log`, `/tmp/a3-animation-arm-suite.log` and
`/tmp/a3-animation-x86-suite.log`.
<!-- claim-lint:ok: the four named local logs retain the compiler output and final release results. -->

Claim-lint invocations: `--files tests/a3_measurement_export_test.py` exits 0;
`--commit-msg /tmp/a3-animation-check-msg.txt` exits 0; the default changed-hunk
run exits 0 (211 files, base 032a81b1; 10 unsupported-extension targets skipped);
`--commit-msg /tmp/a3-animation-merge-msg.txt` exits 0. `git diff --check`
exits 0. Report, PR-body and report-commit lint results are recorded in the
ledger after their final edits. Tests are not repeated after committing.

## Blocked question

Subject to a coordinator ruling, one instrumented run would ask: **which
acquire subphase and dependency consumes the 71.899 ms, and is the client
blocked on that dependency or runnable but off CPU?** Bounded numeric
acquire-phase stamps, dependency/request identities and per-thread CPU/wait
intervals must survive the first slow input and join the existing host trace.
The prediction distinguishes a named dependency wait from runnable/off-CPU
time before a fix is chosen. This is a measurement proposal, not boot
authorization. Coordinator stop lines still govern.
