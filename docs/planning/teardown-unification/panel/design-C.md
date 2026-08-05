# Teardown-Unification Design — Lens C: Invariant First

**Goal:** Route normal exit, fatal fault, fatal signal/SIGKILL, designated-init death, and CLONE_VM group death through one reviewable `request -> quiesce/quarantine -> commit -> grace -> reclaim` lifecycle, using the machinery already on `main` after PR #494 and adding only the state needed to make the invariant checks structural.

**Architecture:** A dying process row is its own retirement record. The first exit request seals the selected member or thread group without moving or freeing resources; scheduler-owned threads then become non-runnable and leave their own TTBR0 and kernel stacks; only after that handoff does the row receive the existing two-epoch retirement fence. A small, non-logging teardown kthread advances one row by one bounded action at a time, and physical release is refused in release builds unless the complete liveness proof passes.

**Tech stack:** Rust `no_std`, existing `ProcessManager`, `Scheduler`, per-CPU TTBR0 shadows, scheduling epochs, `is_kernel_stack_slot_live`, kthreads, SGI_RESCHEDULE, lock-free tracing, QEMU, and Parallels.

---

## 0. Scope, baseline, and size budget

This design was derived from `teardown-design-package.md` and the live tree at `main = eebc8868`. The following live facts were rechecked:

- `sys_exit`/`sys_exit_aarch64` converge on `ProcessScheduler::handle_thread_exit`.
- The four AArch64 EL0 fatal-fault sites already perform local TTBR0 quiescence, single-PID scheduler quarantine, unconditional two-epoch process-resource deferral, and idle redirection.
- `send_signal_to_process(SIGKILL)` still calls `Process::terminate(-9)` under the PM lock and eagerly walks CoW mappings.
- `signal/delivery.rs` has two more direct `Process::terminate` calls; x86_64 `interrupts/context_switch.rs` has another.
- CLONE_VM is represented by separate `Process` rows with an effective group ID of `thread_group_id.unwrap_or(pid)` and a raw shared-root reference in `inherited_cr3`.
- Both AArch64 exec implementations install a new page table without clearing `thread_group_id` or `inherited_cr3`.
- There is no runtime init identity. Production teardown uses PID-1 literals, while `init_shell` independently tests `getpid() == 1`.
- Fork and clone transfer AArch64 kernel-stack ownership to the scheduler; fresh spawn and direct-init/test-disk launch do not.
- Process and stack reclamation still run at the top of `schedule_from_kernel`; the fault-intent ring also drains there.

PR #418's merge diff was measured from local history as **5 files, 166 insertions, 70 deletions (236 lines of churn)**. Each phase below targets no more than about 220 lines of churn. If a phase crosses 236 lines during implementation, it must be split at the named internal seam before review; the size ceiling is a gate, not a suggestion.

This is an AArch64 teardown design with shared lifecycle changes made architecture-honest. x86_64 keeps an explicitly simpler architecture hook where it lacks AArch64's TTBR0/two-epoch proof, but it uses the same first-status, notification, FD, reparent, and row-removal rules.

## 1. The invariant set

The grave branch's machinery is not the starting point. Its 28 distilled invariants are. The following is the required set, restated as implementation contracts and assigned stable IDs used throughout this design.

### Execution-context invariants

| ID | Required invariant | Concrete gate |
|---|---|---|
| TI-01 | No page-table walk, frame free, heap allocation, or blocking teardown lock is reachable from an AArch64 exception-return tail or idle loop. | Source-structure test rejects reclaim/close/reparent calls in `check_need_resched_and_switch_arm64`, `schedule_from_kernel`, and assembly return tails. Runtime `RECLAIM_CONTEXT_VIOLATIONS` must remain zero. |
| TI-02 | No logger, serial formatter, or string formatting is reachable from an exception tail, PM-held fault path, idle loop, or teardown-worker loop. | Call-graph/source scan plus zero `TEARDOWN_LOG_CONTEXT_VIOLATIONS`; worker cleanup reports only atomic counters/lock-free trace events. |
| TI-03 | PM-held/DAIF-masked teardown work is non-allocating, non-blocking, and takes no second lock; group work under PM is flag-only. | Debug assertions around PM transaction entry/exit; source test forbids `Vec::push`, `clone`, FD close, frame APIs, scheduler APIs, and logging in the request/commit bodies. PM iterations and maximum observed scan length are counters. |
| TI-04 | A function's early-return safety gate runs before newly added heavyweight work. | Source tests pin `PREEMPT_ACTIVE` and equivalent checks ahead of any new call; no teardown drain is added before them. |
| TI-05 | All six gold-master regions remain byte-identical. | Hash gate against the pre-phase `main` regions on every phase. |

### TTBR0, stack, and grace invariants

| ID | Required invariant | Concrete gate |
|---|---|---|
| TI-06 | A TTBR0 root is named before installation; `saved`/`next` are cleared or republished only after the hardware switch. | Source test checks Rust writer allowlist and instruction ordering at both `syscall_entry.S` install sites; transition trace counter must show zero under-report states. |
| TI-07 | Reclaim's local liveness observation reads `TTBR0_EL1`; remote CPUs are covered by a proven conservative shadow protocol. | `RootProof` records local-hardware, remote-shadow, cached-thread, live-row, and grace blocker masks. Reclaim refuses on any nonzero mask. |
| TI-08 | Only the CPU leaving an address space mutates that CPU's TTBR0 state; a remote killer never quiesces its own unrelated root. | The only exit primitive is argument-free `leave_process_ttbr0()`. Source gate rejects calls to it from PID-targeted request code. |
| TI-09 | Every idle redirect performs a real hardware leave before its shadows stop naming the old root. | Debug assertion immediately after the leave checks hardware kernel root and zero shadows; source test covers both idle redirect helpers. |
| TI-10 | A thread cannot be reclaim-eligible while still running on its own stack or root. | `ThreadState::ExitPending` is non-runnable and non-reclaimable; only neutral-stack or completed-handoff code may set `Terminated`. Release-mode state checks refuse reclaim, with matching debug assertions. |
| TI-11 | Reclaim uses hardware/shadow/cached-root and live-stack evidence, not `cpu_state.current/previous/idle` name matching. | `RootProof` and `StackProof` are the only release gates; source test rejects CPU-name bookkeeping in eligibility functions. |
| TI-12 | Acquire epoch observations precede volatile/plain liveness observations, including the zero-online edge. | `RetirementFence { epochs, online_mask }` and `RetirementSnapshot`; an empty online mask never elapses. Every liveness function requires an already-acquired snapshot or is called after its explicit release-mode gate. |

### Exit correctness invariants

| ID | Required invariant | Concrete gate |
|---|---|---|
| TI-13 | Exit cleanup is one-shot on every path. | `Live -> ExitRequested` is the only status-writing transition. `EXIT_FIRST_REQUESTS + EXIT_REPEAT_REQUESTS == EXIT_REQUESTS`; CoW/FD/resource completion counters never exceed first requests. |
| TI-14 | Victim and scope predicates are centralized; no per-site main-thread/CR3 heuristics. | All callers construct an `ExitIntent`; only `ProcessManager::request_exit_locked` resolves member/group scope. Fatal-fault attribution uses TID as identity and CR3 only as a cross-check. |
| TI-15 | Every committed exit reparents and produces the parent/test wake/report obligation, including repeated request sequences. | Durable per-row work bits are installed with the first request and cleared only after completion. Repeat requests cannot create a second obligation. End-of-test equalities gate notification counts. |
| TI-16 | No owned state is extracted before all early-return/idempotence checks. | Request only changes scalar state and intrusive links. FDs, stacks, tables, and old-table vectors stay in the row until a worker claim after eligibility. |
| TI-17 | Return-contract changes cannot silently skip mandatory tails. | `#[must_use] ExitRequestResult` has explicit `First`, `Repeat`, `Missing`, `AttributionUncertain`, and `InitDeathLatched` cases; each caller is exhaustively matched. Mandatory work is durable in the row, not encoded in an optional receipt. |

### Locking invariants

| ID | Required invariant | Concrete gate |
|---|---|---|
| TI-18 | FD closure occurs outside PM on both architectures. | Worker takes exactly one `FileDescriptor` under PM, drops PM, then closes it. `FD_CLOSES_UNDER_PM` must remain zero. |
| TI-19 | Broadcast TLBI never runs inside PM/scheduler/IRQ-off fault critical sections. | TLBI site allowlist; `TLBI_WITH_PM_OR_SCHED_HELD` counter is fatal in tests. Batch invalidate/release happens in the worker with no PM/scheduler guard. |
| TI-20 | A lock reachable from fault and normal contexts cannot self-deadlock a preempted owner. | Reclaimer pins preemption while using `FRAME_METADATA`; the AArch64 CoW path uses nonblocking metadata lookup and returns `Retry` without PTE mutation on contention. No metadata guard spans allocation, copying, unmap, or map. |

### Overflow and diagnostics invariants

| ID | Required invariant | Concrete gate |
|---|---|---|
| TI-21 | Bounded-queue overflow never panics under a lock or IRQ mask. | Row-resident teardown queue is capacity-free. Existing fault-intent overflow increments a readable counter and never panics. |
| TI-22 | One wedged CPU cannot turn a reclaim-capacity limit into a fatal kernel event. | No separate bounded reclaim collection exists. A blocked row rotates to the tail; unrelated rows continue. Wedged-CPU test requires progress for unrelated roots and zero panic. |
| TI-23 | Every diagnostic counter has a reader. | Source test enumerates teardown counters and requires a normal-context snapshot reader plus panic-trace exposure. |
| TI-24 | Diagnostic ranges/constants come from their owning module. | Source test rejects literal kernel-stack pool ranges in teardown diagnostics. |
| TI-25 | High-value fatal evidence is emitted before large trace buffers. | Existing postmortem order stays frozen; the compact teardown snapshot is inserted before trace-buffer output, using the existing section wrapper. |

### Architecture and honesty invariants

| ID | Required invariant | Concrete gate |
|---|---|---|
| TI-26 | Shared changes and architecture differences are explicit. | Both-architecture builds and an architecture-hook table in code comments/tests; no `cfg` branch may silently omit first-status, notification, FD, or row-removal rules. |
| TI-27 | Commit/PR descriptions name reachability and removed guards. | Per-phase review template includes path list, lock-context change, and assertion/counter deltas. |
| TI-28 | A safety comment must be true on every reaching path. | Source tests cover the TTBR0 writer/idle-return claims; review requires every new `SAFETY`/`CRITICAL` comment to cite the enforcing state or predicate. |

Two corpus findings describe pre-existing whole-kernel violations outside the teardown surface: the Tier-1 raw TTBR0 writer in `kernel/src/syscall/time.rs`, and broader frame-metadata behavior. The complete implementation plan includes the minimal changes needed to close those dependencies; the Tier-1 edit is explicitly operator-gated. Until that approval lands, the absolute form of TI-06 remains a declared residual rather than a claimed fact.

## 2. Unified architecture

### 2.1 Lifecycle

```text
                         one PM transaction; first status wins
        LIVE ───────────────────────────────────────────────> EXIT_REQUESTED
          │                                                        │
          │                                                        │ local leave, scheduler quarantine,
          │                                                        │ SGI, off-stack finalization
          │                                                        v
          │                                                   QUIESCED
          │                                                        │ batch barrier; capture fenced
          │                                                        │ two-epoch target
          │                                                        v
          │                                                 EXIT_COMMITTED
          │                                                        │ full RootProof / StackProof
          │                                                        v
          │                                                   RECLAIMED
          │
          └──────────────── waitpid is independent ───────────────> REAPED

                           REAPED && RECLAIMED && work_bits == 0
                                             │
                                             v
                                       ROW REMOVED
```

`EXIT_REQUESTED` is the seal. It is not reclaim eligibility. It records the first exit status, source, scope batch, and durable phase-2 obligations, and it makes dispatch and clone/exec admission fail. Page tables, old exec tables, user stack, FDs, and the process row remain owned exactly where they were.

`QUIESCED` means every scheduler-owned thread in the request batch is either already off-stack `Terminated` or has completed the `ExitPending` handoff. Only then does the worker stamp the existing two-epoch grace and publish `EXIT_COMMITTED`.

`RECLAIMED` is earned, never requested. The worker must pass the release-mode root/stack proof. `waitpid` changes only the independent `REAPED` bit; it no longer drops a process row or kernel stack.

### 2.2 Minimal new state

The design adds no grave allocation, no graveyard stack, no `Arc<AddressSpace>`, no address-space generation object, and no general-purpose reclaim capability token.

`Process` gains only row-resident lifecycle state:

```rust
enum ExitLifecycle {
    Live,
    Requested {
        status: i32,
        source: ExitSource,
        batch: u64,
        work: ExitWorkBits,
    },
    Committed {
        status: i32,
        source: ExitSource,
        batch: u64,
        fence: RetirementFence,
        work: ExitWorkBits,
    },
    Reclaimed {
        status: i32,
        work: ExitWorkBits,
    },
}
```

It also gains `reaped: bool`, `teardown_next: Option<ProcessId>`, and an `on_teardown_queue` bit. `ProcessManager` gains `teardown_head`, `teardown_tail`, `next_exit_batch`, and the runtime init designation. The process row is the intrusive queue node, so enqueue is pointer/ID assignment only; no collection grows during exit.

`ThreadState` gains `ExitPending`. `ExitPending` is removed from ready queues and cannot be dispatched or reclaimed. A scheduler-owned AArch64 thread remains the sole owner of its kernel stack.

`RetirementFence` extends today's `[u64; MAX_CPUS]` with the captured online mask. `RetirementSnapshot` is a small stack value proving that Acquire epoch loads and an unconditional Acquire fence have executed before liveness reads.

### 2.3 The only request API

Every death source constructs:

```rust
struct ExitIntent {
    victim: ExitVictim,       // exact PID or exact TID; never CR3-only
    scope: ExitScope,         // Member or ThreadGroup
    status: i32,
    source: ExitSource,
}

#[must_use]
enum ExitRequestResult {
    First { batch: u64, status: i32 },
    Repeat { batch: u64, first_status: i32 },
    Missing,
    AttributionUncertain,
    InitDeathLatched { batch: u64, status: i32 },
}
```

`ProcessManager::request_exit_locked(intent)` is the only state transition. It runs with PM already held, does not acquire the scheduler or any other lock, does not allocate or format, and does not move resources.

Victim resolution is exact:

- `kill(pid, ...)` uses the explicit PID row.
- self exit uses the current scheduler TID, resolved through `find_process_by_thread`.
- a fatal fault uses the stack-slot owner TID and current per-CPU scheduler TID; they must agree when both are available. The process row's `cr3_value()` is only a root-consistency cross-check. A CR3 owner row is never used as thread identity.
- if fault attribution is incomplete or contradictory, the handler performs the local safe redirect and publishes the existing deferred TID intent. It does not trigger the init panic or select a CR3-owning parent. The worker retries attribution in normal context.

For `ExitScope::Member`, exactly one live row is marked. For `ThreadGroup`, PM computes the target's effective group ID and, while retaining the same guard, marks every current live member with the same batch ID. That same transaction is the group seal. A concurrent `sys_clone` either inserted its row before the transaction and is included, or acquires PM afterward and sees a non-live parent. No PID snapshot escapes the lock.

The first transition stores the status and work bits. A repeat returns the stored batch/status and performs no new cleanup, signal, wake, report, or resource action. This differs from the rejected notification-suppression proposal: here the first transition atomically creates a durable notification obligation that every producer shares, so a repeat request is never the only path capable of notifying.

### 2.4 Local quiescence and scheduler quarantine

There are two distinct operations because a remote killer cannot write another CPU's TTBR0:

1. `leave_process_ttbr0()` is argument-free and local. Self exit and local fatal fault invoke it before handing off. It installs the kernel root without a broadcast TLBI, executes the ISB, then clears `saved_process_cr3` and `next_cr3`. The two idle redirect helpers invoke the same operation.
2. `Scheduler::request_owner_exit(pid)` makes every scheduler-owned thread for that row non-runnable. An off-CPU thread whose stack is proven non-live can become `Terminated` immediately. A current/live thread becomes `ExitPending`, is removed from ready queues, and its CPU is included in the returned fixed CPU mask.

The caller or worker sends the existing `SGI_RESCHEDULE` to that mask. On the target CPU, the normal scheduling path treats `ExitPending` like a mandatory switch-away condition and calls the local TTBR0 leave. It does not return the thread to EL0.

`ExitPending -> Terminated` has only three legal edges:

- the existing `exit_schedule_trampoline`, after explicit `sys_exit` has pivoted to the neutral per-CPU scheduler stack;
- the next-scheduler-entry completion of a normal switch-away, after the previous handoff slot proves the old exception return completed;
- immediate quarantine of an already-off-CPU thread after `StackProof` says its slot is not live.

This is intentionally different from the grave's r20 `ExitPending` failure. No worker waits forever for an `ExitPending` thread that has been removed from all queues: a running thread is forced through a scheduler handoff by SGI, and the handoff completion—not the reclaimer—publishes `Terminated`. A non-running thread is finalized at quarantine only after its stack is already proven absent.

Dispatch gains a generic process-lifecycle gate in `set_next_ttbr0_for_thread`: `Creating`, `ExitRequested`, and `Terminated` rows cannot arm TTBR0 or reach EL0. This is not a CPU0-specific branch and does not touch the frozen EL0-dispatch block.

### 2.5 Commit and grace

The teardown worker advances one queued row at a time:

1. Request scheduler quarantine for the row's owner PID; send SGI to any current CPU.
2. Requeue the row until all scheduler-owned threads for its exit batch are `Terminated`.
3. Under one short PM transaction, verify every row in the batch is still `Requested` with the same batch and has completed quarantine; capture one `RetirementFence`; transition all batch rows to `Committed`.

No table, stack, FD, or child state moves at commit. Capturing the fence after batch quiescence makes the required order explicit: seal/request, local leave and scheduler quarantine, grace stamp, reclaim.

An online mask of zero is invalid. The release build refuses to commit/reclaim and increments `RETIRE_EMPTY_ONLINE_MASK`; the paired debug assertion makes the boot assumption visible without using it as the sole safety mechanism.

### 2.6 Row-resident worker

A small `teardown_worker` kthread is required because the existing safe fork drain is not a liveness guarantee, while the existing scheduler/idle drains violate TI-01. The worker is narrower than the grave's `kreclaimd`:

- it owns no separate grave objects or lock-free graveyard;
- it scans no global resource vector;
- it performs one externally visible action per row visit;
- it never logs or formats strings;
- it uses only the row's intrusive queue link and scalar work bits;
- a blocked row rotates to the tail, so a wedged CPU does not head-of-line block unrelated reclaim.

Actions, in order, are:

1. quarantine/finalize one owner PID;
2. commit one fully quiesced batch;
3. reparent one child (after the `children` mirror is removed, `parent` is authoritative);
4. set/complete one SIGCHLD + wait/pause wake + BTRT report obligation;
5. take one FD under PM and close it after dropping PM;
6. claim and clean one eligible process resource set;
7. detach and drop one eligible scheduler `Thread`/kernel stack outside the scheduler lock;
8. remove one row satisfying `reaped && reclaimed && work_bits.is_empty()`.

The worker sleeps with a generation-checked park. Producers increment `TEARDOWN_WORK_GEN` and use a dedicated coalescing deferred-kthread wake: clear the worker's parked flag lock-free, publish its TID in a single atomic wake slot, and let the scheduler consume that slot under its existing lock. The generation is rechecked after the worker advertises parked state, closing the lost-wake window. If work exists but is grace-blocked, the worker uses one timer block before retrying; it never creates more than one outstanding timer wait.

The deferred wake contains no teardown operation—only a bounded thread unblock. Fault handlers never call `kthread_unpark`, take `SCHEDULER`, allocate a `Work`, or push into a growable queue.

### 2.7 Reclaim proof

For a root `R`, release requires every clause below:

1. a valid `RetirementSnapshot` whose captured online CPUs have reached their two-epoch targets;
2. local `TTBR0_EL1 & ROOT_MASK != R` on the worker CPU;
3. every captured online CPU's `saved_process_cr3` and `next_cr3` differ from `R`;
4. no scheduler thread has `cached_ttbr0 & ROOT_MASK == R`;
5. no live or creating process row names `R` through `page_table` or `inherited_cr3`.

There is no architectural remote `mrs TTBR0_EL1`; clause 3 is valid only because Phase 2 makes the shadows a conservative superset. That limitation is stated in the code comment and test.

The proof is checked in release mode. Failure leaves the resources in the process row, rotates it, and increments a blocker-specific counter. Immediately before physical release, the same predicate is repeated as a debug assertion to catch accidental divergence between the gate and diagnostics.

After proof, the worker takes `stack`, `page_table`, and `pending_old_page_tables` under PM with `Option::take`/`mem::take`, marks the row's resource claim in progress, and drops PM. It performs one broadcast TLBI for the claimed batch, drops the user stack, walks/decrements CoW pages, frees table frames, then marks `Reclaimed` in a final short PM transaction. No heavy destructor runs under PM.

Kernel-stack release uses the same ordered snapshot rule plus `!is_kernel_stack_slot_live(top)` and `ThreadState::Terminated`. The scheduler detaches one eligible `Thread` under its lock; dropping and the 64-KiB stack scrub happen after the lock is gone.

### 2.8 Why this is the smallest sufficient mechanism

The row already has stable lifetime until `waitpid`; making reap an independent bit lets that existing allocation pin every heavy resource. That deletes the reason for a preallocated grave and a second ownership graph. The intrusive queue needs two manager IDs and one link per row, rather than a global growable vector or Treiber nodes. `ExitPending` is the only new scheduler state forced by TI-10; the worker is the only new execution context forced by TI-01 plus the requirement that reclaim make progress without a future fork.

The design deliberately retains raw `inherited_cr3`, the existing page-table boxes, the existing two-epoch primitive, scheduler kernel-stack ownership, and SGI_RESCHEDULE. It adds proof checks around them rather than replacing them with a broad identity/refcount rewrite.

## 3. Per-issue mechanisms

### 3.1 #491 — SIGKILL routing

`send_signal_to_process(SIGKILL)` stops borrowing `Process` mutably and never calls `Process::terminate`. It drops the PM guard, calls the unified request API with `ExitVictim::Pid(target_pid)`, `ExitScope::ThreadGroup`, status `-9`, and source `Sigkill`, then kicks the teardown worker and broadcasts SGI_RESCHEDULE to online CPUs.

The PM request atomically seals and marks the complete CLONE_VM group. The process row retains its page table and FDs. Scheduler quarantine makes each member non-runnable; SGI forces a currently executing member through the local leave/off-stack handoff. Grace is captured only after all members are quiesced. SIGCHLD, parent wake, BTRT status, FD close, and reclaim are durable first-request work, not caller-specific side effects.

The same conversion applies to fatal default actions in `signal/delivery.rs` and the direct x86_64 fault termination. A source-structure gate makes direct `.terminate(` calls outside the unified lifecycle a failure. `Process::terminate` is deleted once the last caller moves; `terminate_minimal` is reduced to the internal first-status state transition or deleted if the lifecycle enum subsumes it.

SIGKILL from `kill(-1)` or a POSIX process-group kill remains a loop over target PIDs, but each PID request is idempotent; a CLONE_VM group reached more than once shares the first batch and status. The separate POSIX `pgid` snapshot race is not redesigned here.

### 3.2 #464 — init identity and death policy

`ProcessManager` owns the only runtime designation:

```rust
struct InitDesignation {
    pid: ProcessId,
    death_policy: InitDeathPolicy, // Panic or ReapNormallyForHarness
}
```

All production reparenting and signal-exclusion decisions call `designated_init()`/`is_designated_init(pid)`. No production teardown branch compares with `ProcessId::new(1)` or a local `INIT_PID` constant.

PID 1 remains the userspace ABI for the real init, which is the smallest way to keep `init_shell`'s observable `getpid() == 1` guard coherent. PID 1 is reserved for the explicit init constructor rather than consumed from the ordinary monotonic allocator. The init process is built off-table with provisional PID 1; only after all fallible ELF, page-table, stack, and scheduler publication steps succeed is the row inserted/designated. A failed attempt leaves neither a row nor a designation and can retry PID 1. Ordinary/test process allocation begins at PID 2.

The boot path explicitly selects policy:

- real `/sbin/init`/interactive init: `Panic`;
- a test harness that must act as reparent anchor: `ReapNormallyForHarness`;
- tests needing no reparent anchor: no designation.

This is runtime policy, not a Cargo feature. A source test checks that the real designation is PID 1 and `init_shell` still keys on the same ABI. A creation-failure test injects failure after provisional PID selection, proves `designated_init() == None`, retries, and proves the successful init is PID 1.

If a certainly attributed exit request targets the `Panic` init, `INIT_DEATH_LATCH` records the first status/source and the worker is woken. The worker releases all locks, verifies interrupts/preemption are in the normal state, emits a lock-free trace record, then calls `panic!`. No panic is reachable under PM, scheduler, a fault handler, or DAIF-masked request code. An uncertain CR3/TID attribution can never set this latch.

On current AArch64, the panic handler parks only the panicking CPU; that SMP limitation is recorded in Residuals and Open Questions rather than being mislabeled as stop-the-world.

### 3.3 #471 — group seal and exec detach

The seal is the atomic PM transition of all current effective-TGID members from `Live` to `ExitRequested` with one batch ID. There is no separate group object or stale PID snapshot.

`sys_clone` must pass two admission checks while holding the same PM guard that publishes the child row:

- the parent row is `Live`;
- its group is not represented by an in-progress group-exit batch (equivalently, no effective-TGID member is `ExitRequested` for group scope).

If clone published a `Creating` row before the exit transaction acquired PM, that row is included and cannot become dispatchable. All user-thread creation paths register the scheduler thread initially non-runnable, publish the process row, and only then make it runnable; dispatch also refuses `Creating`/`ExitRequested`. This closes the insert-before-scheduler-spawn race without nesting PM inside scheduler or vice versa.

Both AArch64 exec implementations perform detachment at the successful, no-return-to-old-image commit point:

```rust
process.page_table = Some(new_page_table);
process.inherited_cr3 = None;
process.thread_group_id = None; // effective new singleton TGID is pid
```

The existing live-sibling guard stays. The clear happens only after all fallible new-image work succeeds and before PM is released. On exec failure, both fields remain unchanged. Exec also refuses a non-`Live` row. Therefore PM serializes the race: exec commits first and a later group exit excludes the new singleton, or group seal commits first and exec fails before installing/detaching.

Scope policy follows Linux's relevant distinction:

- `exit(2)`: member only;
- `exit_group`, SIGKILL, default-fatal signal, and fatal userspace fault: thread group;
- a member-only owner exit may leave live shared-root siblings; its row and page table remain pinned until the live-row clause of `RootProof` clears.

## 4. Gate-checkable observability

All producer-side observations are `AtomicU64::fetch_add(Relaxed)` or existing lock-free trace events. No producer formats. A normal-context snapshot reader and the existing panic trace dumper expose every counter.

Required counters:

- requests: `EXIT_REQUESTS`, `EXIT_FIRST_REQUESTS`, `EXIT_REPEAT_REQUESTS`, `EXIT_ATTRIBUTION_UNCERTAIN`;
- groups: `GROUP_BATCHES_SEALED`, `GROUP_MEMBERS_REQUESTED`, `CLONE_REJECTED_SEALED`, `EXEC_DETACHES`;
- scheduler: `THREADS_EXIT_PENDING`, `THREADS_OFFSTACK_TERMINATED`, `EXIT_SGI_SENT`, `EXIT_SGI_OBSERVED`;
- commit/reclaim: `EXIT_BATCHES_COMMITTED`, `PROCESS_ROWS_RECLAIMED`, `ROOT_BLOCKED_EPOCH`, `ROOT_BLOCKED_HW`, `ROOT_BLOCKED_SHADOW`, `ROOT_BLOCKED_CACHED`, `ROOT_BLOCKED_LIVE_ROW`, `RETIRE_EMPTY_ONLINE_MASK`;
- phase 2: `SIGCHLD_FIRST_SET`, `PARENT_WAKE_COMPLETED`, `BTRT_EXIT_REPORTED`, `FDS_CLOSED`, `CHILDREN_REPARENTED`;
- lifecycle: `ROWS_MARKED_REAPED`, `ROWS_REMOVED`, `REMOVE_REFUSED_INCOMPLETE`, `FAULT_EXIT_INTENT_DROPPED`;
- safety: `RECLAIM_CONTEXT_VIOLATIONS`, `FD_CLOSES_UNDER_PM`, `TLBI_WITH_PM_OR_SCHED_HELD`, `TTBR_UNDERREPORT_DETECTED`.

Test-end equalities are assertions, not dashboards:

```text
EXIT_REQUESTS = EXIT_FIRST_REQUESTS + EXIT_REPEAT_REQUESTS + missing/uncertain
EXIT_BATCHES_COMMITTED <= GROUP_BATCHES_SEALED + member_batches
PROCESS_ROWS_RECLAIMED <= EXIT_FIRST_REQUESTS
ROWS_REMOVED <= min(ROWS_MARKED_REAPED, PROCESS_ROWS_RECLAIMED)
FDS_CLOSED = expected_open_fds_at_first_request
SIGCHLD_FIRST_SET = parented_first_commits
PARENT_WAKE_COMPLETED = parented_first_commits
BTRT_EXIT_REPORTED = registered_first_commits
```

For every correctness gate, the release path first refuses unsafe action and records a blocker; the paired `debug_assert!` detects programming mistakes. No safety claim rests on a release-stripped assertion.

## 5. Numbered acceptance-criteria traceability

| # | Criterion | Phase/mechanism | Gate check |
|---:|---|---|---|
| 1 | Init designation occurs only after creation fully succeeds; no phantom PID. | Phase 5 explicit provisional-PID-1 init construction; designation after row and scheduler publication. | Failure injection at page-table, ELF, stack, and publication stages asserts no designation/row; retry succeeds as PID 1. |
| 2 | No panic/fatal action while PM is held with DAIF masked. | Phase 12 init-death latch; only worker panics after all guards drop. | `INIT_PANIC_WITH_LOCK` counter must be zero; test-injected init death records PM owner `None`, scheduler owner `None`, IRQ/preemption-safe snapshot immediately before panic. |
| 3 | Fatal fault victim must be certain; CR3 miss/heuristic must not panic. | Phase 10 common fatal-fault adapter uses stack-owner/current TID agreement; CR3 is cross-check only. | TID/CR3 mismatch injection increments `EXIT_ATTRIBUTION_UNCERTAIN`, does not set init latch, and safely redirects without killing a different row. |
| 4 | One source of truth for reparent init; no PID-1 teardown literals beside runtime designation. | Phases 5 and 7: `ProcessManager::designated_init`, parent relation is authoritative. | Source scan rejects `ProcessId::new(1)`/local `INIT_PID` in production teardown, wait, signal, and reparent code. |
| 5 | Kernel and userspace init guards agree. | Phase 5 reserves PID 1 exclusively for real init and retains `init_shell`'s observable `getpid()==1` ABI. | Cross-tree source assertion plus failed-creation/retry boot test; production designation must be PID 1. |
| 6 | Exec clears both `thread_group_id` and `inherited_cr3`. | Phase 6 successful exec commit in both AArch64 exec functions. | Extend `clonevm_exec_test`: failed exec preserves both; successful no-sibling exec has `None/None`, fresh root, effective TGID=pid. |
| 7 | Group membership is examined atomically; no stale snapshot across PM drop. | Phase 11 group request scans and marks all effective-TGID members under one PM guard; `sys_clone` admission uses same lock/state. | Clone-vs-SIGKILL barrier test proves child is either included or clone returns an error; no unrequested runnable member. Source scan rejects group PID `Vec` snapshot in teardown. |
| 8 | Fork, clone, and spawn kernel stacks are scheduler-owned and grace-reclaimed. | Phases 3–4 transfer all AArch64 user kernel stacks and restrict release to worker `StackProof`. | Ownership assertions after every creation path; 1000-iteration fork/clone/spawn exit stress; allocator assertion never selects a live slot. |
| 9 | No N-member FD/resource teardown loop under PM/IRQ mask in fault context. | Phases 7–8 row work bits and `take_next_for_exit`; worker closes one FD after PM drop. | Source scan forbids loops calling close/reclaim inside request transaction; `FD_CLOSES_UNDER_PM==0`; large-group/256-FD test measures bounded PM action. |
| 10 | No eager CoW cleanup while victim can run; every kill uses grace deferral. | Phases 8–11 remove direct terminate/release paths; resources stay in row through quiescence and fence. | Source scan permits no direct `.terminate(`; peer-CPU SIGKILL stress asserts zero reclaim before `EXIT_BATCHES_COMMITTED` and complete RootProof. |
| 11 | Killed threads are scheduler-quiesced and expedited by SGI_RESCHEDULE. | Phases 3, 9, and 11: `ExitPending`, owner quarantine, fixed CPU mask, SGI. | Remote-running victim test requires `EXIT_SGI_SENT`, target `EXIT_SGI_OBSERVED`, off-stack termination, and no post-request EL0 trace for victim. |
| 12 | Exactly-once SIGCHLD/wake/report with first status under repeat teardown. | Phase 8 durable first-request work bits; repeats return stored status only. | Matrix: exit→fault, SIGKILL→fault, fault→SIGKILL, repeated request/wait. Equality assertions for SIGCHLD, wake, BTRT; wait status is first status. |
| 13 | New drain/reclaim respects lock order, idle work is bounded, fork full drain remains unbounded. | Phases 1, 7–9: worker owns heavy drain; scheduler tail only performs bounded wake/finalization; fork calls explicit full worker/reclaim pass before allocation. | Lock-depth counters; source scan rejects scheduler-under-PM and reclaim in idle/tails; idle action budget assertion; fork pressure test proves full eligible drain rather than shared cap. |

## 6. Lock-ordering analysis

The existing documented order is `SCHEDULER -> PROCESS_MANAGER -> endpoint/console locks`. This design makes the stronger rule that teardown never holds `SCHEDULER` and PM simultaneously. The only exception considered during design—atomic clone/thread publication—was rejected in favor of non-runnable publication plus lifecycle dispatch gating.

| Critical section | Locks taken and order | Work while held | Why no cycle exists |
|---|---|---|---|
| Exit request/group seal | PM only | Resolve exact victim; flag-only BTree scan; assign batch/work bits; intrusive enqueue. | No scheduler, allocator, frame, FD, graphics, logger, or queue lock is reachable. |
| `sys_clone` admission/row insert | PM only | Validate parent `Live`; publish child `Creating`; move already-allocated values. | Scheduler registration happens only after PM drops. Exit sees either the inserted row or the sealed parent; no stale snapshot. |
| Scheduler thread registration/publication | SCHEDULER only | Insert a non-runnable scheduler-owned thread; later make Ready only if publication result permits. | PM validation is performed in a separate earlier/later section. Dispatch independently checks row lifecycle before TTBR arm. |
| Exec commit/detach | PM only | After fallible construction, swap page table and clear both group fields. | No scheduler operation occurs under PM; dispatch cache updates use existing post-exec architecture path after PM release. |
| Worker quarantine | SCHEDULER only | Mark one owner PID's threads `ExitPending`/`Terminated`, remove ready IDs, collect fixed CPU bitmask. | It consumes a PID already stored in the row; it never consults PM while holding scheduler. |
| SGI kick | No kernel lock | Send SGI to fixed `MAX_CPUS` mask. | Runs after scheduler guard drops; no reverse dependency. |
| Batch-quiesced check/commit | PM only | Scan scalar lifecycle/batch flags, capture atomics, set `Committed`. | Scheduler quiescence result was stored before; no scheduler call under PM. A stale optimistic result is harmless because final reclaim re-proves hardware/cached/stack liveness. |
| Notification claim | PM only, then none, then SCHEDULER only | PM sets SIGCHLD/captures scalar parent TID; scheduler performs wake after PM drops; PM later marks work done. | There is no overlapping guard. Repeats are serialized by single worker and work state. |
| Reparent one child | PM only | Find one row with `parent == victim`, change one scalar parent field. | The `children` mirror and its allocating `extend` are removed. No secondary lock. |
| FD close | PM only to `take_next_for_exit`; then endpoint-specific lock with no PM | Take one fixed-array slot; close pipe/PTY/TCP/Unix/FIFO outside. | The edge is PM release before endpoint/scheduler work; no endpoint path can wait on a held PM guard from this operation. |
| Root proof, epoch/hardware | No lock | Acquire epochs/fence; read local register and per-CPU shadows. | Pure observations. |
| Root proof, cached threads | SCHEDULER only | Scan cached roots. | Guard drops before PM revalidation. |
| Root proof, live rows/resource claim | PM only | Revalidate row state/root and no live/creating sharer; `take` resources. | No scheduler held. Group seal and creating-row visibility prevent a new sharer after the last live row disappears. |
| Page-table/CoW physical cleanup | No PM/SCHEDULER; preemption pinned; FRAME_METADATA then release; FRAME_ALLOCATOR later | Metadata ref operation is complete before allocator free. No metadata guard spans page copy/map or allocator. | Same CPU cannot be switched to a CoW fault while holding metadata. Fault context uses try-lock/Retry. There is no allocator→metadata reverse nesting. |
| Kernel-stack reclaim | SCHEDULER only to prove/detach; then stack-pool bitmap on drop | Swap-remove one `Terminated` thread after grace/live check; drop outside. | Stack allocator is never acquired under scheduler; allocator never acquires scheduler. |
| Row removal/waitpid | PM only | `waitpid` sets `reaped`; worker removes only when all predicates hold. | Destructors see an already-empty/reclaimed row; no hidden secondary locks. |
| Worker park/wake | Worker uses kthread state then SCHEDULER through existing block path; producer uses atomics only | Generation-checked park or bounded deferred unblock. | Producer never holds PM/scheduler. Worker owns no teardown guard when it parks. |
| Init fatal action | PM only to read/latch; then no lock for panic | Worker validates no guards, records trace, panics. | Fatal action is strictly after all guards drop. |

`try_manager()` must participate in PM ownership instrumentation; the grave's r20 blind spot is not repeated. Lock-depth/owner tracking is diagnostic only—structural non-nesting and release-mode refusal remain the safety mechanism.

## 7. Phased implementation plan

### Standard gate run after every phase

Every phase must pass all of the following before the next begins:

1. Source/invariant tests for the phase, frozen-region hashes, and zero unread teardown counters.
2. Clean builds with no warnings:
   - `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
   - `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
   - the required warning/error grep must produce no output.
3. `cargo run -p xtask -- boot-stages` plus the AArch64 native QEMU boot test for 10 consecutive clean boots. Every run must have zero abort, fatal-postmortem, dispatch-bug, TTBR-gone, panic, and teardown-invariant-violation markers.
4. All phase-specific targeted QEMU tests, asserting real execution and real exit status.
5. Parallels: fresh epoch-named VM through `./run.sh --parallels`, 10 consecutive PASS with `inject_retries=0`, at most 15 attempts, zero forbidden markers, and `prlctl stop --kill` after each VM.
6. A 90-minute Parallels soak with CPU0 tick-rate parity and teardown counter monotonicity checked.

No phase shares an idle-path cap with fork. Fork's explicit pre-allocation drain remains full/unbounded; worker/tail work is separately bounded.

### Phase 1 — Fence ordering and proof counters (~120–170 lines)

**Files:** `kernel/src/task/scheduler.rs`, `kernel/src/task/process_task.rs`, `kernel/src/tracing/providers/process.rs`, targeted tests.

- Replace bare grace arrays with `RetirementFence`/`RetirementSnapshot` while adapting today's two reclaim users.
- Reject `online_mask == 0`; impose unconditional Acquire fence before liveness.
- Add blocker counters and a normal-context reader.

**Shippable behavior:** Existing exit paths still own their current queue/drains, but grace can no longer pass without an ordered online-CPU observation.

**Phase assertions:** TI-12, TI-23. `RETIRE_EMPTY_ONLINE_MASK==0` in normal boots; a unit injection with zero mask refuses reclaim. Existing epoch-before-stack-liveness test becomes structural.

### Phase 2 — TTBR0 lease closure (~170–220 lines; Tier-1 approval required for one site)

**Files:** `kernel/src/arch_impl/aarch64/ttbr0.rs`, `context_switch.rs`, `exception.rs`, `syscall_entry.S`, `syscall_entry.rs`; with explicit approval, `kernel/src/syscall/time.rs`.

- Add argument-free, no-TLBI `leave_process_ttbr0`; make both idle redirects perform a hardware leave.
- Fix Rust install equal-case publication and the two assembly install sequences: name new root before/through the switch, publish `saved` after ISB, clear `next` last.
- Route in-scope raw writers through constrained helpers. The `syscall/time.rs` edit is not made without operator approval.
- Add source writer allowlist and transition-order tests.

**Shippable behavior:** Today's fault/normal deferral now relies on conservative TTBR0 records rather than known under-report windows.

**Phase assertions:** TI-06–TI-09, TI-19, TI-24, TI-28. Post-leave debug check; `TTBR_UNDERREPORT_DETECTED==0`; gold-master hashes unchanged.

### Phase 3 — Off-stack termination state (~150–210 lines)

**Files:** `kernel/src/task/thread.rs`, `kernel/src/task/scheduler.rs`, `kernel/src/arch_impl/aarch64/context_switch.rs`, `kernel/src/arch_impl/aarch64/exception.rs`.

- Add `ExitPending` and make it non-runnable/non-reclaimable.
- Generalize the existing neutral-stack exit trampoline and next-entry handoff completion.
- Replace fault-site direct `set_terminated` with exit-pending redirect bookkeeping; do not yet change process resource routing.
- Add allocation-free one-thread-at-a-time scheduler reclaim helper, initially callable from existing drains.

**Shippable behavior:** Existing fault/exit resources still use current deferral, but a scheduler thread is no longer reclaim-eligible on its own stack.

**Phase assertions:** TI-10–TI-12. `Terminated` setters are source-allowlisted to constructors/tests and the three legal edges; forced-current-thread exit proves off-stack transition before grace creation.

### Phase 4 — Scheduler ownership for every user kernel stack (~120–190 lines)

**Files:** `kernel/src/process/creation.rs`, `kernel/src/main_aarch64.rs`, `kernel/src/boot/test_disk.rs`, `kernel/src/arch_impl/aarch64/syscall_entry.rs`, `kernel/src/syscall/clone.rs`, scheduler helper/tests.

- Centralize “clone process-side thread and take kernel-stack allocation into scheduler copy.”
- Apply it to fresh spawn, direct init, test-disk, fork, and clone.
- Register new scheduler threads non-runnable until row publication is complete; add the lifecycle gate before TTBR arm.
- Drop PM before every scheduler registration; remove existing PM→scheduler nesting in fresh spawn/test-disk paths.

**Shippable behavior:** Waitpid can no longer free an original spawned thread's live AArch64 kernel stack through `Process` drop.

**Phase assertions:** TI-03, TI-10, TI-18. After each creation path, exactly one copy owns the stack and it is the scheduler copy; process-side copy owns none. Fork/clone/spawn stress verifies grace reuse.

### Phase 5 — Runtime init designation, no death panic yet (~140–210 lines)

**Files:** `kernel/src/process/manager.rs`, `process/mod.rs`, init creation/boot call sites, `kernel/src/syscall/signal.rs`, `kernel/src/task/process_task.rs`, tests; documentation/comment in `userspace/programs/src/init_shell.rs` only if needed.

- Add `InitDesignation` and explicit init construction with reserved provisional PID 1.
- Designate only after row and scheduler publication succeed.
- Replace every production PID-1 reparent/signal exclusion with the manager accessor.
- Test/harness designation policy is explicit runtime data, never a Cargo feature.

**Shippable behavior:** Reparent identity is coherent and failure-safe, while init still exits normally until Phase 12 turns on the fatal policy.

**Phase assertions:** AC 1, 4, 5; TI-14, TI-26. Failure injection, source literal scan, kernel/userspace PID agreement.

### Phase 6 — Exec detach and clone admission (~100–170 lines)

**Files:** `kernel/src/process/manager.rs`, `kernel/src/syscall/clone.rs`, `userspace/programs/src/clonevm_exec_test.rs`, targeted tests.

- Require a live parent for clone publication.
- On successful AArch64 exec, clear both `thread_group_id` and `inherited_cr3` at the page-table commit point in both exec variants.
- Preserve both on every exec failure.
- Refuse exec once exit is requested (the full state appears in Phase 8; until then use the existing terminated predicate and land the final guard in Phase 8).

**Shippable behavior:** An exec'd clone becomes a fresh singleton and cannot be selected by a later old-group sweep.

**Phase assertions:** AC 6; TI-14, TI-16, TI-28. Successful/failed exec field-state tests and root/TGID trace.

### Phase 7 — Parent-authority and one-at-a-time cleanup prerequisites (~160–220 lines)

**Files:** `kernel/src/process/process.rs`, `manager.rs`, `kernel/src/ipc/fd.rs`, wait twins, procfs, fork/clone/creation callers, `process_task.rs`.

- Remove the allocating `children` mirror; enumerate/reparent from the authoritative child `parent` field.
- Add `FdTable::take_next_for_exit` and a shared single-entry close function; keep current behavior by looping only outside PM.
- Make `waitpid` mark a reap bit through a temporary compatibility field rather than immediately requiring destructor-based cleanup; physical removal remains current until Phase 8 activates the full gate.

If the child-mirror removal alone approaches 236 lines, land it as Phase 7a and the FD/reap preparation as Phase 7b, each through the full standard gate.

**Shippable behavior:** Existing exits retain semantics with shorter PM sections and no allocating reparent extension.

**Phase assertions:** TI-03, TI-15, TI-16, TI-18. Source scan for `children`; large-child/large-FD test; no close under PM.

### Phase 8 — Row-resident lifecycle and worker, normal exit first (~180–230 lines)

**Files:** new `kernel/src/task/teardown.rs`; `task/mod.rs`, `process/process.rs`, `process/manager.rs`, `process_task.rs`, main boot initialization, wait twins.

- Add `ExitLifecycle`, intrusive queue fields, work bits, worker, generation-checked deferred wake, and reaped/reclaimed removal gate.
- Route `exit(2)` member scope and `exit_group` group scope through `ExitIntent`.
- Keep resources in rows until worker proof; remove normal-exit's conditional synchronous release.
- Worker owns exactly-once notification, one-FD cleanup, root reclaim, and row removal.
- Retain the explicit fork full drain by invoking worker/reclaim helpers from fork context with an unbounded budget.

**Shippable behavior:** Normal exit is the reference unified path; fault and signals still use their previous paths for one phase.

**Phase assertions:** TI-01–TI-04, TI-13–TI-19, TI-21–TI-23. First/repeat matrix, real wait status, no tail reclaim, row-removal predicate, no worker logging.

### Phase 9 — SIGKILL and group seal (~170–230 lines)

**Files:** `kernel/src/syscall/signal.rs`, `kernel/src/task/teardown.rs`, `process/manager.rs`, `syscall/clone.rs`, scheduler, group tests.

- Route SIGKILL to thread-group `ExitIntent`; delete eager terminate/ready mutation.
- Atomically mark all effective-TGID rows with one batch under PM.
- Make clone fail after seal; process all batch members through scheduler quarantine; send SGI to current-owner CPU masks and broadcast once when exact masks are not yet known.
- Add remote-running SIGKILL and clone-vs-seal barriers.

**Shippable behavior:** #491 and the group-seal half of #471 are closed for explicit SIGKILL.

**Phase assertions:** AC 7, 9, 10, 11, 12; TI-08, TI-10, TI-13–TI-18. Zero post-request victim EL0, no resource claim before batch commit, atomic clone outcome.

### Phase 10 — Fatal fault convergence and drain removal (~170–230 lines)

**Files:** `kernel/src/arch_impl/aarch64/exception.rs`, `context_switch.rs`, `task/teardown.rs`, `process_task.rs`, scheduler, targeted tests.

- Replace four duplicated EL0 fatal-fault bodies with one TID-attributed adapter and group-scope request.
- Use existing deferred TID ring only when PM/attribution is unavailable; the worker, not `schedule_from_kernel`, consumes it.
- Remove fault-exit, process-resource, and kernel-stack reclaim drains from the scheduling/idle tail. The worker owns heavy work; fork retains the explicit full drain.
- Fault ring drop/overflow remains non-panicking and becomes readable.

**Shippable behavior:** Fatal faults use the same row/batch/grace lifecycle and no heavy teardown remains on the idle return path.

**Phase assertions:** AC 3, 10, 13; TI-01–TI-05, TI-14, TI-19, TI-21–TI-25. TID/CR3 mismatch test, fault storm, frozen hashes, tail source scan.

### Phase 11 — Default-fatal signal and x86 direct-caller convergence (~140–220 lines)

**Files:** `kernel/src/signal/delivery.rs`, AArch64 delivery callers, `kernel/src/interrupts/context_switch.rs` (Tier 2), x86 interrupt callers, teardown tests.

- Make signal delivery return a fatal intent rather than call `terminate` or scheduler mutation while PM is borrowed.
- After PM release, submit the same group request/local handoff.
- Route the x86 direct fault termination through the shared lifecycle/architecture hook.
- Delete `Process::terminate` after the last caller is removed.

Tier-2 `interrupts/context_switch.rs` changes are required because a live direct termination caller is there; GDB cannot remove an architectural bypass. The diff is limited to intent construction and existing redirect behavior, with no logging added.

**Shippable behavior:** All known direct terminate callers are gone; every death source has one state machine.

**Phase assertions:** TI-02, TI-13–TI-18, TI-26. Source scan for `.terminate(`, both-architecture build/run, default-fatal repeat matrix.

### Phase 12 — Init fatal policy and final invariant gate (~60–120 lines)

**Files:** `kernel/src/task/teardown.rs`, init designation code, tracing/counter reader, init-death tests.

- Activate `Panic` policy through `INIT_DEATH_LATCH`.
- Worker panics only in normal context after all guards drop; uncertain attribution cannot latch.
- Add final 13-criterion test report and all 28 invariant source/runtime gates.

**Shippable behavior:** Real designated init death is kernel-fatal; test-harness init behavior remains explicit runtime policy.

**Phase assertions:** AC 2–5 and all TI invariants. The init-death test must capture the pre-panic lock/IRQ snapshot and expected panic marker; a PID-1 non-designated test process exits normally.

## 8. What this design deliberately does not solve

### #448 — idle-path CoW-walk latency

This design removes teardown CoW walks from `schedule_from_kernel`, so it removes the specific new idle-path reachability rather than measuring it. It does not claim a general latency measurement or chunking policy for other idle work. Reclaim has an explicit worker budget, and fork's full pre-allocation drain remains separate, so a future #448 measurement/chunking change is easier, not foreclosed.

### #492 — deferred fault-exit ring bound/backpressure

The existing 8x16 producer remains. Moving its consumer to a preemptible worker removes the 128-pass IRQ-masked drain, but does not solve producer overflow, slot fairness, or backpressure. `FAULT_EXIT_INTENT_DROPPED` becomes visible and non-panicking. A future #492 implementation can add a cursor, per-pass cap, or quarantine-on-full policy without changing the row lifecycle.

### #493 — disposition-aware EINTR

No signal-deliverability predicate or interruptible-wait behavior changes. Fatal signals that are actually selected for default termination route through teardown; whether a pending signal should interrupt a wait remains #493. The lifecycle API takes a resolved fatal intent, so a later disposition fix sits before it and does not need teardown changes.

### Other non-goals

- No `Arc<AddressSpace>`/generation refactor.
- No POSIX process-group (`pgid`) signal snapshot redesign.
- No core-dump implementation.
- No general scheduler allocation redesign.
- No stop-the-world panic IPI unless the operator selects it below.
- No global deletion of all raw TTBR writers without the required Tier-1 approval.

## 9. Open questions for the operator

1. **Tier-1 TTBR0 closure:** approve the minimal `kernel/src/syscall/time.rs` change that routes its raw install through the constrained TTBR helper? Recommendation: yes. Without it, the design's post-quarantine grace still protects teardown, but the whole-kernel conservative-shadow invariant cannot honestly be declared closed.
2. **Init panic scope:** should `Panic` mean the current panic handler's behavior (panic CPU stops; peers may continue) or should this work add an SMP stop broadcast analogous to Linux `smp_send_stop`? Recommendation for this round: keep current panic semantics and file the stop-the-world improvement separately.
3. **Exit scope:** confirm Linux-like `exit = member`, `exit_group`/fatal fault/default-fatal/SIGKILL = thread group. This design recommends that split; keeping today's collapsed `Exit | ExitGroup` would undermine #471's group semantics.
4. **PID-1 reservation:** accept ordinary/test PIDs starting at 2 unless an explicit runtime init/reparent anchor is created? Recommendation: yes; it is the smallest kernel/userspace coherence rule and makes init-creation retries deterministic.
5. **Fault attribution fallback:** on contradictory current-TID/stack-owner evidence, should the CPU remain quarantined indefinitely pending operator diagnostics, or may the worker terminate the exact scheduler `owner_pid` after a second normal-context cross-check? Recommendation: allow the second cross-check, never CR3-only escalation.
6. **x86 rollout:** land shared lifecycle and x86 direct-caller convergence in this series, as designed, or initially cfg-gate the worker to AArch64 and retain an explicitly audited synchronous x86 architecture hook? Recommendation: land shared notification/FD/reap semantics now; keep only root-proof mechanics architecture-specific.

## 10. Residuals and honesty ledger

1. **A wedged online CPU can pin memory indefinitely.** The design prefers a leak/stall over a premature free. There is no capacity panic and unrelated rows progress, but allocator pressure can still surface as `ENOMEM`.
2. **The fault-intent ring can still overflow.** This is #492. The design exposes the loss; it does not claim to prevent it.
3. **Group seal is an O(process-count) PM scan.** It performs only scalar, non-allocating mutations, but its masked latency grows with process count. A central group object could make it O(group-size/O(1) admission) later; this design avoids that machinery now.
4. **Reparent-one-child scans process rows.** Removing the child mirror removes allocation and dual-source bugs at the cost of O(process-count) worker PM scans. They are one child per visit, not an N-child destructive loop.
5. **Raw `inherited_cr3` remains a lifetime name, not a typed reference.** Exec detach plus the live-row reclaim clause makes the covered lifecycle safe, but stale-root semantics outside these paths are not type-impossible. A future `AddressSpaceRef(owner,generation)` remains compatible.
6. **Whole-kernel TTBR0 writer closure needs Tier-1 approval.** Without the `syscall/time.rs` edit, TI-06 is only proven for the teardown/dispatch/exec writers changed here. This is not hidden behind a whitelist claim.
7. **AArch64 panic is not currently stop-the-world.** Panic-on-init-death makes the system unusable and parks the panicking CPU, but peer CPUs are not actively stopped by today's panic handler.
8. **The worker is a liveness dependency.** Generation-checked wake prevents the designed lost-wake race, but scheduler failure or a dead worker delays notifications and reclaim. Counters/watchdog tests expose the condition; the kernel does not reclaim unsafely as a fallback.
9. **Exactly-once means one durable kernel obligation, not exactly one userspace observation under all signal coalescing.** SIGCHLD is a bit and can coalesce as POSIX permits. The worker invokes the set/wake/report operation once per first commit; waitpid remains the authoritative per-child status consumer.
10. **Release assertions are diagnostic, not the safety boundary.** Every reclaim/panic/removal action has a real release-mode predicate. Debug assertions and trace equalities catch drift but are never the sole enforcement.
11. **Frame-metadata contention may cause repeated CoW refaults.** The try-lock path chooses retry over deadlock or process death. Under pathological contention this is a liveness cost; counters and soak tests must show it remains bounded in practice.
12. **Fresh-thread publication requires two independent gates.** The scheduler thread is initially non-runnable, and dispatch validates the process lifecycle before arming TTBR0. Omitting either in a future creator reopens the clone/seal race; source tests enumerate every user-thread creation call site.

## 11. Completion definition

The implementation is complete only when:

- all five death classes can be traced through `ExitRequested -> scheduler quiescence -> fenced commit -> proof-gated reclaim`;
- the direct terminate caller scan is empty;
- the 13-row acceptance table is green with real outcome assertions;
- all 28 TI gates pass in source tests and runtime counters;
- every phase has passed clean builds, QEMU streak, Parallels streak, and 90-minute soak;
- no prohibited Tier-1 file was modified without explicit operator approval;
- all six gold-master regions are byte-identical;
- the residuals above remain stated in the PR rather than being converted into unsupported completeness claims.
