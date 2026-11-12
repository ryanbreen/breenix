# A.3 investigation scaffolding removal

This unit implements coordinator ledger line 313 (1)–(2). It does not run
acceptance boots, deploy tools, change the board, open a PR, or merge the
branch. The retained failed capture is `20261112T091724Z-metal`; the subsequent
acceptance unit owns the ARM/native pair and the eventual disposition of
#2901. #2872 remains the separate earlier animation observation.

## Check first

Commit `85053234` adds `tests/a3_passing_exports_test.py` and
`tests/fixtures/a3_passing_exports.rs`. The host executes the production
`input_latency` body for typing, pointer and animation, and the shared
`WaitObservation` implementation/destructor and diagnostic writer. Guest I/O,
input delivery and GPU completion are supplied by the fixture. Its populated
readback/completion records represent preceding ordinary activity; the case
itself receives successful input and 12 ms completions.

<!-- claim-lint:ok: 85053234; tests/a3_passing_exports_test.py and tests/fixtures/a3_passing_exports.rs; red assertion recorded during this unit. -->
The stated bound is **2,048 `VALUE wm-` bytes per passing case**. Before the
production edit, typing returned success but the assertion failed at
**342,176 bytes**. After removing the exports, the check passes for the three
input variants. The fixture retains its populated proc inputs and absent
artifact route; making the fixture modules public merely keeps its boundary
APIs available after production stops calling them. The bound is unchanged. The same check also stresses the retained failure
writer with repeated Unicode text and checks its shared byte cap.

## Removed

| Piece | Removal and reason |
| --- | --- |
| Stall recorder | Delete `window/stall_trace.rs`, `/proc/breenix/window-stall` and `window-stall-sample`, their proc dispatch, and the `WM_STALL` devfs command parser. The investigation is finished; these records were exported onto input's UART. |
| Scheduler snapshots | Remove `StallThread`, `stall_row`, scheduler/runqueue snapshot collectors, `WaitSite`, diagnostic signal fields, and the added last-run/CPU/syscall/wait stamps. These existed to reconstruct the investigation's stalls. |
| Accounting | Delete `window/accounting.rs` and `accounting_hooks.rs`: exec-to-composite intervals, transitions, queues, disk/file request and IRQ stages, wake/join identities, wait budgets and input freezes. Remove their hooks in ARM context switching, thread/scheduler state transitions, syscall providers, block drivers, GPU waits, window exec and scene completion. State transitions return to their direct assignments. |
| GPU command archive | Delete `gpu_command_trace.rs` and `gpu_core_trace.rs`, command-origin fields, submit/doorbell/completion stamps, per-case boundaries and retention. Remove the matching legacy virtio GPU hooks. No command history is needed for the retained failure summary. |
| Acquire observation | Remove acquire-phase scopes, dependency attribution, replay-copy phase stamps and input begin/end/reset/close controls. Preserve the same admission, dependency and completion decisions. |
| Readback observation | Delete `readback_trace.rs`, its proc entry and refusal records, `ReadPhase`, and the readback/silent-wait counters and proc export. Pending-composition admission, interruption, staging and device observation remain runtime behavior. |
| Suite parent export | Remove parent polling snapshots, case-budget/control commands, unconditional case-boundary records, post-case/finalizer dumps and watchdog exports. No experimental recorder remains for a parent to drain. |
| Case success exports | Remove `WaitObservation::export` and its drop hook, observer-wait records, reap/exit-phase serial reports, frame-endpoint dumps, input observation uploads/skipped-route reports, and successful startup timeline export. Retain numerical case verdict metrics and thresholds. |
| Bulk failure export | Remove `pipeline_blob`, `full_pipeline`, and the unbounded serial pipeline loop. Failure reports use the bounded writer and a short pipeline tail instead. |
| Startup subphase measurements | Remove the detail ring, band/join events, CPU-cost counters and scopes, checker subphase costs and slow-check serial reports. Keep the checker duration used by validation scheduling; it is a runtime scheduling input. |
| Memory investigation | Delete `memory_owners.rs`, `ownership_snapshot.rs`, the page-table ownership walker and proc entry, snapshot fd capture, `ownership_detail`, and `SUITE_MEMORY_OWNERS` export. These full snapshots were investigation payloads. The outstanding-receipt census and reclaim settling remain. |
| Host observation | Delete `serial-clock.py`, `qemu-case-sample.py` and `analyze-a3-sample-join.py`; remove watcher sampling, trace inventory/flags, pidfile and Vigil sidecar wiring. The acceptance gate no longer requires observation labels, command traces, case samplers or CPU top-process captures. |
| Measurement checks | Delete `a3_accounting`, `a3_animation_observe`, `a3_font_trace`, `a3_gpu_case_preserve`, `a3_gpu_lock_attribution`, `a3_gpu_lock_observe`, `a3_irq_observe`, `a3_measurement_export`, `a3_observation_route`, `a3_sample_join`, `a3_stall_snapshot`, `a3_startup_workload`, `memory_ownership_snapshot` and `wm_pipeline_blob` Python checks, plus the obsolete observation-route Rust fixture. Their asserted machinery has been retired by line 313. |
| Mock observation APIs | Remove retired counter, CPU-cost, detail, readback and wait-site mock APIs from retained host fixtures. Their behavior assertions remain. |

Historical investigation reports and capture excerpts remain as evidence of
what happened at their recorded commits; they do not describe the final
runtime. The native-input audit now points here and identifies line 313 as
superseding its earlier proposed measurement boot.

## Kept

| Piece | Why it remains |
| --- | --- |
| Failure writer | `failure_diagnostic` clips a line to 160 characters and a label to 32 characters, and uses a process-local atomic byte budget of 2,048 including prefixes/newlines. The case child owns that budget. Its remaining callers report failures. |
| Last observation | `WaitObservation` keeps the latest error, completed record and representative pixel. Only an unsuccessful ready/client wait formats these; there is no ring or destructor export. This distinguishes a refused readback from the last completed binding. |
| Sparse startup stamps | The fixed `libuitest::startup` phase slots and per-process trace files remain available through the startup environment. Startup failures can report them through the bounded writer, inspecting at most eight process traces. Dropping the trace directory only cleans it up. These explain which startup phase was reached. |
| Failure context | RSS before/after and idle tick summaries, the first slow input's small metric set, existing GPU counter summaries and at most eight pipeline tail rows help identify a failed case. The `wm-` text shares the byte budget. |
| Latest GPU refusal | VirGL retains the latest refused command in three atomics; the reset worker reports it through its existing bounded notice node. Target identity, scene epoch and compositor resource failures retain their short failure records. This is device-failure diagnosis, not a command archive. |
| Ordinary suite evidence | Latency distributions, receipt/commit validation, frame-floor arithmetic, reap elapsed values, case verdicts and memory verdicts remain. Existing pipeline/input ledgers remain on-demand interfaces; they are not automatically dumped on successful cases. |
| Acceptance gate | Retain disk/built-suite equality, required runtime labels, clean-head and warning checks, signed b4 QEMU identity, coordinator freshness, the load guard with its two-second load series, prediction-before-exec and unchanged argv exec. Only the investigation observer requirements are removed. The prediction describes acceptance again. |
| QEMU fork | `renderer-worker.patch`, `angle-blob-cache.patch` and the build recipe remain unchanged. Retain the renderer, EGL-init, patched cache-callback and catalogue-cache checks; `measure-angle-cache.py` remains because the patched callback check uses its real shader workload. These exercise the host fix without a guest. |
| Reclaim settling | Keep outstanding retirement receipt accounting, the frames census and the settle loop's outstanding-receipt condition. `suite_memory_reclaim_settle_test.py` retains its receipt, free-page, stable-interval and verdict assertions; only the retired ownership API assertion/mocks are removed. |
| Startup and recovery fixes | Keep the renderer pipeline requirement masks, concurrent font/target preparation, ready-band/check workspace reuse, scene admission, completion-driven pacing, restart adoption and ARM deferred reap implementation. The real QEMU GPU drain, resource-creation/lifetime, input channel and pointer-frame fixes remain. |

The partial-resource-creation host check now extracts
`create_context_with_requirements`, where the production allocation body lives,
instead of the catalogue wrapper. Its resource retirement assertions are
unchanged. The pending-readback fixture drops only observation fields. The
failure-pipeline check still requires its exact evidence record with stdout
discarded, now using the bounded writer's `wm-pipeline` label.

## Validation

The retained-check logs for this unit are local files under `/tmp/a3-u34-`.

<!-- claim-lint:ok: /tmp/a3-u34-kept-checks.log and /tmp/a3-u34-adapted-checks.log. -->
The scoped host checks passed: startup pipeline selection, reclaim settling,
ARM exit deferral, pixel/readback batching, pending readback admission,
GPU progress draining, context sync lifetime and partial-creation cleanup,
startup recovery/font/target overlap, reply deadlines, clock coalescing,
readback interleavings, and bounded failure pipeline evidence. Retired mock
fields initially produced fixture compiler warnings; removing those unused
mocks resolved them without suppressions or assertion changes.

<!-- claim-lint:ok: /tmp/a3-u34-qemu-checks.log; /tmp/a3-u34-gate.log. -->
The four QEMU host checks passed. The gate's three checks passed. These host
checks do not launch a VM or open a display.

<!-- claim-lint:ok: /tmp/a3-u34-arm-kernel-final.log; /tmp/a3-u34-arm-neon.log; scripts/rust-fork.sh verify. -->
The production ARM64 release kernel built with no new source warnings; the
pinned-core future-compatibility advisory matches the earlier build logs.
The FP/SIMD guard passed, and the pinned Rust fork verified.

<!-- claim-lint:ok: /tmp/a3-u34-x86-kernel.log; /tmp/a3-u34-arm-userspace-fixed.log; /tmp/a3-u34-x86-userspace.log. -->
The x86 release kernel/UEFI launcher built without warnings. ARM64 and x86
release builds of `bwm`, `wm-client_test` and `suite-window-manager` passed
without warnings; x86 used `framebuffer-v1`. Builds used four Cargo jobs under
`taskpolicy -b`. The initial userspace command needed a canonical fork-library
path; a later compile exposed a dangling doc comment after the retired stall
type. Both are corrected in the successful builds named above.

<!-- claim-lint:ok: /tmp/a3-u34-retained-fixtures.log; /tmp/a3-u34-polled-wait.log; /tmp/a3-u34-exports-final.log; /tmp/a3-u34-artifact-labels.log. -->
The retained native admission/readback checks and production completion-kill
fixture passed. The native wait fixture needed the current pipeline requirement
field and completion/presentation interfaces; its wait-policy assertions stayed
unchanged and then passed. The passing-export/failure-cap check passed, and the
gate's runtime labels were found in the built kernels and suite binaries.

<!-- claim-lint:ok: /tmp/a3-u34-claim-lint-final.log; git diff --check. -->
Final claim-lint exits 0. No boot was performed in this unit. Final-head guest
acceptance remains outstanding for the acceptance unit; the earlier ARM capture
is not presented as acceptance of this runtime edit.
