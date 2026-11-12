# A.3 animation acquire observation

<!-- claim-lint:ok: docs/planning/arm64-catchup/a3-animation-blocker.md and coordinator ledger line 298. -->
This unit implements coordinator ledger line 298(a)-(c) after retained proof
`20261112T070425Z-qemu-31990`. It does not identify the cause of that proof's
110.545 ms input sample. No boot is run in this unit, and acceptance remains
separate. The retained capture analysis is
[a3-animation-blocker.md](a3-animation-blocker.md).

## Guest observation

<!-- claim-lint:ok: accounting.rs reset_input/begin_input/finish_input and a3_animation_observe_test.py first_slow_input_is_immutable; cases.rs input_latency diff. -->
`kernel/src/window/accounting.rs` reuses the fixed-capacity numeric journal
for one input-to-completion window at a time. Fast samples can be replaced;
the first completed sample strictly over 75,000,000 ns freezes its window.
That includes warm-up inputs. Later input windows and compositor accounting
cannot replace the frozen records until the observation scope closes.
The existing latency sample selection, median/max checks and deadlines stay
unchanged in `wm_suite/cases.rs::input_latency`.

<!-- claim-lint:ok: accounting.rs acquire_phase/text; accounting_hooks.rs AcquirePhase/phase_at; thread.rs and tracing/providers/sched.rs existing accounting hooks. -->
`account-input` identifies the input's monotonic start and completion times.
`acquire-phase` rows carry monotonic ns, pid, tid, phase name, edge, object
and cumulative thread CPU ns. Edges 1 and 0 bracket work, including error
returns; edge 2 names a refused dependency predicate. A thread's outer
`acquire` phase names its window; nested rows name GPU resources and requests.
The export also contains timestamped `account-interval` rows and existing
`account-thread`/`account-wait` totals, distinguishing CPU, runnable and blocked
time using existing scheduler publications. No scheduler hook is added.

| Phase or dependency | Object |
| --- | --- |
| acquire, pending-front, free-buffer, replay-copy, seal, tree-protect | Window id |
| source-ready, source-writer, target-writer, target-reader | GPU target id |
| dependency, context-busy | GPU context id |
| context-inflight, accepted-writer, sync-reader | Outstanding request serial |
| source-attach, virgl-copy-completion, copy-completion | Sync request serial |

<!-- claim-lint:ok: window_targets.rs ready/copy_from/execute/replay_request; ops.rs acquire; accounting.rs push/text. -->
Source readiness, attachment, context/resource dependencies and completion
waits are observed in `gpu/window_targets.rs`; replay and protection are
observed in `window/ops.rs`. The predicates retain their decisions and order.
Numeric writes use the existing bounded journal; formatting occurs on export.
Its existing overwritten/lost/unavailable counters remain visible, so missing
records must not be treated as complete evidence.

<!-- claim-lint:ok: devfs/mod.rs WM_STALL dispatch; cases.rs input_observation and input_latency; suite.rs upload_diagnostic. -->
The root-only evidence command path accepts input reset/begin/end/close.
The latency case reads the retained journal after its 110 inputs and uploads
one diagnostic artifact with a distinct typing, pointer or animation label.
The serial receipt identifies the client window, artifact label, byte count
and SHA-256. Command/read/upload errors propagate; Drop closes the observation
scope on an earlier case error. The artifact avoids printing the interval
journal line by line during the case.

## Host observation

<!-- claim-lint:ok: qemu-case-sample.py observe/start_window/tick/finish; a3_animation_observe_test.py performance sampling fixture and existing GPU observation/attribution checks. -->
`qemu-case-sample.py` starts read-only macOS `sample` for performance case
START records, repeats its three-second windows until END or verdict, and
samples the actual QEMU pidfile rather than a launcher supervisor. The command
samples the process's threads without a thread filter, including the renderer
worker. Existing recovery and click sampling remains. Only owned sample
children receive cleanup signals; QEMU is the read-only sample target.

<!-- claim-lint:ok: scripts/a3-proof-gate.py check_observers/check_artifacts/run; scripts/watch-qemu.py main; tests/a3_proof_gate_test.py. -->
The proof gate now refuses a missing watcher binding or a mismatched trace
or QEMU pidfile, and checks the performance observer's continuous policy.
Its artifact manifest records the observer source hashes and sample directory;
artifact labels require the new guest observations. `watch-qemu.py` provides
the binding along the existing launch chain. The gate's prediction describes
the unresolved dependency-versus-runnable question, without claiming a fix.

<!-- claim-lint:ok: a3-proof-gate.py cpu_top/sample_load; a3_animation_observe_test.py timestamped CPU top fixture. -->
The existing two-second load sampler also writes `cpu-top.jsonl` beside
`load.jsonl`. Matching wall and monotonic timestamps correlate the records.
Each tick retains up to ten processes sorted by macOS ps CPU percentage, with
pid, command and cumulative CPU time. The percentage is ps's reported estimate,
not an exact two-second utilization delta. The boot record must be read with
the QEMU thread samples before attributing contention; load alone is not cause.

## Validation

<!-- claim-lint:ok: /tmp/a3-u32-red.log; tests/a3_animation_observe_test.py; suite_collection_test.py run_rust. -->
Check-first commit: `719e4169`. Its three checks fail on the original code:
missing input-window/phase API, no performance sample child, and no CPU top-10
ps call. The production Rust fixture checks the strict 75 ms boundary, a
70 ms completion block plus 10 ms runnable interval, immutable first-slow
retention, and resumed restart accounting after close. It compiles with
`-Dwarnings`.

<!-- claim-lint:ok: /tmp/a3-u32-gate.log and /tmp/a3-u32-host-final.log; 13/13 scoped checks. -->

The scoped host checks pass: observation 3, accounting 2, proof gate 3, GPU
observation 3, GPU attribution 2. After switching observation export to an
artifact, the affected observation check passes again. Logs are
`/tmp/a3-u32-{host,account,gate,sampler,attribution,host-final}.log`.
<!-- claim-lint:ok: /tmp/a3-u32-arm-kernel.log; /tmp/a3-u32-x86-kernel.log; /tmp/a3-u32-arm-suite.log; /tmp/a3-u32-x86-suite.log. -->
Release builds use `CARGO_BUILD_JOBS=4` under `taskpolicy -b`:

| Target | Scope | Result |
| --- | --- | --- |
| ARM64 | Production kernel | Exit 0, 1m58s |
| x86 | Kernel and qemu-uefi launcher, testing/external_test_bins | Exit 0, 3m46s |
| ARM64 | suite-window-manager | Exit 0, 35.13s |
| x86 | suite-window-manager, framebuffer-v1 | Exit 0, 23.68s |

<!-- claim-lint:ok: four build logs above and /tmp/a3-animation-arm-build.log baseline advisory; /tmp/a3-u32-arm-neon.log; scripts/rust-fork.sh verify. -->
No new source warnings appear. ARM's kernel build repeats the baseline
pinned-core future-compatibility advisory. The ARM FP/SIMD guard reports
0 forbidden instructions with 32 allowlisted. The Rust fork verifies as
`666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d`. Observation artifact labels are
present in the ARM kernel, the x86 launcher's kernel artifact, and both suites.
The standalone x86 `release/kernel` path is stale; the launcher uses
`release/deps/artifact/kernel-7514b1055e3b757e/bin/kernel-7514b1055e3b757e`.

Claim-lint invocations and exits for this unit:

- `scripts/claim-lint.py --files tests/a3_animation_observe_test.py`: 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u32-check-msg`: 0.
- `scripts/claim-lint.py --base 719e4169`: 1 for a sampler comment;
  0 after replacing its unquantified wording.
- `scripts/claim-lint.py --files docs/planning/arm64-catchup/a3-u32-animation-observe.md`:
  1, 1, then 0 while correcting annotation placement and resolving citations.
- `scripts/claim-lint.py`: 0, 192 files checked against `origin/main`;
  10 existing unsupported-extension targets skipped, with existing findings
  outside changed hunks unreported. The final invocation is recorded below.

Fresh `origin/main` (`b26fbda8`) is already an ancestor, so no merge or rebase
is needed. Issue #2872 remains the acceptance/attribution follow-up. This unit
runs no QEMU or hardware boot, opens no window or debugger, and creates or
merges no PR. The next measurement must retain the phase/CPU artifact, macOS
QEMU sample reports and their windows, serial clock mapping, load and CPU
process records; missing acquire or QEMU thread observations void that run
under coordinator line 298. No animation repair or acceptance is claimed here.

Final `scripts/claim-lint.py --commit-msg /tmp/a3-u32-change-msg`: exit 0.
Final `scripts/claim-lint.py`: exit 0; `git diff --cached --check`: exit 0.
