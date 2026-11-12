# A.3 active GPU command attribution and replacement sample windows

Coordinator ARM ledger line 233(a)–(b) authorizes this observation unit. The
runtime mechanism remains open in [issue #2890](https://github.com/ryanbreen/breenix/issues/2890),
linked from #2872. This unit performs no boot, deployment, behavior repair,
deadline change, PR creation or merge. Line 233(c)'s measurement belongs to a
later unit, subject to its gates. PR #2874 remains the existing draft.

## Active guest path

`gpu/virgl/mod.rs::Request::new` captures the requesting PID/TID before a request
can enter the backlog. `Device::send` retains that identity, context, control
header type, header/payload sizes, queue sequence and wire fence before queue
publication. Deferred sending by kgpu therefore retains the caller. Worker-built
synchronous requests with a context inherit its initiating identity when their
constructor recorded kernel work; this includes catalogue warm-up and progress
readback. Kernel commands without such a context retain their kernel identity.
PID is the current thread's owner PID, matching the existing accounting journal;
clone workers can have distinct owner PIDs. TID identifies the requesting thread.

`Device::reap` stamps the device response after observing its returned buffer.
A reset/drop does not manufacture a fence-return timestamp. `fence_return_ns=0`
therefore retains a pending/unobserved response; for unfenced commands (`fence=0`),
the field is their response-observation time. This is a guest boundary, not a
host GL finish timestamp; core fence validation happens separately.

`gpu/mod.rs::ring_doorbell` records entry and return around the existing MMIO
write, outside the GPU core lock. `through` is the highest published queue
sequence covered by that kick. Join it to command `queue_seq`; a kick may cover
several commands and its PID/TID can be the service thread. The command rows
supply originating-client identity. The queue sequence continues across device
resets. Sampling does not change queue ordering or completion handling.

The journal shares the legacy ring's versioned atomic slot implementation. Its
2048 slots retain command and doorbell rows together, with overwritten/lost
counts. Stale completion cannot replace a newer reservation, as the host ring
check exercises. It is independent of accounting-window churn and adds numeric
atomic updates on submission/completion. Snapshot formatting and allocation
remain on the proc read path. Observation is ARM-only; x86 compiles out the
stamps and identity fields.

The existing root-only `/proc/breenix/window-stall` export appends these rows;
the existing suite dump supplies `VALUE wm-`:

```text
VALUE wm-gpu-core-command-ring retained=<n> overwritten=<n> lost=<n>
VALUE wm-gpu-core-command seq=<n> submit_ns=<ns> fence_return_ns=<ns> pid=<pid> tid=<tid> context=<ctx> type=<type> cmd_bytes=<bytes> payload_bytes=<bytes> queue_seq=<n> fence=<n>
VALUE wm-gpu-core-doorbell seq=<n> begin_ns=<ns> return_ns=<ns> pid=<pid> tid=<tid> through=<queue-seq>
```

Numbers are decimal, including context and type. Sizes describe command and
payload descriptors, not backing-resource transfer volume. The export is a
bounded snapshot; overwritten/lost rows are missing evidence.

## Host windows

For `recovery/stack-restored` and `recovery/supervisor-restarts`, the serial
watcher starts a sample on the complete CASE START line and starts another as
each sample child finishes or reaches its watchdog. The existing three-second,
one-millisecond sample invocation and QEMU pidfile authority remain in use.
Windows have distinct `.0001.sample.txt`, `.0002.sample.txt`, … names under the
existing Vigil `qemu-samples/` link. Other recovery/click cases retain one window.

The existing producer's `SUITE window-manager CASE <case> PASS|FAIL|SKIP ...`
record closes the case. An explicit `CASE window-manager <case> END` is also
accepted. An unterminated fragment remains pending. At the end, or watcher cleanup,
only the saved sample child handle is signalled/reaped; QEMU is the sampling
argument, not the signal target. SIGINT asks sample to write the partial final
report, with the existing bounded wait and kill fallback for an unresponsive
child. A host experiment on a sleep process created by this unit found SIGTERM
returned -15 with no report, while SIGINT returned 0 with a call graph. Both
sample children and the target were reaped; no QEMU was involved.

JSON adds `sample-case-start` and `sample-case-end` events. Existing
`sample-start`/`sample-stop` rows now carry `window`. Each stop includes
`host_start_unix_ns`, `host_start_monotonic_ns`, `host_end_unix_ns` and
`host_end_monotonic_ns`, plus the recorded sample PID, case and QEMU PID. Start
rows' `host_unix_ns`/`host_monotonic_ns` are captured before spawning; stop's end
fields are captured after reaping. These bound the sample process lifetime,
including startup and symbolication. Inter-window gaps and the actual sampling
interval must be read from these records and the sample reports; these are not
per-stack timestamps or measured mutex-owner intervals. Failed/missing windows
remain missing evidence for the later measurement's gate.

## Verification

Check-first commits: `66c26d31` and `efeaaa18`. The first run of
`python3 tests/a3_gpu_lock_attribution_test.py` fails twice on the input head:
missing active-send stamps and no repeated sampling past three seconds. The two
checks subsequently pass in scoped runs. The journal check executes the
production atomic slots and active journal, including pending/fence-return,
doorbell return, eviction and stale completion; source checks pin the active
send/reap, origin capture/inheritance and export calls. The sampler check uses
fake process handles and clocks, repeating both replacement cases through their
end, preserving unique files and checking start/end times and owned-child
cleanup. Its added SIGINT assertion fails before the signal repair and passes
after it.

The first journal run after implementation found a fixture module-import error,
corrected without changing an assertion. The existing observation checks pass:
trace inventory and legacy ring in their original scoped run, and sampler cleanup
after giving its concurrent fake children distinct PIDs. Its cleanup assertions
are retained, with an explicit CASE END before the artificial infinite watchdog
tick. After the SIGINT change only the two affected sampler checks were repeated.
No suite or build is rerun for commit/push or this report.

Logs: `/tmp/a3-u21-red.log`, `/tmp/a3-u21-command-green.log`,
`/tmp/a3-u21-interrupt-red.log`, `/tmp/a3-u21-sample-green.log`,
`/tmp/a3-u21-legacy.log`, `/tmp/a3-u21-legacy-sampler-final.log`.

The production ARM64 release kernel builds with four jobs under `taskpolicy -b`;
it retains the prior pinned-core future-compatibility advisory and adds no
compiler warning. The FP/SIMD guard passes: 0 forbidden, 32 allowlisted. The ARM
ELF contains the three new journal labels. The scoped x86 release build (`cargo build --release --bin qemu-uefi`)
passes without warnings under the same four-job background policy. Build logs are `/tmp/a3-u21-arm-build.log`
and `/tmp/a3-u21-x86-build.log`.

Claim-lint invocations so far:

- `scripts/claim-lint.py --files tests/a3_gpu_lock_attribution_test.py`: exit 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u21-check-msg`: exit 0.
- `scripts/claim-lint.py --files /tmp/a3-u21-issue.md`: exit 0.
- `scripts/claim-lint.py`: exit 1 (two absolute comments), then exit 0 after wording corrections.
- `scripts/claim-lint.py --files tests/a3_gpu_lock_attribution_test.py tests/a3_gpu_lock_observe_test.py`: exit 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u21-interrupt-msg`: exit 0.
- `scripts/claim-lint.py`: exit 0 after the SIGINT implementation.
- `scripts/claim-lint.py`: exit 1 for two report wording findings, corrected before the final invocation.

Final lint/commit/push results are recorded in the ledger. The older TSV's data
extension is explicitly skipped by claim-lint; this unit does not edit it.
