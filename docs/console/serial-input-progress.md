# Serial input progress capture (#2572)

This change adds diagnostics for an unresolved stopped serial consumer. It does
not repair a proven lost wake. The RX queue remains 256 bytes; the queue-full
warning, sender retries and 3,000 ms acknowledgement bound retain their behavior.

Atomic counters record successful RX pushes/pops, stream and executor poll
entry/return, TaskWaker enqueue/unpark entry/return, packet admission and refusal,
input-service entry/return, and ACK output entry/return. Last accepted and
completed ACK sequence numbers accompany those counts. The command task, input
service and console renderer retain their current execution site, including
their stream, service and render sleep sites. Nested sites restore their caller's
site on return. The renderer's shared VT helpers record only the renderer's site;
a syscall cannot overwrite a suspended renderer's witness.

The first RX overflow claims fixed storage once. It freezes these witnesses and
the actual RX/task queue lengths, executor TID and parked flag, and scheduler
observations for the executor, input service and renderer. It also samples up to
16 outstanding saved-context census TIDs with their PID and thread name. These
are a superset of census candidates, including legitimate waiters; `census_total`
reports truncation. Thread names retain at most 32 bytes, with `cut=true` when
truncated. Scheduler probes try each existing owner lock once, revalidate its
owner, and never wait. An unavailable state/name means that probe could not
inspect the thread; `queued=false` alone then proves nothing. Held-wake bits
retain the run-queue index's raw encoding.

These are concurrent observations over a bounded sampling interval, not a
globally atomic scheduler image. Counter increments can race a sample. Task
queue length uses `usize::MAX` when the executor has not registered its queue.
The first three thread records, when present, identify the executor, input
service and renderer in that order; remaining records map census TIDs.

The IRQ records no new output, allocates nothing, and acquires no blocking lock.
Later overflows neither wait for nor overwrite the first snapshot. The existing
`kstrandd` task emits one `SERIAL_INPUT_STALL` header, the fixed counter list as
`SERIAL_INPUT_PROGRESS` records, and at most 19 `SERIAL_INPUT_THREAD` records,
through the existing deferred log ring. A passing run emits none of them. The
frozen snapshot remains in memory even if that independent reporter cannot run.
ARM64 builds the shared input/VT counters and markers; the overflowing x86 RX
queue and its failure reporter are x86-specific.

The deterministic host checks use the production recorder and RX body:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test --manifest-path tests/serial_rx_host/Cargo.toml
```

They check nested-site restoration, silent success, first-snapshot retention,
one-time consumption, and overflow after exactly 256 retained bytes. The old RX
body fails the snapshot assertion. That failure proves missing diagnostics, not
the initiating defect in #2572. Hardware acceptance belongs to the subsequent
unit; #2572 stays open.
