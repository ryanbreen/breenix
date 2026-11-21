# ARM virtio read attribution for E24 24.15r

Coordinator ledger 520(r) governs this unit; 488 keeps the broader sleepable-context
redesign separate. The input capture is ARM run `20261118T021733Z-qemu-19157` at
`44df847c`, serial line 18001: observed-metadata EIO, generation 1074, node 55622.
It follows SIGKILL teardown during sessions/start-desktop. The old wrapper discarded
the driver's reason, so that capture cannot identify which driver branch returned.
This unit makes no physical-device-fault finding and runs no boot.

## Teardown and execution context

`sys_munmap` removes VMAs under PROCESS_MANAGER. Exit transfers its file VMAs to
`ProcessPageTable::retired_vmas`; deferred root retirement eventually drops them.
`memory/file_map.rs::Binding::drop` removes the reverse binding and queues writeback
and eviction through `BreenFs::request_eviction`. That callback only publishes a
signal; it does not read a node, wait for a device, or synchronously run the worker.
The deferred reclamation bracket can carry an additional preemption brake, but no
BreenFS node read was found in this binding-drop path.

`BreenFs::init_service` runs `commit_due` in the breenfs-commit kthread with one
preemption brake. `service_mappings` can reach `writeback_snapshot`, then
`mapping_io -> regular -> inode -> Files::inode -> CachedNodeStore::read_node`.
The mount exclusion is a sleeping MountLock and adds no preemption brake. A cold
16 KiB node is read through NodeDevice, the shared cache and the ARM wrapper.
The capture's node fits its 80,384-block image.

Unit o's failed-mount retirement also runs in that worker, under the same one-brake
bracket. Once failed, `service_mappings` skips storage writeback and uses MapState
retirement, including `discard_unobserved`; `mapped_nodes` only collects retained
handles. That failed-state path does not itself read the backing inode. The
original failure must precede its retirement behavior.

Ordinary foreground filesystem and mapping I/O is different: ARM syscall entry
holds one preemption brake and one KillCustody section. A concurrent SIGKILL can
mark that section pending before syscall return. This is reachable while desktop
processes are being killed, but the capture does not name the thread that performed
the failed read. It does not establish whether the failing reader was a syscall
or the commit worker.

## Read-error branch census

This table follows `VirtioBlockWrapper::read_block/read_blocks`, then the MMIO
read entry points, gate, submit, completion wait and finish functions. “Worker”
means the queued mapping-service path above, before an integrity latch.

| Branch | Reach from teardown/mapping service | Attribution |
|---|---|---|
| Wrapper checked byte multiplication or short buffer | Its node caller supplies NODE_SIZE bytes and 32 sectors on a 512-byte disk; would require a different caller/geometry defect | Existing validation error; no driver request |
| Wrapper end beyond device capacity | NodeDevice checks the node span against the mounted geometry; not explained by node 55622 in this image | Existing range error; no driver request |
| Invalid device index / uninitialized state | Root wrapper is created only for initialized index 0; the driver has no runtime device-removal path | `device-state`; no request |
| Empty/non-sector read length or end beyond capacity | Fixed node size and NodeDevice geometry exclude it on the traced path | `read-length` / `read-range`; no request |
| Chunk start beyond capacity | Outer range check and bounded sector increments exclude it for this node | `chunk-range`; no request |
| No current thread and IRQs disabled | Worker and syscall both have current threads; not their context | `irq-unavailable`; no request |
| Gate already wedged, or wedged before/after its wait | Reachable only after a previous request was abandoned; cannot be the first abandonment | `gate-wedged`; no new request |
| Contended gate cannot sleep | Production predicate requires a current thread, preempt_count > 0, and initialized timer. Worker/syscall normally have count 1; binding drop only queues this read | `gate-cannot-sleep`; pre-issue context refusal |
| Waiter cannot be prepared | WaitQueue can fail allocation, lose its current-thread/scheduler publication, or be refused by Thread::must_not_sleep for pending SIGKILL. The syscall case has a deterministic model; worker is not a killed user thread | `gate-publish-refused`; pre-issue refusal, not media evidence |
| Kill already claimed the caller | New custody acquisition refuses before taking the gate | `gate-kill-claimed`; no request |
| Submit length exceeds negotiated DMA limit / invalid alignment | read_sectors splits at max_transfer_bytes, negotiated in whole sectors; not expected for this node | `submit`; no notify from that validation branch |
| Submit sector end beyond capacity | Earlier checks exclude it without geometry/state corruption | `submit`; no notify |
| MMIO probe says device disappeared | Possible transport/device-state candidate; not established by the capture. Descriptors have been prepared but queue notification is refused | `submit`, exact driver reason retained |
| Device failure while completion is outstanding | Possible candidate: status u32::MAX, DRIVER_OK absent or DEVICE_NEEDS_RESET set | `wait-completion`, gate wedged after the receipt snapshot |
| Non-sleeping bootstrap wait reaches its existing limit | wait_sleeps is true for the ordinary initialized worker/syscall at count 1. No such transition is derived for this teardown path | `wait-completion`, original limit unchanged |
| Wake without completed descriptor / without status | Would require a broken completion receipt or publication ordering; no deterministic lost-receipt interleaving found in the traced path | `take-completion`, consumed fields retained |
| Unexpected descriptor | Single-request driver submits chain head 0; a nonzero returned head is a completion/ring defect candidate | `finish-descriptor` |
| Non-OK virtio request status | Real failed request candidate; original status byte is retained | `finish-status`; still latches metadata EIO |

The request-gate predicate actually tests `preempt_count > 0`, not equality to one.
Its wait releases exactly one brake. Higher nesting and ordinary kthreads entered
at count 0 remain part of 488's separate explicit-context work; this repair changes
neither that predicate nor Completion's preemption policy.

## Deterministic refusal and source repair

`tests/virtio_read_context_test.py` compiles the production gate, KillCustody atomic
entry/drop and Thread::must_not_sleep predicate. It forces an already-owned gate,
a syscall custody count of one and a pending SIGKILL. Its scheduler double applies
the production no-sleep predicate when publishing the wait. Before the repair,
the gate returns “waiter publication refused”; the metadata check in
`tools/breenfs-host-tests/src/metadata_failure.rs` separately shows a pre-issue
refusal becoming EIO at the production observed() boundary.

The gate now enters nested KillCustody before queuing and retains it in its
request guard through completion and unlock. This lets the queued/owning caller
finish its device ownership before SIGKILL takes it, as the existing custody
contract specifies. A kill-claimed caller takes no gate. No preemption counter,
retry, timeout, read re-issue, completion handshake or IRQ producer was changed.
The guard is shared by reads, writes and flushes because those operations use the
same MMIO request ownership.

The typed read receipt distinguishes pre-issue context refusals from device
errors. Node metadata reports those refusals as `io-context` and returns EAGAIN
without an observed-metadata latch. The real non-OK request-status fixture still
returns EIO and latches read-only. Raw driver write errors retain their existing
interface; this unit's typed attribution applies to reads.

The receipt includes reason, stage, device/sector/length, current thread,
preemption and kill-custody state, interrupt/timer state, gate state, pending and
completed tokens, waiter thread, completed descriptor/status, ring indices and
MMIO status. It is sampled on failure before cleanup; finish_read preserves
fields consumed by take_completed_request. A refusal without gate ownership
samples the current owner's state concurrently. Serial and filesystem report
use the same formatter. A non-latching context refusal emits one failure-only
serial receipt per mount; later propagation of the refusal does not repeat it. No success-path serial output was added.

## Evidence limits and ARM gap

The deterministic model reproduces a pending-kill refusal defect; it does not identify the mechanism
of serial line 18001. In particular, binding release itself does not perform the
metadata read, and the normal/failed mapping worker has a different kill context.
Device-status failures, an earlier wedge and completion/ring anomalies remain
candidates to distinguish with the new receipt. No speculative completion repair
was made. Issue #3065 remains open for that runtime attribution.

This unit has host acceptance only, as its brief requires. BreenFS-root aggregate
acceptance on ARM remains pending, including coordinator 520(s)'s observations;
the R240 acceptance and Parallels/VMware catch-up remain pending as well. No board
cell is established here.
