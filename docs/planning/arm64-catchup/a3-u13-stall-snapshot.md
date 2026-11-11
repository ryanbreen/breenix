# A.3 stall snapshots — a3-u13-stall-snapshot

Coordinator ledger line 190(1)(a)–(d) governs this instrumentation unit.
No boot, mechanism repair, deployment, acceptance, deadline adjustment, case
assertion change or manifest change occurs here. The mechanisms in
a3-mechanism.md remain unresolved.

The check-first commit is `72c94b02`. Its three host checks initially failed
because the snapshot ring, pre-kill dump and serial clock sidecar were missing.
The ring check executes the production ring with a deterministic clock and a
scheduler fixture: 499/1000 ns does not sample, 500/1000 ns samples, ending the
wait retains its rows, repeated sampling does not duplicate them, and overflow
stays bounded. The other checks pin parent export/arming and execute the host
line clock over split lines and byte offsets.

## Collection

`wm_suite/desktop.rs::StallWait` arms the original deadline at entry to
`until_from` and `read_n`, through a single-buffer `WM_STALL arm` write to the
root-only evidence device. These sites cover recovery, positive-click and
startup polling, including nested client reply waits. Ending a wait samples it
if its midpoint was crossed before the parent polled, then disarms it without
removing retained rows.

The window-manager runner's existing `wait_exit` loop reads
`/proc/breenix/window-stall-sample` while the case executes. Thus a case blocked
inside a syscall does not have to resume before its snapshot is taken. Sampling
happens on the first parent poll after the 50% crossing, rather than claiming
an interrupt at that exact nanosecond. Each snapshot records its actual time,
original begin/deadline and observer TID. Kernel scheduler records cover user
threads, including the suite, bwm, clients and restarted/reparented processes.
An unavailable x86 owner lock is reported explicitly rather than silently
presented as a runnable or sleeping thread.

Thread rows contain PID/TID, scheduler state, current syscall number (`none`
outside a syscall), wait identity, time since dispatch last stopped (zero while
running), last dispatch CPU, cumulative user+system CPU nanoseconds including
the active interval, ready queue CPU/index/length when queued, and timer wake
nanoseconds. Wait identities name completion objects, wait queues, futex user
addresses, poll-set objects and child selectors; timer/signal waits also retain
their scheduler state. Object values are identities, not contents read from a
kernel stack. The syscall trace provider publishes syscall number+1 and clears
it at exit; the dispatch/accounting and wait sites publish numeric state only.
No Tier 1 or Tier 2 file is edited. <!-- claim-lint:ok: kernel/src/task/thread.rs::StallThread::text emits the literal none for a zero diagnostic_syscall word; trace_entry/trace_exit in kernel/src/tracing/providers/syscall.rs publish and clear that word. -->

`window/stall_trace.rs` retains 512 rows of at most 384 bytes in fixed kernel
storage, with retained/overwritten counts, and up to 128 armed waits with an
explicit arm-overflow record. Formatting is in task context. The suite parent
exports the ring before `stop_child` sends SIGKILL and at its suite finale
(after DONE/MEMORY/ELAPSED, before READY). The case does not own this export, so
case death does not remove it. The parent collector is enabled for the
window-manager suite; other suites keep their existing wait loops.

## Labels for the artifact gate

The parent emits `VALUE wm-` followed by the kernel row. Check the suite binary
for `VALUE wm-`, `WM_STALL`, `arm `, `end `, `click-request` and `click-observed`,
and both procfs paths. Check the kernel for the row labels below. As in line
185, a constructed full `wm-...` string need not be a contiguous ELF string.

| Serial label | Producer retained in the kernel ring | Export point |
| --- | --- | --- |
| `wm-stall-buffer` | `window/stall_trace.rs::text`, retained and overwritten counts | runner `dump_stall_evidence` |
| `wm-stall-snapshot` | `stall_trace::sample`, midpoint wait/clock header | same parent dump |
| `wm-stall-thread` | scheduler `stall_snapshot`, thread accounting and run queue state | same parent dump |
| `wm-stall-arm-overflow` | `stall_trace::command`, armed-wait capacity reached | same parent dump |
| `wm-click-request` | suite `Desktop::pointer`, after emitting HOSTPOINTER; serial field is the host request sequence and window field is the button mask | same parent dump |
| `wm-click-delivery` | kernel `window/ops.rs::write_events`, after accepting POINTER_BUTTON into the target event ring; event serial and window ID | same parent dump |
| `wm-click-handled` | `wm_client.rs::run`, when processing a POINTER_BUTTON from its event batch, before the handling commit; event serial and window ID | same parent dump |
| `wm-click-observed` | suite `Client::await_event`, after finding the requested button event; event serial and window ID | same parent dump |

The client binary contains `WM_STALL click-handled`. The kernel contains
`click-delivery`, `stall-buffer`, `stall-snapshot`, `stall-thread`,
`stall-arm-overflow` and the thread field names. Its other click labels are
constructed from the consumed command: optimized comparisons do not require
contiguous `click-handled`, `click-observed` or `click-request` kernel strings. Delivery/handling/observation correlate by window and event
serial; request sequences are a separate namespace. These records make no
claim that a request was delivered before delivery evidence exists.

## Host clocks

`watch-qemu.py` uses `serial-clock.py::SerialClock` to write
`<serial-log>.host-clock.jsonl`. Each complete serial line has `host_unix_ns`,
`host_monotonic_ns`, its original byte `offset`, and `line`. A partial line is
retained until newline, and shutdown drains remaining complete lines after
reaping this launcher's process. These are host receipt times at the watcher's
poll cadence (normally 200 ms), not inferred guest emission times. Correlate
the embedded guest timestamps with these receipt clocks and the two-second
load samples, retaining that timing uncertainty.

`boot-interactive.sh` links the sidecar as `serial-host-clock.jsonl` beside
Vigil's linked serial capture. No QEMU display or debugger is involved.

## Validation

The new host check passes 3/3 after the implementation. Retained measurement
exports pass 3/3; their fixture now includes the production StallWait and its
command writer, with the existing assertions retained. The completion host
fixture passes 1/1, with a fixture-only numeric publisher for the new metadata
hook. The launcher watcher checks pass 36/36 and desktop-ready checks pass 5/5,
using synthetic child processes rather than VM boots.

With `CARGO_BUILD_JOBS=4` and `taskpolicy -b`, production release builds passed
for the ARM64 and x86 kernels, and for suite-window-manager and wm-client_test
on both architectures. Compiler warnings/errors: 0 in those builds. Cargo's
ARM kernel build prints the existing pinned-core future-compatibility advisory.
The ARM64 FP/SIMD guard passed: 0 forbidden loads/stores, 32 allowlisted.
The userspace command initially failed when invoked from the root directory
without the programs workspace's build-std configuration; running it from that
workspace resolved the command error without changing source.

The listed labels/paths were checked in these built artifacts:

| Artifact | SHA-256 |
| --- | --- |
| ARM64 kernel | `5244c097d83fd389d3b32ed6e429bf35e844e7a0e234287fa5ca6038101d7a83` |
| x86 kernel | `3bdf67c4f6e7fd6bc2cdcd7f644f8dd8a7ac95e57609c00eb3fcbd805cf02d46` |
| ARM64 suite-window-manager | `c3d164308ba510f20f07c763b2c17f6d29112c792e1306b8a759d6ab6a45175d` |
| ARM64 wm-client_test | `d978abff9335fe68e44a3031f00db3eb835f446a968a8274a56bc437e042dc0c` |
| x86 suite-window-manager | `b8db853ae8fea18c6992891eda50bf4042080141d859df6fa73672edca9de023` |
| x86 wm-client_test | `5d9906a74e31f1b702634165d4f2efdce88956087194bcff926f83b1b8e9c0bc` |

These are build artifacts, not disk-staging or boot evidence. The separately
directed measurement unit must still follow the line-185 disk identity gate,
using the label inventory above, and the line-182 load gate.
