# R16 Findings — verbatim extraction from wf_12f308ca-93d journal

Source: /Users/wrb/.claude/projects/-Users-wrb-fun-code-breenix/06f628f3-e6a0-47fc-93f0-63d8534b3cd1/subagents/workflows/wf_12f308ca-93d/journal.jsonl


---

## opus-review-grave (journal entry idx 9, agentId ace85f18ce55cc29f) — FULL 20-finding review

```json
{
  "findings": [
    {
      "file": "kernel/src/signal/delivery.rs",
      "line": 217,
      "summary": "deliver_default_action no longer terminates the process \u2014 it only builds a ParentNotification \u2014 but kernel/src/syscall/handler.rs:684 discards that notification, so an x86_64 fatal default-action signal delivered on the syscall-return path never kills anything.",
      "failureScenario": "On x86_64, a process is sent SIGTERM/SIGSEGV (SIG_DFL) while blocked in a syscall. check_signals_on_syscall_return calls deliver_pending_signals; deliver_default_action clears the pending bit, returns Terminated(notification); handler.rs:684 matches `Terminated(_notification)`, sets need_resched and switches to idle, dropping the notification. retire_process is never called, the row stays ExitStage::Live and Ready, and the scheduler re-dispatches the process, which keeps running with the signal consumed. On main, process.terminate() had already run under the PM lock. handler.rs is Tier-1 prohibited so the branch could not fix it, which means the regression ships.",
      "category": "correctness"
    },
    {
      "file": "kernel/src/process/manager.rs",
      "line": 1360,
      "summary": "find_terminated_child never filters on `reaped`, and on aarch64 mark_reaped leaves the row in the map, so waitpid returns the same dead child repeatedly instead of ECHILD.",
      "failureScenario": "Parent forks one child; child exits. waitpid(-1) \u2192 complete_wait \u2192 mark_reaped(child) returns None on aarch64 (row kept until kreclaimd's detach_one_removable_row). Parent calls waitpid(-1) again: child_count still counts the row (parent field unchanged), so no ECHILD; find_terminated_child matches ProcessState::Terminated again and returns the same pid/status. A `while (waitpid(-1,&st,0) > 0)` shell loop spins on one child forever, and a double-reap corrupts any exit-status accounting. The window lasts until kreclaimd finishes reaped \u2227 Reclaimed \u2227 work-bits-empty, and is unbounded if a CLONE_VM sibling still names the root.",
      "category": "correctness"
    },
    {
      "file": "kernel/src/process/manager.rs",
      "line": 1196,
      "summary": "service_one_reparent lost main's `if pid != init_pid` guard, so if PID 1 ever commits an exit the reparent loop reassigns its children to itself forever and kreclaimd livelocks reporting DidWork.",
      "failureScenario": "init (PID 1) exits or is killed. retire_process sets REPARENT_CHILDREN. kreclaimd calls service_one_reparent: parent_pid = 1, init_pid = 1; it finds a child whose parent == Some(1), sets child.parent = Some(1), returns true (DidWork). The bit is never cleared and the same child matches on every pass, so kreclaimd loops on this one item and never services graves, exit-pending threads, FDs, or row removal \u2014 the whole reclaim pipeline stalls while the system leaks address spaces.",
      "category": "correctness"
    },
    {
      "file": "kernel/src/arch_impl/aarch64/syscall_entry.S",
      "line": 368,
      "summary": "The two-word lease record is published with two plain stores and read with two independent volatile loads, so the \u00a76.3 \"no row under-reports\" proof does not hold: a reader can observe {saved=old, next=0} while hardware TTBR0 holds the new root.",
      "failureScenario": "Writer (asm site or install_process_ttbr0 at ttbr0.rs:55) executes `str x1,[x0,#80]` then `str xzr,[x0,#64]` with no dmb ishst between them; AArch64 permits another observer to see the next-clear before the saved-publish. Independently, ttbr0_shadow_snapshot (per_cpu_aarch64.rs:188) reads saved first and next second as two separate volatile loads, so even with program-ordered stores a reaper that samples saved at T1 (still the old root O) and next at T2 (already cleared to 0) concludes root R is unoccupied. root_liveness then reports not-live for a root a peer CPU is actively executing under at EL0, and reclaim_pass frees its frames \u2014 use-after-free of a live page table. Fix requires a release barrier between the two stores and a next-then-saved (or re-validated) read order.",
      "category": "memory-ordering"
    },
    {
      "file": "kernel/src/arch_impl/aarch64/context_switch.rs",
      "line": 4734,
      "summary": "switch_ttbr0_if_needed dropped its `current != next` comparison and now calls install_process_ttbr0 unconditionally, adding an msr + broadcast `tlbi vmalle1is` + 2 dsb + 2 isb to every user-thread dispatch \u2014 under the scheduler lock with IRQs off.",
      "failureScenario": "A userspace thread blocked in a syscall is re-dispatched onto the same address space (the common case). On main, next_ttbr0 == current TTBR0 so the whole msr/tlbi block was skipped and only next_cr3 was cleared. Now every such dispatch at context_switch.rs:3148 and :3358 (inside dispatch_thread_locked, scheduler lock held, DAIF masked) issues an inner-shareable broadcast TLB invalidate. At 1000 Hz scheduling with N CPUs this is a large new inner-shareable broadcast rate on the dispatch path, directly increasing the class of stall the spec's invariant 19 exists to prevent \u2014 and the branch's own commit message claims TLBIs were removed, not added.",
      "category": "hot-path-regression"
    },
    {
      "file": "kernel/src/arch_impl/aarch64/exception.rs",
      "line": 348,
      "summary": "defer_current_user_thread_sigsegv_exit pushes a fault-exit intent onto the per-CPU ring but never bumps RECLAIM_WORK_GEN or unparks kreclaimd, so a parked reclaimer can sleep through the intent indefinitely.",
      "failureScenario": "An EL1 kernel fault on a user thread reaches defer_current_user_thread_sigsegv_exit; defer_fault_sigsegv_exit(tid) pushes the tid and request_exit_pending(tid) marks the thread. kreclaimd previously returned Empty and parked with observed_generation == RECLAIM_WORK_GEN. Nothing on this path calls kreclaim_wake(), so the park predicate stays true and the reclaimer never wakes. The victim process is never retired: no grave, no FD close, no SIGCHLD wake, and the parent's waitpid blocks forever. On main both ERET tails drained this ring on every exception return; the branch deleted both drains (context_switch.rs:3435, :4457) without adding a wake.",
      "category": "liveness"
    },
    {
      "file": "kernel/src/process/mod.rs",
      "line": 86,
      "summary": "ExitReceipt::complete() ends with a log::debug! and calls with_scheduler + kthread_unpark, and it is invoked from all four aarch64 EL0 fault arms via kill_current_user_process_and_redirect while DAIF is fully masked.",
      "failureScenario": "handle_sync_exception (DAIF masked on entry, never unmasked \u2014 the only daifclr in exception.rs is inside handle_irq) reaches kill_current_user_process_and_redirect at exception.rs:298, which calls receipt.complete(). complete() acquires the SERIAL lock via log::debug! and the SCHEDULER lock twice (unblock_for_child_exit/unblock_for_signal, then kreclaim_wake \u2192 kthread_unpark \u2192 with_scheduler). If the CPU faulted while another context on that CPU held SERIAL or SCHEDULER, the fault handler spins forever with interrupts masked \u2014 the exact PM/SERIAL-under-IRQ-off deadlock class the design set out to remove. The spec claims invariant 2 (\"no logging on tails / under the PM lock\") is satisfied; this path violates it.",
      "category": "deadlock-risk"
    },
    {
      "file": "kernel/src/task/reclaim.rs",
      "line": 243,
      "summary": "The aarch64 reclaim path frees only leaf user frames (cleanup_cow_page_table) plus the L0 root, leaking every L1/L2/L3 table frame, while the x86 path frees all of them via cleanup_for_exec \u2014 an undisclosed arch asymmetry.",
      "failureScenario": "A process with a sparse but wide address space exits. reclaim_pass calls cleanup_cow_page_table(&page_table, ctx), which only walks mapped pages and decrefs USER_ACCESSIBLE leaves, then deallocate_frame(root) for L0. The L1/L2/L3 intermediate tables that ProcessPageTable allocated are never returned to the frame allocator (there is no Drop for ProcessPageTable). Every exit leaks one table frame per populated L1/L2/L3 node; a fork/exec/exit stress loop exhausts the frame allocator. x86's arch_retire_address_space at reclaim.rs:395 calls page_table.cleanup_for_exec(&context), which does free them \u2014 same lifecycle, two different outcomes, contradicting the design's invariant 26.",
      "category": "resource-leak"
    },
    {
      "file": "kernel/src/process/manager.rs",
      "line": 2571,
      "summary": "drain_old_page_tables was deleted from Process and removed from all four exec entry points (leaving three orphaned comments), so pending_old_page_tables now accumulates for the whole lifetime of a repeatedly-exec'ing process instead of being drained at the next exec.",
      "failureScenario": "A long-lived shell or supervisor execs N programs. On main each exec drained the previous old page table at the start of the next exec. Now nothing drains them; each exec pushes another Box<ProcessPageTable> onto pending_old_page_tables, which is only swapped into the grave at exit and freed by kreclaimd. A process that execs 1000 times pins 1000 old address spaces (leaf frames included) until it dies. The comments at manager.rs:2571, :2914 and :3452 still say \"Drain any pending old page tables from previous exec() calls\" with no statement under them, and no commit message mentions the retention change.",
      "category": "resource-retention"
    },
    {
      "file": "kernel/src/task/process_task.rs",
      "line": 132,
      "summary": "handle_thread_exit's rewrite silently dropped the `#[cfg(feature = \"btrt\")] btrt::on_process_exit(pid, exit_code)` hook; the function now has zero callers in the tree.",
      "failureScenario": "With --features btrt (the in-tree test harness), the framework relies on on_process_exit to count process completions and auto-finalize the run (see main_aarch64.rs:1086 / main.rs:1645, \"auto-finalize happens via on_process_exit()\"). After this branch no code path calls it, so btrt never observes a process exit, never auto-finalizes, and btrt-gated tests hang or report incomplete results. Nothing in the commit messages discloses the removal.",
      "category": "test-coverage"
    },
    {
      "file": "kernel/src/process/manager.rs",
      "line": 1271,
      "summary": "detach_one_removable_row drops the Process row \u2014 and with it main_thread, which owns the forked child's KernelStack allocation on aarch64 \u2014 without any of CP9's kernel-stack predicates.",
      "failureScenario": "complete_fork_aarch64 (manager.rs:1935-1966) allocates the child's kernel stack and stores the KernelStack in child_thread, which becomes child_process.main_thread; the scheduler's copy gets kernel_stack_allocation: None (Thread::clone, thread.rs:519). CP9's guard (\u00acis_kernel_stack_slot_live \u2227 fence elapsed \u2227 TERMINATED, scheduler.rs:1040) therefore gates a Thread that owns nothing. detach_one_removable_row gates only on reaped \u2227 Reclaimed \u2227 work-bits-empty \u2227 no live sharer; when the row is dropped, KernelStack::drop (kernel_stack.rs:85) returns the 64 KiB slot to the pool. If the grave's fence elapsed but the dying thread is still standing on that stack, the slot is re-handed-out and zero-filled underneath it.",
      "category": "lifecycle-hole"
    },
    {
      "file": "kernel/src/task/reclaim.rs",
      "line": 368,
      "summary": "kreclaimd executes `yield_current(); wfi` after every successful unit of work, so the CPU idles up to a full tick per item even when the run queue has runnable threads and the graveyard is backlogged.",
      "failureScenario": "A fork/exit stress loop queues 50 graves plus FD, reparent and row-removal work. service_one returns DidWork for each item, and each iteration then executes wfi with IRQs enabled, halting the CPU until the next interrupt (~1 ms at 1000 Hz). Draining 50 items therefore takes \u226550 ms of wall time with the CPU parked, while userspace threads sit ready. Under sustained exit pressure the graveyard grows faster than it drains and memory pressure surfaces as ENOMEM in fork.",
      "category": "efficiency"
    },
    {
      "file": "kernel/src/task/reclaim.rs",
      "line": 349,
      "summary": "block_for_liveness_retry blocks the thread on a timer, then calls schedule(), then yield_current(), then wfi \u2014 four consecutive reschedule mechanisms on a thread that has already been marked Blocked.",
      "failureScenario": "When a grave is blocked on liveness, kreclaimd calls block_current_for_timer(now+10ms) which sets the thread Blocked and pushes a timer_heap entry; schedule() switches away. When the timer fires and the thread resumes, it continues at yield_current() (a second reschedule) and then wfi (a third stall). If schedule() instead returns without switching (no other runnable thread), the thread executes wfi while its scheduler state says Blocked, which is a state the scheduler's blocked/ready invariants do not model. The spec's \u00a75.3 asked for exactly one blocking call per pass.",
      "category": "correctness"
    },
    {
      "file": "kernel/src/arch_impl/aarch64/context_switch.rs",
      "line": 3390,
      "summary": "Two dispatcher arms still set ThreadState::Terminated directly, so the branch's claim that ExitPending is the only edge for a faulted or remotely killed thread is not accurate.",
      "failureScenario": "set_next_ttbr0_for_thread returns None for an EXIT_COMMITTED row, which falls into the ProcessGone arm; dispatch_thread_locked then writes `thread.state = ThreadState::Terminated` at context_switch.rs:3181 and :3390, bypassing request_exit_pending. set_terminated() immediately stamps a fresh RetirementFence, so the thread becomes reclaimable two epochs later regardless of how it got there. x86 does the same at interrupts/context_switch.rs:1015/:1018, signal.rs:171, handlers.rs:174 and delivery.rs:709. Commit 14ce7db8's \"ExitPending prevents a faulted or remotely killed thread from becoming reclaimable while still on its own stack\" overstates what the code enforces.",
      "category": "commit-honesty"
    },
    {
      "file": "tests/teardown_structure.rs",
      "line": 14,
      "summary": "The structure tests are built on `git diff main`, so every diff-based assertion becomes vacuous or fails outright the moment this branch merges to main.",
      "failureScenario": "exception_return_tails_do_not_drain_or_reclaim asserts the diff *contains* the removed drain line. After merge, `git diff main -- context_switch.rs` is empty and the assertion fails permanently, turning a merged-and-correct tree red. frozen_regions_remain_outside_the_branch_diff and only_the_reviewed_assembly_file_changed become vacuously true, silently stopping enforcement of exactly the invariants the commit says they lock. The tests also shell out to `rg` and `git`, which are not guaranteed in a CI container (rg exits 1 with no matches, tripping the `status.success()` assert).",
      "category": "test-design"
    },
    {
      "file": "tests/teardown_structure.rs",
      "line": 152,
      "summary": "frozen_regions_remain_outside_the_branch_diff checks that the literal strings \"idle_loop_arm64\"/\"dispatch_thread_locked\"/\"aarch64_enter_exception_frame\" do not appear in the diff text, which is a hunk-header grep, not the hash comparison the spec required.",
      "failureScenario": "A hunk that edits code inside dispatch_thread_locked passes as long as `git diff`'s @@ context line happens to name an enclosing or preceding symbol (the branch does modify code at context_switch.rs:3148/:3358, inside that function's body, and the test still passes). Conversely, an unrelated hunk whose context line mentions idle_loop_arm64 fails the test with no frozen-region change at all. The gate gives false confidence about the six gold-master regions named in CLAUDE.md.",
      "category": "test-design"
    },
    {
      "file": "kernel/src/process/process.rs",
      "line": 435,
      "summary": "Process::cleanup_cow_frames now has zero callers on both arches after the ReclaimContext parameter was threaded through it, leaving dead code that the project's zero-tolerance policy forbids.",
      "failureScenario": "grep shows the only occurrences are the two definitions (process.rs:396 x86, :435 aarch64) plus a comment in frame_metadata.rs:186. Its previous callers (Process::terminate, release_process_resources) were both deleted. It is `pub`, so rustc emits no dead_code warning and the zero-warning gate does not catch it. The spec lists it as one of the four ReclaimContext-gated entry points, so the capability's stated blast radius is one function wider than reality.",
      "category": "dead-code"
    },
    {
      "file": "kernel/src/process/process.rs",
      "line": 167,
      "summary": "Process::live_thread_count is written at birth (=1) and at commit (=0) and never read; ParentNotification::parent_pid is likewise written by exit_request and never read.",
      "failureScenario": "grep for live_thread_count returns only process.rs:167 (declaration), :287 (init) and :342 (assignment). grep for `.parent_pid` on the notification returns no reads \u2014 notify_parent_of_termination_deferred (delivery.rs:695) resolves the parent inside retire_process instead. Both are `pub` so no warning fires. A future reader will assume live_thread_count tracks thread-group liveness (the spec introduces it for exactly that) and will build a multithreaded-exit decision on a field that is never maintained.",
      "category": "dead-code"
    },
    {
      "file": "kernel/src/arch_impl/aarch64/syscall_entry.rs",
      "line": 411,
      "summary": "The `SyscallNumber::Exit | SyscallNumber::ExitGroup` arm was split into two arms that call the identical function, so the spec's \"split Exit vs ExitGroup dispatcher arms\" item is churn with no behavioral difference.",
      "failureScenario": "Both arms are `sys_exit_aarch64(arg1 as i32)`. exit_group on a multi-threaded (CLONE_VM) process therefore still retires only the caller's row and marks only the caller's thread, leaving sibling rows Live and their threads Ready \u2014 the same behavior as exit(2). A reader diffing against the spec would conclude thread-group exit semantics were addressed; they were not.",
      "category": "commit-honesty"
    },
    {
      "file": "kernel/src/task/reclaim.rs",
      "line": 163,
      "summary": "grave_reclaimable takes the blocking, all-DAIF-masked PM lock once per page table per grave inside the reclaim_pass scan, and reclaim_pass is also called inline from sys_fork_aarch64 where it drains the entire graveyard.",
      "failureScenario": "With K graves each holding 1 current + M old page tables, one reclaim_pass performs K*(1+M) separate acquisitions of PROCESS_MANAGER (each masking all DAIF bits and scanning the whole BTreeMap in root_has_live_sharer) plus K*(1+M) SCHEDULER acquisitions via cached_ttbr0_holder. Invoked from sys_fork_aarch64 (syscall_entry.rs:918) this runs inline in the fork syscall with no bound on K, so a fork after a burst of exits pays an unbounded, IRQ-masking scan before it allocates the child page table.",
      "category": "efficiency"
    }
  ],
  "summary": "Reviewed all 8 commits on fix/teardown-grave vs 31126c2a against the grave+lease+reclaimer spec. The aarch64 kernel builds clean (0 warnings), and the operator-approved Tier-1 syscall_entry.S edit is correct as written \u2014 but the surrounding Rust lifecycle carries several live correctness and liveness regressions, the most serious being an x86 signal path that no longer kills, a waitpid double-reap, and a fault-exit path that can never wake the reclaimer.\\n\\nTier-1 assembly verdict (kernel/src/arch_impl/aarch64/syscall_entry.S). The full-file diff is exactly two hunks, 6 insertions / 6 deletions; `git diff -- '*.S'` names only this file, and no other line in it moved. Site 1 (:365-372): x0 = per-CPU base from `mrs x0, tpidr_el1` (:359), x1 = next root from `ldr x1,[x0,#64]` (:361); both are still live and unclobbered at the two new stores, and both are pure scratch here (x0/x1 are restored from the frame after .Lafter_ttbr_check). Site 2 (:466-473) is the same shape with x9/x10, re-derived later at .Lskip_sp_fix. No new register is consumed at either site. `str` sets no flags and the only nearby flag consumer (`cbz`) precedes both stores. Offset 80 is verified as saved_process_ttbr0 and 64 as next_ttbr0 against per_cpu_aarch64.rs:45-49. Barrier placement matches the spec exactly: both stores sit after the post-`msr` `isb` and before `tlbi vmalle1is`; per-CPU data is a TTBR1 VA so it is unaffected by the TTBR0 change. Path length is +1 store per site, and commit be116df7 says so explicitly rather than claiming parity. `.Lrestore_saved_ttbr` / `.Lfirst_entry_restore_ttbr` are untouched, and no guard, ERET/ISB ordering or banner changed. The one defect is not local to the edit: there is no release barrier between the saved-publish and the next-clear, and the reader (per_cpu_aarch64.rs:188) samples the two words as independent volatile loads, so the \u00a76.3 \\\"no row under-reports\\\" proof does not actually hold (finding 4).\\n\\n20 findings are reported spanning correctness (x86 signal-kill no longer terminates; waitpid double-reap; init reparent livelock), memory ordering, hot-path regression (an unconditional broadcast TLBI added to every dispatch under the scheduler lock, which the commit message describes as a TLBI *reduction*), liveness (deferred fault exits never wake kreclaimd), deadlock risk (logging + scheduler locks from DAIF-masked fault arms), grave-lifecycle holes (intermediate page-table frames leaked on aarch64 only; kernel stacks freed by row removal without the CP9 predicate; old exec tables never drained), dead code, test design (the structure tests break permanently on merge), and commit honesty. Frozen regions are byte-for-byte untouched and the four gold-master files outside context_switch.rs show no diff; no runtime validation gates (QEMU boots, fork/exit stress, CLONE_VM owner-exit, Parallels streak) appear to have been run, which commit 14ce7db8 discloses."
}
```


---

## gate-classify-grave (journal entry idx 11, agentId a9a849faf807839e4) — gate classification

```json
{
  "blocking": [
    "kernel/src/signal/delivery.rs:217 - deliver_default_action no longer terminates the process on x86_64 syscall-return path; handler.rs discards the ParentNotification so fatal signals never kill the process (lifecycle hole).",
    "kernel/src/process/manager.rs:1360 - find_terminated_child doesn't filter on `reaped`; on aarch64 waitpid returns the same dead child repeatedly instead of ECHILD (double-reap/lifecycle hole).",
    "kernel/src/process/manager.rs:1196 - service_one_reparent lost the `pid != init_pid` guard; if PID 1 exits, reparent loop livelocks and kreclaimd stalls on the whole reclaim pipeline (lifecycle/liveness hole).",
    "kernel/src/arch_impl/aarch64/syscall_entry.S:368 - lease record published/read without proper barriers/ordering, allowing a reaper to free a live page table (UAF) - syscall_entry.S issue beyond approved sites.",
    "kernel/src/arch_impl/aarch64/context_switch.rs:4734 - switch_ttbr0_if_needed dropped the current!=next check, unconditionally issuing broadcast TLBI on every user dispatch under scheduler lock with IRQs off, while commit message claims TLBIs were removed (hot-path regression + dishonest commit message).",
    "kernel/src/arch_impl/aarch64/exception.rs:348 - defer_current_user_thread_sigsegv_exit never bumps RECLAIM_WORK_GEN or wakes kreclaimd, so a parked reclaimer can sleep through the fault-exit intent forever (missed wakeup lifecycle hole).",
    "kernel/src/process/mod.rs:86 - ExitReceipt::complete() calls log::debug! and acquires SERIAL/SCHEDULER locks while DAIF is fully masked on all four aarch64 EL0 fault arms, violating the no-logging/no-blocking-lock-on-tail invariant (deadlock risk on ERET-reachable path).",
    "kernel/src/task/reclaim.rs:243 - aarch64 reclaim path frees only leaf frames and the L0 root, leaking all L1/L2/L3 page table frames on every process exit (undisclosed leak-forever lifecycle hole, x86 path unaffected).",
    "kernel/src/process/manager.rs:2571 - drain_old_page_tables deleted from all four exec entry points, leaving orphaned comments; pending_old_page_tables now accumulates for the process's whole lifetime instead of draining at next exec (leak-forever lifecycle hole, undisclosed in commit message)."
  ],
  "nonBlocking": [],
  "notes": [
    "All nine findings were classified BLOCKING: they span dishonest/discarded fatal-signal delivery, double-reap and livelock lifecycle holes, a UAF-inducing ordering bug in syscall_entry.S beyond the two approved sites, an unconditional broadcast TLBI added to the aarch64 dispatch hot path (paired with a commit message that claims the opposite), a missed-wakeup deadlock in reclaim, a logging/lock-acquisition deadlock risk reachable from EL0 fault ERET tails, and two distinct leak-forever regressions (page-table frames, pending old page tables). None qualified as pre-existing-unworsened, style, or reviewer-stated-impossible hypotheticals.",
    "The provided finding list appears truncated at the end (final entry cut off mid-path 'kernel/s...'); only the nine complete findings were classified."
  ]
}
```
