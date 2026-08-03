# AArch64 Process Teardown/Reclaim Lifecycle Design B

**Goal:** Make AArch64 process exit and reclamation safe by construction for syscall exits, fault exits, remote termination, CLONE_VM thread groups, TTBR0 roots, user stacks, file descriptors, and kernel stacks, while keeping exception-return tails free of teardown work.

**Architecture:** Exit becomes an allocation-free, one-shot logical state transition. The process table remains the stable owner and backlog for all heavyweight resources; a dedicated, ordinary kernel thread later claims one resource at a time only after typed quiescence proof, then destroys it outside process-manager and scheduler locks. TTBR0 and kernel-stack handoffs use conservative per-CPU leases whose publication order mirrors the hardware transition.

**Tech stack:** Rust `no_std`, AArch64 TTBR0_EL1 and TLBI, Breenix ProcessManager and scheduler, per-CPU state, kernel threads, atomic retirement epochs.

---

## 1. Decision summary

Do not extend either incremental design on `fix/teardown-followups`. Keep its two independently correct small fixes—the decoded last-dispatch owner and epoch-before-liveness observation—but do not adopt its global pending-reclaim `Vec`, unconditional enqueue, fault-exit cleanup result plumbing, periodic polling loop, capacity panic, main-thread ownership gate, early `GuardedStack` drop, or deferred-exit drains in scheduling paths.

The replacement has five defining decisions:

1. **Logical exit does not move or destroy resources.** It only changes lifecycle state, records exit status, prevents future dispatch, reparents children, and records deferred side effects. The process record continues to own its page table, user-stack object, old exec tables, and FD table.
2. **There is no process-reclaim queue.** Terminated process records and retired-exec nodes are the backlog. Queue allocation, queue capacity, overflow panic, and queue-loss policy therefore disappear.
3. **A dedicated reclaimer is the only heavy teardown context.** It runs as a normal, IRQ-enabled, preemptible kernel thread. It claims work under one short lock, drops the lock, and then closes an FD, walks a page table, performs broadcast TLBI, frees frames, cleans graphics state, wakes a waiter, or drops a kernel stack.
4. **A raw CR3 is not an address-space lifetime reference.** Replace `inherited_cr3` with a stable `AddressSpaceRef` naming an owning process record plus an address-space generation. The owner record cannot be physically removed while any live process references that identity or while its resources are unreclaimed.
5. **“Terminated” means architecturally off-stack.** A fault or remote kill first makes a scheduler thread `ExitPending`, which is non-runnable but not reclaimable. Only a neutral-stack handoff or a completed retirement fence can promote it to `Terminated`. The scheduler alone owns and retires kernel stacks.

This is the smallest safe shape relative to current `main`: it preserves the existing process table, page-table `Box`, scheduler-owned kernel-stack allocation, retirement epochs, kthread facility, and current fault-to-idle redirection. The material structural additions are an exit lifecycle on each process, a stable address-space identity in place of `inherited_cr3`, `ExitPending` plus an embedded retirement fence on each scheduler thread, and one reclaimer kthread.

## 2. Non-negotiable diff boundaries

The implementation must preserve these regions byte-for-byte:

- `idle_loop_arm64`.
- The EL0 dispatch banner.
- The `aarch64_enter_exception_frame` ISB placement and its surrounding gold-master ordering.
- `kernel/src/arch_impl/aarch64/gic.rs`, including the GICv3 SGI-enable block.
- The timer handler’s re-arm-at-top sequence and CPU0 regression alarm.

The exception-return boundaries get **no new calls, scans, locks, allocations, logs, queue drains, or teardown instructions**:

- `check_need_resched_and_switch_arm64` loses the deferred-fault drain and gains nothing in its place. Its existing `PREEMPT_ACTIVE` gate remains before epoch bookkeeping.
- `schedule_from_kernel` loses the deferred-fault drain and gains nothing in its place.
- `boot.S` exception-return epilogues are unchanged.
- `syscall_entry.S` gains no instruction. Its existing `next_cr3` clear is moved after the hardware switch and replaced with a same-count paired store that publishes `saved_process_cr3 = installed_root` while clearing `next_cr3`.
- `sys_fork_aarch64` no longer performs process or kernel-stack reclaim inline.

Changing `setup_idle_return_locked` so that an idle selection actually leaves a user root is architectural dispatch work, not reclamation. It replaces the currently ineffective shadow-only idle preparation. It must call only a local, non-allocating TTBR0 transition: no global TLBI, no PM/scheduler acquisition, no output. No assembly return tail is changed for this.

## 3. Lifecycle state machines

### 3.1 Process lifecycle

Each process record has a teardown lifecycle separate from the externally visible POSIX process state:

| Stage | Meaning | Allowed next stage |
|---|---|---|
| `Live` | May accept syscalls and be dispatched; owns normal resources. | `ExitCommitted` |
| `ExitCommitted` | Exit code/cause is immutable; no new dispatch or resource acquisition; deferred side-effect bits identify remaining work. The record still owns all resources. | `ResourcesClaimable` (derived), `ReapedPendingResources` |
| `ReapedPendingResources` | `waitpid` has consumed status, but the record is retained as a tombstone because it still owns or anchors resources/address-space references. | `Reclaimed` |
| `Reclaimed` | FD table is empty, graphics cleanup and parent notification have been consumed, all owned address-space resources are gone, and no scheduler thread still depends on the record. | physical record removal if reaped; otherwise remain as a lightweight zombie |

`ResourcesClaimable` is not a stored state that an exit caller can set. It is a permit returned only to the reclaimer after it validates the exit generation, address-space sharers, retirement fence, per-CPU roots, and scheduler cached roots. This prevents “Terminated therefore freeable” from being representable.

The one-shot transition is `Live -> ExitCommitted`. Re-entry returns an explicit outcome—`Committed`, `AlreadyCommitted`, or `Missing`—rather than `bool`/`Option`. `Committed` and `AlreadyCommitted` both mean that the architectural caller must finish its own control-flow obligation (pivot/schedule or redirect to idle). Only `Committed` performs one-shot state mutation. Neither outcome closes FDs, decrements CoW references, drops stacks, or moves children.

### 3.2 Scheduler-thread and kernel-stack lifecycle

| Stage | Dispatchable? | Kernel stack reclaimable? | Owner |
|---|---:|---:|---|
| `Ready` / `Running` / blocked states | As today | No | Scheduler `Thread` |
| `ExitPending` | No | No | Scheduler `Thread`; may still be current, previous, in an exception frame, or named by a per-CPU resume SP |
| `Terminated` | No | Not until its embedded retirement fence is satisfied and the ordered live-stack snapshot is empty | Scheduler `Thread` |
| `StackClaimed` | No | Worker owns the detached `KernelStack`; scheduler no longer contains the thread | Reclaimer local variable |
| dropped | No | Freed | Kernel-stack allocator |

There are two legal `ExitPending/Running -> Terminated` commit paths:

- **Syscall exit:** `schedule_terminated_from_exit` pivots to the per-CPU scheduler stack first. `exit_schedule_trampoline`, now on the neutral stack, sets `Terminated`, clears `cached_ttbr0`, records the retirement fence, and selects a successor.
- **Fault or remote exit:** the thread is made `ExitPending` and removed from ready queues. A scheduler neutral-stack handoff may finalize it immediately after switching away. If the fault path redirected directly through ERET to idle, the reclaimer promotes it only after its retirement fence has elapsed and the ordered stack-liveness snapshot excludes the stack.

No fault handler calls `set_terminated`; no exit path drops a scheduler `Thread`; and no allocator caller directly invokes kernel-stack reclamation.

### 3.3 Address-space lifecycle

An address-space identity is `(owner_pid, generation)`, not a physical root address.

- The owner process record stores the `Box<ProcessPageTable>`, associated `GuardedStack`, and current generation.
- Every task-like process row (including a CLONE_VM child) stores an `AddressSpaceRef` to that identity.
- CLONE_VM copies the identity. It never copies a raw TTBR0 physical address as a lifetime promise.
- The scheduler resolves an `AddressSpaceRef` through ProcessManager when filling `next_cr3`. `Thread.cached_ttbr0` is only a conservative dispatch lease, not ownership.
- The owner process record is retained as a tombstone until no live process row references the identity and its page table has been reclaimed.
- A clone member may outlive the owner’s logical exit and continue to use the owner’s retained page table. The owner’s waitpid reaping does not free or remove that backing record.
- Exec by an address-space owner remains rejected while another live CLONE_VM member references the old generation, matching current behavior with a sound identity check. Exec by a clone can become a new owner/generation without affecting peers on the old identity.

Successful exec prepares a retired-table node outside the PM lock. The PM commit swaps to the new page table/address-space generation and links the preallocated node in O(1); it performs no `Vec::push`. Each retired node carries its own retirement fence and root. The reclaimer later unlinks one safe node under PM and destroys it outside.

This replaces `pending_old_page_tables: Vec<Box<...>>` with a linked set of preallocated retired nodes. It also removes the false assumption that “next exec/exit means CR3 definitely switched.”

## 4. Resource ownership by stage

| Resource | Live owner | After logical exit | After reclaim claim | Physical release point |
|---|---|---|---|---|
| Exit code/cause | Process record | Immutable process tombstone | Tombstone | Process row removal after waitpid and cleanup |
| Parent relation | Child’s `parent` field (single source of truth) | Reparented idempotently in exit commit | N/A | Process row removal |
| Child relation | Derived by scanning `parent`; no `children` mirror | Cannot be taken/dropped | N/A | N/A |
| FD table | Process record | Same record; closed one entry at a time by worker | Worker owns one `FileDescriptor` | Outside PM lock in worker |
| Graphics/window cleanup | Exit-work bit on process record | Pending until worker claims bit | Worker owns copied PID | Outside PM lock |
| Parent wake/SIGCHLD | SIGCHLD pending bit plus exit-work wake bit | Survives all re-entry paths | Worker owns copied parent TID/PID | Scheduler wake outside PM lock |
| Current page-table root | Address-space owner record | Retained, even if logically reaped | Worker local `RetiredAddressSpace` bundle | After global TLBI and CoW/table cleanup |
| Old exec page table | Preallocated retired node linked to owner | Same | Worker local retired node | After root quiescence and TLBI |
| `GuardedStack` | Address-space owner record | Retained with page table | Same `RetiredAddressSpace` bundle | Explicit stack cleanup while table is valid, then table cleanup |
| Scheduler kernel stack | Scheduler `Thread` only | `ExitPending`/`Terminated` thread | Worker-owned detached `Thread`/stack | Outside scheduler lock |
| TTBR0 hardware reference | Current CPU | Local CPU until actual `msr ttbr0_el1`+`isb` completes | None | Root can be claimed only after proof |
| `saved_process_cr3` | Per-CPU return lease | Retains old root until hardware switch completes | Kernel/new root published | Never owns memory; participates in proof |
| `next_cr3` | Per-CPU pending-install lease | Retains target until installed or cancelled | Cleared only after saved shadow publishes installed root | Never owns memory; participates in proof |
| `Thread.cached_ttbr0` | Scheduler thread dispatch lease | Retained through `ExitPending` if still in flight | Cleared at off-stack `Terminated` commit | Never owns memory; participates in proof |

No destructor’s current implementation is assumed to remain a no-op. `GuardedStack` is safe if its `Drop` later frees memory because ownership does not leave the address-space bundle until quiescence is proven.

## 5. Exact commit points

### CP0 — dispatch lease acquisition

Before a scheduler can install a userspace root, it resolves the thread’s `AddressSpaceRef` while the process is still dispatchable. It stores that root into `next_cr3` with release semantics. From this point the target root is live even before hardware installation. If resolution sees `ExitCommitted`, a stale generation, or a missing owner tombstone, dispatch is refused and the thread becomes `ExitPending`; it is never sent toward EL0.

### CP1 — local TTBR0 detach commit

Only the CPU executing the affected context may detach its own root. The helper receives the expected root captured from TTBR0 or resolved for the current thread. It does not accept a PID and cannot be called to “quiesce an arbitrary process.”

Order:

1. Read `TTBR0_EL1` and mask ASID bits.
2. If it equals the expected retiring root (or the caller explicitly requests “leave any current user root”), execute local `dsb ishst; msr ttbr0_el1, kernel_root; isb`.
3. Only after the ISB, publish `saved_process_cr3 = kernel_root` and `next_cr3 = 0`.
4. Publish the per-CPU handoff generation/release ordering used by retirement fences.

This helper performs no TLBI. Stale translations are harmless while the old tables remain owned; the reclaimer performs the global invalidation immediately before physical reuse. Avoiding TLBI here removes broadcast synchronization from PM-locked fault paths.

If the actual hardware root does not equal `expected_root`, the helper must not clobber shadows. That is the uniform ownership predicate at every call site; it is root-based and works for any thread in a multithreaded process.

### CP2 — logical process exit commit

Under PM lock, with no allocation, output, scheduler call, FD close, frame operation, page walk, or TTBR operation:

1. Locate by PID, TID, or the root captured before CP1.
2. If already `ExitCommitted`, leave exit code and all one-shot bits unchanged and return `AlreadyCommitted` with enough identity for the caller to complete redirect/pivot behavior.
3. Otherwise set immutable exit code/cause and teardown generation.
4. Mark the process non-dispatchable and remove its legacy process-ready entry.
5. Set SIGCHLD pending on the logical parent and set a durable parent-wake bit.
6. Set a durable reparent-work bit. The worker changes one matching child’s authoritative `parent` to init per short PM transaction; it clears the bit only after a scan finds none. For PID 1 this is an idempotent no-op: those relationships already name init and, because no child vector is taken, they cannot disappear on re-entry.
7. Set FD/graphics/reclaim work bits and capture a retirement fence.
8. Return `Committed`.

For thread groups, `exit` commits the calling member. `exit_group` sets a durable group-exit flag on the group leader and commits the caller. Dispatch resolves that leader flag and immediately refuses every member, so no member can newly enter EL0. The worker commits one remaining member per short PM transaction; the group leader produces the single externally visible parent notification/status. An address-space owner is not reclaimable while any live group/member row still references its address-space identity.

### CP3 — scheduler off-stack commit

The scheduler changes `ExitPending -> Terminated` only after the CPU no longer executes on that thread’s kernel stack. At the commit it clears `cached_ttbr0`, installs a non-empty retirement fence, and guarantees the thread is absent from all ready/deferred queues. This is done on the scheduler stack in syscall exit, on a neutral context-switch trampoline, or by the worker after an elapsed fence proves a fault redirect completed.

### CP4 — reclaim claim commit

The reclaimer first obtains a satisfied `RetirementSnapshot` capability. Producing it performs Acquire loads for every CPU captured in the fence and then an unconditional Acquire fence before any volatile liveness read. The captured online mask is required to be nonzero (CPU0 is an enforced boot invariant), so the operation never degenerates to an unfenced all-zero short circuit.

For an address-space claim it then proves:

- the candidate generation is still exit/exec-retired;
- no live process references the address-space identity;
- no schedulable or `ExitPending` thread can newly publish the root;
- no scheduler `cached_ttbr0` matches;
- the worker CPU’s actual `TTBR0_EL1` does not match;
- no online CPU’s `saved_process_cr3` or `next_cr3` matches; and
- the retirement snapshot remains valid for the candidate generation.

The worker reacquires PM, revalidates generation/identity and the absence of live logical sharers, then moves the table and coupled stack into a local `RetiredAddressSpace` value. No operation can newly acquire that identity after the first proof because dispatch and exec validate lifecycle/generation.

For a kernel stack, the worker obtains the analogous scheduler permit, removes the `Thread` with a non-allocating swap, releases the scheduler lock, and only then drops the stack allocation.

### CP5 — physical reuse commit

In worker context and with no PM/scheduler lock:

1. Issue the broadcast TLB invalidation required before old ASID-1 translations can survive frame reuse (`dsb ishst; tlbi vmalle1is; dsb ish; isb`, until Breenix has per-mm ASIDs).
2. Explicitly release the coupled guarded user stack while its table metadata remains available.
3. Decrement CoW references and free user frames.
4. Free lower-level tables and the root table.
5. Mark the process/address-space resource stage reclaimed in a short PM transaction.

This is the only teardown path that broadcasts TLBI. It is normal kernel-thread work, never a fault handler, PM critical section, idle loop, syscall/exception tail, or fork path.

### CP6 — logical reap and record removal

`waitpid` consumes status exactly once and marks the process record `reaped`. If resources or address-space references remain, the row becomes `ReapedPendingResources` and stays as the owner tombstone. Once the worker marks it `Reclaimed` and no live `AddressSpaceRef` names it, the row may be removed. Thus waitpid latency is not coupled to frame teardown, and reaping cannot trigger an unsafe destructor.

## 6. TTBR0 and per-CPU shadow protocol

### 6.1 Meaning of the two shadows

- `next_cr3` is a pending-install lease. A nonzero value means hardware may soon install that root.
- `saved_process_cr3` is the last root known to be installed or intentionally restorable. It remains conservative until a hardware switch has completed.

At all times, an installed non-kernel hardware root is named by at least one software lease. During a process-to-process handoff, old hardware is covered by `saved`, and the target is covered by `next`. After `msr`+`isb`, the target is published to `saved`, then `next` is cleared. There is no interval in which hardware names a root while both shadows deny it.

For the two assembly syscall-return switch sites, replace the existing pre-switch `str xzr, [next]` with a post-switch paired store of `{next=0, saved=installed}`. This uses the same number of tail instructions. The restore-saved branch already keeps the correct conservative lease and needs no new instruction.

`switch_ttbr0_if_needed` follows the same order even when hardware already equals `next`: publish `saved = next`, then clear `next`. This incorporates the correct part of branch commit `28a7933e` without its idle-path assumption.

### 6.2 Idle transition

Every route through `setup_idle_return_locked` performs CP1 locally before publishing the idle stack and clearing process ownership. Every fault path performs CP1 before taking PM; `set_idle_stack_for_eret` then verifies/records kernel-root state rather than promising that a later user-only dispatcher will switch it. The hardware transition therefore occurs on paths that go idle forever.

There is no comment saying `switch_ttbr0_if_needed` will run later. It need not run later.

### 6.3 Liveness oracle

Delete public boolean oracles callable without ordering context. The replacement accepts a satisfied retirement snapshot and returns a diagnostic `RootLiveness` value, not just a bool. It reads:

- local actual `TTBR0_EL1` (authoritative);
- every captured online CPU’s `saved` and `next` shadows;
- scheduler `cached_ttbr0` values; and
- the matching CPU/tid masks for diagnostics.

A CPU cannot read another CPU’s TTBR0_EL1 directly. Safety comes from the publication protocol: the remote saved shadow remains conservative across the actual switch, and the retirement snapshot’s Acquire relationship proves the worker observes a post-handoff publication. A future CPU-offline path must switch to the kernel root, publish kernel shadows and stack state, advance its handoff epoch, and only then clear its online bit. Until that protocol exists, online CPUs are never silently excluded.

## 7. Exit path walkthroughs

### 7.1 `exit` / `exit_group` syscall

1. Complete `clear_child_tid` while the user root is installed; perform futex wake outside PM as today.
2. Capture the current TID, process/address-space identity, and actual root.
3. Execute CP1 before any teardown state is published.
4. Execute CP2 through the common ProcessManager transaction. Missing/already-committed results never allow return to EL0.
5. Atomically request worker service; do not close or reclaim inline.
6. Call the existing final scheduler-stack pivot. CP3 occurs in `exit_schedule_trampoline`.

The function remains non-returning for all outcomes. `Exit` and `ExitGroup` are separate dispatcher arms so group semantics are explicit rather than accidentally identical.

### 7.2 Synchronous EL0 fault

All four terminating exception classes use one helper and one control-flow rule:

1. Capture the faulting TTBR0 root before changing it.
2. CP1 switches locally to kernel TTBR0 without TLBI, before PM acquisition.
3. CP2 commits by the captured root. `Committed` and `AlreadyCommitted` both mean “the faulting context is dead”; `Missing` is separately diagnosed but still cannot resume the faulting frame.
4. Set the safe idle ELR/SPSR/frame and pending idle stack as current code does.
5. Mark only a worker request and scheduler reschedule request using atomics. Do not acquire the scheduler, close FDs, walk tables, or mark the current thread `Terminated` in the handler.
6. Redirect to idle. The thread stays `ExitPending` until CP3.

This removes the branch’s implicit coupling between `exit_process -> Option` and whether the handler redirects, and it makes an already-terminated second fault follow the same safe redirect path.

### 7.3 EL1/kernel-mode fault attributed to a user thread

The stack-slot/last-dispatched owner identifies the victim, as fixed on current `main`; `current_thread_id()` is not trusted after an idle redirect. The handler locally detaches any actual user root, commits exit by victim TID if possible, records diagnostics, and redirects. There is no `DEFERRED_FAULT_EXIT_SLOTS` buffer to fill or lose and no later drain on an exception return.

If PM is temporarily unavailable in a nested fatal path, the process record is not reclaimed because no exit commit exists. The handler records an atomic fault-exit intent `(cpu, victim_tid, observed_root)` in that CPU’s single current-fault slot and parks/redirects that CPU. `dispatch_thread_locked` already decides whether an intended target may enter EL0; it must refuse every user dispatch on a CPU whose fault-intent slot is nonempty. The reclaimer consumes and clears the slot in normal context before that CPU may dispatch another user task. One slot per CPU is therefore a structural bound on one quarantined CPU, not an arbitrary work queue. Failure is visible and pins resources; it never overwrites/drops an intent or panics.

### 7.4 Remote SIGKILL/default fatal signal

Signal code requests CP2 through ProcessManager and never invokes `Process::terminate` directly. It does not touch the caller CPU’s TTBR0 because the caller may be running an unrelated process. Worker/scheduler processing moves all affected scheduler threads to `ExitPending` and sends the existing reschedule request. Each CPU leaves the root through its ordinary context switch/idle transition; reclamation waits for every lease.

Default-signal delivery returns a common exit outcome and notification identity. Parent SIGCHLD/wakeup ownership is the process exit lifecycle, not a second signal-specific notification implementation.

### 7.5 Multithreaded/CLONE_VM owner exit

If the address-space owner exits while clone members remain live, CP2 closes only the exiting row’s logical lifetime. The page table stays owned by the tombstone and resolvable through the shared `AddressSpaceRef`. The reclaimer’s logical-sharer check blocks CP4. As members exit or exec into new identities, their references cease to be live. The last member’s off-stack commit plus TTBR0/cached-root quiescence permits reclamation exactly once.

Kernel stacks remain per scheduler thread and retire independently. No `main_thread.id` predicate participates in root ownership.

## 8. Deferred execution context and scheduling

### 8.1 Worker context

Create `kprocess-reclaim` after scheduler, kthread, and workqueue initialization. It always runs with interrupts enabled and normal preemption. It is the only caller of page-table teardown and the only teardown caller that may allocate, log, take blocking non-hot locks, or issue broadcast TLBI.

The worker performs one claim/action at a time in this priority order:

1. Consume per-CPU fault-exit intents and commit logical exits.
2. Commit one remaining member of an exiting thread group.
3. Reparent one child whose parent has exited.
4. Finalize scheduler `ExitPending` threads whose CPU handoff is complete.
5. Close one FD outside PM.
6. Deliver one parent wake or graphics cleanup outside PM.
7. Claim and reclaim one retired exec table.
8. Claim and reclaim one exited address space.
9. Claim and drop one retired kernel stack/thread.
10. Remove one fully reclaimed, logically reaped tombstone.

Each claim returns owned work by value; no heavy destructor runs while PM or scheduler is locked. PM and scheduler are never held together. Frame-metadata and allocator locks are never acquired while either is held.

### 8.2 Wake/sleep protocol

Producers perform only an atomic false-to-true work-generation transition, `set_need_resched`, and a reserved slot in the existing lock-free ISR-style wake buffer for the known worker TID. Ordinary ISR wakeups use the other slots, so teardown’s single idempotent wake cannot fail because the general buffer is full. Producers do not allocate or take the worker/PM/scheduler lock. Duplicate exit/re-entry collapses into the same pending generation.

When there is no backlog, the worker uses untimed `BlockedOnIO` with a lost-wake-safe sequence: publish blocked state, recheck the work generation, then schedule only if unchanged. It creates no timer-heap entry. When a backlog exists but only retirement epochs or a live root/stack block progress, the lowest-priority worker remains runnable and uses exponential yield/WFI backoff. Each unsuccessful pass explicitly yields; if the scheduler reports no switch, it executes WFI with IRQs enabled before retrying. It therefore neither accumulates duplicate timer entries nor spins at full CPU. CP1/CP3 and new exit work reset the backoff and issue the reserved wake.

Scheduler absence is an initialization failure handled before the worker is published. A failed block/schedule transition records an atomic diagnostic and enters the yield/WFI fallback; it never becomes a full-speed reclaim loop.

No worker call is added to `check_need_resched_and_switch_arm64`, `schedule_from_kernel`, an idle loop, or an assembly return tail.

## 9. Locking and frame metadata

The PM-held exit transaction is bounded and allocation-free. It mutates scalar lifecycle fields, scans existing process records for authoritative parent/reference relationships, and takes no other lock. It never formats or emits output.

FD cleanup is one-entry-at-a-time: `FdTable::take_next_for_exit` removes one existing entry without allocating under PM; `close_extracted_fd` runs after the guard is dropped. This applies to every architecture/caller using the common exit transaction, avoiding a hidden AArch64-only contract.

Frame metadata needs one additional structural rule because CoW faults and normal reaping share it:

- Normal-context metadata operations use a guard that disables preemption for the duration of the metadata lock, so the owner cannot be scheduled out while holding it.
- Fault-context CoW operations use a `try_lock` transaction. On contention they make no PTE/refcount mutation and return `Retry`; the exception returns to the same EL0 instruction and retries later. They never block on a lock held by a preempted task.
- No logging occurs while the metadata lock is held. Underflow and contention are recorded atomically and emitted from normal diagnostic context.

This removes the same-CPU self-deadlock structurally. The reclaimer is not merely moved off ERET while retaining a preemptible blocking lock owner.

## 10. Overflow, stuck-CPU, and observability policy

There is no bounded pending-process reclaim collection and therefore no 257th-exit panic. Backlog capacity is the already-existing process/retired-exec ownership graph. A wedged online CPU can pin resources but cannot overflow a teardown queue or cause a panic while locks/IRQs are held.

Policy for a root blocked by a wedged CPU:

- Preserve the owner tombstone and all frames; never weaken the proof and never force-free.
- Continue servicing unrelated reclaimable records (no head-of-line blocking).
- Let finite allocator APIs return their existing `ENOMEM`/pool-exhausted error rather than panic in teardown.
- Record first-retired epoch/time, last scan time, PID, address-space generation, root, hardware-local match, saved-shadow CPU mask, next-shadow CPU mask, cached-thread count, and blocked-scan count.
- Emit a rate-limited warning only from the worker after an age threshold. Never emit from the producer, PM lock, fault handler critical section, idle loop, or return path.

The following atomics are both written and read:

- logical exits committed / re-entries;
- pending process side effects;
- retired address spaces and old exec tables;
- reclaimed address spaces/tables/stacks;
- blocked scans and oldest blocked age;
- worker sleep/schedule failures and backoff level;
- last blocked PID/root and blocker masks;
- frame-metadata fault retries.

`dump_teardown_reclaim_state_raw` prints them as an early, lock-free section of `dump_fatal_postmortem_once`, before trace-buffer dumping. A normal procfs/debug reader may expose the same snapshot, but the fatal dumper alone is the required in-tree reader. Stack ranges continue to import `ARM64_KERNEL_STACK_BASE/END`; no diagnostic re-hardcodes them.

## 11. Already-terminated and waitpid semantics

An already-committed exit is not an early return from the overall lifecycle. It is only an early return from the one-shot mutation block.

- FDs and CoW references are not touched again because their owner and progress bits remain in the process record.
- Reparenting is an idempotent invariant over each child’s authoritative `parent` field; no children vector is taken before a guard and no duplicate list can diverge.
- SIGCHLD remains pending and the durable parent-wake bit remains until the worker consumes it. Re-entry requests worker service again, so a parent blocked in `waitpid`/`pause` cannot be stranded.
- Fault handlers redirect on both `Committed` and `AlreadyCommitted`; they never infer architectural safety from an optional cleanup payload.
- Syscall exit pivots and terminates the scheduler thread on `Committed`, `AlreadyCommitted`, and `Missing`; it can never return to the userspace exit loop.
- `waitpid` marks logical reaping and leaves an owning tombstone when needed. It never triggers page-table, FD, user-stack, or kernel-stack destruction under PM.

`Process.parent` becomes the sole parent/child relation. Wait selection scans process records by `parent`, and fork/clone no longer push a mirrored `children` vector. This is a deliberate small structural expansion: it removes the state whose premature `mem::take` produced the round-2 reparenting violation.

## 12. File:function-level change list

### AArch64 architectural transitions

- `kernel/src/arch_impl/aarch64/ttbr0.rs`
  - Split `switch_ttbr0_to_kernel` into local CP1 detach (no TLBI), ordered shadow publication, ordered liveness reporting, and worker-only global retired-root invalidation.
  - Remove `quiesce_ttbr0_for_exit()` as a pid-agnostic clobber API.
  - Replace unqualified `is_ttbr0_root_live`/`current_cpu_retains_ttbr0_root` booleans with a root-specific detach API and a liveness API requiring a satisfied retirement snapshot.
- `kernel/src/arch_impl/aarch64/mod.rs`
  - Export only the constrained TTBR0 APIs; do not expose arbitrary shadow clearing.
- `kernel/src/arch_impl/aarch64/context_switch.rs`
  - `dump_all_eret_frame_anomaly_snapshots`: keep branch commit `5781442b`’s decoded owner fix.
  - `setup_idle_return_locked`: actually perform the local kernel-root transition before replacing shadows; remove claims about a later user dispatcher.
  - `set_next_ttbr0_for_thread`: resolve `AddressSpaceRef`, reject exit-committed/stale-generation tasks, and treat `cached_ttbr0` as a dispatch lease.
  - `switch_ttbr0_if_needed`: publish installed root to `saved` before clearing `next`, including the already-equal case.
  - `dispatch_thread_locked`: treat `ExitPending` as non-runnable, quarantine user dispatch while the local fault-intent slot is occupied, and redirect safely.
  - `check_need_resched_and_switch_arm64`: delete deferred-fault draining; keep `PREEMPT_ACTIVE` before epoch update; add no replacement work.
  - `schedule_from_kernel`: delete deferred-fault draining; add no replacement work.
  - `schedule_terminated_from_exit` / `exit_schedule_trampoline` / neutral inline trampoline: implement CP3, clear cached root, and attach the thread’s retirement fence only after the stack pivot.
- `kernel/src/arch_impl/aarch64/exception.rs`
  - `set_idle_stack_for_eret`: publish idle stack only after CP1; do not clear a shadow before hardware switch or promise a later switch.
  - Replace `defer_current_user_thread_sigsegv_exit` and all four terminating fault arms with the common captured-root/TID exit commit and unconditional safe redirect rule.
  - Remove `terminate_current_scheduler_thread` from fault cleanup.
  - Preserve the current source-of-truth kernel-stack range and sectioned/high-value-first postmortem fixes; add early lock-free teardown stats.
- `kernel/src/arch_impl/aarch64/syscall_entry.rs`
  - `sys_exit_aarch64`: separate `Exit`/`ExitGroup` scope, use CP1+CP2, always perform final pivot, and perform no reclaim/FD cleanup.
  - `sys_fork_aarch64`: remove inline process-resource and terminated-thread reclaim calls.
  - Exec success paths: supply preallocated retired-table nodes and capture the retirement fence at the address-space swap commit.
- `kernel/src/arch_impl/aarch64/syscall_entry.S`
  - At the two existing `next_cr3` switch sites, move the existing clear after `msr`/`isb` and make it a same-instruction-count paired publication of `next=0,saved=installed`.
  - Do not alter guards, ERET/ISB ordering, banners, or add instructions.
- `kernel/src/per_cpu_aarch64.rs` and `kernel/src/arch_impl/aarch64/percpu.rs`
  - Document shadows as leases, give snapshots Acquire/fence-compatible semantics, and return blocker masks for diagnostics without changing the frozen per-CPU layout.
- `kernel/src/arch_impl/aarch64/smp.rs`
  - Document/enforce the nonempty online-mask boot invariant and the required future CPU-offline quiesce protocol; no GIC/timer change.

### Process and address-space ownership

- `kernel/src/process/process.rs`
  - Add the teardown lifecycle/progress bits, logical-reap bit, address-space owner/generation identity, and retired-exec-node head.
  - Replace `inherited_cr3` with `AddressSpaceRef`.
  - Remove `children` as a second source of truth.
  - Make direct `terminate`/`terminate_minimal`, CoW cleanup, old-table drain, FD close, and resource-taking unavailable to exit callers; worker-owned cleanup remains private.
  - Keep page table and `GuardedStack` coupled until the worker claims both.
- `kernel/src/process/manager.rs`
  - Replace `exit_process` with the explicit idempotent CP2 transaction and typed outcome.
  - Replace `find_live_clone_vm_sibling_holding_cr3` with address-space-identity membership.
  - Add no-allocation worker claim/revalidate methods for one FD/side effect/retired table/current address space/tombstone at a time.
  - Change `remove_process` into logical `mark_reaped`; physical removal requires `Reclaimed` and no address-space references.
  - Reparent and enumerate children using authoritative `parent` fields.
  - Exec paths never call `drain_old_page_tables`; they link a preallocated retired node.
- `kernel/src/process/mod.rs`
  - `exit_current`: honor all typed outcomes, request worker service, preserve manager-unavailable diagnostics in task context, and never conflate “manager absent” with “already committed.”
- `kernel/src/syscall/clone.rs`
  - CLONE_VM copies `AddressSpaceRef` rather than `inherited_cr3`; group identity and last-member semantics are explicit.
- `kernel/src/syscall/wait.rs` and `kernel/src/syscall/handlers.rs` (`complete_wait` twin)
  - Find children by `parent`, consume status once, and call `mark_reaped` rather than dropping the process row/resources.
- All fork/creation helpers in `kernel/src/process/manager.rs` and `kernel/src/process/creation.rs`
  - Stop maintaining a mirrored `children` vector; initialize owned address-space identities/generations.

### Reclaimer, FDs, scheduler, and frames

- `kernel/src/task/process_task.rs`
  - Delete `DeferredFaultExitBuffer`, `PendingProcessReclaim`, both global reclaim queues, enqueue/drain/reclaim functions, `defer_live_process_resources`, and inline `release_process_resources` from AArch64 exit.
  - Reduce `ProcessScheduler::handle_thread_exit` to the common logical exit request; it never takes children/resources or performs phase-2 cleanup itself.
  - Add worker initialization, atomic work-generation/wake, one-action claim loop, rate-limited blocked-root reporting, and `dump_teardown_reclaim_state_raw`.
  - Change FD cleanup to accept one already-owned descriptor.
- `kernel/src/ipc/fd.rs`
  - Replace exit-time `take_all -> Vec` with allocation-free `take_next_for_exit`/empty inspection. Ensure the process-row destructor sees an already-drained table and cannot repeat active close semantics.
- `kernel/src/task/thread.rs`
  - Add `ThreadState::ExitPending`, embedded optional retirement fence, and precise cached-root lease documentation.
  - Initialize/clone these fields explicitly in every constructor; a clone never inherits retirement state.
- `kernel/src/task/scheduler.rs`
  - Make `RetirementFence` capture online mask+targets and return a `RetirementSnapshot` only after ordered Acquire observation. Preserve the correct epoch-before-liveness direction from `867ce0c6` and eliminate the zero-load/zero-mask gap.
  - Replace the allocating `retirement_grace` Vec and allocating `reclaim_terminated_threads` temporaries with embedded per-thread fences and an allocation-free claim of one thread.
  - Add APIs used only by the worker/neutral trampoline to request/finalize exits, query cached-root leases, and detach a reclaimable thread for dropping outside the scheduler lock.
  - Reserve one already-scanned ISR wake slot per CPU for the idempotent reclaimer wake; do not add a new drain/check to a scheduling tail and do not use the timer heap for worker sleep.
  - Do not add any reclaimer call to scheduling entry/return paths.
- `kernel/src/memory/kernel_stack.rs`
  - Require the ordered retirement snapshot for release eligibility, retain the allocator live-slot assertion and full scrub, and import pool constants everywhere diagnostics need them.
- `kernel/src/memory/frame_metadata.rs`
  - Add the preemption-pinned normal guard and nonblocking fault transaction; remove lock-held logging and expose atomic error/retry counters.
- `kernel/src/arch_impl/aarch64/exception.rs` CoW helper and x86 CoW helpers in `kernel/src/interrupts.rs`
  - Use fault-context try semantics and retry without PTE mutation on metadata contention.
- `kernel/src/memory/process_memory.rs`
  - Keep cleanup accounting/logging behavior intentionally symmetric between architecture twins; teardown logging is now worker-only, so the branch’s AArch64-only removal is unnecessary.
- `kernel/src/signal/delivery.rs` and `kernel/src/syscall/signal.rs`
  - Replace every direct `Process::terminate` with a request for the common PM exit transaction; remove signal-specific scheduler termination and duplicate parent notification ownership.
- `kernel/src/interrupts.rs` and `kernel/src/interrupts/context_switch.rs`
  - Adapt x86 fault/signal callers to the typed exit outcome and common lock-free FD/parent side-effect contract; explicitly retain architecture-specific address-space reclaim mechanics.
- `kernel/src/main_aarch64.rs`
  - Start the reclaimer only after scheduler/kthread/workqueue readiness and before userspace can exit. Do not touch timer/GIC/frozen boot sections.
- `xtask/src/main.rs`
  - Add a boot-stage marker for successful reclaimer initialization only if emitted from safe boot/task context; no worker-loop logging.

### Tests and review guards

- Add unit/model tests for process and thread state transitions, typed re-entry outcomes, parent-authoritative reparenting, logical reap before physical reclaim, address-space-owner exit with live clones, and retirement snapshot ordering.
- Add an AArch64 integration test that concurrently loops fork/exit/waitpid and CLONE_VM member exit, including owner-first exit and exit_group.
- Add fault-exit tests for all four exception classes’ common outcome behavior, including already-committed re-entry and PM-contention intent handling.
- Add source-structure checks that forbidden tail functions contain no reclaim/drain calls, the assembly switch does not clear `next` before `msr`+`isb`, every TTBR writer publishes shadows in protocol order, and frozen gold-master hashes are unchanged.
- Add diagnostics tests proving every teardown counter has an in-tree read and kernel-stack diagnostics import the real constants.

## 13. Validation gates

1. Build both architectures with zero warnings. The existing x86 cleanup behavior difference must be stated in the commit that introduces the common logical-exit/FD contract; there must be no silent twin divergence.
2. Run `cargo run -p xtask -- boot-stages` after killing stale QEMU processes.
3. Run the targeted fork/exit/kernel-stack reuse test for at least 1,000 iterations and require actual child execution/status, not creation markers.
4. Run owner-first and non-main-thread CLONE_VM exit tests while sibling threads continuously touch shared memory. The owner row may be logically reaped early; the root must not reclaim until the last live address-space reference and CPU lease are gone.
5. Inject an online CPU whose scheduling epoch stops. Continue creating/exiting processes until allocator pressure is visible: no teardown panic, no queue overflow, unrelated roots reclaim, and diagnostics name the blocking CPU/root.
6. Exercise double exit through syscall, fault-after-signal-termination, signal-after-fault-termination, and repeated wait wake. CoW decrements and FD closes occur once; reparent/wake remains effective.
7. Assert no retired root matches actual local TTBR0, any saved/next shadow, or cached thread root immediately before physical release. Assert no reclaimed kernel-stack slot matches any ordered per-CPU live/resume stack snapshot.
8. Run ten consecutive AArch64 boots plus the project’s longer SMP/Parallels streak and soak, with zero abort/panic/ERET anomaly markers and no CPU0 timer regression.

## 14. Invariant coverage matrix

| Invariant | Structural mechanism |
|---:|---|
| 1 | Only the reclaimer destroys resources; all drains/reclaim calls are removed from return/schedule/fork paths. |
| 2 | Exit transaction and hot transitions contain no output/formatting; diagnostics are worker/postmortem only. |
| 3 | Process records are the backlog; exit does not allocate, push, close, walk, or take blocking secondary locks. |
| 4 | Deferred drain is removed; existing `PREEMPT_ACTIVE` precedes epoch work. |
| 5 | Explicit byte-for-byte frozen-region boundary and source-hash tests. |
| 6 | Hardware `msr`+`isb` precedes saved publication and next clear; assembly clear is reordered with no instruction increase. |
| 7 | Ordered liveness API reads local actual TTBR0 and conservative remote leases. |
| 8 | CP1 accepts an expected root/current-CPU authority, never arbitrary PID; ProcessManager never clobbers TTBR0. |
| 9 | Every idle selection performs CP1; no dependency on a later user-dispatch-only function. |
| 10 | `ExitPending` is non-reclaimable; `Terminated` is committed only after pivot/fence. |
| 11 | CP4 checks hardware/shadows/cached roots and ordered stack snapshots, not scheduler name matching. |
| 12 | `RetirementSnapshot` capability enforces Acquire epoch observation before liveness reads and forbids empty-mask success. |
| 13 | One-shot process lifecycle owns FD/CoW/resource progress; re-entry cannot repeat it. |
| 14 | Address-space identity/root is the uniform predicate for every thread/caller; no main-thread gate. |
| 15 | Parent relation is authoritative/idempotent; durable wake bit is worker-consumed on every outcome. |
| 16 | No children collection is taken; mirrored collection is removed. |
| 17 | Typed `Committed/AlreadyCommitted/Missing` outcome explicitly separates logical result from mandatory architectural redirect/pivot. |
| 18 | Worker takes and closes one FD outside PM for all common exit callers. |
| 19 | Local detach has no TLBI; broadcast invalidate occurs only in worker after CP4. |
| 20 | Normal metadata owners are preemption-pinned; fault users try-lock and retry without mutation. |
| 21 | No bounded reclaim queue and no teardown overflow panic. |
| 22 | A wedged CPU pins only affected resources; allocator failure is reported normally and unrelated work continues. |
| 23 | All counters are read by early raw postmortem output (and optionally procfs). |
| 24 | Stack diagnostics/liveness import the one source of truth. |
| 25 | Existing sectioned, high-value-first postmortem is preserved; teardown state is added before trace buffers. |
| 26 | Common exit/FD changes and any intentional x86/AArch64 reclaim difference are explicit; cleanup twins retain intentional parity. |
| 27 | Commit slices below state actual reachability and removed guards/ownership changes. |
| 28 | Comments describe completed local handoffs and type-enforced protocols only; control-flow/source checks prevent aspirational rationale. |

## 15. Recommended implementation/commit slices

1. **`refactor(process): make exit a one-shot logical transaction`** — disclose all shared callers, removal of direct terminate cleanup, parent-authoritative relationships, typed re-entry outcomes, and waitpid tombstones.
2. **`refactor(aarch64): replace raw CLONE_VM roots with address-space identities`** — disclose owner-row retention, exec behavior, scheduler resolution, and multithread owner-first exit semantics.
3. **`fix(aarch64): publish TTBR0 leases after hardware handoff`** — disclose idle paths and the same-instruction-count assembly store reorder; state explicitly that no frozen/tail work was added.
4. **`fix(aarch64): make off-stack termination a scheduler state transition`** — disclose syscall pivot, fault/remote `ExitPending`, embedded grace, and kernel-stack ownership.
5. **`feat(task): reclaim exited resources from a dedicated worker`** — disclose removal of drains from exception/schedule/fork reachability, process-table backlog, wake fallback, global TLBI context, and lack of a capacity panic.
6. **`fix(memory): make frame metadata safe across faults and preemption`** — disclose both architectures’ CoW callers and retry behavior.
7. **`test(aarch64): enforce teardown ownership and frozen-tail invariants`** — include stress, fault re-entry, stuck-CPU diagnostics, source-structure checks, and gold-master hashes.

Each slice must be independently build-clean. No message may describe worker reclaim as merely “during scheduling,” omit fault-handler reachability, or hide a removed double-terminate guard behind a return-type change.
