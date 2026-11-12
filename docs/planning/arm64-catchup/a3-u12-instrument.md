# A.3 measurement exports — a3-u12-instrument

Coordinator ledger lines 167 and 176 govern this instrumentation unit. It runs
no boot and makes no mechanism-repair or acceptance claim. The historical
mechanisms and margins in a3-mechanism.md remain unresolved.

The stopped work is preserved on `arm-catchup/a-3-readback-hypothesis` at
`4d13c6a9`, including `3acaef60` and `21b08cae` and the complete stopped edit
snapshot. That branch was pushed before local A.3 was moved back to `b7eafd54`.
The pushed A.3 history was retained. The instrumentation keeps the font phases
and reap-elapsed measurement, without the speculative admission, deadline or
readback-storage changes. The admission predicate, polling deadlines, case
assertions, manifests and frame-floor calculation retain their input behavior.

## Export records

Records below use the existing `/dev/screen-evidence` channel, as `VALUE` lines.
Times are monotonic nanoseconds. Export formatting occurs after the measured
wait returns, including a successful return. The kernel retains numeric records;
it does not print on the observed path.

| Record | Producer and export point | Contents |
| --- | --- | --- |
| `wm-readback` | `syscall/window.rs::readback`, `gpu/mod.rs::observe`; exported by `wm_suite/desktop.rs::WaitObservation::export` on drop | Per-refusal sequence, syscall begin/end, observer group/thread, claimant/epoch, first/current serial, busy, latest successful completion and acknowledged presentation. Branch is `busy`, `claim-baseline`, or `gpu-fallback` for EAGAIN; other GPU errors are labelled `gpu-error`. Busy has priority when both gate conditions hold; the fields retain both conditions. |
| `wm-readback` with `branch=retained-writer-fallback` | Actual `screen::observation_context` call in `gpu/mod.rs::observe`, same export | Fallback begin/end, observer, target, retained writer context/serial/fence/answer and errno, including successful fallback creation. Its fields have writer-specific labels. |
| `wm-readback-buffer` | Same observer export | Retained and overwritten record counts from the root-only `/proc/breenix/window-readback` ring (2048 records). |
| `wm-observer` | `WaitObservation::export`, on drop | First read begin, last read end and whether this wait obtained a validated completed record. |
| `wm-observer-wait` | `Desktop::launch` and `Desktop::shown_after`, immediately after their wait | Desktop/initial-client/client wait kind, requested client/commit when applicable, begin/end and wait result. These are measured wait intervals, separate from process exec-to-phase intervals. |
| `wm-completion` | `WaitObservation::export` | Pipeline stages 10–13 within the observer interval: render, PRESENT, presentation and per-client receipt, with their original five columns. |
| `wm-startup` | `StartupTrace::failure`, also called by its destructor before directory removal | PID, phase, absolute time and exec-relative time, including retained font config/file/parse marks, font join begin/return and startup-workers-return. |
| `wm-startup-detail` | `Trace::details`, exported by `StartupTrace::failure` | PID and event index, phase, subject, band y/rows and time. Phases: band-begin, band-scanout, band-desktop, band-pixels, whole-check-return, report-begin/return and join-begin/return. The join surrounds the title worker join after the check closure; font joining uses the phase marks above. |
| `wm-reap` | `Child::stop` via `report_reap`, after polling | Stopped PID, kill request and kill return. |
| `wm-exit-phase` | `ProcessScheduler::handle_thread_exit` stages 17–22, exported by `report_reap` | Thread group, thread, stage, row PID and time. Group identity is retained on each stage on both architectures. Export selects only the stopped group, rather than an unrelated global tail. |
| `wm-reap-wait` | `report_reap`, following the stage rows | Parent wait return, kill-return-to-wait elapsed time and wait result, on pass or fail. |
| `reap-elapsed` | Recovery cases after `Child::stop` | Same measured interval in the suite value format, including a failed stop. |
| `wm-frame-endpoints` | `frame_floor`, before the following scanout check | Client ID, measurement begin/end and optional first/last receipt timestamps. |

Startup details occupy 48 five-word slots in the existing shared trace page;
the export includes event and dropped counts. Detail collection stops at
checks-ready. A killed or unfinished producer has only the stamps it reached;
no missing stage or band completion is inferred. The pipeline and readback rings
are bounded; a missing late exit stage must be interpreted against the parent
wait timestamp, since zombie publication can precede GPU teardown completion.

## Host check and validation

Check-first commit `0dc9957f` introduced `tests/a3_measurement_export_test.py`.
Its extracted production stop path failed because a passing reap emitted no
kill/wait/stage records; its cleanup check also failed because successful startup
cleanup removed the trace without exporting it. After instrumentation, the three
host checks pass: pass/fail reap export with unrelated threads excluded,
unconditional startup/observer export sites, and mapped startup/ring payloads
with bounded overflow accounting and distinct band endpoints.

The existing admission, pending-readback, band-validation and font-overlap checks
pass. The pending-readback and band fixtures gain diagnostic fields/stubs only;
their assertions are unchanged. Release validation completed with `CARGO_BUILD_JOBS=4` under `taskpolicy -b`:

- ARM64 production kernel: 1m 32s; no compiler warnings/errors. Cargo reports
  the existing pinned-core future-incompatibility advisory, as in a3-reap.md.
- x86 production kernel (release): 2m 38s, without warnings/errors.
- Scoped `bwm` and `suite-window-manager`: ARM64 40.04s and x86 23.92s,
  without warnings/errors, using the verified pinned Rust library.
- ARM64 FP/SIMD guard: PASS, 0 forbidden loads/stores, 32 allowlisted.
- Branch claim-lint: exit 0, 94 files; 280 pre-existing outside-hunk findings
  are reported separately. Whitespace check: exit 0.

 No disk installation, deployment, PR creation,
PR merge, board update or acceptance occurs in this unit. A measurement boot
belongs to a separately directed unit under line 176(5).
