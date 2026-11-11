# A.3 IRQ and wait observation repair — a3-u17-irq-observe

This unit implements coordinator ledger line 208(1)(a–d) and launcher support
for its pre-exec artifact gate. It starts at `b3e66c03` on `arm-catchup/a-3`.
Fetched `origin/main` was `ac3cdf6c`, already an ancestor. No rebase, force push,
boot, staging, deployment, PR creation or merge is performed here. The retained
measurement and unresolved mechanisms in a3-mechanism.md remain the evidence;
this instrumentation does not identify the cause of the long requests.

Check-first commit `338d10b0` adds four host checks. The idle fixture first
needed an unused fixture-helper reference to compile under `-Dwarnings`.
It then failed numerically: an idle thread left Running plus a dispatched
worker charged **160 ns against 100 ns of one-vCPU capacity**. The other
checks failed on absent host trace arguments, device IRQ entry recording,
and ordinary FUTEX_WAIT object identity. Production changes followed that
commit. Additional assertions exercise join naming, wake export and a case
killed inside diagnostic output.

## Host trace and pre-exec gate

`scripts/qemu-arm64.py` recognizes the serial chardev's `logfile` and adds:

```text
-d trace:virtio_blk_handle_*,trace:virtio_blk_req_complete,trace:virtio_blk_rw_complete,trace:virtio_queue_notify,trace:virtio_notify,trace:virtio_mmio_queue_write,trace:virtio_mmio_setting_irq
-D <serial-log>.qemu-trace.log
```

An existing `-d` value is extended and an explicit `-D` path is preserved.
The normal interactive launcher supplies neither, so its trace uses the path
above.

Both installed Homebrew QEMU and the GL QEMU list these seven patterns' events
with `-trace help`; no guest is started by that query. The handle wildcard
includes the read/write handlers and the listed zone handlers. The completion
patterns distinguish backend read/write completion from request completion;
the notify and MMIO IRQ records add the device notification/IRQ boundaries.
The launcher links the trace into the Vigil boot directory as `qemu-trace.log`,
beside its existing `serial-host-clock.jsonl` link. Host wall-clock trace
stamps can be compared with the sidecar's `host_unix_ns`; its accompanying
`host_monotonic_ns` correlates the load sampler. Sidecar stamps are serial
**receipt** times, with buffering/polling delay, rather than exact guest event
arrival times. The next directed measurement must inspect that uncertainty
when aligning guest monotonic records; this unit supplies no measured mapping.

Set `BREENIX_QEMU_PRE_EXEC_GATE` to an executable artifact/load-check wrapper.
It receives the native QoS launcher followed by the real QEMU and its arguments.
After its checks succeed it must `exec "$@"`; a failure exits without doing so.
The execution order is gate → native QoS launcher → real Mach-O QEMU. The gate
therefore runs outside QEMU's DYLD injection. `qemu-ui-launch.c` retains its
name/Mach-O validation, application-role policy and thread-library injection;
renderer attestation and check-host-renderer.py are unchanged. Do not put an
interpreter gate in `BREENIX_QEMU_GL_BINARY`.

The host gate fixture verifies ordered execution and refusal exit 42 without
executing its payload. A separate native-launcher fixture confirmed priority
46 for main/child/grandchild and preservation of `BREENIX_HOST_RENDERER_JSON`
through the gate. This is host scheduling/environment evidence, not a renderer
verdict or guest boot.

## Guest boundaries and object names

The MMIO block handler records device index, guest monotonic ns and vCPU at its
entry, before device-state validation, MMIO acknowledge or used-ring consumption.
The existing request `irq_ns` still marks used-descriptor completion. Matching
the device and interval separates handler-entry→used-consumption from the
existing submit→completion span; the host trace supplies independent backend
and IRQ-raise observations. The host fixture exports an IRQ entry while the
request's `irq_ns=0`, then retains both after consumption.

The existing context-switch trace closes a still-Running outgoing journal row.
This repairs idle accounting without changing the scheduler's intentional
Running idle state. The journal uses mode 4 for that off-CPU interval and does
not charge it as CPU, runnable or blocked time. The idle fixture now measures
40 ns idle plus 60 ns worker, exactly 100 ns. Existing Ready/blocked/terminated
publications keep their intervals. These remain guest dispatch/state intervals:
host vCPU descheduling and IRQ work are not independently subtracted from them.
No Tier 1 or Tier 2 file is edited.

Ordinary FUTEX_WAIT now retains its word through WaitSite, as the bitset path
already did. Clone records the clear-on-exit word and target PID/TID while the
worker is created. For a creator's matching futex wait, the exporter names that
object `thread-exit-futex` before the target exits; the registration row names
the target. Clear-on-exit publication and actual futex dequeues have separate
wake rows. A shared futex dequeue uses the table's resolved key, which can be a
shared mapping identity rather than the earlier user virtual address; use its
target TID and the explicit clear-word record when correlating those rows.

GPU wait-event publication names WAITERS as `gpu-events`; kgpu's own wait names
REAPER as `gpu-reaper`. Stall snapshots retain these names and object addresses.
Task/IRQ GPU notifications, reaper wakes and completion-waiter publications
add numeric wake records. `target=0` denotes an event publication without one
selected waiter. `count` is the publication's requested wake bound (0 for a GPU
event), does not measure how many threads the scheduler dispatched. Completion
and actual futex dequeues name a target. Numeric join/wake/IRQ rows use the
existing fixed accounting journal and its loss/overwrite reporting during
exec-to-first-composite windows. GPU WaitSite names are also available in the
stall snapshot outside those windows.

## Outer-case budget

The case records fork/setup, logic and result-report boundaries through the
existing root-only evidence device. Diagnostic writes accumulate their elapsed
time and retain an unfinished diagnostic begin. The parent stamps its outcome
and stamps before its pre-kill evidence dump. These measurements change no
case predicate, deadline, manifest or verdict. The existing wait arm/end path
retains named begin/deadline/end rows; a tracked case also accumulates completed
wait time and preserves the current/last wait. Missing logic/report ends at a
watchdog retain `0`. Wait and diagnostic totals describe portions of the
logic interval and must not be added to the enclosing logic time again.

Budget summaries retain at most 16 thread/case entries independently of the
512-row stall ring; oldest entries are evicted as subsequent cases arrive.
Completed report summaries are emitted after updates, while incomplete ones
remain readable. Named wait rows use the stall ring's existing overwrite
counter. A host fixture leaves a diagnostic open at a simulated kill and checks
that case/logic boundaries, completed diagnostic time, open diagnostic begin
and wait timing survive the parent read. The existing retained-stall check
passes unchanged: a caller without an outer-case budget does not add an end
budget row.

## Stored label inventory

The suite exports kernel rows as `VALUE wm-` plus the stored kernel suffix.
The following row suffixes are new and belong in both kernel ELFs:

| Serial row | Stored form |
| --- | --- |
| `wm-disk-irq-entry` | `disk-irq-entry` |
| `wm-account-join` | `account-join` |
| `wm-account-wake` | `account-wake` |
| `wm-case-budget` | `case-budget` |
| `wm-wait-budget` | `wait-budget` |

New object-name values stored in both kernels are `thread-exit-futex`,
`gpu-events` and `gpu-reaper`. Existing `account-wait` rows gain `name=`.
The new budget phase strings stored in the suites and interpreted in the
kernel are `parent-fork`, `case-begin`, `logic-begin`, `logic-end`,
`report-begin`, `report-end`, `parent-outcome`, `watchdog-before-kill`,
`diagnostic-begin` and `diagnostic-end`. Kernel parsing only distinguishes the
phases it aggregates; `parent-outcome` and `watchdog-before-kill` are retained
as the latest phase text. The suite command format is stored as
`WM_STALL budget `, followed by formatted fields. Prior line-185/u13/u16
inventories, `VALUE wm-`, `/proc/breenix/window-stall` and
`/proc/breenix/window-stall-sample` remain required by the artifact gate.

## Validation

- `python3 tests/a3_irq_observe_test.py`: four checks pass after the implementation.
- The named-wait/budget check was scoped again after the final GPU wake adjustment: one passes.
- The trace/gate check was scoped again after preserving explicit logging arguments: one passes.
- `python3 tests/qemu_arm64_test.py`: 11 pass.
- `python3 tests/a3_accounting_test.py`: two pass.
- `python3 tests/a3_stall_snapshot_test.py`: three pass unchanged after restricting end-budget export to tracked cases.
- Native QoS fixture through the pre-exec gate: one passes; priority 46 on its three measured threads, renderer path preserved.
- ARM64 and x86 `suite-window-manager` release builds: exit 0, no compiler warnings.
- x86 release build (`cargo build --release --bin qemu-uefi`): exit 0, no compiler warnings.
- Final ARM64 kernel release build: exit 0; no compiler warnings, with the existing pinned-core future-compatibility advisory. The earlier build also passed; subsequent observation edits required the final build.
- `scripts/check-kernel-no-neon.sh` on the ARM kernel: exit 0, 0 forbidden loads/stores and 32 allowlisted.
- `scripts/rust-fork.sh verify`: exit 0, pinned fork `666252fd`.
- `bash -n scripts/boot-interactive.sh`: exit 0.
- `git diff --check`: exit 0.
- `python3 scripts/claim-lint.py --files tests/a3_irq_observe_test.py`: exit 0 before the check commit.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-u17-check-msg.txt`: exit 0.
- `python3 scripts/claim-lint.py --base 338d10b0`: exit 0 after replacing two absolute comment claims with precise wording.
- `python3 scripts/claim-lint.py --base origin/main`: exit 0.
- `python3 scripts/claim-lint.py --files docs/planning/arm64-catchup/a3-u17-irq-observe.md`: exit 0 after making the timing descriptions precise.

The remaining measurement/acceptance work stays under coordinator line 208
and issue #2872. No mechanism repair, deadline headroom, acceptance verdict or
board change is claimed here.

Built artifact identities (no disk staging is claimed):

| Artifact | SHA-256 |
| --- | --- |
| x86 Cargo artifact kernel | `634df8b8b1e74a462ef2d7778e5ee13f336ce7908075eb9deedc0829c23f064b` |
| ARM64 kernel | `29c77dffc3133b82aa349a8adb874df680d845bf1f60112517eaab4c8ce79175` |
| ARM64 window-manager suite | `c071f77bf570af7cec52ff358637b5e579477da8851119c2b594209391867119` |
| x86 window-manager suite | `89ed3623a9133f0f2c7d5bceb58bee80f06f8f6ffd583de8a7eeb176be8320bf` |

Both kernels contain the five new row suffixes and three new object names
listed above. Both suite ELFs contain the ten budget phase strings, the command
format prefix and the existing VALUE/procfs strings. The userspace builds use
build.sh's pinned std root, native libc path and link flags; x86 selects
`framebuffer-v1`. Kernel builds use `cargo build --release --bin qemu-uefi`
for x86 and the explicit `aarch64-breenix-kernel.json` production target with
`build-std=core,alloc` for ARM. Builds export `CARGO_BUILD_JOBS=4` and run under
`taskpolicy -b`.

The x86 ELF is Cargo's artifact dependency at
`target/x86_64-unknown-none/release/deps/artifact/kernel-cb6f47ce5ef18fd1/bin/kernel-cb6f47ce5ef18fd1`,
not the conventional top-level `release/kernel` left by an older build. <!-- claim-lint:ok: Byte inspection of this artifact found 8 of 8 new row suffixes and object names; its SHA-256 is recorded in the artifact table above. -->
`python3 scripts/claim-lint.py --commit-msg /tmp/a3-u17-implementation-msg.txt`
exits 0 before the implementation commit.

The final artifact-path paragraph initially triggered claim-lint's universal
word check. Its inline 8 of 8 ELF-label evidence discharges that finding.
The implementation commit had already been made, so this correction is
recorded in a documentation commit.
