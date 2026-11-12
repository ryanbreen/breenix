# A.3 GPU lock observation transport

Coordinator ARM ledger line 221 (a)–(c) authorizes this instrumentation. This unit
adds observations only. It performs no boot, runtime repair, deadline or case
assertion change, PR creation or merge. The mechanism in the u19 report remains
unresolved; a later authorized measurement must distinguish host lock contention
from vCPU descheduling.

## Host trace

`scripts/qemu-arm64.py` queries the executable passed by the launcher with
`-trace help`, before the native QoS launcher execs that same executable. GL
launches retain its inventory as `<serial>.qemu-trace-events.txt`, linked as
`qemu-trace-events.txt` in the Vigil record. The header fields are `binary=` and
`patterns=`. The pre-exec artifact gate and native QoS/renderer path remain in
place.

The selected bundled GL executable reports these 26 GPU events:

```text
virtio_gpu_features
virtio_gpu_cmd_get_display_info
virtio_gpu_cmd_get_edid
virtio_gpu_cmd_set_scanout
virtio_gpu_cmd_set_scanout_blob
virtio_gpu_cmd_res_create_2d
virtio_gpu_cmd_res_create_3d
virtio_gpu_cmd_res_create_blob
virtio_gpu_cmd_res_unref
virtio_gpu_cmd_res_back_attach
virtio_gpu_cmd_res_back_detach
virtio_gpu_cmd_res_xfer_toh_2d
virtio_gpu_cmd_res_xfer_toh_3d
virtio_gpu_cmd_res_xfer_fromh_3d
virtio_gpu_cmd_res_flush
virtio_gpu_cmd_ctx_create
virtio_gpu_cmd_ctx_destroy
virtio_gpu_cmd_ctx_res_attach
virtio_gpu_cmd_ctx_res_detach
virtio_gpu_cmd_ctx_submit
virtio_gpu_update_cursor
virtio_gpu_fence_ctrl
virtio_gpu_fence_resp
virtio_gpu_inc_inflight_fences
virtio_gpu_dec_inflight_fences
virtio_gpu_cmd_suspended
```

These include the VirGL context submission/resource/transfer/fence path under
`virtio_gpu_*`. This executable lists no `virgl*` events and no generic
per-command finish event. Do not interpret a guest return stamp or fence response
as a measured host command-return boundary. The host sample is needed to locate
host GL work during a stall. If a selected executable lists `virgl*`, the launcher
also enables that family; it does not request an unavailable family.

The effective trace patterns for this executable are:

```text
trace:virtio_blk_handle_*
trace:virtio_blk_req_complete
trace:virtio_blk_rw_complete
trace:virtio_queue_notify
trace:virtio_notify
trace:virtio_mmio_queue_write
trace:virtio_mmio_setting_irq
trace:virtio_gpu_*
```

They share the existing `-D <serial>.qemu-trace.log` and `-msg timestamp=on`.
The Vigil link remains `qemu-trace.log`.

## Host samples and trigger lines

The runner emits `CASE window-manager <category>/<case> START` immediately before
forking a window-manager case. These records carry no verdict and change no
selection. The host serial watcher parses complete start lines, retaining an
unterminated fragment for the next read. It reads QEMU's own `-pidfile <serial>.qemu.pid` (linked as Vigil's
`qemu.pid`). The native QoS launcher can switch to a fresh launchd task, leaving
its initial Popen PID as a supervisor rather than QEMU. The pidfile identifies
the actual QEMU in both paths. The watcher clears a stale pidfile before launch;
a missing or invalid PID emits `sample-error` and skips sampling rather than
falling back to the supervisor. An explicit `-pidfile` is honored.

The trigger lines are:

```text
CASE window-manager recovery/clients-notified START
CASE window-manager recovery/restart-adopts START
CASE window-manager recovery/interaction-after-restart START
CASE window-manager recovery/stack-restored START
CASE window-manager recovery/supervisor-restarts START
CASE window-manager stacking/click-raises START
CASE window-manager stacking/all-minimized START
CASE window-manager focus/pair-click START
```

The matcher admits the `recovery/` category and the three listed positive-click
cases. Duplicate starts in a capture are ignored; PASS/FAIL records do not
trigger sampling. On macOS it starts `/usr/bin/sample <qemu-pid> 3 1 -file <file>`:
a three-second window at a one-millisecond sampling interval. The watcher reaps
finished children, terminates an overrun after four seconds, and terminates any
remaining sample children during cleanup, using their recorded process handles.
It does not signal the sampled QEMU target. Existing QEMU cleanup stays separate.

Files beside the capture, linked as Vigil's `qemu-samples/`:

- `<serial>.qemu-samples/<category>-<case>.sample.txt`
- `<serial>.qemu-samples/<category>-<case>.sample.stderr.txt`
- `<serial>.qemu-samples/samples.jsonl`

The stored JSON `event` labels are `sample-start`, `sample-stop`, and
`sample-error`. Rows carry `host_unix_ns`, `host_monotonic_ns`, `launch_pid`, `qemu_pid`, and
`case`; start/stop rows add `sample_pid`, start adds `file`, stop adds
`returncode`, and error adds `error`. A missing/failed sample remains missing
evidence; these additions do not relax a validity gate or scorer.

## Guest stamps, in stored form

The ARM GPU PCI two-descriptor, three-descriptor and polled paths, and its MMIO
2D counterpart, record a numeric submission before queue notification. A guard
records function return, including error returns; `return_ns=0` retains a pending
command if a watchdog interrupts it. Stamping begins at notification, after queue preparation succeeds. The journal uses 2048 atomic slots, with CAS contention and
reservation checks instead of a producer lock. It has no submission-time
allocation, formatting or serial output. It is independent of accounting-window
churn. The existing root-only `/proc/breenix/window-stall` read includes it on
ARM, and the suite's existing pass/fail/watchdog dump adds `VALUE wm-`.

The new serial labels and fields are exactly:

```text
VALUE wm-gpu-command-ring retained=<n> overwritten=<n> lost=<n>
VALUE wm-gpu-command seq=<n> begin_ns=<ns> return_ns=<ns> pid=<pid> tid=<tid> transport=<n> type=<n> cmd_bytes=<n> payload_bytes=<n>
```

Transport 1 is MMIO 2D; 2 is PCI, including VirGL. Type is the decimal virtio GPU
control-header type. `cmd_bytes` counts the command/header descriptor;
`payload_bytes` counts the separate three-descriptor payload, otherwise 0.
These sizes do not claim to count backing-resource bytes moved by a transfer
command. PID/TID come from `current_accounting_identity`, including kernel work
as PID 0; the existing identity journal supplies process names. Sequence numbers
join pending and returned states, and stale completion cannot replace a newer
reservation. The ring reports cumulative overwritten/lost counts, so truncation
is visible. Export is a bounded snapshot, not complete boot history.

## Host verification

Check-first commits: `7cc24931` and `229b3cbe` (fresh-task PID checks). Before implementation,
`python3 tests/a3_gpu_lock_observe_test.py` failed for missing trace inventory,
sampler module and command journal (one assertion failure, two missing-API errors).
After implementation its three checks pass. The later PID assertions then fail
on the missing pidfile argument and missing sampler PID resolution; after that
repair the two affected checks pass again. The unchanged ring check is retained
from its earlier passing run. The Rust check executes the
production ring with pending commands, eviction and a stale completion; Python
checks execute selected-binary discovery and sampler child lifecycle with fake
processes. No VM or real process sampling is part of these checks.

The existing `a3_irq_observe_test.py` checks pass (5), as do
`qemu_arm64_test.py` (11). `bash -n scripts/boot-interactive.sh` and
`git diff --check` exit 0. The existing synthetic watcher checks pass (36).
After the PID-file addition, the trace/QoS check and child-reaping check pass
again; unrelated watcher cases are not repeated. The scoped x86 window-manager
release build and x86 release kernel build pass without warnings. The ARM64 and x86
release builds used `CARGO_BUILD_JOBS=4` under `taskpolicy -b`. Logs are retained
locally as `/tmp/a3-u20-{arm,x86}{,-suite}-build.log`. After the sampler cleanup
was put inside a nested finally, a fake cleanup error confirms the owned QEMU
cleanup still runs. The PID-file changes are host-only and need no new guest
build. The complete unit result is recorded in the final ledger line.

Claim-lint invocations:

- `scripts/claim-lint.py --files tests/a3_gpu_lock_observe_test.py` → exit 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u20-check-msg` → exit 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u20-pid-check-msg` → exit 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u20-implementation-msg` → exit 0.
- `scripts/claim-lint.py` invocations, in order: exit 1 (two absolute comments),
  exit 0 after comment corrections, exit 1 (the report's wording), exit 0 after
  wording corrections, exit 0 at final review, exit 0 after these notes.

The red-check logs are `/tmp/a3-u20-red.log` and `/tmp/a3-u20-pid-red.log`.
The first green check log is `/tmp/a3-u20-green.log`; the PID repair's affected
checks were run directly and passed 2/2. The existing watcher log is
`/tmp/a3-u20-watcher.log`. Claim-lint's final output is
`/tmp/a3-u20-claim-final.log`.

The first scoped ARM suite build invocation failed before compilation because its
Rust source-root path was not canonical. The corrected invocation passed. The
ARM kernel build passes with the existing pinned-core future-incompatibility
notice under G35(2), and its FP/SIMD guard reports 0 forbidden/32 allowlisted.
The ARM window-manager release binary builds without warnings. No disk was
staged or booted in this unit.
