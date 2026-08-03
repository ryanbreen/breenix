# r20 Findings — Full 27-Finding Review (opus-review-r20) + Gate Classification (gate-classify-r20b)

Source: wf_26479cca-41d/journal.jsonl (opus agent a13a5e4612877b65c = opus-review-r20; sonnet agent a1467e17949d47928 = gate-classify-r20b)

This is the final-review round for `fix/teardown-grave` before the branch was
re-parked. Findings and gate classification are reproduced verbatim from the
workflow journal (no paraphrasing, no re-ranking beyond the review's own
numbering).

## opus-review-r20 — summary

Final review of fix/teardown-grave (diff vs 31126c2a, 16 commits) found the branch structurally sound and its prior-round closures genuinely held, but surfaced one new blocking-class EL0 fault regression plus 26 further findings across correctness, efficiency, simplification, test-coverage and documentation.

WHAT HELD (verified, not taken on trust):
- Builds: aarch64 (`cargo build --release --target aarch64-breenix.json ... kernel-aarch64`) and x86_64 (`--features testing,external_test_bins --bin qemu-uefi`) both complete with 0 warnings and 0 errors.
- `cargo test --test teardown_structure`: 9 passed, 0 failed, including the six gold-master region hashes.
- Tier-1 assembly (`syscall_entry.S`): still exactly two hunks, 6+/6-. Verified instruction-by-instruction. At :364-373, x0 = tpidr_el1 and x1 = next_cr3 are both live and unclobbered across `str x1,[x0,#80]` / `dmb ishst` / `str xzr,[x0,#64]`; offsets 80/64 match `PerCpuData.saved_process_ttbr0`/`next_ttbr0` (per_cpu_aarch64.rs:46/:50, backed by the 192-byte size assert); `str`/`dmb` set no flags and the only nearby flag consumer (`cbz` at :362) precedes them; the `tlbi vmalle1is; dsb ish; isb` that follows is unchanged, so ASID-1 reuse across roots is still covered. Site 2 (:466-475) is the same shape with x9/x10. The writer-side `dmb ishst` pairs correctly with the new reader-side `dmb ishld` in per_cpu_aarch64.rs:194, which was reordered to read `next` before `saved` — a textbook message-passing pair: observing `next == 0` implies observing `saved == installed root`, and the window between `msr` and the `saved` store still has `next` naming the new root, so the protocol over-reports liveness rather than under-reporting it. `.Lrestore_saved_ttbr`/`.Lfirst_entry_restore_ttbr` install exactly what `saved` already names and correctly leave the shadows alone. Every path still reaches an `eret`; both post-`eret` tails (`mov x0,#0xDEAD; b .` and `mov x0,#0xCC; b .`) remain unreachable-by-design markers.
- r18 item (b) is closed as claimed: I extracted `frame_metadata.rs` at base 31126c2a and compared function-by-function — the five x86_64 bodies (`frame_register`, `frame_incref`, `frame_decref`, `frame_refcount`, `frame_metadata_stats`) are byte-identical to main, including both `log::error!`/`log::trace!` diagnostic paths, and all reclaimer plumbing (PinnedMetadataGuard, FaultMetadataTransaction, try_fault_transaction, FrameMetadataRetry, decref_locked, the two counters) is behind `#[cfg(target_arch = "aarch64")]` matching its only two call sites.
- r18 item (a) is a comment-only ratification; `handle_thread_exit`'s x86 body is otherwise unchanged.
- r17's x86 restorations held: `kernel/src/interrupts.rs` is untouched vs base, and x86 `exit_process`/`terminate`/`terminate_minimal`/`cleanup_cow_frames`/`close_all_fds` are semantically identical to main (only renames and comment deletions).
- The kreclaimd park/wake protocol is correct on the point r17 flagged: `kthread_park_if` publishes `parked=true` before re-testing the generation, and because every producer publishes its work before `kreclaim_wake()` bumps `RECLAIM_WORK_GEN`, the read-gen/scan/compare-gen ordering admits no lost wakeup. `scheduler.unblock` ignores non-Blocked states, so a spurious unpark of a Running kreclaimd cannot double-enqueue it, and the `RECLAIM_KTHREAD`-not-yet-set path correctly restores `RECLAIM_WAKE_PENDING`.

HEADLINE NEW ISSUE: `kill_current_user_process_and_redirect` (exception.rs:298) has five early returns — try_manager miss, PM uninitialised, CR3 lookup miss, main_thread None/non-User, and `try_request_exit_pending` try_lock failure — and all four EL0 call sites (:789, :1106, :1154, :1223) `return` unconditionally after it. Any of those five leaves the exception frame untouched, so the handler ERETs back to the faulting user instruction and refaults immediately, forever, with the process never dying. The pre-branch code fell through to `defer_current_user_thread_sigsegv_exit` + postmortem + idle redirect in exactly these situations. Two of the triggers (PM/SCHEDULER try_lock contention) are routine under SMP load, and one (CR3 lookup miss after `commit_grave` takes the page table) is the direct regression of the deleted `already_terminated` branch. Related: the victim is now identified as the CR3-owning row's `main_thread` rather than the actual faulting thread, which mis-targets CLONE_VM threads and leaves the real faulter runnable.

Also worth the reviewer's attention beyond the list: `kernel/src/syscall/time.rs`'s raw TTBR0 writer is a live, self-documented violation of the lease invariant that the structural test whitelists rather than fixes; `ReclaimContext`'s capability checks are entirely `debug_assert!` and so vanish in release; the aarch64 exec drain is now an out-of-band caller contract enforced only by a release-stripped `debug_assert!`; and x86 `sys_waitpid`/procfs picked up undocumented behavior changes beyond the two ratified in PARK_STATE.md.

Per instruction, findings are reported at every severity without ranking or a mergeability verdict.

## opus-review-r20 — findings (27)

### Finding 1: kernel/src/arch_impl/aarch64/exception.rs:306 [correctness]

**Summary:** `kill_current_user_process_and_redirect` has five silent early-return paths that leave the exception frame unmodified, and all four EL0 call sites `return` unconditionally afterwards, so the handler ERETs back to the faulting instruction forever.

**Failure scenario:** A user process takes a data abort at EL0 while any other CPU holds PROCESS_MANAGER (fork/exec/exit are frequent). `crate::process::try_manager()` at :306 returns None, the function returns without touching `frame.elr`/`frame.spsr`, and :789 does `return`. The assembly ERETs to the same faulting user instruction, which aborts again immediately — a permanent per-CPU fault storm with the process never dying, the parent's waitpid never waking, and no log output. Identical outcomes for :310 (PM uninitialised), :314 (no CR3 match), :319 (main_thread None/non-User), and :329 (`try_request_exit_pending` SCHEDULER try_lock failure, which is contended on every timer tick). The pre-branch code fell through to `defer_current_user_thread_sigsegv_exit` + postmortem + idle redirect in exactly these cases.

### Finding 2: kernel/src/arch_impl/aarch64/exception.rs:314 [correctness]

**Summary:** The victim is looked up by `find_process_by_cr3_mut`, which requires `process.page_table` to still be `Some`, so a second fault on an already-exit-committed process (whose page table moved into the grave) finds nothing and returns without redirecting.

**Failure scenario:** Thread A of a process faults, `kill_current_user_process_and_redirect` runs `retire_process`→`commit_grave`, which does `self.page_table.take()`. Another thread of the same address space (or the same thread re-dispatched before the ExitPending edge lands) faults on the same root. `find_process_by_cr3_mut(page_table_phys)` at manager.rs:1619 skips every process with `page_table == None`, so `victim` is None → early return → refault loop. The removed `already_terminated` branch in the pre-branch code existed precisely to redirect this case to idle.

### Finding 3: kernel/src/arch_impl/aarch64/exception.rs:317 [correctness]

**Summary:** The fault victim is taken as `process.main_thread` of the row that owns the faulting CR3, not the thread that actually faulted, so a CLONE_VM thread's fault kills the wrong thread and never quarantines the faulting one.

**Failure scenario:** `sys_clone` (clone.rs:208-217) creates a child row with `page_table: None` and `inherited_cr3: Some(parent_cr3)`. When that clone thread faults at EL0, `find_process_by_cr3_mut` matches the PARENT row (the only one with a real `page_table`), so `try_request_exit_pending`/`defer_fault_sigsegv_exit` target the parent's main thread. The parent is killed, and the actually-faulting clone thread stays Ready in the scheduler with an unmodified frame → re-dispatched → refaults → livelock. The pre-branch code called `terminate_current_scheduler_thread()` (current_thread_id) and got the right thread; the sibling helper `fault_victim_tid` (:349) still uses the correct last-dispatched-tid scheme, so the two fault paths now disagree about who the victim is.

### Finding 4: kernel/src/syscall/time.rs:141 [correctness]

**Summary:** `ensure_current_address_space` still writes TTBR0_EL1 with raw asm without publishing `saved_process_cr3`/`next_cr3`, a live violation of the lease invariant the whole branch is built on, and the invariant test enshrines it as an accepted exception.

**Failure scenario:** A thread returns from `nanosleep`/`clock_nanosleep` and this helper installs process root R into TTBR0_EL1 without setting either shadow. kreclaimd concurrently evaluates a grave holding R: `root_liveness` (ttbr0.rs:150) reads only `local_hardware` for its own CPU plus other CPUs' `saved`/`next` shadows, both of which are 0 for this CPU, so it concludes R is dead, runs `invalidate_user_tlb_broadcast` and `cleanup_for_exec`, and returns R's frames to the allocator while this CPU's hardware TTBR0 still points at them. The next EL0 access reads recycled memory. tests/teardown_structure.rs:174 lists this file as `deferred_writer` so the structural gate passes with the violation in place.

### Finding 5: kernel/src/task/process_task.rs:339 [correctness]

**Summary:** A full 16-slot per-CPU deferred-fault-exit ring silently drops the exit intent (only a counter is bumped), stranding the process forever with no retirement, no reparent, and no parent wake.

**Failure scenario:** Sixteen faults land on one CPU faster than kreclaimd drains (`take_deferred_fault_sigsegv_exit` returns one tid per `service_one` pass, and each pass re-scans process/scheduler tables). `DeferredFaultExitBuffer::push` returns false, `FAULT_EXIT_INTENT_DROPPED` is incremented at exception.rs:340, and execution continues to the idle redirect. Because `handle_thread_exit`/`retire_process` are only ever reached via that queue on this path, the victim is left in `ExitPending` with its page tables, kernel stack, FDs and window buffers intact indefinitely; its parent blocks in waitpid forever. Recovery requires reading a counter in a fatal postmortem.

### Finding 6: kernel/src/task/scheduler.rs:1031 [correctness]

**Summary:** `finalize_exit_pending` requires the victim's kernel-stack slot to be non-live, but an ExitPending thread is removed from every ready queue and never re-dispatched, so a thread quarantined mid-syscall can never make its own stack quiesce.

**Failure scenario:** `sys_kill` SIGKILL on aarch64 (signal.rs) calls `request_exit_pending(main_thread_id)` on a process blocked inside a syscall on its own kernel stack. `request_exit_pending` sets ExitPending and strips it from all `per_cpu_queues`, and `schedule()` now treats ExitPending like Terminated (scheduler.rs:1173, :1236, :1260). If that stack slot is reported live by `is_kernel_stack_slot_live` (its resume SP is a per-CPU live slot), `finalize_exit_pending` returns false on every pass, the thread never reaches Terminated, `detach_reclaimable_thread` never fires, and its Thread + kernel-stack allocation leak permanently.

### Finding 7: kernel/src/task/reclaim.rs:378 [efficiency]

**Summary:** `blocked |= has_pending_thread_reclaim()` makes kreclaimd re-arm a 10 ms timer forever whenever any non-idle ExitPending/Terminated thread cannot be reclaimed, so the reclaimer never parks again for the life of the boot.

**Failure scenario:** One thread gets stuck per the finding above (or any Terminated thread whose stack stays live). `has_pending_thread_reclaim()` at :378/:432 returns true on every pass, `service_one` returns `Blocked`, and `kreclaimd_main` (:477) calls `block_for_liveness_retry()` which blocks for exactly 10 ms and reschedules. The reclaimer then wakes 100 times a second forever, each wake taking PROCESS_MANAGER (which masks IRQs) and SCHEDULER several times and doing O(processes)+O(threads) scans — a permanent power and lock-contention regression that never self-heals and produces no diagnostic beyond the 30 s one-shot `log::warn!`.

### Finding 8: kernel/src/process/manager.rs:3386 [correctness]

**Summary:** The aarch64 exec paths replaced an unconditional `process.drain_old_page_tables()` with a `debug_assert!`, which compiles to nothing in release, so any exec caller that omits the new out-of-band drain silently accumulates retired page tables.

**Failure scenario:** Only `sys_exec_aarch64` (syscall_entry.rs:1164) calls `crate::task::reclaim::drain_old_page_tables_for_exec(current_pid)` before entering exec. `kernel/src/test_exec.rs` (6 sites), `kernel/src/userspace_test.rs:886` and `kernel/src/syscall/handlers.rs:2038/2380/2597` all call `manager.exec_process*` directly and are compiled (ungated modules) on aarch64; on that arch they resolve to the aarch64 bodies at :3349/:3642 where the drain is now only a `debug_assert!`. In a release build each such exec pushes another `Box<ProcessPageTable>` onto `pending_old_page_tables` that is never walked or freed until the process exits — an unbounded frame leak with no warning. Same issue at manager.rs:3672.

### Finding 9: kernel/src/arch_impl/aarch64/exception.rs:1968 [efficiency]

**Summary:** `CowFaultOutcome::Retry` returns to EL0 to re-execute the faulting instruction, so FRAME_METADATA contention turns into an unbounded exception-retry loop, each iteration re-acquiring PROCESS_MANAGER with interrupts masked.

**Failure scenario:** kreclaimd runs `cleanup_for_exec` on a large address space, repeatedly taking FRAME_METADATA via `PinnedMetadataGuard`. A userspace thread CoW-faults: `handle_cow_fault_arm64` acquires PROCESS_MANAGER (blocking, IRQ-masking), reaches `try_fault_transaction()` at :1968, gets `Err(FrameMetadataRetry)`, drops PM and ERETs. The instruction re-executes and faults again immediately. Under a fork-heavy workload this spins the faulting CPU through full exception round trips plus PM acquisitions for as long as the reclaimer holds the metadata map, and inflates `cow_stats::TOTAL_FAULTS`/`MANAGER_PATH` so those counters no longer mean what they used to.

### Finding 10: kernel/src/arch_impl/aarch64/exception.rs:1957 [correctness]

**Summary:** The new `FaultMetadataTransaction` holds FRAME_METADATA across `allocate_frame`, a 4 KiB page copy, `unmap_page` and `map_page`, introducing a PM→FRAME_METADATA→FRAME_ALLOCATOR lock nesting that did not exist before.

**Failure scenario:** Previously `frame_is_shared`, `frame_register` and `frame_decref` each took and released the metadata lock, so FRAME_ALLOCATOR was never acquired underneath it. Now the transaction spans :1968-:2035, and `allocate_frame()` at :2005 plus any intermediate table allocation inside `map_page` take FRAME_ALLOCATOR while FRAME_METADATA is held (with IRQs masked by the PM guard). Any future code path that takes FRAME_ALLOCATOR and then FRAME_METADATA deadlocks; and every peer CPU calling `frame_decref` now spins on FRAME_METADATA with preemption disabled for the whole copy+remap window instead of a few instructions.

### Finding 11: kernel/src/task/reclaim.rs:55 [correctness]

**Summary:** `ReclaimContext::assert_preemptible` is composed entirely of `debug_assert!`s, so in the shipping release build the capability token proves nothing about interrupts, preempt count, or PM-lock ownership.

**Failure scenario:** `ReclaimContext` is threaded through `cleanup_for_exec`, `release_stack` and `reclaim_pass` as a proof that destructive reclaim runs preemptibly, and tests/teardown_structure.rs:249 enforces exactly three mint sites on that basis. In release all three checks at :56, :58 and :63 vanish, so `reclaim_one` called from `sys_fork_aarch64` (syscall_entry.rs:917) with interrupts masked or with PROCESS_MANAGER already held would mint a valid-looking context and then take PM and SCHEDULER inside `grave_reclaimable` — a self-deadlock that the design says is impossible.

### Finding 12: kernel/src/process/mod.rs:140 [correctness]

**Summary:** `process_manager_lock_held_by_current_cpu` reads owner bookkeeping that `try_manager()` never writes, so the ReclaimContext ownership check is blind to the non-blocking acquisition path.

**Failure scenario:** `try_manager()` (mod.rs:209) returns a bare `spin::MutexGuard` and never calls `note_process_manager_lock_acquired()`, so `PROCESS_MANAGER_OWNER_CPU` stays `PM_LOCK_OWNER_NONE`. Any code holding PM via `try_manager` — e.g. `set_next_ttbr0_for_thread` (context_switch.rs:4764) or `kill_current_user_process_and_redirect` (exception.rs:306) — that reached `ReclaimContext::assert_preemptible()` would pass the `!process_manager_lock_held_by_current_cpu()` assert even in a debug build.

### Finding 13: kernel/src/process/process.rs:521 [correctness]

**Summary:** `can_remove_row()` requires `reaped`, which is only set by waitpid, so orphans reparented to init leak their Process row, main-thread copy and kernel-stack allocation forever unless init reaps them.

**Failure scenario:** A parent exits with live children. `service_one_reparent` (manager.rs:1310) sets each child's `parent = Some(ProcessId::new(1))` but nothing ever calls waitpid on their behalf. When those children exit, `retire_process` commits their graves and the reclaimer frees their page tables and marks them `Reclaimed`, but `reaped` stays false, so `removable_row_candidate` never selects them. The `Process` row — including `main_thread`, which is the sole owner of the ARM64 kernel-stack bitmap slot (`kernel_stack_allocation`, cloned as `None` into the scheduler at creation.rs:85) — is retained for the life of the boot, exhausting `ARM64_STACK_BITMAP` under a fork-heavy orphan workload.

### Finding 14: kernel/src/syscall/handlers.rs:2760 [correctness]

**Summary:** x86 `sys_waitpid` was rewritten to route through the new `child_count`/`is_child_of`/`find_terminated_child` helpers, an undocumented third x86 behavior change beyond the two ratified in PARK_STATE.md.

**Failure scenario:** The new x86 `child_pids` (manager.rs:1496) filters children through `self.processes.contains_key(child)`, which the old `current_process.children.is_empty()` / `.contains(&target_pid)` checks did not. A parent whose `children` vec still lists a pid whose row was already removed now gets `ECHILD` from :2782 or :2797 where it previously proceeded into the blocking/polling path. Combined with the analogous `procfs::generate_pid_status` change (fs/procfs/mod.rs:1060), this is shared-path x86 behavior that was never enumerated for review as `frame_metadata.rs` and the reparent guard were.

### Finding 15: kernel/src/process/manager.rs:1137 [simplification]

**Summary:** `mark_reaped` is declared `-> Option<Process>` but unconditionally returns `None`, and both call sites bind the result and `drop()` it as if it could carry a row.

**Failure scenario:** wait.rs:352 and handlers.rs:3057 do `let reaped_row = manager.mark_reaped(child_pid); ... drop(manager_guard); drop(reaped_row);` — a deliberate-looking 'drop the heavy row outside the PM lock' pattern that is a no-op, because the body is `self.processes.get_mut(&pid)?.mark_reaped(); None`. A future reader will assume row destruction already happens outside the lock and will not notice it is actually deferred to `detach_removable_row`.

### Finding 16: kernel/src/task/reclaim.rs:343 [efficiency]

**Summary:** `GRAVE_DETACH_LOCK` guards only an atomic swap and provides no mutual exclusion between concurrent consumers, and `reclaim_one` detaches the entire graveyard on the fork hot path to reclaim at most one node.

**Failure scenario:** `reclaim_pass` (:343) and `reclaim_one` (:353) both take the lock solely around `GRAVEYARD.swap(null, Acquire)`, which is already atomic, then release it before `reclaim_detached` runs — so two consumers can process disjoint detached lists concurrently regardless of the lock. Meanwhile `sys_fork_aarch64` calls `reclaim_one`, which walks every queued grave: nodes past the limit are re-pushed via `push_grave_inner` at :277 without evaluation, reversing list order each fork and turning the fork path into an O(queued graves) operation for one reclamation.

### Finding 17: kernel/src/task/scheduler.rs:566 [correctness]

**Summary:** `RetirementSnapshot::acquire` treats a CPU that was online at capture time as permanently blocking, so a CPU that goes offline or stops entering the scheduler pins every outstanding fence forever.

**Failure scenario:** `RetirementFence::capture()` records `epochs[cpu] = SCHEDULING_EPOCHS[cpu] + 2` for each CPU online at that instant and never re-consults `is_cpu_online` afterwards. If that CPU subsequently parks permanently (offline, wedged in a spin, or a boot-time secondary that never re-enters `schedule()`), `SCHEDULING_EPOCHS[cpu]` stops advancing, `acquire` returns None for every fence captured before it stalled, and every grave and every ExitPending/Terminated thread is blocked forever — with kreclaimd pinned in the 10 ms `Blocked` retry loop.

### Finding 18: kernel/src/process/manager.rs:1305 [correctness]

**Summary:** `service_one_reparent` reparents orphans to `ProcessId::new(1)` without checking that PID 1 still exists in the process table.

**Failure scenario:** If init's row has been removed (or on a configuration where PID 1 is not the reaper), children get `parent = Some(ProcessId::new(1))` pointing at a missing row. `retire_process` then computes `NOTIFY_PARENT` as false because `self.processes.contains_key(&parent_pid)` fails (manager.rs:1240), so no SIGCHLD is ever raised, `child_pids(1)` is never consulted by any live waiter, `reaped` stays false, and the rows accumulate exactly as in the orphan-leak finding.

### Finding 19: kernel/src/arch_impl/aarch64/exception.rs:346 [correctness]

**Summary:** The EL0 kill path downgraded from blocking `switch_to_idle()` to `switch_to_idle_best_effort()`, so on SCHEDULER try_lock failure the CPU's `cpu_state.current_thread` still names the thread being torn down.

**Failure scenario:** `switch_to_idle_best_effort` (scheduler.rs:3591) silently does nothing when `SCHEDULER.try_lock()` fails. The frame has already been repointed at `idle_loop_arm64` on the idle stack and `set_idle_stack_for_eret()` has nulled `current_thread_ptr`, but `sched.cpu_state[cpu].current_thread` still holds the victim tid and `resolve_exception_cleanup_previous_thread` never runs. The next `schedule()` on that CPU treats the ExitPending victim as the outgoing current thread and may write a save into its context slot — the exact stale-`cpu_state` class of bug the recent `0cfa03e0`/`d27c2362` commits were chasing.

### Finding 20: kernel/src/arch_impl/aarch64/exception.rs:2025 [correctness]

**Summary:** Late `CowFaultOutcome::NotHandled` returns after `metadata.register(new_frame)` leak the freshly allocated frame and now also cause the caller to kill the process rather than retry.

**Failure scenario:** `allocate_frame` succeeds, `metadata.register(new_frame)` inserts it at refcount 1 (:2011), the page is copied, then `page_table.unmap_page(page)` or `map_page` fails and the function returns `NotHandled` at :2025/:2028. `new_frame` is now tracked at refcount 1, mapped nowhere, and unreachable from any page-table walk, so `cleanup_for_exec` will never decref it — it is leaked for the boot. `handle_sync_exception` :593 then treats `NotHandled` as 'not a CoW fault' and proceeds to SIGSEGV the process, and (per the first finding) the page table has by then been unmapped, so a re-fault would take the `find_process_by_cr3_mut` miss path.

### Finding 21: kernel/src/task/reclaim.rs:296 [correctness]

**Summary:** `log::warn!` is invoked from inside kreclaimd's grave-scanning loop, taking the SERIAL/framebuffer locks in the same worker that other exit paths deliberately avoid logging from.

**Failure scenario:** A grave stalls 30 s and `grave.warned` is false, so kreclaimd formats and emits a warning while it is the sole servicer of deferred fault exits, FD closes, graphics cleanup and parent wakes. If the logger lock is contended (or held by a CPU blocked waiting on work only kreclaimd can do), the entire teardown pipeline stalls behind it. The whole point of the surrounding redesign was to move logging and heavy destructors off the PM-lock path; this reintroduces a logger dependency into the single-threaded reclaim engine.

### Finding 22: docs/planning/teardown-lifecycle/PARK_STATE.md:41 [documentation]

**Summary:** PARK_STATE.md asserts the two `syscall_entry.S` hunks are the only Tier-1-listed file touched, but `kernel/src/syscall/time.rs` is on the Tier-1 prohibited list in CLAUDE.md and is modified on this branch.

**Failure scenario:** A reviewer or gate pass trusting :41 and :162 concludes no Tier-1 approval is needed beyond commit be116df7. In fact `kernel/src/syscall/time.rs` (Tier-1, 'clock_gettime precision - called in tight loops') carries a two-line comment added at :142-143, and the same file is the one carrying the unreviewed raw TTBR0 writer that tests/teardown_structure.rs:174 whitelists. The comment is codegen-neutral, but the claim as written is factually wrong and the file it concerns is exactly the one with the outstanding invariant violation.

### Finding 23: tests/teardown_structure.rs:190 [test-coverage]

**Summary:** The 'no direct Terminated transitions' invariant is a syntactic check for the literal `.state = ThreadState::Terminated`, which the branch's own code routes around by calling `thread.set_terminated()` instead.

**Failure scenario:** `context_switch.rs:3226`, `:3320` and `:4083` still transition threads straight to Terminated without going through the ExitPending quarantine edge — they just spell it `set_terminated()`. The assertion at :190 passes, so the 9/9 invariant score reports the ExitPending protocol as universally enforced when three dispatch/exit sites bypass it. Similarly :112 forbids the string `drain_deferred_fault_sigsegv_exits` but nothing prevents a future ERET tail from calling `take_deferred_fault_sigsegv_exit` outside the two function bodies enumerated at :113-116.

### Finding 24: kernel/src/process/manager.rs:834 [simplification]

**Summary:** An `#[cfg(target_arch = "x86_64")]` statement guard was added inside `spawn_process`, which is itself `#[cfg(target_arch = "aarch64")]`, making the block unconditionally dead.

**Failure scenario:** `spawn_process` at manager.rs:808 is aarch64-only, so the `parent.children.push(child_pid)` at :835 can never compile in. The intent was clearly to drop the push on aarch64 (which derives children from `child.parent`), but the result is a permanently-dead x86 cfg inside an aarch64 function that reads like live x86 code to anyone grepping for children-list maintenance.

### Finding 25: kernel/src/task/reclaim.rs:33 [simplification]

**Summary:** `task::reclaim` is already gated `#[cfg(target_arch = "aarch64")]` at task/mod.rs:43, yet ~20 items inside it repeat the same cfg, and the paired `#[cfg(not(target_arch = "aarch64"))] pub struct RetirementFence;` is unreachable.

**Failure scenario:** Every `#[cfg(target_arch = "aarch64")]` at reclaim.rs:33, :36, :38, :40, :43, :45, :48, :57, :110, :150, :167, :179, :206, :263, :340, :350, :359, :366, :459, :470, :485 is redundant, and the non-aarch64 `RetirementFence` ZST at scheduler.rs:552 can never be instantiated because the only field of that type (`ProcessGrave.fence`) is itself aarch64-gated. Meanwhile `Thread.retirement_fence` (thread.rs:506) is NOT gated, so x86 `Thread` grows a dead `Option<ZST>` field that `set_terminated` never writes — a small, unadvertised x86 struct change.

### Finding 26: kernel/src/syscall/graphics.rs:1732 [correctness]

**Summary:** `has_window_buffers` is latched true before the registry allocation is attempted and is never cleared, so the CLEANUP_GRAPHICS exit obligation can be scheduled for a process that owns no window.

**Failure scenario:** `handle_create_window_buffer` sets the flag at :1732 but the registry insert does not happen until `reg.allocate(...)` at :1809; if allocation or the intervening mmap fails, the flag stays set. On exit `retire_process` (manager.rs:1239) sets `ExitWorkBits::CLEANUP_GRAPHICS`, kreclaimd claims it, takes WINDOW_REGISTRY, finds nothing to remove, and moves on. Harmless today (coverage is otherwise correct — `allocate` has exactly one caller and it is inside this function), but it is a latch that can only ever over-report, and the flag also survives `exec`, so the obligation persists across a full program replacement.

### Finding 27: kernel/src/ipc/fd.rs:338 [efficiency]

**Summary:** `take_next_for_exit` rescans the descriptor table from fd 0 on every call and is driven one descriptor per full `service_one` pass, making FD teardown quadratic in MAX_FDS and serialised behind the reclaimer's whole scan loop.

**Failure scenario:** A process exits holding N open descriptors. kreclaimd's `service_one` (reclaim.rs:412) claims exactly one fd per pass; each pass first re-walks the deferred-fault ring, `finalize_one_exit_pending`, `service_one_reparent`, `claim_one_parent_wake` and `claim_one_graphics_cleanup`, then calls `claim_one_exit_fd`, which itself rescans `self.fds[0..MAX_FDS]` under the PM lock (with IRQs masked). Closing N descriptors therefore costs N full reclaimer passes and N table scans, delaying pipe-EOF wakeups for readers by that many scheduling rounds.

## gate-classify-r20b — blocking (15)

- 1. [correctness] kernel/src/arch_impl/aarch64/exception.rs:306 — `kill_current_user_process_and_redirect` has five silent early-return paths that leave the exception frame unmodified, and all four EL0 call sites `return` unconditionally afterwards, so the handler ERETs back to the fau || scenario: A user process takes a data abort at EL0 while any other CPU holds PROCESS_MANAGER (fork/exec/exit are frequent). `crate::process::try_manager()` at :306 returns None, the function returns without tou
- 2. [correctness] kernel/src/arch_impl/aarch64/exception.rs:314 — The victim is looked up by `find_process_by_cr3_mut`, which requires `process.page_table` to still be `Some`, so a second fault on an already-exit-committed process (whose page table moved into the grave) finds nothing a || scenario: Thread A of a process faults, `kill_current_user_process_and_redirect` runs `retire_process`→`commit_grave`, which does `self.page_table.take()`. Another thread of the same address space (or the same 
- 3. [correctness] kernel/src/arch_impl/aarch64/exception.rs:317 — The fault victim is taken as `process.main_thread` of the row that owns the faulting CR3, not the thread that actually faulted, so a CLONE_VM thread's fault kills the wrong thread and never quarantines the faulting one. || scenario: `sys_clone` (clone.rs:208-217) creates a child row with `page_table: None` and `inherited_cr3: Some(parent_cr3)`. When that clone thread faults at EL0, `find_process_by_cr3_mut` matches the PARENT row
- 4. [correctness] kernel/src/syscall/time.rs:141 — `ensure_current_address_space` still writes TTBR0_EL1 with raw asm without publishing `saved_process_cr3`/`next_cr3`, a live violation of the lease invariant the whole branch is built on, and the invariant test enshrines || scenario: A thread returns from `nanosleep`/`clock_nanosleep` and this helper installs process root R into TTBR0_EL1 without setting either shadow. kreclaimd concurrently evaluates a grave holding R: `root_live
- 5. [correctness] kernel/src/task/process_task.rs:339 — A full 16-slot per-CPU deferred-fault-exit ring silently drops the exit intent (only a counter is bumped), stranding the process forever with no retirement, no reparent, and no parent wake. || scenario: Sixteen faults land on one CPU faster than kreclaimd drains (`take_deferred_fault_sigsegv_exit` returns one tid per `service_one` pass, and each pass re-scans process/scheduler tables). `DeferredFault
- 6. [correctness] kernel/src/task/scheduler.rs:1031 — `finalize_exit_pending` requires the victim's kernel-stack slot to be non-live, but an ExitPending thread is removed from every ready queue and never re-dispatched, so a thread quarantined mid-syscall can never make its  || scenario: `sys_kill` SIGKILL on aarch64 (signal.rs) calls `request_exit_pending(main_thread_id)` on a process blocked inside a syscall on its own kernel stack. `request_exit_pending` sets ExitPending and strips
- 8. [correctness] kernel/src/process/manager.rs:3386 — The aarch64 exec paths replaced an unconditional `process.drain_old_page_tables()` with a `debug_assert!`, which compiles to nothing in release, so any exec caller that omits the new out-of-band drain silently accumulate || scenario: Only `sys_exec_aarch64` (syscall_entry.rs:1164) calls `crate::task::reclaim::drain_old_page_tables_for_exec(current_pid)` before entering exec. `kernel/src/test_exec.rs` (6 sites), `kernel/src/userspa
- 10. [correctness] kernel/src/arch_impl/aarch64/exception.rs:1957 — The new `FaultMetadataTransaction` holds FRAME_METADATA across `allocate_frame`, a 4 KiB page copy, `unmap_page` and `map_page`, introducing a PM→FRAME_METADATA→FRAME_ALLOCATOR lock nesting that did not exist before. || scenario: Previously `frame_is_shared`, `frame_register` and `frame_decref` each took and released the metadata lock, so FRAME_ALLOCATOR was never acquired underneath it. Now the transaction spans :1968-:2035, 
- 12. [correctness] kernel/src/process/mod.rs:140 — `process_manager_lock_held_by_current_cpu` reads owner bookkeeping that `try_manager()` never writes, so the ReclaimContext ownership check is blind to the non-blocking acquisition path. || scenario: `try_manager()` (mod.rs:209) returns a bare `spin::MutexGuard` and never calls `note_process_manager_lock_acquired()`, so `PROCESS_MANAGER_OWNER_CPU` stays `PM_LOCK_OWNER_NONE`. Any code holding PM vi
- 14. [correctness] kernel/src/syscall/handlers.rs:2760 — x86 `sys_waitpid` was rewritten to route through the new `child_count`/`is_child_of`/`find_terminated_child` helpers, an undocumented third x86 behavior change beyond the two ratified in PARK_STATE.md. || scenario: The new x86 `child_pids` (manager.rs:1496) filters children through `self.processes.contains_key(child)`, which the old `current_process.children.is_empty()` / `.contains(&target_pid)` checks did not.
- 19. [correctness] kernel/src/arch_impl/aarch64/exception.rs:346 — The EL0 kill path downgraded from blocking `switch_to_idle()` to `switch_to_idle_best_effort()`, so on SCHEDULER try_lock failure the CPU's `cpu_state.current_thread` still names the thread being torn down. || scenario: `switch_to_idle_best_effort` (scheduler.rs:3591) silently does nothing when `SCHEDULER.try_lock()` fails. The frame has already been repointed at `idle_loop_arm64` on the idle stack and `set_idle_stac
- 20. [correctness] kernel/src/arch_impl/aarch64/exception.rs:2025 — Late `CowFaultOutcome::NotHandled` returns after `metadata.register(new_frame)` leak the freshly allocated frame and now also cause the caller to kill the process rather than retry. || scenario: `allocate_frame` succeeds, `metadata.register(new_frame)` inserts it at refcount 1 (:2011), the page is copied, then `page_table.unmap_page(page)` or `map_page` fails and the function returns `NotHand
- 21. [correctness] kernel/src/task/reclaim.rs:296 — `log::warn!` is invoked from inside kreclaimd's grave-scanning loop, taking the SERIAL/framebuffer locks in the same worker that other exit paths deliberately avoid logging from. || scenario: A grave stalls 30 s and `grave.warned` is false, so kreclaimd formats and emits a warning while it is the sole servicer of deferred fault exits, FD closes, graphics cleanup and parent wakes. If the lo
- 22. [documentation] docs/planning/teardown-lifecycle/PARK_STATE.md:41 — PARK_STATE.md asserts the two `syscall_entry.S` hunks are the only Tier-1-listed file touched, but `kernel/src/syscall/time.rs` is on the Tier-1 prohibited list in CLAUDE.md and is modified on this branch. || scenario: A reviewer or gate pass trusting :41 and :162 concludes no Tier-1 approval is needed beyond commit be116df7. In fact `kernel/src/syscall/time.rs` (Tier-1, 'clock_gettime precision - called in tight lo
- 23. [test-coverage] tests/teardown_structure.rs:190 — The 'no direct Terminated transitions' invariant is a syntactic check for the literal `.state = ThreadState::Terminated`, which the branch's own code routes around by calling `thread.set_terminated()` instead. || scenario: `context_switch.rs:3226`, `:3320` and `:4083` still transition threads straight to Terminated without going through the ExitPending quarantine edge — they just spell it `set_terminated()`. The asserti

## gate-classify-r20b — non-blocking (12)

- 7. [efficiency] kernel/src/task/reclaim.rs:378 — `blocked |= has_pending_thread_reclaim()` makes kreclaimd re-arm a 10 ms timer forever whenever any non-idle ExitPending/Terminated thread cannot be reclaimed, so the reclaimer never parks again for the life of the boot. || scenario: One thread gets stuck per the finding above (or any Terminated thread whose stack stays live). `has_pending_thread_reclaim()` at :378/:432 returns true on every pass, `service_one` returns `Blocked`, 
- 9. [efficiency] kernel/src/arch_impl/aarch64/exception.rs:1968 — `CowFaultOutcome::Retry` returns to EL0 to re-execute the faulting instruction, so FRAME_METADATA contention turns into an unbounded exception-retry loop, each iteration re-acquiring PROCESS_MANAGER with interrupts maske || scenario: kreclaimd runs `cleanup_for_exec` on a large address space, repeatedly taking FRAME_METADATA via `PinnedMetadataGuard`. A userspace thread CoW-faults: `handle_cow_fault_arm64` acquires PROCESS_MANAGER
- 11. [correctness] kernel/src/task/reclaim.rs:55 — `ReclaimContext::assert_preemptible` is composed entirely of `debug_assert!`s, so in the shipping release build the capability token proves nothing about interrupts, preempt count, or PM-lock ownership. || scenario: `ReclaimContext` is threaded through `cleanup_for_exec`, `release_stack` and `reclaim_pass` as a proof that destructive reclaim runs preemptibly, and tests/teardown_structure.rs:249 enforces exactly t
- 13. [correctness] kernel/src/process/process.rs:521 — `can_remove_row()` requires `reaped`, which is only set by waitpid, so orphans reparented to init leak their Process row, main-thread copy and kernel-stack allocation forever unless init reaps them. || scenario: A parent exits with live children. `service_one_reparent` (manager.rs:1310) sets each child's `parent = Some(ProcessId::new(1))` but nothing ever calls waitpid on their behalf. When those children exi
- 15. [simplification] kernel/src/process/manager.rs:1137 — `mark_reaped` is declared `-> Option<Process>` but unconditionally returns `None`, and both call sites bind the result and `drop()` it as if it could carry a row. || scenario: wait.rs:352 and handlers.rs:3057 do `let reaped_row = manager.mark_reaped(child_pid); ... drop(manager_guard); drop(reaped_row);` — a deliberate-looking 'drop the heavy row outside the PM lock' patter
- 16. [efficiency] kernel/src/task/reclaim.rs:343 — `GRAVE_DETACH_LOCK` guards only an atomic swap and provides no mutual exclusion between concurrent consumers, and `reclaim_one` detaches the entire graveyard on the fork hot path to reclaim at most one node. || scenario: `reclaim_pass` (:343) and `reclaim_one` (:353) both take the lock solely around `GRAVEYARD.swap(null, Acquire)`, which is already atomic, then release it before `reclaim_detached` runs — so two consum
- 17. [correctness] kernel/src/task/scheduler.rs:566 — `RetirementSnapshot::acquire` treats a CPU that was online at capture time as permanently blocking, so a CPU that goes offline or stops entering the scheduler pins every outstanding fence forever. || scenario: `RetirementFence::capture()` records `epochs[cpu] = SCHEDULING_EPOCHS[cpu] + 2` for each CPU online at that instant and never re-consults `is_cpu_online` afterwards. If that CPU subsequently parks per
- 18. [correctness] kernel/src/process/manager.rs:1305 — `service_one_reparent` reparents orphans to `ProcessId::new(1)` without checking that PID 1 still exists in the process table. || scenario: If init's row has been removed (or on a configuration where PID 1 is not the reaper), children get `parent = Some(ProcessId::new(1))` pointing at a missing row. `retire_process` then computes `NOTIFY_
- 24. [simplification] kernel/src/process/manager.rs:834 — An `#[cfg(target_arch = "x86_64")]` statement guard was added inside `spawn_process`, which is itself `#[cfg(target_arch = "aarch64")]`, making the block unconditionally dead. || scenario: `spawn_process` at manager.rs:808 is aarch64-only, so the `parent.children.push(child_pid)` at :835 can never compile in. The intent was clearly to drop the push on aarch64 (which derives children fro
- 25. [simplification] kernel/src/task/reclaim.rs:33 — `task::reclaim` is already gated `#[cfg(target_arch = "aarch64")]` at task/mod.rs:43, yet ~20 items inside it repeat the same cfg, and the paired `#[cfg(not(target_arch = "aarch64"))] pub struct RetirementFence;` is unre || scenario: Every `#[cfg(target_arch = "aarch64")]` at reclaim.rs:33, :36, :38, :40, :43, :45, :48, :57, :110, :150, :167, :179, :206, :263, :340, :350, :359, :366, :459, :470, :485 is redundant, and the non-aarc
- 26. [correctness] kernel/src/syscall/graphics.rs:1732 — `has_window_buffers` is latched true before the registry allocation is attempted and is never cleared, so the CLEANUP_GRAPHICS exit obligation can be scheduled for a process that owns no window. || scenario: `handle_create_window_buffer` sets the flag at :1732 but the registry insert does not happen until `reg.allocate(...)` at :1809; if allocation or the intervening mmap fails, the flag stays set. On exi
- 27. [efficiency] kernel/src/ipc/fd.rs:338 — `take_next_for_exit` rescans the descriptor table from fd 0 on every call and is driven one descriptor per full `service_one` pass, making FD teardown quadratic in MAX_FDS and serialised behind the reclaimer's whole scan || scenario: A process exits holding N open descriptors. kreclaimd's `service_one` (reclaim.rs:412) claims exactly one fd per pass; each pass first re-walks the deferred-fault ring, `finalize_one_exit_pending`, `s

## gate-classify-r20b — notes

- Classification basis: BLOCKING = frozen/Tier-1 violation, ERET-tail/interrupt-context logging or lock-taking, new race/deadlock/UAF, lifecycle hole (stranding/leaking/killing-wrong-thread), hot-path/teardown regression, cfg-seam leak, unjustified x86 divergence, or dishonest doc/commit; severe uncategorized correctness bugs default to BLOCKING.
- #4 and #22 are linked: time.rs is an explicit CLAUDE.md Tier-1 file ('clock_gettime precision'), and PARK_STATE.md's claim that no additional Tier-1 file was touched is contradicted by the branch's own diff — treated as a Tier-1-violation/dishonest-doc pair, both BLOCKING.
- #19 (stale cpu_state.current_thread on switch_to_idle_best_effort fallback) matches the exact bug class the repo's own recent commit history (0cfa03e0, d27c2362) was fighting — high-confidence BLOCKING given project history.
- #11 and #12 both concern the ReclaimContext/PM-ownership safety-token machinery: #11 (debug_assert-only preemptibility proof) is judged a weakened-invariant/test-gap issue with no stated active manifestation → NON-BLOCKING; #12 (try_manager() never registers PM ownership) is judged a concrete blind-spot that can cause the ownership check to give a false answer during real PM-lock contention → BLOCKING (potential self-deadlock/race).
- #17 and #18 hinge on preconditions (CPU hot-offlining, PID 1 row removal) that do not appear reachable in Breenix's current single-configuration design — classified NON-BLOCKING as impossible-today hypotheticals; revisit if CPU hotplug or init-exit paths are later added.
- #13 (orphan zombie leak absent an init reaper) mirrors standard POSIX zombie-reaping semantics rather than a regression introduced by this branch — NON-BLOCKING unless the branch is shown to have broken init's reaping loop specifically.
- #23 is reviewer-tagged test-coverage but its content (context_switch.rs routes around the Terminated-transition invariant via set_terminated()) describes an actual bypass of the teardown redesign's core state-machine guarantee, not merely a weak assertion — treated as a substantive lifecycle-hole regression and classified BLOCKING; verify directly whether those set_terminated() call sites are pre-existing/exempt or new.
