# r17 Findings — Full 21-Finding Review (opus-review-hardened) + Gate Classification (gate-classify-hardened)

Source: wf_6ceb9a6f-ae9/journal.jsonl (opus agent a1b6904fd0711eab7 = opus-review-hardened; sonnet agent a7854b1b621097521 = gate-classify-hardened)

## opus-review-hardened — summary

Both architectures build clean (0 warnings), the 11-commit branch closes most of the 20 r16 findings, and the syscall_entry.S edit is correct instruction-by-instruction — but the hardening round introduced a new headline liveness bug that makes the entire reclaimer dead after its first park, plus an x86 free-of-the-live-CR3 root and several capability/context violations. Verdict on the Tier-1 assembly (kernel/src/arch_impl/aarch64/syscall_entry.S): the diff is still exactly two hunks, 6+/6-, and both are correct. Site 1 (:358-374): x0=tpidr_el1, x1=next_cr3 are both live and unclobbered at `str x1,[x0,#80]` / `str xzr,[x0,#64]`; offsets 80/64 match PERCPU_SAVED_PROCESS_CR3/NEXT_CR3; `str`/`dmb` set no flags and the only nearby flag consumer (`cbz`) precedes them; x0/x1 are re-derived at :395/:416 so nothing downstream is corrupted; the per-CPU record is a TTBR1 VA so it is unaffected by the TTBR0 change; and the added `dmb ishst` between the saved-publish and the next-clear now pairs correctly with the new reader-side `dmb ishld` in per_cpu_aarch64.rs:194 (which was reordered to read `next` first). That is a textbook message-passing pair, so r16 finding 4 (`no row under-reports`) is genuinely closed: observing next==0 implies observing saved==installed-root, and the transient window between `msr` and `str x1,[x0,#80]` still has `next` naming the new root, so it over-reports rather than under-reports. Site 2 (:460-476) is the same shape with x9/x10 and is equally correct. `.Lrestore_saved_ttbr` / `.Lfirst_entry_restore_ttbr` install exactly what `saved` already names, so they keep the shadow accurate without writing it; no guard, ERET/ISB ordering, banner or other line moved; `git diff -- '*.S'` names only this file and the structure test at tests/teardown_structure.rs:249 pins that. Closure verification on the prior list: r16 #1 (x86 fatal signal) closed via the DEFERRED_SIGNAL_EXITS slot ring + switch_to_idle arming + x86 idle drain; #2 (waitpid double-reap) closed — find_terminated_child/child_pids/is_child_of all filter `reaped`; #3 (init reparent livelock) closed by the pid!=1 fall-through that clears the bit; #4 closed (above); #5 closed — switch_ttbr0_if_needed restored the same-root fast path via complete_armed_process_ttbr0; #6 closed — defer_fault_sigsegv_exit now calls kreclaim_wake(); #7 closed for aarch64 — ExitReceipt::complete() is now bare atomics there; #8 closed — the aarch64 reclaim path calls cleanup_for_exec which frees L1/L2/L3 + L0; #9 closed — drain_old_page_tables_for_exec at all four exec entry points; #10 closed — btrt::on_process_exit restored; #11 closed — process_row_stack_reclaimable gates row removal; #12/#13 closed — no more yield+wfi, one block_current_for_timer per Blocked pass; #15/#16 closed — the tests are now source-tree + FNV-hash based, no git/rg; #17/#18 closed — cleanup_cow_frames/cleanup_cow_page_table/live_thread_count/parent_pid all deleted; #19 closed — the Exit/ExitGroup arms are merged again and the commit discloses that thread-group exit is unimplemented; #20 partly closed — fork now uses reclaim_one. #14 is only PARTLY closed: the two ProcessGone arms were converted, but two other dispatcher arms still write Terminated directly (finding below). 21 findings reported, unranked.

## opus-review-hardened — findings (21)

### Finding 0: kernel/src/task/reclaim.rs:479 [liveness]

**Summary:** kreclaimd parks with kthread_park_if but the only wake path (drain_teardown_intents -> Scheduler::unblock) never clears the kthread `parked` flag, so once the reclaimer parks it can never resume and the whole teardown pipeline stops permanently.

**Failure scenario:** At boot kreclaimd_main runs once, service_one returns Empty, and kthread_park_if sets handle.inner.parked = true and enters `while handle.inner.parked` (kthread.rs:164). A process then exits: kreclaim_wake() bumps RECLAIM_WORK_GEN and sets RECLAIM_WAKE_PENDING; the next Scheduler::schedule() calls drain_teardown_intents (scheduler.rs:1022) which does take_reclaim_wake_tid() -> self.unblock(tid). unblock() sets ThreadState::Ready and queues the thread but leaves `parked == true`, and the only writer of `parked = false` is kthread_unpark(&KthreadHandle) — whose sole handle was dropped at main_aarch64.rs:816. kreclaimd is dispatched, re-evaluates `while parked`, marks itself Blocked again, removes itself from the ready queue and WFIs. It never returns to kreclaimd_main. From that moment nothing services graves, exit-pending threads, FD closes, reparenting, parent wakes, graphics cleanup or row removal: page tables, kernel stacks and process rows leak forever and waitpid() on aarch64 never gets its NOTIFY_PARENT wake.

### Finding 1: kernel/src/arch_impl/aarch64/context_switch.rs:3226 [lifecycle-hole]

**Summary:** Two dispatcher arms still assign `thread.state = ThreadState::Terminated` directly instead of calling set_terminated(), so the thread gets no RetirementFence and its cached_ttbr0 is never cleared.

**Failure scenario:** restore_kernel_context_inline fails (context_switch.rs:3218-3227, IdleRedirectReason::KernelRestoreFailed) or a userspace dispatch sees a corrupt ELR/SPSR (:3313-3322, UserBadContext). The thread becomes Terminated with retirement_fence == None and a stale cached_ttbr0. Three consequences: (a) Scheduler::detach_reclaimable_thread bails at `let Some(fence) = thread.retirement_fence.as_ref() else { return false }`, so the Thread is never removed from scheduler.threads; (b) has_pending_thread_reclaim() therefore returns true forever, so service_one always returns ReclaimWork::Blocked and kreclaimd busy-loops through block_for_liveness_retry every 10 ms indefinitely instead of parking; (c) cached_ttbr0_holder_matching (scheduler.rs:615) still matches that dead thread against the dying process's root, so RootLiveness::is_live() returns true via cached_thread and grave_reclaimable blocks that grave permanently — the process's entire page-table tree is never freed. Commit 12c396b9's claim that fault, remote-kill and ProcessGone dispatch edges all route through ExitPending does not hold for these two arms.

### Finding 2: kernel/src/task/reclaim.rs:498 [use-after-free]

**Summary:** On x86 the process page table is freed while CR3 still points at it: sys_exit -> handle_thread_exit -> ExitReceipt::complete -> arch_retire_address_space -> cleanup_for_exec -> deallocate_frame(level_4_frame), with no x86 equivalent of leave_process_ttbr0().

**Failure scenario:** A userspace process on x86_64 calls exit(). syscall/handlers.rs:169 calls ProcessScheduler::handle_thread_exit, which retires the row and completes the receipt on the still-current thread whose CR3 is the process root. arch_retire_address_space (reclaim.rs:497-510) calls page_table.cleanup_for_exec(&context), which walks the tree and ends with `deallocate_frame(self.level_4_frame)` (process_memory.rs) — returning the live L4 root, plus every L3/L2/L1 table, to the frame allocator while CR3 still holds them and the kernel continues executing through them. The next allocate_frame() (x86 MAX_CPUS==1, so possibly the very next kernel allocation on this same path) hands out the L4 frame and its first write corrupts the active page table -> triple fault. On 31126c2a this was safe by accident: exit_process only did `process.page_table.take()`, and ProcessPageTable has no Drop impl, so the frames leaked instead of being freed. The aarch64 side avoids this by calling leave_process_ttbr0() before handle_thread_exit (syscall_entry.rs:359); x86 has no such step.

### Finding 3: kernel/src/process/mod.rs:71 [deadlock-risk]

**Summary:** x86 ExitReceipt::complete() mints a ReclaimContext (whose whole contract is 'preemptible, PM lock not held') from at least four interrupt-gate contexts with IF cleared, and performs page-table teardown, log macros and per-FD PM lock round-trips there.

**Failure scenario:** interrupts.rs:1467 (page_fault_handler fatal arm), interrupts.rs:1778 (general_protection_fault_handler), interrupts/context_switch.rs:1139 (restore_userspace_thread_context, timer IRQ) and interrupts/context_switch.rs:831 (switch_to_thread -> notify_parent_of_termination_deferred, timer IRQ) all call receipt.complete(). complete() runs `while let Some(fd) = manager()...take_next_fd_for_exit()` (a fresh PROCESS_MANAGER acquisition per descriptor, each close_owned_fd reaching into pipe/pty/tcp/unix-socket locks), then arch_retire_address_space -> ReclaimContext::assert_preemptible(), whose debug_assert!(crate::arch_interrupts_enabled()) is false inside an interrupt gate — debug builds panic inside the page-fault handler; release builds silently proceed and run cleanup_for_exec (which ends in log::info!, i.e. the SERIAL lock) plus complete()'s own trailing log::debug! from the fault handler. If the faulting context already held SERIAL or a pipe/pty lock, that is an unbreakable self-deadlock; either way it violates the project's 'no logging, no heavy work in interrupt handlers' rule and makes the capability's stated proof false at every x86 mint site.

### Finding 4: kernel/src/arch_impl/aarch64/exception.rs:317 [correctness]

**Summary:** kill_current_user_process_and_redirect identifies the EL0 fault victim with the LAST_DISPATCHED_TID heuristic (fault_victim_tid) instead of the exact TTBR0 -> find_process_by_cr3 lookup it replaced, and never checks that the tid is a user thread.

**Failure scenario:** On 31126c2a the four EL0 fault arms read TTBR0_EL1, masked off the ASID, and called pm.find_process_by_cr3_mut(page_table_phys) — an exact identification of the faulting address space. The branch replaces all four with kill_current_user_process_and_redirect (exception.rs:298), which calls fault_victim_tid: last_dispatched_tid_for_stack_address(frame_addr) with a fallback to last_dispatched_tid(cpu_id). If the stack-address lookup misses (a frame on a shared/boot stack, or a stack slot already re-handed-out) the fallback returns whatever tid was last stamped on this CPU — which after an inline-schedule or an idle redirect can name a completely different thread, including a kernel thread. service_one then runs handle_thread_exit(tid, -11) on that thread's process (retiring the wrong process with SIGSEGV status) and unconditionally request_exit_pending(tid), which will quarantine and later reclaim a kthread such as kreclaimd, softirqd or the render thread. The correct root is available for free in TTBR0_EL1 at the fault site.

### Finding 5: kernel/src/arch_impl/aarch64/exception.rs:298 [race]

**Summary:** The EL0 fault arms no longer make the victim non-runnable before redirecting to idle; quarantine is deferred to the next Scheduler::schedule() via drain_teardown_intents, weakening the guarantee whose deletion the old code explicitly warned about.

**Failure scenario:** terminate_current_scheduler_thread() (deleted at exception.rs:305 on main) existed precisely so that 'other CPUs can pick up the still-Ready thread and ERET into a freed address space' could not happen; it ran before switch_to_idle. kill_current_user_process_and_redirect now only queues a tid into DEFERRED_FAULT_EXIT_BUFFERS and calls switch_to_idle_best_effort(), which may fail its try_lock entirely (scheduler.rs:3584) and leave cpu_state stale. The victim's ThreadState stays Running/Ready until some CPU next enters Scheduler::schedule() and for_each_deferred_fault_exit marks it ExitPending. In that window check_need_resched_and_switch_arm64 processes DEFERRED_REQUEUE before schedule() and can push the victim back onto a ready queue; on an 8-CPU aarch64 build a peer CPU can dispatch it. set_next_ttbr0_for_thread only rejects it once the process is exit-committed, which has not happened yet because kreclaimd (not the fault handler) performs the retire, so the peer re-enters EL0 at the faulting PC.

### Finding 6: kernel/src/interrupts/context_switch.rs:714 [lock-order]

**Summary:** The x86 signal-Terminated arm now calls switch_to_idle() — which takes the SCHEDULER lock and emits log::info! — while PROCESS_MANAGER is still held, inside the timer interrupt.

**Failure scenario:** switch_to_thread's blocked_in_syscall path holds `manager_guard` (PROCESS_MANAGER, taken via process_manager_guard/try_manager at :583) across lines 585-816. On main the Terminated arm simply returned with a comment; the branch adds setup_idle_return(interrupt_frame) + crate::task::scheduler::switch_to_idle() at :713-714. switch_to_idle -> with_scheduler acquires SCHEDULER and then runs two log::info!/log::error! calls (scheduler.rs:3539-3555), so the timer-interrupt path now nests PROCESS_MANAGER -> SCHEDULER -> SERIAL with interrupts disabled. Any context that holds SCHEDULER (or SERIAL) and then blocks on PROCESS_MANAGER deadlocks the CPU. This is the exact nesting the aarch64 side of this same branch went out of its way to avoid — see the comment on process_row_stack_reclaimable (scheduler.rs:3018): 'This runs between two short PM transactions so PROCESS_MANAGER and SCHEDULER are never held together.'

### Finding 7: kernel/src/signal/delivery.rs:721 [lifecycle-hole]

**Summary:** defer_signal_exit's failure return is discarded with `let _ =` and there is no drop counter, so when all 16 DEFERRED_SIGNAL_EXITS slots are busy the x86 fatal-signal fix silently degrades back to the r16 finding-1 regression.

**Failure scenario:** exit_request() publishes the durable intent that lets the syscall-return path (handler.rs:684, Tier-1, discards the notification) still kill the process. DEFERRED_SIGNAL_EXITS has 16 slots and a slot is only released by cancel_deferred_signal_exit or by the idle-loop consumer. Send SIGTERM to 17 processes blocked in syscalls before the idle loop drains any of them: the 17th defer_signal_exit returns false, delivery.rs:721 throws the result away, handler.rs discards the notification, and no code path ever calls retire_process for that pid. The row stays ExitStage::Live and Ready with its signal bit cleared, so the process keeps running as if the signal were handled and its parent's waitpid never returns. Unlike the aarch64 fault ring, which increments FAULT_EXIT_INTENT_DROPPED and emits a raw-UART marker on overflow (exception.rs:302-306), this path is completely silent and dump_reclaim_state has no counter for it.

### Finding 8: kernel/src/arch_impl/aarch64/context_switch.rs:5042 [correctness]

**Summary:** Signal delivery now routes through install_process_ttbr0 with the untagged root, which publishes that raw value into saved_process_cr3, so the syscall-return path restores TTBR0 with ASID=0 instead of the dispatch-installed ASID=1.

**Failure scenario:** set_next_ttbr0_for_thread deliberately arms `tagged_ttbr0 = ttbr0 | (1u64 << 48)` with the comment that ASID=1 'combined with nG bits on process page table entries ensures ASID-based separation' from kernel ASID=0 entries. The old signal-delivery sites (context_switch.rs:5042 and syscall_entry.rs:239) executed a bare inline `msr ttbr0_el1, raw_ttbr0` and left saved_process_cr3 alone, so .Lrestore_saved_ttbr still restored the ASID=1 value on the way back to EL0. install_process_ttbr0 now calls complete_armed_process_ttbr0(root), which writes saved_process_cr3 = raw (ASID=0). After any signal delivery the thread returns to EL0 running under ASID 0 and keeps doing so on every subsequent syscall return until its next full dispatch re-arms the tagged root — silently defeating the ASID separation for an arbitrary number of user/kernel transitions. Nothing in the commit messages mentions an ASID behaviour change.

### Finding 9: kernel/src/task/process_task.rs:376 [hot-path-regression]

**Summary:** for_each_deferred_fault_exit is invoked from Scheduler::schedule() on every scheduling decision and scans 8x16 atomic slots, doing an O(threads) lookup plus a per-CPU-queue retain for each pending tid, all under the scheduler lock with DAIF masked.

**Failure scenario:** drain_teardown_intents is called at scheduler.rs:1169 and :1484, i.e. on both schedule paths. While DEFERRED_FAULT_EXIT_COUNT != 0 — which lasts from the fault until kreclaimd consumes the intent, and forever if kreclaimd is parked (see the kreclaimd finding) — every single schedule() performs 128 Acquire loads over DEFERRED_FAULT_EXIT_BUFFERS and, for each non-empty slot, Scheduler::request_exit_pending, which linearly scans self.threads and then runs `retain` over all 8 per-CPU ready queues. With 1000 Hz scheduling on 8 CPUs and a stuck intent, this is a permanent per-tick cost added to the scheduler critical section that the branch's own spec exists to keep short. Nothing bounds the residency of an intent.

### Finding 10: kernel/src/task/reclaim.rs:307 [efficiency]

**Summary:** reclaim_detached re-pushes a blocked grave onto the head of the Treiber stack, so reclaim_one from sys_fork_aarch64 repeatedly re-examines the same blocked grave and never advances to the rest of the list.

**Failure scenario:** take_one_grave pops the head, grave_reclaimable finds it blocked (a live sharer, an unelapsed fence, or a cached_ttbr0 holder), and push_grave_inner puts it straight back at the head. The next fork calls reclaim_one, pops the same head grave, pays another DAIF-masked PROCESS_MANAGER acquisition (root_has_live_sharer scans the whole BTreeMap) plus a SCHEDULER acquisition via cached_ttbr0_holder_matching, and re-pushes it. With one long-blocked grave at the head — e.g. a CLONE_VM root still named by a live sibling — every fork in the system pays that scan and zero graves behind it are ever reclaimed from the fork path, which is exactly the pressure-relief the 'bounded fork reclaim' mint site was added for.

### Finding 11: kernel/src/arch_impl/aarch64/ttbr0.rs:145 [correctness]

**Summary:** root_liveness documents the per-CPU saved/next shadows as 'the conservative superset maintained by the handoff protocol', but three in-tree TTBR0 writers install a root without touching either shadow, and the new structure test lists those files as reviewed without requiring lease maintenance.

**Failure scenario:** syscall/wait.rs:31, syscall/time.rs:145 and syscall/graphics.rs:126 each execute a bare `msr ttbr0_el1, ttbr0_value` to reach user memory, with no set_next_cr3/set_saved_process_cr3 and (in graphics.rs) not even the tlbi/dsb/isb tail. A reaper on another CPU sampling ttbr0_shadow_snapshot for that CPU therefore sees whatever the last lease transition left, not the root hardware actually holds — the exact under-report the §6.3 proof and the new dmb ishst/dmb ishld pair were added to eliminate. tests/teardown_structure.rs:110 enumerates all three files in the 'reviewed TTBR0 writer set' assertion, so the gate certifies them as reviewed while enforcing nothing about the two-word record; a future writer added to any of those files passes the test unchanged.

### Finding 12: kernel/src/task/reclaim.rs:150 [correctness]

**Summary:** take_one_grave is an unsynchronised Treiber pop whose ABA-safety depends entirely on an undocumented invariant that GRAVE_DETACH_LOCK serialises every consumer; nothing at the pop site says so and blocked-grave re-pushes happen outside that lock.

**Failure scenario:** take_one_grave loads head=A, reads next=B, then CAS(A->B). If any second consumer could pop between those steps — pop A, pop B, re-push A — the CAS would succeed with a stale `next`, dropping B's successors and handing B to two reclaimers (double free of a Box<ProcessGrave> plus double cleanup_for_exec on its page tables). Today that is prevented only because reclaim_pass (reclaim.rs:344) and reclaim_one (:354) both wrap their take in GRAVE_DETACH_LOCK, and push_grave_inner (called from :307 outside the lock) only ever pushes. Neither take_one_grave nor push_grave_inner carries a comment stating this; the ProcessGrave Send comment (:28-29) only reasons about producers. Adding a third consumer — e.g. an exec-time or ENOMEM-time drain — silently reintroduces the ABA double-free.

### Finding 13: kernel/src/process/mod.rs:55 [test-coverage]

**Summary:** The aarch64 ExitReceipt::complete() debug_assert is a tautology: mark_exit_committed unconditionally sets ExitWorkBits::all(), so a Committed outcome can never have empty work bits.

**Failure scenario:** complete() asserts `self.outcome != ExitOutcome::Committed || !self.work_bits.is_empty()` as the only aarch64-side enforcement that a committed exit published durable worker obligations. But retire_process copies process.exit_work_bits immediately after commit_grave -> mark_exit_committed (process.rs:340) sets exit_work_bits = ExitWorkBits::all(). The assert therefore cannot fire under any input, giving false confidence that the receipt protocol is checked. A future change that computes work bits conditionally (e.g. skipping CLOSE_FDS when the table is empty, or skipping NOTIFY_PARENT for an orphan) would produce a legitimately empty bit set and trip the assert for the wrong reason.

### Finding 14: kernel/src/process/process.rs:101 [correctness]

**Summary:** ExitWorkBits::contains is implemented as `self.0 & work.0 != 0`, which is 'intersects', not 'contains'.

**Failure scenario:** Every current caller passes a single bit (REPARENT_CHILDREN, NOTIFY_PARENT, CLEANUP_GRAPHICS, CLOSE_FDS) so the bug is latent. The moment anyone writes `bits.contains(ExitWorkBits::all())` or a two-bit composite to ask 'is all remaining work of this kind still pending', the predicate returns true when only one of the bits is set, so a claim_one_* helper will act on a row whose work is already partially drained — and the row-removal gate can_remove_row (which uses is_empty(), not contains()) will disagree with it.

### Finding 15: kernel/src/task/reclaim.rs:439 [race]

**Summary:** The row-removal stack proof and the row detach run in two separate PM transactions, and detach_removable_row re-verifies only can_remove_row and the live-sharer test — not the kernel-stack predicate the whole split exists to enforce.

**Failure scenario:** service_one snapshots RowRemovalCandidate under one PROCESS_MANAGER acquisition, releases it, calls process_row_stack_reclaimable under the SCHEDULER lock, then re-acquires PROCESS_MANAGER for detach_removable_row (manager.rs:1319-1329). The detach re-checks can_remove_row() and root_has_live_sharer() but never re-reads the stack liveness, so the Process row — and with it main_thread, which owns the forked child's KernelStack allocation on aarch64 — is dropped on the strength of a stale observation taken with both locks released in between. Today a Terminated thread cannot legitimately re-acquire a live stack slot, so the window is benign; but the comment at scheduler.rs:3016-3018 presents the two-transaction split as the proof, and nothing in detach_removable_row makes that proof structural.

### Finding 16: kernel/src/process/manager.rs:1291 [lifecycle-hole]

**Summary:** removable_row_candidate silently and permanently skips any row whose main thread has kernel_stack_allocation.is_some() but kernel_stack_top == None, because the `?` inside the find_map closure yields None for that entry.

**Failure scenario:** For a row in that state the closure returns None on every scan, so the row is never proposed to service_one, never passes through detach_removable_row, and stays in ProcessManager.processes forever with its 64 KiB kernel-stack slot and its Process allocation pinned. There is no counter, log or postmortem field that would surface it — dump_reclaim_state reports grave counts, not stuck rows — so the leak is invisible. The row also keeps satisfying child_count/child_pids filters only by virtue of `reaped`, so a stuck row with reaped == false additionally keeps its parent's waitpid from ever returning ECHILD.

### Finding 17: kernel/src/arch_impl/aarch64/context_switch.rs:195 [correctness]

**Summary:** Inserting ExitPending renumbered Terminated from 8 to 9 in trace_thread_state_code while the parallel encoding in try_dump_state uses a different, now-conflicting mapping (ExitPending=8, Terminated=6).

**Failure scenario:** trace_thread_state_code now emits 8 for ExitPending and 9 for Terminated; scheduler.rs:483-489 emits 8 for ExitPending, 7 for BlockedOnIO and 6 for Terminated. Any consumer that decodes a trace buffer captured before this branch (or any tool/doc that hard-codes 8 == Terminated) now reads every ExitPending event as a termination, and anyone who assumes the two dumps share an encoding will mis-read one of them. Neither encoding is documented next to the other and neither commit message mentions the renumbering, so a postmortem taken from a mixed-vintage kernel is silently wrong rather than obviously wrong.

### Finding 18: kernel/src/process/manager.rs:1163 [efficiency]

**Summary:** Every exit unconditionally arms ExitWorkBits::all(), so each retirement needs at least four separate kreclaimd passes plus one pass per open FD, each doing a full O(processes) BTreeMap scan under the DAIF-masked PM lock.

**Failure scenario:** mark_exit_committed sets all four bits regardless of whether the process has children, a parent, graphics windows or open descriptors. service_one then calls, in order, service_one_reparent, claim_one_parent_wake, claim_one_graphics_cleanup and claim_one_exit_fd — each of which re-acquires PROCESS_MANAGER (masking all DAIF) and runs `self.processes.iter().find_map(...)` over the whole table just to discover there is nothing to do, then clears its bit and returns DidWork so kreclaimd loops again. A process with 10 open FDs costs at least 14 PM acquisitions and 14 full table scans with interrupts masked; a fork/exit stress loop multiplies that by the exit rate. The bits could be computed from the row's actual contents at commit time for free, inside the transaction that already has the row in hand.

### Finding 19: tests/teardown_structure.rs:199 [test-design]

**Summary:** The frozen-region gate is six hard-coded FNV-1a hashes with no pointer to the autopsy doc or an update procedure, and nothing in the project's documented test scripts runs `cargo test` for this file.

**Failure scenario:** frozen_regions_match_the_reviewed_gold_masters asserts fnv1a64(region) == 0xeb434b54929bf2bf and five siblings. When someone legitimately edits a gold-master region with owner signoff (the process CLAUDE.md prescribes), the failure message is 'frozen region changed: idle_loop_arm64 body' with no instruction on how to re-derive the constant, so the likely response is to paste in whatever hash the test printed — turning the gate into a rubber stamp. Worse, slice_until/function_source call .expect(...) on find(), so an unrelated refactor that moves a banner comment panics the test rather than reporting a frozen-region change. And CLAUDE.md's Build & Run section documents only docker/qemu/*.sh and xtask boot-stages; if CI never invokes `cargo test -p <root>` on the host triple, none of these seven assertions ever run.

### Finding 20: kernel/src/arch_impl/aarch64/context_switch.rs:3432 [dead-code]

**Summary:** Deleting drain_deferred_fault_sigsegv_exits() from both ERET tails left a stray blank line immediately after each function signature.

**Failure scenario:** check_need_resched_and_switch_arm64 (context_switch.rs:3429-3433) and schedule_from_kernel (:4454-4456) now open with `) {` followed by an empty line before the first statement, a formatting artifact of the removal rather than deliberate spacing. It is cosmetic, but it is the visible residue of a semantic change (the tails no longer drain fault intents) in two of the most safety-sensitive functions in the tree, where a reader scanning for what the tail does is exactly the audience the blank line misleads. `cargo fmt --check` would flag it.

## gate-classify-hardened — classification

### Blocking

0. kernel/src/task/reclaim.rs:479 - kreclaimd parks but the wake path never clears the parked flag, permanently stalling teardown pipeline (lifecycle/liveness hole).
1. kernel/src/arch_impl/aarch64/context_switch.rs:3226 - two dispatcher arms set ThreadState::Terminated directly instead of via set_terminated(), skipping RetirementFence and leaving stale cached_ttbr0, causing permanent leaks of thread rows and process page tables (lifecycle hole).
2. kernel/src/task/reclaim.rs:498 - x86 frees the process page table while CR3 still points at it, with no equivalent of aarch64's leave_process_ttbr0() (use-after-free leading to triple fault).
3. kernel/src/process/mod.rs:71 - x86 ExitReceipt::complete() mints a 'must be preemptible' ReclaimContext and performs teardown/logging/PM-lock work from multiple interrupt-gate contexts with IF cleared (deadlock risk / hot-path & interrupt-handler rule violation).
4. kernel/src/arch_impl/aarch64/exception.rs:317 - EL0 fault handling now identifies the faulting thread via a heuristic (LAST_DISPATCHED_TID) instead of exact TTBR0 lookup, risking killing/reclaiming the wrong process or a kthread (correctness regression).
5. kernel/src/arch_impl/aarch64/exception.rs:298 - EL0 fault arms no longer make the victim thread non-runnable before redirecting to idle, reintroducing a race the deleted code explicitly guarded against (a peer CPU can re-dispatch the faulting thread into a freed address space).
6. kernel/src/interrupts/context_switch.rs:714 - x86 signal-Terminated arm calls switch_to_idle() (which takes SCHEDULER lock and logs) while PROCESS_MANAGER is still held, inside the timer interrupt (deadlock risk).

### Non-blocking

(none)

### Notes

- All seven findings fall into explicitly BLOCKING categories per the gating criteria: lifecycle holes (leak-forever/double-free/missed wakeup), use-after-free, deadlock-risk in interrupt/syscall paths, race conditions, and severe correctness regressions (wrong-thread-killed). None qualify as style/docs/pre-existing-unworsened/hypothetical-cannot-occur/efficiency-only.
