# Invariant Corpus — aarch64 Process Teardown/Reclaim Lifecycle Redesign

Source materials assembled verbatim (facts and quotes only, no design proposals) for a ground-up
redesign of Breenix's aarch64 process exit / TTBR0 quiesce / page-table & kernel-stack reclaim
lifecycle. Compiled 2026-08-02.

Sources:
1. Round-1 review (workflow `wf_62710eaf-a0f`) — 14 findings (`opus-review-all`) + gate classification (`gate-classify`)
2. Round-2 review (workflow `wf_0df56b38-9f5`) — 14 findings (`opus-review-all-2`) + gate classification (`gate-classify-2`)
3. Current branch state: `git log main..fix/teardown-followups` + full `git diff main...fix/teardown-followups`
4. r10 reconciliation: `ttbr0-abort-reconciliation.md` — the original mechanism the r11 fix (first two commits on this branch) addressed
5. Distilled INVARIANTS section

---

## 1.1 Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e) — 14 review findings (`opus-review-all`)

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 1 — `kernel/src/task/process_task.rs:344` — category: `hot-path-violation`

**Summary:** Adding reclaim_deferred_process_resources() to drain_deferred_fault_sigsegv_exits() puts full page-table teardown — including a log::info! inside the aarch64 cleanup_for_exec — onto the aarch64 exception-return path.

**Failure scenario:** drain_deferred_fault_sigsegv_exits() is called at kernel/src/arch_impl/aarch64/context_switch.rs:3439, the top of check_need_resched_and_switch_arm64, which boot.S:669 and syscall_entry.S:244 branch to on every exception return. A process that exec'd at least once exits while a peer CPU still holds its root, so its pending_old_page_tables are queued in PENDING_PROCESS_RECLAIMS. Once the grace elapses, the next timer-tick or syscall return on any CPU runs PendingProcessReclaim::reclaim() -> ProcessPageTable::cleanup_for_exec(), which walks 256 L4 entries, allocates three heap Vecs, frees every table frame, and finishes with log::info! at kernel/src/memory/process_memory.rs:1990. That takes the SERIAL and framebuffer locks on the ERET path: if the interrupted context on that CPU already held SERIAL the CPU self-deadlocks, and even without contention the inline serial write costs milliseconds per exception return, which can starve CPU0 ticks enough to trip the frozen CPU0 regression alarm in timer_interrupt.rs.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 2 — `kernel/src/task/process_task.rs:344` — category: `efficiency`

**Summary:** The reclaim sweep runs unconditionally, so a single global spinlock is now acquired with interrupts disabled on every exception return on every CPU.

**Failure scenario:** Even with the queue empty, every exception return executes arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock() ...) at process_task.rs:352. On an 8-CPU ARM64 system with a 1 kHz timer plus every syscall return, that is tens of thousands of acquisitions per second of one cache line shared across all CPUs. When the queue is non-empty, the lock holder also runs retirement_grace_elapsed (MAX_CPUS atomic loads) and root_is_live (MAX_CPUS remote volatile per-CPU reads) while holding it, blocking every other CPU's exception return behind that scan. The pre-existing caller of this function (sys_fork, syscall_entry.rs:932) ran once per fork with interrupts enabled — a completely different cost profile than the new site.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 3 — `kernel/src/arch_impl/aarch64/context_switch.rs:3439` — category: `correctness`

**Summary:** The drain (and now the reclaim) executes before the PREEMPT_ACTIVE early return, so heavyweight teardown runs in exactly the state the function declares unsafe for scheduling work.

**Failure scenario:** check_need_resched_and_switch_arm64 calls drain_deferred_fault_sigsegv_exits() at line 3439, then reads preempt_count and returns at line 3450 when bit 0x10000000 (PREEMPT_ACTIVE, 'in the middle of returning from a previous exception') is set. A CPU re-entering with PREEMPT_ACTIVE set therefore still performs a page-table walk, frame frees, and heap allocation with a partially restored exception-return state, instead of the intended immediate return.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 4 — `kernel/src/memory/frame_metadata.rs:81` — category: `lock-order`

**Summary:** Reaching frame_decref from the exception-return path introduces a same-CPU self-deadlock on the blocking FRAME_METADATA spinlock.

**Failure scenario:** frame_decref uses FRAME_METADATA.lock() (blocking, not try_lock, unlike deallocate_frame at frame_allocator.rs:338). A kernel thread takes a CoW fault and is inside the frame-metadata critical section with interrupts enabled; the 1 kHz timer fires; the interrupt return path calls check_need_resched_and_switch_arm64 -> drain_deferred_fault_sigsegv_exits -> reclaim_deferred_process_resources -> cleanup_cow_page_table -> frame_decref -> lock() on the mutex the interrupted context on the same CPU still holds. The CPU spins forever.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 5 — `kernel/src/process/manager.rs:1137` — category: `correctness`

**Summary:** The aarch64 exit_process path drops terminate()'s double-terminate guard, so an already-Terminated process gets a second CoW refcount decrement on every user frame.

**Failure scenario:** Process::terminate (process.rs:284) opens with 'if matches!(self.state, Terminated(_)) { return; }' explicitly to prevent double-decrementing CoW refcounts. The replacement runs process.terminate_minimal(exit_code) — which early-returns on an already-terminated process without signalling that to the caller — and then unconditionally runs close_all_fds() and release_or_defer_process_resources(). If exit_process is ever invoked for a process already terminated with its page_table intact (e.g. sys_kill's SIGKILL at syscall/signal.rs:162 or signal/delivery.rs:224, both of which call terminate() and leave page_table in place), cleanup_cow_frames walks the table a second time and frame_decref drops every user frame's refcount again, freeing frames still mapped by a fork sibling. Latent today only because all four aarch64 exception callers pre-check is_terminated() and process::exit_current (process/mod.rs:258) has zero callers; the old code was self-protecting and the new code silently delegates that to every caller.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 6 — `kernel/src/process/manager.rs:1126` — category: `api-contract`

**Summary:** quiesce_ttbr0_for_exit() clobbers the calling CPU's TTBR0 shadows regardless of which pid is being exited, so exit_process(pid) is now only safe when pid is the process this CPU is returning to.

**Failure scenario:** quiesce_ttbr0_for_exit (ttbr0.rs:42) installs the kernel root and zeroes both set_saved_process_cr3 and set_next_cr3 for the current CPU, but exit_process takes an arbitrary pid. If a caller exits a non-current process from CPU1 while CPU1 is on its way back to its own user thread, switch_ttbr0_if_needed (context_switch.rs:4738) sees next_cr3 == 0 and returns immediately without restoring TTBR0, so the innocent thread ERETs to EL0 with the kernel root installed and takes an immediate instruction abort — repeatedly. Nothing in the signature, doc comment, or commit message states this new precondition.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 7 — `kernel/src/arch_impl/aarch64/ttbr0.rs:54` — category: `race-window`

**Summary:** The deferral oracle is_ttbr0_root_live consults only the two software shadows and never TTBR0_EL1, and exception.rs clears both shadows without switching TTBR0, leaving a window where a live root reports as dead.

**Failure scenario:** exception.rs:290-296 sets next_cr3 = kernel_ttbr0 and saved_process_cr3 = 0 but does not touch TTBR0_EL1; the real switch happens later in switch_ttbr0_if_needed. Between those two points CPU1's shadows report 'root not held' while TTBR0_EL1 still names P's root. If CPU0 runs exit_process(P) in that window, defer_live_process_resources (process_task.rs:107) sees root_is_live == false, takes the release_process_resources branch, and returns every user frame to the allocator; CPU1's page-table walker or a TLB refill can still walk P's root and touch reallocated memory. Commit 54dc3a99 newly routes the fault-exit path through this oracle, so the window is reachable from a caller that previously did not use it. Additionally switch_ttbr0_if_needed (context_switch.rs:4762) skips set_saved_process_cr3 when current_ttbr0 == next_ttbr0 while still zeroing next_cr3, which can leave both shadows zero with the root installed.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 8 — `kernel/src/task/scheduler.rs:994` — category: `memory-ordering`

**Summary:** The epoch-before-stack-liveness reorder is correct but its entire guarantee rests on the Acquire loads inside retirement_grace_elapsed, an unstated and unenforced dependency.

**Failure scenario:** is_kernel_stack_slot_live (kernel_stack.rs:283) reads per-CPU state with plain core::ptr::read_volatile and no acquire semantics, so the only thing preventing the CPU from hoisting the stack-liveness read above the epoch read is the Ordering::Acquire load at scheduler.rs:565. retirement_grace_elapsed short-circuits on 'target[cpu_id] == 0' and executes zero atomic loads when every entry is zero — which happens if retirement_grace_target ran with no CPU reported online. In that case grace_elapsed is true with no barrier at all and the two observations can be reordered back, letting a stale 'stack not live' pair with 'grace elapsed' and free a kernel stack that a peer is still unwinding. Unreachable today because is_cpu_online(0) holds post-boot, but nothing documents or asserts it.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 9 — `kernel/src/task/process_task.rs:142` — category: `correctness`

**Summary:** In the deferred branch the process's GuardedStack is dropped immediately even though the deferral exists precisely because a peer CPU still has that address space installed.

**Failure scenario:** release_or_defer_process_resources defers the page table when the root is live but unconditionally runs drop(process.stack.take()) at line 142. Today GuardedStack::drop (memory/stack.rs:264) is a stub that only emits log::debug! and never unmaps or frees, so this is a leak rather than a use-after-free — but that stub log runs under the PM lock with interrupts disabled on aarch64, which is exactly the PM -> SERIAL -> framebuffer ordering that the surrounding comments in process.rs and process_task.rs forbid. If GuardedStack::drop is ever completed, this becomes a live UAF: CPU0 frees P's user stack frames while CPU1 still holds P's root in TTBR0_EL1. Commit 54dc3a99 extends this pattern from handle_thread_exit to the fault-exit path.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 10 — `kernel/src/process/manager.rs:1138` — category: `lock-order`

**Summary:** exit_process closes file descriptors inline under the PM lock instead of using the take_fd_entries + close_extracted_fds two-phase pattern that handle_thread_exit documents as mandatory on ARM64 SMP.

**Failure scenario:** process.close_all_fds() runs with the PM lock held and interrupts disabled (ProcessManagerGuard disables DAIF on aarch64). A dying process holding a pipe write end triggers buffer.lock().close_write(), which wakes a blocked reader and reaches the scheduler; a PtyMaster fd reaches crate::tty::pty::release; a TcpConnection reaches tcp_close. The handle_thread_exit doc comment (process_task.rs:206-222) states this combination 'creates an unbreakable deadlock' on ARM64 SMP. Behaviourally identical to the old terminate() call so it is not a regression, but the commit rewrote this block and re-hardcoded the form its own sibling path exists to avoid.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 11 — `kernel/src/process/manager.rs:1126` — category: `efficiency`

**Summary:** Each aarch64 fault exit now performs the TTBR0 quiesce three times, one of them a broadcast TLB invalidate issued with the PM lock held and interrupts disabled.

**Failure scenario:** exception.rs:755, 1116, 1207 and 1303 already call switch_ttbr0_to_kernel() before acquiring the PM lock, exit_process now calls quiesce_ttbr0_for_exit() at line 1126 inside it, and defer_current_user_thread_sigsegv_exit calls it again at exception.rs:326. Each invocation issues 'dsb ishst; msr ttbr0_el1; isb; tlbi vmalle1is; dsb ish; isb' — two inner-shareable DSB waits plus a broadcast TLB invalidate. Only the middle one is inside the PM lock with interrupts disabled, extending the window during which every other CPU blocks on the PM lock and cannot take interrupts. Correct but redundant; only the shadow-clearing part of quiesce is actually new behaviour at this site.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 12 — `kernel/src/task/process_task.rs:110` — category: `observability`

**Summary:** PENDING_PROCESS_RECLAIMS is unbounded and silent, so a root that never quiesces pins an entire address space with no diagnostic.

**Failure scenario:** If a CPU parks with a stale TTBR0 shadow naming a retired root, or goes offline while is_cpu_online still reports true, root_is_live() stays true forever and the entry is never swap_removed by reclaim_deferred_process_resources. There is no cap on the Vec, no age counter, and no log or trace point, so the leaked page table, its old exec tables, and every user frame they reference are pinned indefinitely and the only symptom is eventual frame-allocator exhaustion with no attribution.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 13 — `kernel/src/arch_impl/aarch64/context_switch.rs:1663` — category: `correctness`

**Summary:** The decode_last_dispatched fix is correct: the ERET_ANOMALY dumper was printing the raw encoded word as the owner tid.

**Failure scenario:** Before this change dump_all_eret_frame_anomaly_snapshots printed LAST_DISPATCHED_TID[cpu].load() directly, but stamp_last_dispatched_tid (line 1547) stores (tid << 9) | slot_code. Every [ERET_ANOMALY] line therefore reported an owner tid 512x the real value plus a stack-slot code, so any investigator comparing it against tid= on the same line saw a spurious mismatch. The fix matches the three other consumers at lines 1558, 1570 and 1578, is confined to a postmortem dumper reached only from exception.rs:243 and 1514 (both fatal paths), and touches no gold-master region — the markers in this file sit at lines 721, 3262 and 4902.

### [Round 1 (fix/teardown-followups pre-r13, i.e. review of the ORIGINAL 4 commits: 5781442b/867ce0c6/54dc3a99/fd261b0e)] Finding 14 — `kernel/src/task/process_task.rs:344` — category: `commit-message-honesty`

**Summary:** Commit messages understate the blast radius of two of the four changes.

**Failure scenario:** fd261b0e says 'drain deferred process reclaims during scheduling', but drain_deferred_fault_sigsegv_exits is invoked from check_need_resched_and_switch_arm64 (every aarch64 exception return, via boot.S:669 and syscall_entry.S:244) as well as schedule_from_kernel — a reviewer reading only the message would not expect page-table teardown on the ERET path. 54dc3a99 says it 'shares the handle_thread_exit defer-or-release machinery' but does not mention that swapping terminate() for terminate_minimal + close_all_fds silently removes the double-terminate guard that terminate() carried. The other two messages (867ce0c6, 5781442b) accurately describe their changes, and all four carry the required 'Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>' trailer.


---

## 2.1 Round 1 — gate classification (`gate-classify`)

**Blocking (5):**

- Adding reclaim_deferred_process_resources() to drain_deferred_fault_sigsegv_exits() puts full page-table teardown — including a log::info! inside the aarch64 cleanup_for_exec — onto the aarch64 exception-return path.
- The drain (and now the reclaim) executes before the PREEMPT_ACTIVE early return, so heavyweight teardown runs in exactly the state the function declares unsafe for scheduling work.
- Reaching frame_decref from the exception-return path introduces a same-CPU self-deadlock on the blocking FRAME_METADATA spinlock.
- quiesce_ttbr0_for_exit() clobbers the calling CPU's TTBR0 shadows regardless of which pid is being exited, so exit_process(pid) is now only safe when pid is the process this CPU is returning to.
- The deferral oracle is_ttbr0_root_live consults only the two software shadows and never TTBR0_EL1, and exception.rs clears both shadows without switching TTBR0, leaving a window where a live root reports as dead.

**Non-blocking (3):**

- The reclaim sweep runs unconditionally, so a single global spinlock is now acquired with interrupts disabled on every exception return on every CPU.
- The aarch64 exit_process path drops terminate()'s double-terminate guard, so an already-Terminated process gets a second CoW refcount decrement on every user frame.
- The epoch-before-stack-liveness reorder is correct but its entire guarantee rests on the Acquire loads inside retirement_grace_elapsed, an unstated and unenforced dependency.

**Notes:**

- Classification #2 treated as efficiency/contention rather than a deadlock (no cyclic lock dependency shown), so non-blocking.
- Classification #5 (double-terminate guard) is non-blocking per reviewer's own statement that all current callers pre-check is_terminated() and the newly-exposed caller path has zero current callers — a hypothetical requiring conditions that don't currently occur.
- Classification #8 is non-blocking because the reviewer states the reorder is correct today; the concern is an unenforced invariant/documentation gap, not a demonstrated bug on the changed path.

---

## 1.2 Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c) — 14 review findings (`opus-review-all-2`)

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 1 — `kernel/src/arch_impl/aarch64/context_switch.rs:2969` — category: `correctness`

**Summary:** The comment justifying removal of set_saved_process_cr3(0) is wrong — switch_ttbr0_if_needed() is never reached on the idle-redirect path, so an idle CPU keeps the retired root in its saved shadow indefinitely and blocks reclaim forever.

**Failure scenario:** Two CPUs. Process P's thread runs on CPU 1, is preempted, and CPU 1 is redirected to idle via setup_idle_return_locked (next_cr3=kernel_ttbr0, saved_process_cr3 left = P_root). switch_ttbr0_if_needed is only called from the three user-dispatch sites (2229/3153/3363) and the boot.S IRQ-return tail never touches TTBR0, so nothing ever replaces the shadow. P then exits on CPU 0 and is enqueued. Every 10 ms the reclaim worker calls PendingProcessReclaim::root_is_live(), is_ttbr0_root_live(P_root) matches CPU 1's stale saved shadow, and P's whole address space is never freed while the system idles — PENDING_PROCESS_RECLAIM_BLOCKED_SWEEPS climbs forever. (The removal does close a real window in syscall_entry.S where `str xzr,[x0,#64]` clears next_cr3 before `msr ttbr0_el1` — the direction is right; the missing piece is any code that publishes kernel_ttbr0 into the saved shadow on the idle path.)

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 2 — `kernel/src/arch_impl/aarch64/exception.rs:294` — category: `correctness`

**Summary:** Same removal in set_idle_stack_for_eret is justified by "until the ERET dispatcher has installed kernel_ttbr0 and replaced the saved shadow" — aarch64_enter_exception_frame never touches TTBR0 at all.

**Failure scenario:** A synchronous fault kills a process on CPU A; the handler redirects the frame to idle_loop_arm64 via set_idle_stack_for_eret. The dispatch ERET path (context_switch.rs:690-760) contains no TTBR0 access, and boot.S's exception return contains none either (its only ttbr0 references are boot-time page-table construction at lines 288-376). The CPU therefore ERETs to idle with hardware TTBR0_EL1 still holding the terminated process root, next_cr3 stuck at kernel_ttbr0 forever, and saved_process_cr3 stale — the exact opposite of what the comment asserts.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 3 — `kernel/src/task/process_task.rs:151` — category: `correctness`

**Summary:** enqueue_process_reclaim panics on queue overflow while holding the PM lock and the PENDING lock with interrupts disabled, so the panic will most likely hang instead of reporting.

**Failure scenario:** One online CPU stops entering check_need_resched_and_switch_arm64 (the CPU0 timer-death class this project has fought). note_scheduling_epoch is called from exactly one site (context_switch.rs:3457), so SCHEDULING_EPOCHS for that CPU freezes, retirement_grace_elapsed never returns true for any entry, and every process exit accumulates. On the 257th exit, panic! fires from inside arch_without_interrupts with PROCESS_MANAGER held (via with_process_manager in the fault handlers). The aarch64 panic handler (main_aarch64.rs:1768) immediately calls serial_println! and trace_dump_counters(), both lock-taking, with all interrupts masked — the kernel dies silently instead of printing the diagnostic. The previous code released such roots inline when no CPU retained them, so a wedged CPU could not cause this.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 4 — `kernel/src/task/process_task.rs:147` — category: `lock-order`

**Summary:** Every aarch64 process exit now takes the PENDING lock and does a potentially heap-allocating Vec::push inside arch_without_interrupts with the PM lock held, and this is now reachable from all four synchronous-fault handlers.

**Failure scenario:** defer_live_process_resources (which released resources inline whenever no CPU retained the root) was replaced by defer_process_resources, which enqueues unconditionally. exit_process now calls it at manager.rs:1171 from inside with_process_manager — interrupts disabled, PROCESS_MANAGER held. pending.push() can trigger a Vec realloc and hence the heap allocator lock. This is the precise anti-pattern documented at syscall_entry.rs:900-906 as the root cause of the intermittent 1-in-5 boot hang: if any thread was preempted holding the heap lock, a single-CPU ARM64 system deadlocks permanently because the lock holder can never be scheduled.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 5 — `kernel/src/task/process_task.rs:249` — category: `correctness`

**Summary:** handle_thread_exit takes process.children before the new terminate_minimal early return, so on the already-terminated path the children vector is emptied and dropped without reparenting to init.

**Failure scenario:** PID 1 exits. exit_process's reparent block is guarded by `if pid != init_pid` (manager.rs:1198), so init's children list is left intact and non-empty. The subsequent thread exit reaches handle_thread_exit, `core::mem::take(&mut process.children)` moves the list out at line 249, `terminate_minimal` returns false at line 252, and the function returns at 253 — the taken Vec is dropped. Those children keep `parent = Some(1)` pointing at a dead process, are never added to init.children, and can never be reaped. The guard is correct in intent (it prevents a genuine double cleanup_cow_frames double-decrement) but is placed one statement too late.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 6 — `kernel/src/task/process_task.rs:253` — category: `correctness`

**Summary:** The same early return skips phase 2, which contains the only code in the tree that wakes a parent blocked in waitpid().

**Failure scenario:** A parent calls waitpid() and blocks. Its child faults and is killed by handle_sync_exception -> pm.exit_process(pid, -11), which sets SIGCHLD pending on the parent (manager.rs:1230) but performs no scheduler wakeup. The child's queued thread exit later reaches handle_thread_exit, hits the already-terminated guard, and returns at line 253 — so sched.unblock_for_child_exit(parent_tid) / unblock_for_signal(parent_tid) at lines 311-312 (the only call site for child-exit wakeup outside tty/pty and syscall/signal) never run. The parent sleeps until an unrelated wake event. Before this branch terminate_minimal returned unit and phase 2 always ran.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 7 — `kernel/src/process/manager.rs:1140` — category: `correctness`

**Summary:** The new pid-ownership gate keys on process.main_thread.id, so any exit driven by a non-main thread skips the quiesce and leaves the CPU pinning the retired root — and the same file's other quiesce caller applies no ownership check at all.

**Failure scenario:** A multithreaded process (the tree has thread groups: futex_wake_for_thread_group, has_userspace_threads_other_than) faults on a non-main thread T. find_process_by_cr3_mut resolves the process, but last_dispatched_tid(cpu) == T.id != main_thread.id, so local_cpu_runs_process is false, local_cpu_retains_root is false, and quiesce_ttbr0_for_exit is skipped. Combined with the removal of set_saved_process_cr3(0) from set_idle_stack_for_eret, the CPU's saved shadow keeps the retired root and blocks reclaim of that address space indefinitely. Separately, defer_current_user_thread_sigsegv_exit (exception.rs:327) calls quiesce_ttbr0_for_exit() unconditionally with no ownership check whatsoever — because quiesce switches hardware to kernel_ttbr0 before zeroing the shadows, the "authority" the commit message argues for is not actually required for local safety, so the invariant is neither uniform nor load-bearing.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 8 — `kernel/src/process/manager.rs:1153` — category: `hot-path`

**Summary:** quiesce_ttbr0_for_exit issues a broadcast TLB invalidate (tlbi vmalle1is; dsb ish; isb) from inside exit_process, i.e. under the PM lock with interrupts disabled on the synchronous-fault handler path.

**Failure scenario:** handle_sync_exception -> with_process_manager (aarch64 variant disables interrupts and takes PROCESS_MANAGER, process/mod.rs:167-175) -> exit_process -> quiesce_ttbr0_for_exit -> switch_ttbr0_to_kernel, which executes an inner-shareable broadcast TLBI plus two dsb/isb barriers. On a contended SMP system the broadcast completion wait extends the PM-lock hold and the interrupts-off window in the fault handler. Not on the ERET tail, but it is new work added to the exception path under a blocking lock.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 9 — `kernel/src/task/process_task.rs:107` — category: `commit-honesty`

**Summary:** The three new reclaim counters are written but never read anywhere in the tree, contradicting the "exposed for postmortem diagnostics" doc comment and the commit message's "expose depth, blocked-sweep, and capacity-failure counters".

**Failure scenario:** grep across the whole repo for PENDING_PROCESS_RECLAIM_DEPTH / _BLOCKED_SWEEPS / _CAPACITY_FAILURES returns only the definitions (107/111/115) and the three write sites (150/154/378/381). No postmortem dumper, /proc handler, or GDB helper reads them, so when the queue stalls or the capacity panic fires there is no way to observe the counters short of attaching GDB and knowing the symbol names. They are pub statics, so no dead_code warning flags this.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 10 — `kernel/src/task/process_task.rs:407` — category: `correctness`

**Summary:** The reclaim worker's sleep is a fixed 10 ms poll with no wake-on-enqueue and no handling for with_scheduler returning None, and block_current_for_timer pushes a timer_heap entry unconditionally.

**Failure scenario:** If with_scheduler returns None (SCHEDULER uninitialized) the thread is never marked BlockedOnTimer, and if schedule_from_kernel takes its early-return path (lock_for_context_switch yields None, context_switch.rs:4470-4476) it returns without switching — the loop then spins through drain/reclaim at full CPU. Each iteration also does self.timer_heap.push(Reverse((wake_time_ns, current_id))) with no dedup (scheduler.rs:2164), so repeated blocks for the same tid accumulate stale heap entries. Separately, with MAX_PENDING_PROCESS_RECLAIMS = 256 and a fixed 10 ms drain, a burst of process exits has no backpressure path other than the panic at line 151.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 11 — `kernel/src/arch_impl/aarch64/exception.rs:772` — category: `correctness`

**Summary:** `terminated` is now conditional on exit_process returning Some, so a None return makes the fault handler fall through without terminating the scheduler thread or redirecting the frame to idle.

**Failure scenario:** exit_process returns None on three paths (process row missing, already terminated, terminate_minimal false). Today the preceding is_terminated() check runs under the same PM lock so None cannot occur, but the coupling is implicit: if exit_process ever gains another None path, `terminated` and `already_terminated` both stay false, the `if terminated || already_terminated` block at line 783 is skipped, terminate_current_scheduler_thread() and the idle redirect never run, and the handler ERETs back into the faulting userspace context — an infinite fault loop. The old code set `terminated = true` unconditionally after the call.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 12 — `kernel/src/task/process_task.rs:138` — category: `correctness`

**Summary:** The early drop of process.stack is guarded only by a prose comment asserting GuardedStack::drop is a no-op, and the same branch edits that Drop impl.

**Failure scenario:** defer_process_resources drops the GuardedStack immediately while the page-table root is still deferred and possibly retained by a peer CPU's TTBR0. Correctness rests entirely on memory/stack.rs:264-268 remaining an empty TODO stub — which commit 28a7933e itself touched (removing the log::debug). The moment anyone implements the TODO, a peer CPU still running on the retired root can observe unmapped/freed stack frames, and nothing in the build enforces the coupling (no assertion, no test, no cfg gate).

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 13 — `kernel/src/process/mod.rs:263` — category: `maintainability`

**Summary:** exit_current has no callers in the tree, and the rewrite silently drops its "Process manager not available!" error log by collapsing that case into None.

**Failure scenario:** grep -rn "exit_current(" kernel/src returns only the definition at line 258 (it carries #[allow(dead_code)]). The new `manager_guard.as_mut().and_then(...)` folds "manager is None" and "exit_process declined" into the same None, so the previous log::error! branch is gone. If a caller is ever added and the manager is uninitialized, the exit silently no-ops with no diagnostic.

### [Round 2 (review of the REDESIGNED branch after Codex's r13 pass replaced 54dc3a99/fd261b0e with c0be17e7/28a7933e/5d7ab37c)] Finding 14 — `kernel/src/memory/process_memory.rs:1849` — category: `commit-honesty`

**Summary:** Only the aarch64 copy of cleanup_for_exec lost its counters and log::info!; the x86_64 copy keeps both, so the two arches now report differently for the same operation.

**Failure scenario:** cleanup_for_exec exists twice — the x86_64 version around line 1780-1855 (log::info! "cleanup_for_exec: freed {} user frames..." retained at 1849) and the #[cfg(target_arch = "aarch64")] version at 1861 (counters and log removed). Anyone debugging exec/reclaim frame accounting on aarch64 now has no output and no counters, while the same code path on x86 still logs; the commit message describes this as removing logging from "AArch64 page-table teardown" without noting the arch divergence.


---

## 2.2 Round 2 — gate classification (`gate-classify-2`)

**Blocking (4):**

- Panic on process-reclaim queue overflow fires from inside arch_without_interrupts while holding PROCESS_MANAGER and PENDING locks; the aarch64 panic handler's serial_println!/trace_dump_counters() calls will deadlock instead of reporting, silently killing the kernel on the 257th deferred exit when a CPU stalls scheduling.
- defer_process_resources now unconditionally enqueues via a heap-allocating Vec::push inside arch_without_interrupts with PROCESS_MANAGER held, reachable from all four synchronous-fault handlers — this reproduces the documented syscall_entry.rs:900-906 heap-lock deadlock anti-pattern on a single-CPU ARM64 system.
- The new pid-ownership gate in quiesce_ttbr0_for_exit keys on main_thread.id, so exits driven by non-main threads skip the quiesce entirely (leaving the CPU pinning a retired TTBR0 root indefinitely), while the sibling call site (defer_current_user_thread_sigsegv_exit) applies no ownership check at all — an inconsistent, unenforced invariant on the changed exit path.
- quiesce_ttbr0_for_exit issues a broadcast TLB invalidate (tlbi vmalle1is + dsb ish + isb) from inside exit_process while PROCESS_MANAGER is held and interrupts are disabled on the synchronous-fault handler path, extending an IRQ-off lock hold with cross-CPU synchronization — a deadlock/priority-inversion risk introduced on the changed path.

**Non-blocking (3):**

- The comment justifying removal of set_saved_process_cr3(0) in context_switch.rs claims switch_ttbr0_if_needed will replace the saved shadow, but that function is never reached on the idle-redirect path, so the stated rationale is incorrect (resource-leak consequence, not a race/deadlock/lock/atomic issue on the exception-return tail itself).
- The equivalent comment in exception.rs's set_idle_stack_for_eret asserts the ERET dispatcher touches TTBR0 before idle runs, but neither the dispatch path nor boot.S's exception return does so — same incorrect-rationale issue as the context_switch.rs finding, not itself a lock/deadlock/race defect.
- handle_thread_exit takes process.children before the new terminate_minimal early-return check, so on the already-terminated path the children vector is dropped without reparenting to init and the phase-2 waitpid() wakeup is skipped — a logic/ordering bug rather than a race, deadlock, frozen-region, atomic-ordering, or dishonest-message issue.

**Notes:**

- Findings 1 and 2 (and to a lesser extent 5) describe genuinely severe correctness regressions (permanent address-space leak, orphaned unreapable children) introduced by the diff, but per the stated gating rubric they don't fall into the enumerated BLOCKING categories (no frozen-region touch, no lock/teardown-on-exception-tail, no race/deadlock, no atomic-ordering bug, no dishonest commit message) — flagging them here so the human merge-gate owner can decide whether severity alone should override the category-based gate.
- Findings 1, 2, and 6 are causally linked (removal of set_saved_process_cr3(0) + the main-thread-only ownership gate combine to make TTBR0-root leaks the systemic failure mode) — worth reviewing together rather than as independent fixes.

---

## 3. Current branch state

```
$ git -C /Users/wrb/fun/code/breenix log --oneline main..fix/teardown-followups
5d7ab37c fix(aarch64): require pid ownership before TTBR0 quiesce
28a7933e fix(aarch64): move process retirement off ERET paths
c0be17e7 fix(aarch64): make fault-exit retirement owner-safe
867ce0c6 fix arm64 reclaim observation order
5781442b fix arm64 last-dispatch owner decoding
```

### Full diff: `git diff main...fix/teardown-followups` (the artifact under redesign)

```diff
diff --git a/kernel/src/arch_impl/aarch64/context_switch.rs b/kernel/src/arch_impl/aarch64/context_switch.rs
index b9f083ca..999b3620 100644
--- a/kernel/src/arch_impl/aarch64/context_switch.rs
+++ b/kernel/src/arch_impl/aarch64/context_switch.rs
@@ -1660,7 +1660,8 @@ pub fn dump_all_eret_frame_anomaly_snapshots() {
     for cpu_id in 0..crate::arch_impl::aarch64::constants::MAX_CPUS {
         if let Some((tid, frame_elr, ctx_elr, x26, spsr)) = eret_frame_anomaly_snapshot(cpu_id) {
             any_recorded = true;
-            let owner_tid = LAST_DISPATCHED_TID[cpu_id].load(Ordering::Acquire);
+            let (owner_tid, _) =
+                decode_last_dispatched(LAST_DISPATCHED_TID[cpu_id].load(Ordering::Acquire));
             raw_uart_str("[ERET_ANOMALY] cpu=");
             raw_uart_dec(cpu_id as u64);
             raw_uart_str(" tid=");
@@ -2965,7 +2966,8 @@ fn setup_idle_return_locked(
             kernel_ttbr0 = 0x4200_0000;
         }
         Aarch64PerCpu::set_next_cr3(kernel_ttbr0);
-        Aarch64PerCpu::set_saved_process_cr3(0);
+        // Keep the old root visible until switch_ttbr0_if_needed() has changed
+        // TTBR0_EL1 and published the kernel root into the saved shadow.
         Aarch64PerCpu::set_current_thread_ptr(core::ptr::null_mut());
         Aarch64PerCpu::clear_preempt_active();
     }
@@ -3435,8 +3437,6 @@ pub extern "C" fn check_need_resched_and_switch_arm64(
     frame: &mut Aarch64ExceptionFrame,
     from_el0: bool,
 ) {
-    crate::task::process_task::drain_deferred_fault_sigsegv_exits();
-
     // ── Lock-free pre-checks ──────────────────────────────────────
     let preempt_count = Aarch64PerCpu::preempt_count();
     let cpu_id_early = Aarch64PerCpu::cpu_id() as usize;
@@ -4456,8 +4456,6 @@ fn cpu0_breadcrumb(cpu_id: usize, id: u64) {
 }
 
 pub fn schedule_from_kernel() {
-    crate::task::process_task::drain_deferred_fault_sigsegv_exits();
-
     let saved_daif = read_daif();
     let cpu_id = Aarch64PerCpu::cpu_id() as usize;
     cpu0_breadcrumb(cpu_id, 1); // entry
@@ -4759,13 +4757,12 @@ fn switch_ttbr0_if_needed(_thread_id: u64) {
                 options(nomem, nostack)
             );
         }
-
-        unsafe {
-            Aarch64PerCpu::set_saved_process_cr3(next_ttbr0);
-        }
     }
 
     unsafe {
+        // Publish the hardware value before clearing the pending shadow. This
+        // also covers the current_ttbr0 == next_ttbr0 case.
+        Aarch64PerCpu::set_saved_process_cr3(next_ttbr0);
         Aarch64PerCpu::set_next_cr3(0);
     }
 }
diff --git a/kernel/src/arch_impl/aarch64/exception.rs b/kernel/src/arch_impl/aarch64/exception.rs
index 8d04c7a8..73d41ffc 100644
--- a/kernel/src/arch_impl/aarch64/exception.rs
+++ b/kernel/src/arch_impl/aarch64/exception.rs
@@ -291,7 +291,8 @@ fn set_idle_stack_for_eret() {
         Aarch64PerCpu::set_kernel_stack_top(idle_stack);
         let kernel_ttbr0 = super::kernel_ttbr0();
         Aarch64PerCpu::set_next_cr3(kernel_ttbr0);
-        Aarch64PerCpu::set_saved_process_cr3(0);
+        // Keep the prior root visible to teardown until the ERET dispatcher has
+        // installed kernel_ttbr0 and replaced the saved shadow.
         Aarch64PerCpu::set_current_thread_ptr(core::ptr::null_mut());
         Aarch64PerCpu::clear_preempt_active();
     }
@@ -757,6 +758,7 @@ pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr:
                 // Find and terminate the process
                 let mut terminated = false;
                 let mut already_terminated = false;
+                let mut exit_cleanup = None;
                 crate::process::with_process_manager(|pm| {
                     if let Some((pid, _process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                         if _process.is_terminated() {
@@ -767,12 +769,17 @@ pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr:
                             pid.as_u64() as u16,
                             (-11i16) as u16,
                         );
-                        pm.exit_process(pid, -11); // SIGSEGV exit code
-                        terminated = true;
+                        if let Some(entries) = pm.exit_process(pid, -11) {
+                            exit_cleanup = Some((pid, entries));
+                            terminated = true;
+                        }
                     } else {
                         // trace_data_abort already captured the fault
                     }
                 });
+                if let Some((pid, entries)) = exit_cleanup {
+                    crate::task::process_task::finish_extracted_process_exit(pid, entries);
+                }
 
                 if terminated || already_terminated {
                     // CRITICAL: Mark the scheduler's thread as Terminated BEFORE
@@ -1118,6 +1125,7 @@ pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr:
                 let mut terminated = false;
                 let mut already_terminated = false;
                 let mut killed_pid: u64 = 0;
+                let mut exit_cleanup = None;
                 crate::process::with_process_manager(|pm| {
                     if let Some((pid, _process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                         if _process.is_terminated() {
@@ -1129,10 +1137,15 @@ pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr:
                             pid.as_u64() as u16,
                             (-11i16) as u16,
                         );
-                        pm.exit_process(pid, -11); // SIGSEGV
-                        terminated = true;
+                        if let Some(entries) = pm.exit_process(pid, -11) {
+                            exit_cleanup = Some((pid, entries));
+                            terminated = true;
+                        }
                     }
                 });
+                if let Some((pid, entries)) = exit_cleanup {
+                    crate::task::process_task::finish_extracted_process_exit(pid, entries);
+                }
                 // Lock-free diagnostic AFTER releasing process manager lock
                 if terminated {
                     use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};
@@ -1205,13 +1218,19 @@ pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr:
                 }
                 let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;
                 super::switch_ttbr0_to_kernel();
+                let mut exit_cleanup = None;
                 crate::process::with_process_manager(|pm| {
                     if let Some((pid, process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                         if !process.is_terminated() {
-                            pm.exit_process(pid, -11);
+                            if let Some(entries) = pm.exit_process(pid, -11) {
+                                exit_cleanup = Some((pid, entries));
+                            }
                         }
                     }
                 });
+                if let Some((pid, entries)) = exit_cleanup {
+                    crate::task::process_task::finish_extracted_process_exit(pid, entries);
+                }
                 terminate_current_scheduler_thread();
             }
             // CRITICAL: Set frame values BEFORE switch_to_idle_best_effort()
@@ -1302,13 +1321,19 @@ pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr:
                 }
                 let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;
                 super::switch_ttbr0_to_kernel();
+                let mut exit_cleanup = None;
                 crate::process::with_process_manager(|pm| {
                     if let Some((pid, process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                         if !process.is_terminated() {
-                            pm.exit_process(pid, -11);
+                            if let Some(entries) = pm.exit_process(pid, -11) {
+                                exit_cleanup = Some((pid, entries));
+                            }
                         }
                     }
                 });
+                if let Some((pid, entries)) = exit_cleanup {
+                    crate::task::process_task::finish_extracted_process_exit(pid, entries);
+                }
                 terminate_current_scheduler_thread();
             }
             // CRITICAL: Set frame values BEFORE switch_to_idle_best_effort()
diff --git a/kernel/src/arch_impl/aarch64/mod.rs b/kernel/src/arch_impl/aarch64/mod.rs
index f0b36acd..777f8675 100644
--- a/kernel/src/arch_impl/aarch64/mod.rs
+++ b/kernel/src/arch_impl/aarch64/mod.rs
@@ -55,7 +55,8 @@ pub use syscall_entry::{is_el0_confirmed, syscall_return_to_userspace_aarch64};
 #[allow(unused_imports)]
 pub use timer::Aarch64Timer;
 pub use ttbr0::{
-    is_ttbr0_root_live, kernel_ttbr0, quiesce_ttbr0_for_exit, switch_ttbr0_to_kernel,
+    current_cpu_retains_ttbr0_root, is_ttbr0_root_live, kernel_ttbr0,
+    quiesce_ttbr0_for_exit, switch_ttbr0_to_kernel,
 };
 
 // Re-export interrupt control functions for convenient access
diff --git a/kernel/src/arch_impl/aarch64/ttbr0.rs b/kernel/src/arch_impl/aarch64/ttbr0.rs
index 12435289..ea7ca49f 100644
--- a/kernel/src/arch_impl/aarch64/ttbr0.rs
+++ b/kernel/src/arch_impl/aarch64/ttbr0.rs
@@ -2,6 +2,15 @@
 
 const TTBR0_ROOT_MASK: u64 = !0xFFFF_0000_0000_0FFF;
 
+#[inline(always)]
+fn read_ttbr0_el1() -> u64 {
+    let ttbr0: u64;
+    unsafe {
+        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
+    }
+    ttbr0
+}
+
 /// Return the kernel TTBR0 root, falling back to the boot identity table before
 /// per-CPU state has been populated.
 #[inline(always)]
@@ -40,13 +49,31 @@ pub fn switch_ttbr0_to_kernel() {
 /// reinstalling it. This must complete before publishing deferred exit work.
 #[inline(always)]
 pub fn quiesce_ttbr0_for_exit() {
-    switch_ttbr0_to_kernel();
+    if read_ttbr0_el1() != kernel_ttbr0() {
+        switch_ttbr0_to_kernel();
+    }
     unsafe {
         super::percpu::Aarch64PerCpu::set_saved_process_cr3(0);
         super::percpu::Aarch64PerCpu::set_next_cr3(0);
     }
 }
 
+/// Return whether this CPU's hardware TTBR0 or either return shadow retains
+/// `root_phys`. Process exit uses this local ownership check before clearing
+/// the CPU's return state; exiting an unrelated PID must not clobber it.
+pub fn current_cpu_retains_ttbr0_root(root_phys: u64) -> bool {
+    let root_phys = root_phys & TTBR0_ROOT_MASK;
+    if root_phys == 0 {
+        return false;
+    }
+
+    let saved_process_ttbr0 = super::percpu::Aarch64PerCpu::saved_process_cr3();
+    let next_ttbr0 = super::percpu::Aarch64PerCpu::next_cr3();
+    read_ttbr0_el1() & TTBR0_ROOT_MASK == root_phys
+        || saved_process_ttbr0 & TTBR0_ROOT_MASK == root_phys
+        || next_ttbr0 & TTBR0_ROOT_MASK == root_phys
+}
+
 /// Return whether any online CPU still retains `root_phys` in a TTBR0 shadow.
 ///
 /// TTBR0 values may carry an ASID, so compare only the physical root bits using
@@ -57,6 +84,12 @@ pub fn is_ttbr0_root_live(root_phys: u64) -> bool {
         return false;
     }
 
+    // The local register is directly observable. Remote CPUs are represented
+    // by shadows that are kept live until their hardware switch completes.
+    if read_ttbr0_el1() & TTBR0_ROOT_MASK == root_phys {
+        return true;
+    }
+
     (0..super::constants::MAX_CPUS).any(|cpu_id| {
         if !super::smp::is_cpu_online(cpu_id) {
             return false;
diff --git a/kernel/src/interrupts.rs b/kernel/src/interrupts.rs
index 21910914..e1c0bee0 100644
--- a/kernel/src/interrupts.rs
+++ b/kernel/src/interrupts.rs
@@ -1426,7 +1426,7 @@ extern "x86-interrupt" fn page_fault_handler(
                         pid.as_u64(),
                         cr3
                     );
-                    pm.exit_process(pid, -11); // SIGSEGV exit code
+                    let _ = pm.exit_process(pid, -11); // SIGSEGV exit code
                 } else {
                     log::error!(
                         "Could not find process with CR3={:#x} - cannot terminate",
@@ -1732,7 +1732,7 @@ extern "x86-interrupt" fn general_protection_fault_handler(
                     pid.as_u64(),
                     cr3
                 );
-                pm.exit_process(pid, -11); // SIGSEGV exit code
+                let _ = pm.exit_process(pid, -11); // SIGSEGV exit code
             } else {
                 log::error!(
                     "Could not find process with CR3={:#x} - cannot terminate",
diff --git a/kernel/src/main_aarch64.rs b/kernel/src/main_aarch64.rs
index e4632214..0e23db51 100644
--- a/kernel/src/main_aarch64.rs
+++ b/kernel/src/main_aarch64.rs
@@ -806,6 +806,9 @@ pub extern "C" fn kernel_main(hw_config_ptr: u64) -> ! {
     #[cfg(feature = "btrt")]
     kernel::test_framework::btrt::pass(kernel::test_framework::catalog::WORKQUEUE_INIT);
 
+    kernel::task::process_task::init_process_reclaim_worker()
+        .expect("failed to start process reclaim worker");
+
     // Initialize softirq subsystem (depends on kthread infrastructure)
     kernel::task::softirqd::init_softirq();
     serial_println!("[boot] Softirq subsystem initialized");
diff --git a/kernel/src/memory/process_memory.rs b/kernel/src/memory/process_memory.rs
index 9750c46e..bdb513f3 100644
--- a/kernel/src/memory/process_memory.rs
+++ b/kernel/src/memory/process_memory.rs
@@ -1864,9 +1864,6 @@ impl ProcessPageTable {
         use alloc::vec::Vec;
 
         let phys_offset = crate::memory::physical_memory_offset();
-        let mut user_frames_freed = 0u64;
-        let mut user_frames_still_shared = 0u64;
-        let mut table_frames_freed = 0u64;
 
         // Collect page table structure frames to free after walking
         let mut l1_frames: Vec<PhysFrame> = Vec::new();
@@ -1904,9 +1901,6 @@ impl ProcessPageTable {
                         let frame = PhysFrame::containing_address(l1_entry.addr());
                         if frame_decref(frame) {
                             deallocate_frame(frame);
-                            user_frames_freed += 1;
-                        } else {
-                            user_frames_still_shared += 1;
                         }
                         continue;
                     }
@@ -1932,9 +1926,6 @@ impl ProcessPageTable {
                             let frame = PhysFrame::containing_address(l2_entry.addr());
                             if frame_decref(frame) {
                                 deallocate_frame(frame);
-                                user_frames_freed += 1;
-                            } else {
-                                user_frames_still_shared += 1;
                             }
                             continue;
                         }
@@ -1959,9 +1950,6 @@ impl ProcessPageTable {
                             let frame = PhysFrame::containing_address(l3_entry.addr());
                             if frame_decref(frame) {
                                 deallocate_frame(frame);
-                                user_frames_freed += 1;
-                            } else {
-                                user_frames_still_shared += 1;
                             }
                         }
                     }
@@ -1971,28 +1959,17 @@ impl ProcessPageTable {
             // Free page table structure frames (L3 first, then L2, then L1)
             for frame in l3_frames {
                 deallocate_frame(frame);
-                table_frames_freed += 1;
             }
             for frame in l2_frames {
                 deallocate_frame(frame);
-                table_frames_freed += 1;
             }
             for frame in l1_frames {
                 deallocate_frame(frame);
-                table_frames_freed += 1;
             }
 
             // Free the L0 frame itself
             deallocate_frame(self.level_4_frame);
-            table_frames_freed += 1;
         }
-
-        log::info!(
-            "cleanup_for_exec [ARM64]: freed {} user frames, {} still shared, {} table frames",
-            user_frames_freed,
-            user_frames_still_shared,
-            table_frames_freed
-        );
     }
 }
 
diff --git a/kernel/src/memory/stack.rs b/kernel/src/memory/stack.rs
index e0914ae3..93bd6438 100644
--- a/kernel/src/memory/stack.rs
+++ b/kernel/src/memory/stack.rs
@@ -263,8 +263,7 @@ impl GuardedStack {
 
 impl Drop for GuardedStack {
     fn drop(&mut self) {
-        // TODO: Implement proper cleanup (unmap pages, deallocate frames)
-        log::debug!("GuardedStack dropped (cleanup not yet implemented)");
+        // TODO: Implement proper cleanup (unmap pages, deallocate frames).
     }
 }
 
diff --git a/kernel/src/process/manager.rs b/kernel/src/process/manager.rs
index 09a3ef71..8ffd926c 100644
--- a/kernel/src/process/manager.rs
+++ b/kernel/src/process/manager.rs
@@ -1117,11 +1117,44 @@ impl ProcessManager {
 
     /// Exit a process with the given exit code
     #[allow(dead_code)]
-    pub fn exit_process(&mut self, pid: ProcessId, exit_code: i32) {
+    pub fn exit_process(
+        &mut self,
+        pid: ProcessId,
+        exit_code: i32,
+    ) -> Option<alloc::vec::Vec<(usize, crate::ipc::fd::FileDescriptor)>> {
         // Get parent PID before we borrow the process mutably
         let parent_pid = self.processes.get(&pid).and_then(|p| p.parent);
+        #[cfg(target_arch = "aarch64")]
+        let fd_entries;
+        #[cfg(not(target_arch = "aarch64"))]
+        let fd_entries = alloc::vec::Vec::new();
 
         if let Some(process) = self.processes.get_mut(&pid) {
+            // Preserve Process::terminate()'s one-shot cleanup contract even on
+            // AArch64, where cleanup is split into lock-held and lock-free phases.
+            if process.is_terminated() {
+                return None;
+            }
+
+            #[cfg(target_arch = "aarch64")]
+            let local_cpu_runs_process = process.main_thread.as_ref().is_some_and(|thread| {
+                let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
+                crate::arch_impl::aarch64::context_switch::last_dispatched_tid(cpu_id)
+                    == Some(thread.id)
+            });
+            #[cfg(target_arch = "aarch64")]
+            let local_cpu_retains_root = local_cpu_runs_process
+                && process.page_table.as_ref().is_some_and(|page_table| {
+                    crate::arch_impl::aarch64::current_cpu_retains_ttbr0_root(
+                        page_table.level_4_frame().start_address().as_u64(),
+                    )
+                });
+            #[cfg(target_arch = "aarch64")]
+            if local_cpu_retains_root {
+                crate::arch_impl::aarch64::quiesce_ttbr0_for_exit();
+            }
+
+            #[cfg(not(target_arch = "aarch64"))]
             log::info!(
                 "Process {} (PID {}) exiting with code {}",
                 process.name,
@@ -1129,10 +1162,20 @@ impl ProcessManager {
                 exit_code
             );
 
-            // Drain any pending old page tables from previous exec() calls
-            process.drain_old_page_tables();
-
-            process.terminate(exit_code);
+            #[cfg(target_arch = "aarch64")]
+            {
+                if !process.terminate_minimal(exit_code) {
+                    return None;
+                }
+                fd_entries = process.take_fd_entries();
+                crate::task::process_task::defer_process_resources(process);
+            }
+            #[cfg(not(target_arch = "aarch64"))]
+            {
+                // Drain any pending old page tables from previous exec() calls
+                process.drain_old_page_tables();
+                process.terminate(exit_code);
+            }
 
             // Remove from ready queue
             self.ready_queue.retain(|&p| p != pid);
@@ -1142,16 +1185,17 @@ impl ProcessManager {
                 self.current_pid = None;
             }
 
-            // Free heavy resources immediately rather than waiting for waitpid reap.
-            // CoW refcounts were already decremented by terminate() -> cleanup_cow_frames(),
-            // so it's safe to drop the page table now.
-            process.page_table.take();
-            process.stack.take();
-            process.pending_old_page_tables.clear();
-
-            // Clean up window buffers so the compositor stops reading freed pages
-            #[cfg(target_arch = "aarch64")]
-            crate::syscall::graphics::cleanup_windows_for_pid(pid.as_u64());
+            #[cfg(not(target_arch = "aarch64"))]
+            {
+                // Free heavy resources immediately rather than waiting for waitpid reap.
+                // CoW refcounts were already decremented by terminate() -> cleanup_cow_frames(),
+                // so it's safe to drop the page table now.
+                process.page_table.take();
+                process.stack.take();
+                process.pending_old_page_tables.clear();
+            }
+        } else {
+            return None;
         }
 
         // Reparent children to init (PID 1)
@@ -1183,6 +1227,7 @@ impl ProcessManager {
             if let Some(parent_process) = self.processes.get_mut(&parent_pid) {
                 use crate::signal::constants::SIGCHLD;
                 parent_process.signals.set_pending(SIGCHLD);
+                #[cfg(not(target_arch = "aarch64"))]
                 log::debug!(
                     "Sent SIGCHLD to parent process {} for child {} exit",
                     parent_pid.as_u64(),
@@ -1190,6 +1235,8 @@ impl ProcessManager {
                 );
             }
         }
+
+        Some(fd_entries)
     }
 
     /// Get the next ready process to run
diff --git a/kernel/src/process/mod.rs b/kernel/src/process/mod.rs
index 94fe97e8..6b85beb2 100644
--- a/kernel/src/process/mod.rs
+++ b/kernel/src/process/mod.rs
@@ -260,11 +260,18 @@ pub fn exit_current(exit_code: i32) {
 
     if let Some(pid) = current_pid() {
         log::debug!("Current PID is {}", pid.as_u64());
-        if let Some(ref mut manager) = *manager() {
-            manager.exit_process(pid, exit_code);
-        } else {
-            log::error!("Process manager not available!");
+        let cleanup = {
+            let mut manager_guard = manager();
+            manager_guard
+                .as_mut()
+                .and_then(|manager| manager.exit_process(pid, exit_code))
+        };
+        #[cfg(target_arch = "aarch64")]
+        if let Some(entries) = cleanup {
+            crate::task::process_task::finish_extracted_process_exit(pid, entries);
         }
+        #[cfg(not(target_arch = "aarch64"))]
+        let _ = cleanup;
     } else {
         log::error!("No current PID set!");
     }
diff --git a/kernel/src/process/process.rs b/kernel/src/process/process.rs
index 59b31419..d18d153d 100644
--- a/kernel/src/process/process.rs
+++ b/kernel/src/process/process.rs
@@ -317,15 +317,18 @@ impl Process {
     /// a system-wide hang on ARM64 SMP where logging, pipe wakeups, and scheduler
     /// calls inside close_all_fds create lock ordering violations with the serial
     /// output lock and framebuffer lock while all CPUs have interrupts disabled.
-    pub fn terminate_minimal(&mut self, exit_code: i32) {
+    /// Returns false when the process was already terminated, so callers must not
+    /// repeat cleanup that decrements CoW references.
+    pub fn terminate_minimal(&mut self, exit_code: i32) -> bool {
         if matches!(self.state, ProcessState::Terminated(_)) {
-            return;
+            return false;
         }
         self.state = ProcessState::Terminated(exit_code);
         self.exit_code = Some(exit_code);
         if let Some(ref mut thread) = self.main_thread {
             thread.set_terminated();
         }
+        true
     }
 
     /// Extract all file descriptor entries for deferred cleanup outside PM lock.
diff --git a/kernel/src/task/process_task.rs b/kernel/src/task/process_task.rs
index 5c6a52fe..89fa68c8 100644
--- a/kernel/src/task/process_task.rs
+++ b/kernel/src/task/process_task.rs
@@ -11,6 +11,8 @@ use core::sync::atomic::{AtomicU64, Ordering};
 
 const DEFERRED_FAULT_EXIT_SLOTS: usize = 16;
 const DEFERRED_FAULT_EXIT_EMPTY: u64 = 0;
+#[cfg(target_arch = "aarch64")]
+const PROCESS_RECLAIM_INTERVAL_NS: u64 = 10_000_000;
 
 struct DeferredFaultExitBuffer {
     slots: [AtomicU64; DEFERRED_FAULT_EXIT_SLOTS],
@@ -97,6 +99,22 @@ impl PendingProcessReclaim {
 static PENDING_PROCESS_RECLAIMS: spin::Mutex<alloc::vec::Vec<PendingProcessReclaim>> =
     spin::Mutex::new(alloc::vec::Vec::new());
 
+#[cfg(target_arch = "aarch64")]
+const MAX_PENDING_PROCESS_RECLAIMS: usize = 256;
+
+/// Current deferred-address-space queue depth, exposed for postmortem diagnostics.
+#[cfg(target_arch = "aarch64")]
+pub static PENDING_PROCESS_RECLAIM_DEPTH: AtomicU64 = AtomicU64::new(0);
+
+/// Number of non-empty sweeps that found no root safe to reclaim.
+#[cfg(target_arch = "aarch64")]
+pub static PENDING_PROCESS_RECLAIM_BLOCKED_SWEEPS: AtomicU64 = AtomicU64::new(0);
+
+/// Number of attempts to exceed the hard deferred-address-space queue cap.
+#[cfg(target_arch = "aarch64")]
+pub static PENDING_PROCESS_RECLAIM_CAPACITY_FAILURES: AtomicU64 = AtomicU64::new(0);
+
+#[cfg(not(target_arch = "aarch64"))]
 fn release_process_resources(process: &mut crate::process::Process) {
     process.cleanup_cow_frames();
     process.drain_old_page_tables();
@@ -106,32 +124,35 @@ fn release_process_resources(process: &mut crate::process::Process) {
 }
 
 #[cfg(target_arch = "aarch64")]
-fn defer_live_process_resources(
-    process: &mut crate::process::Process,
-) -> Option<PendingProcessReclaim> {
-    let root_is_live = process
-        .page_table
-        .iter()
-        .chain(process.pending_old_page_tables.iter())
-        .any(|page_table| {
-            crate::arch_impl::aarch64::is_ttbr0_root_live(
-                page_table.level_4_frame().start_address().as_u64(),
-            )
+pub(crate) fn defer_process_resources(process: &mut crate::process::Process) {
+    let page_table = process.page_table.take();
+    let old_page_tables = core::mem::take(&mut process.pending_old_page_tables);
+    if page_table.is_some() || !old_page_tables.is_empty() {
+        enqueue_process_reclaim(PendingProcessReclaim {
+            page_table,
+            old_page_tables,
+            after_epoch: scheduler::retirement_grace_target(),
         });
-    if !root_is_live {
-        return None;
     }
 
-    Some(PendingProcessReclaim {
-        page_table: process.page_table.take(),
-        old_page_tables: core::mem::take(&mut process.pending_old_page_tables),
-        after_epoch: scheduler::retirement_grace_target(),
-    })
+    // This early drop is safe only while GuardedStack::drop remains a no-op
+    // stub that does not unmap or free stack frames. If that Drop implementation
+    // starts releasing frames, the stack must move into PendingProcessReclaim so
+    // a peer CPU retaining this process root cannot observe freed memory.
+    drop(process.stack.take());
 }
 
 #[cfg(target_arch = "aarch64")]
 fn enqueue_process_reclaim(reclaim: PendingProcessReclaim) {
-    crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().push(reclaim));
+    crate::arch_without_interrupts(|| {
+        let mut pending = PENDING_PROCESS_RECLAIMS.lock();
+        if pending.len() >= MAX_PENDING_PROCESS_RECLAIMS {
+            PENDING_PROCESS_RECLAIM_CAPACITY_FAILURES.fetch_add(1, Ordering::Relaxed);
+            panic!("pending process reclaim queue exhausted");
+        }
+        pending.push(reclaim);
+        PENDING_PROCESS_RECLAIM_DEPTH.store(pending.len() as u64, Ordering::Relaxed);
+    });
 }
 
 /// Close extracted file descriptor entries outside the PM lock.
@@ -142,7 +163,7 @@ fn enqueue_process_reclaim(reclaim: PendingProcessReclaim) {
 /// PTY refcounting, TCP close, etc.
 ///
 /// CRITICAL: No PM lock is held when this runs.
-fn close_extracted_fds(entries: alloc::vec::Vec<(usize, FileDescriptor)>) {
+pub(crate) fn close_extracted_fds(entries: alloc::vec::Vec<(usize, FileDescriptor)>) {
     use crate::ipc::FdKind;
 
     for (_fd, fd_entry) in entries {
@@ -190,6 +211,16 @@ fn close_extracted_fds(entries: alloc::vec::Vec<(usize, FileDescriptor)>) {
     }
 }
 
+/// Complete the lock-free phase of an AArch64 process exit.
+#[cfg(target_arch = "aarch64")]
+pub(crate) fn finish_extracted_process_exit(
+    pid: ProcessId,
+    entries: alloc::vec::Vec<(usize, FileDescriptor)>,
+) {
+    close_extracted_fds(entries);
+    crate::syscall::graphics::cleanup_windows_for_pid(pid.as_u64());
+}
+
 /// Integration functions for scheduling processes as tasks
 pub struct ProcessScheduler;
 
@@ -218,15 +249,12 @@ impl ProcessScheduler {
                     let children = core::mem::take(&mut process.children);
 
                     // Mark terminated and extract FDs without closing them
-                    process.terminate_minimal(exit_code);
+                    if !process.terminate_minimal(exit_code) {
+                        return;
+                    }
                     let fd_entries = process.take_fd_entries();
                     #[cfg(target_arch = "aarch64")]
-                    if let Some(reclaim) = defer_live_process_resources(process) {
-                        enqueue_process_reclaim(reclaim);
-                        drop(process.stack.take());
-                    } else {
-                        release_process_resources(process);
-                    }
+                    defer_process_resources(process);
                     #[cfg(not(target_arch = "aarch64"))]
                     release_process_resources(process);
 
@@ -272,11 +300,10 @@ impl ProcessScheduler {
         // Phase 2: No PM lock — safe to do pipe wakeups, scheduler calls, logging
         if let Some((pid, process_name, fd_entries, parent_tid)) = phase1_result {
             // Close FDs outside PM lock (pipe close_write wakes readers, etc.)
-            close_extracted_fds(fd_entries);
-
-            // Clean up window buffers so the compositor stops reading freed pages
             #[cfg(target_arch = "aarch64")]
-            crate::syscall::graphics::cleanup_windows_for_pid(pid.as_u64());
+            finish_extracted_process_exit(pid, fd_entries);
+            #[cfg(not(target_arch = "aarch64"))]
+            close_extracted_fds(fd_entries);
 
             // Wake parent thread if blocked on waitpid or pause()
             if let Some(parent_tid) = parent_tid {
@@ -347,7 +374,12 @@ pub fn reclaim_deferred_process_resources() {
                 scheduler::retirement_grace_elapsed(&reclaim.after_epoch)
                     && !reclaim.root_is_live()
             });
-            ready.map(|index| pending.swap_remove(index))
+            if ready.is_none() && !pending.is_empty() {
+                PENDING_PROCESS_RECLAIM_BLOCKED_SWEEPS.fetch_add(1, Ordering::Relaxed);
+            }
+            let reclaim = ready.map(|index| pending.swap_remove(index));
+            PENDING_PROCESS_RECLAIM_DEPTH.store(pending.len() as u64, Ordering::Relaxed);
+            reclaim
         });
 
         match reclaim {
@@ -357,6 +389,31 @@ pub fn reclaim_deferred_process_resources() {
     }
 }
 
+/// Start the AArch64 process-retirement worker.
+///
+/// Deferred fault exits and address-space teardown run only from this normal,
+/// preemptible kernel-thread context. In particular, neither operation is tied
+/// to the assembly exception-return tails or to the idle loop's scheduler call.
+#[cfg(target_arch = "aarch64")]
+pub fn init_process_reclaim_worker() -> Result<(), crate::task::kthread::KthreadError> {
+    crate::task::kthread::kthread_run(
+        || loop {
+            drain_deferred_fault_sigsegv_exits();
+            reclaim_deferred_process_resources();
+
+            let (secs, nanos) = crate::time::get_monotonic_time_ns();
+            let now_ns = secs as u64 * 1_000_000_000 + nanos as u64;
+            let wake_time_ns = now_ns.saturating_add(PROCESS_RECLAIM_INTERVAL_NS);
+            scheduler::with_scheduler(|sched| {
+                sched.block_current_for_timer(wake_time_ns);
+            });
+            crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
+        },
+        "kprocess-reclaim",
+    )
+    .map(|_| ())
+}
+
 /// Extension trait for Thread to support process operations
 #[allow(dead_code)]
 pub trait ProcessThread {
diff --git a/kernel/src/task/scheduler.rs b/kernel/src/task/scheduler.rs
index 6af07ac4..4b67fac7 100644
--- a/kernel/src/task/scheduler.rs
+++ b/kernel/src/task/scheduler.rs
@@ -991,17 +991,17 @@ impl Scheduler {
                 return true;
             }
 
+            let grace_elapsed = graces
+                .iter()
+                .find(|grace| grace.thread_id == thread.id())
+                .map(|grace| retirement_grace_elapsed(&grace.after_epoch))
+                .unwrap_or(false);
             let stack_is_live = thread
                 .kernel_stack_top
                 .map(|top| {
                     crate::memory::kernel_stack::is_kernel_stack_slot_live(top.as_u64())
                 })
                 .unwrap_or(false);
-            let grace_elapsed = graces
-                .iter()
-                .find(|grace| grace.thread_id == thread.id())
-                .map(|grace| retirement_grace_elapsed(&grace.after_epoch))
-                .unwrap_or(false);
             if stack_is_live || !grace_elapsed {
                 return true;
             }
```

---

## 4. r10 reconciliation — the original mechanism the r11 fix addressed

(Source: `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/06f628f3-e6a0-47fc-93f0-63d8534b3cd1/scratchpad/ttbr0-abort-reconciliation.md`, reproduced verbatim below.)

```markdown
# Reconciliation — bterm fork/exec TTBR0 abort cascade (round 10 → round 11 fix-forward)

Reconciler: independent verification pass against `/Users/wrb/fun/code/breenix` @ `2b98725d` (clean tree)
and `logs/parallels-launcher-test/run-20260802-045109/`. Every disputed claim below was re-derived from
source or from the log; nothing is averaged between the two analysts.

---

## 1. Where A and B agree (and the agreement is CORRECT — verified)

| # | Claim | Verification |
|---|---|---|
| A1 | `ProcessScheduler::handle_thread_exit()` frees the dying process's CoW frames, old page tables, page-table root and user stack **synchronously, in-syscall, under the PM lock**, on a CPU that still has that root live | `kernel/src/task/process_task.rs:150-156` (`cleanup_cow_frames` / `drain_old_page_tables` / `page_table.take()` / `stack.take()` / `pending_old_page_tables.clear()`); same pattern `kernel/src/process/manager.rs:1130-1147`. CONFIRMED |
| A2 | Nothing scrubs the per-CPU TTBR0 shadows on exit | No writer of `saved_process_cr3` (percpu off 80) or `next_cr3` (off 64) anywhere on the exit path. CONFIRMED |
| A3 | The TTBR0 change between abort #2 and #3 (`0x1_0000_440e_8000` → `0x1_4001_7000`) is **the fault handler's own repair**, not a second process | `exception.rs:1119-1128`: the `[INSTRUCTION_ABORT] deferring process cleanup` arm calls `terminate_current_scheduler_thread()` then `switch_ttbr0_to_kernel()` (`exception.rs:295-317`). Abort #2's log line prints that exact string; aborts #3/#4 then read the kernel root. CONFIRMED — both analysts are right, this is a reporting artifact and must not be chased. |
| A4 | Aborts #2-#4 are cascade damage: IFSC=0x0e permission faults with ELR/FAR inside kernel **data** (`TRACE_BUFFERS`, `ALL_CPU_DATA`, `PC_ALIGN_VERBOSE_CAPTURED`) — i.e. EL1 branching into PXN'd data | ESR `0x8600000e`, EC=0x21. CONFIRMED |
| A5 | The fix direction is a refcounted address-space object + CPU quiescence before reclamation | Both fix specs converge. Adopted below. |

---

## 2. Conflicts, resolved with evidence

### C1 — "bterm's pid 10/11 forks recycled blauncher's kernel stack and caused abort #1" (A) vs "those forks happen AFTER abort #1, so they cannot be the trigger" (B)
**B WINS on the timeline; A's mechanism survives for aborts #2-#4 only.**
- The `F123456789SC` breadcrumb is the complete `sys_fork_aarch64` char trail (`syscall_entry.rs:929-996`: `1`…`9`,`S`,`C`). The trail printed **immediately before** `[syscall] exit(0) pid=8 name=blauncher` is blauncher forking **pid 9 (bterm)** — i.e. *before* the exit, when tid 14 was not yet `Terminated` and therefore not reclaimable.
- The next two trails (`[bterm] spawned child pid=10 / pid=11`) sit between abort #2 and abort #4.
- ⇒ **No `reclaim_terminated_threads()` + `allocate_kernel_stack()` 64 KiB zero-fill can be placed between blauncher's exit and abort #1.** A's "recycled + zero-filled by the very next fork" is not established for abort #1.
- It *is* correctly placed for the #2→#4 degradation (two forks land in that window), which matches the monotonic decay to `ELR=0x1`.

### C2 — "The silent PIVOT_ALIAS evidence shows this is a separate issue, not a regression of the three fixed defects" (B)
**INVALID — absence proves nothing here.** `dump_stack_pivot_alias_history()` is called only from `dump_fatal_postmortem_once()` (`exception.rs:356-388`), and that function **died mid-dump**: after `dump_defer_requeue_snapshots()` its very next unconditional print is `"\n  Trace buffers:\n"` — `grep -c "Trace buffers:" run-sh.log` = **0**. Abort #2's `FAR = ALL_CPU_DATA+0x300` is exactly where the postmortem was executing. So PIVOT_ALIAS / idle-redirect histories / stack-half canaries / SAVE_SKEW were **never emitted at all**. Neither analyst may use their absence as evidence in either direction.

### C3 — "Abort #1 is the ERET of a TORN frame; every dispatcher path that could emit EL1-SPSR + non-kernel ELR is guarded and prints, and all those greps are 0" (A)
**A's deduction is SOUND and is now stronger than A stated.** The guard A needed and did not cite exactly is on the **user resume** arm of `dispatch_thread_locked`:
```rust
// kernel/src/arch_impl/aarch64/context_switch.rs:3189-3205
if frame.elr < 0x1000 || (frame.spsr & 0xF) != 0 {
    raw_uart_str("\n[BUG] dispatch_thread: bad context tid=");   // ← exactly abort #1's shape
```
Verified greps over the full `run-sh.log`: `invalid context for kernel dispatch` = 0, `WARN: bad elr=` = 0, `bad context` = 0, `TTBR_GONE` = 0, `BAD_THREAD_SP` = 0. All are `raw_uart_str` at the point of the event (not postmortem-deferred), so their absence *is* meaningful.
Also ruled out by inspection: every `frame.spsr` writer in the tree is adjacent to a `frame.elr` writer (`context_switch.rs:2582/2589/2684/2709/2811/4467`, `exception.rs:745/772/1102/1124/1173/1270/1486`, `syscall_entry.rs:1212+1254`, `context.rs:156`), and all the idle-redirect writers additionally zero `x0..x30` — abort #1's frame carried **live user** `x29=0xfffffefffc40` / `x30=0x40002e44`, so no idle-redirect wrote it.
⇒ No single code path can produce abort #1's frame. Something wrote part of it.

### C4 — Is the frame corruption real, or is A over-reading?
**REAL — two hardware-level proofs neither analyst used:**
1. **Impossible SPSR.** The abort-#4 DIAG regdump (`run-sh.log:2371`) prints `spsr=0x134598`. That is M[3:0]=0x8 with M[4]=1 (AArch32 EL2t) on an AArch64-only kernel taking a fault at EL1. Hardware never writes that value into a frame.
2. **FAR vs frame.ELR disagreement, 3 for 3.** `far` reaches the handler as a **live register read at exception entry** (`boot.S:511` and `boot.S:784`: `mrs x2, far_el1` immediately before `bl handle_sync_exception`); `frame_ref.elr` is a **memory word** stored at entry (`boot.S:489`) and read later by Rust. For a same-EL instruction abort with `ESR.FnV = 0` (ESR `0x8600000e`, bit 10 clear) they are architecturally required to be equal. Observed: #2 `FAR=ALL_CPU_DATA+0x300` vs `ELR=TRACE_BUFFERS+0x100d0`; #3 `FAR=PC_ALIGN_VERBOSE_CAPTURED` vs `ELR=TRACE_BUFFERS+0`; #4 `FAR=TRACE_BUFFERS+0x100d0` vs `ELR=0x1`. Nothing in `handle_sync_exception` mutates `frame.elr` before the print (`exception.rs:452-520` then the `INSTRUCTION_ABORT` arm prints first).
⇒ **The exception frames on this stack are being mutated by a second writer.** A's "the stack was not exclusively owned" is established for aborts #2-#4, independent of any grep.

### C5 — Which half is proven *at abort #1*: the dead address space (B) or the non-owned stack (A)?
**B's half is proven at abort #1; A's half is proven at #2-#4.**
- Abort #1 is `IFSC=0x05` = **translation fault, LEVEL 1** for user VA `0x40002e84` under a process TTBR0. A live user table cannot produce that: the L1 descriptor covering the whole `0x4000_0000` 1 GiB user region would have to be invalid. If the table were intact and EL1 merely fetched a user page, ARM would report a **permission** fault (PXN, `mmu.rs:12,66-70`), not a translation fault. ⇒ **the address space was already torn down when abort #1 fired.** That is `process_task.rs:154` firing while CPU1 still had the root in `TTBR0_EL1`.
- The control-flow edge (how EL1 came to be executing at a user PC) is **not proven** by either analyst. A's "torn ERET" and an "indirect branch through a corrupted register" (directly evidenced at abort #4: `x26=x27=ALL_CPU_DATA+0x300`, `x30=0x8087c276`) are both live, and both require the same root cause: frames/stacks not exclusively owned.

### C6 — Corroborations of A that DO hold
- `sp=0xffff000054266000` is exactly the top of aarch64 kernel-stack pool **slot 5**: `kernel_stack.rs:237-251` gives base `HHDM+0x5420_0000`, slot stride `0x11000`, top = base + (i+1)·0x11000 → `0x54200000 + 6·0x11000 = 0x54266000`. And `DISPATCH_TRACE [0]/[6]` both show `U …->tid=14 … sp=0xffff000054266000` — it is **tid 14's** kernel stack, while the scheduler believed CPU1 was running **tid 3 (swapper/1)**. CONFIRMED.
- `Scheduler::reclaim_terminated_threads` (`scheduler.rs:895-917`) frees a `Terminated` thread's 64 KiB pool slot (`KernelStack::drop` → `free_kernel_stack`) on a test that is *name-matching against `cpu_state[].current/previous/idle`*, not a liveness proof. `allocate_kernel_stack()` then `write_bytes(..., 0, 64 KiB)` the whole slot (`kernel_stack.rs:335-341`), and `sys_fork_aarch64` calls reclaim immediately before allocating (`syscall_entry.rs:945`). CONFIRMED — real, live defect.
- `setup_idle_return_locked` (`context_switch.rs:2797-2857`) never sets `next_cr3`; `syscall_entry.S:.Lrestore_saved_ttbr` therefore re-installs `saved_process_cr3` unconditionally on the way out. CONFIRMED — idle runs on the exited process's (freed) root.

### C7 — Corroborations of B that DO hold, and one B under-stated
- `sys_exit_aarch64` (`syscall_entry.rs:328-412`) calls `handle_thread_exit()`, then `set_terminated()`, then **spins in `wfi` with IRQs unmasked, at EL1, on the dying thread's own kernel stack**, with the freed root still in `TTBR0_EL1`. CONFIRMED — this is the window that makes both halves reachable at once.
- B under-stated it: the SVC return path doesn't merely *fail to switch away* — it **actively re-installs** the freed root (`syscall_entry.S`, `.Lrestore_saved_ttbr`).
- B's item 6 is real: `[INSTRUCTION_ABORT] deferred_tid=3 queued=1` — the handler defers cleanup for **swapper/1**, not for the victim, because `defer_current_user_thread_sigsegv_exit` uses `current_thread_id()`.

### C8 — New defect found while reconciling (neither analyst)
`dump_stack_classification` (`exception.rs:396-416`) classifies against `KSTACK_BASE = HHDM+0x5200_0000 .. 0x5400_0000`, but the aarch64 pool is `0x5420_0000..0x5620_0000` (`kernel_stack.rs:237-238`). Every real pool frame therefore prints `STACK=unknown` — exactly what this run printed for `sp_at_frame=0xffff000054265880`. The one diagnostic that would have named the stack's owner is mis-ranged.

---

## 3. Surviving mechanism (single statement)

> **Breenix retires a dying process's two hot resources — its address space AND its kernel stack — from inside the dying thread's own execution context, with no architectural quiescence and no ownership refcount.**
>
> `handle_thread_exit()` frees the page-table root (`process_task.rs:154`) while the CPU still holds it in `TTBR0_EL1` **and** in per-CPU `saved_process_cr3`; `sys_exit_aarch64()` then marks the thread `Terminated` and spins at EL1 **on that same thread's kernel stack** with IRQs enabled. That single window simultaneously (a) makes the stack eligible for `reclaim_terminated_threads()` → pool free → zero-filling re-handout on the next fork, and (b) leaves every idle/redirect return path re-installing the freed root (`setup_idle_return_locked` never sets `next_cr3`; `syscall_entry.S` falls back to `saved_process_cr3`).
>
> Observed consequence: CPU1 at EL1 with a **dead TTBR0** (abort #1, IFSC=0x05, proven) and an exception frame that **no single code path can have written** (user ELR + EL1 SPSR, all guards silent) → the fatal handler keeps running on the same non-owned stack → aborts #2-#4, with provably corrupt frames (impossible SPSR `0x134598`; FAR ≠ frame.ELR ×3) while two more forks run reclaim + 64 KiB zero-fill underneath it.

A and B are **two facets of one defect**, not competing theories. B's facet is proven at abort #1; A's facet is proven at aborts #2-#4 and is mechanistically present (verified in source) but not timeline-placeable before abort #1. Fix both — neither alone is safe.

---

## 4. Round-11 FIX-FORWARD spec

Land in the stated order. Every step is independently testable.

### Step 0 (zero risk, land first — makes everything else observable)
| File | Change |
|---|---|
| `kernel/src/arch_impl/aarch64/exception.rs:356-388` | Reorder `dump_fatal_postmortem_once()`: print **idle-redirect histories, stack-pivot-alias history, save-skew slots, stack-half canaries, last-dispatched-tid** BEFORE `crate::tracing::dump_all_buffers()`. Wrap each section so a nested abort truncates only the remaining sections. (This run lost every one of them.) |
| `kernel/src/arch_impl/aarch64/exception.rs:396-416` | Fix `dump_stack_classification` KSTACK range to the real pool (`kernel_stack.rs` `ARM64_KERNEL_STACK_BASE/END`, import them — do not re-hardcode). Also print slot index and the tid recorded by `stamp_last_dispatched_tid` for that slot. |
| `kernel/src/arch_impl/aarch64/exception.rs` (`defer_current_user_thread_sigsegv_exit`) | Capture the victim tid from the faulting frame's stack-slot owner / `last_dispatched_tid`, not `current_thread_id()` (which was idle here). |

### Step 1 — make the two impossible states unrepresentable (small, high value)
| File:fn | Change |
|---|---|
| `context_switch.rs:2647 restore_userspace_context_inline` | For `ThreadPrivilege::User`, force `frame.spsr = dispatch_spsr(ctx.spsr_el1) & !SPSR_MODE_MASK` (EL0t). A user thread can never legitimately resume at EL1; today M[3:0] is passed straight through. |
| `context_switch.rs:4374-4389` (inline-schedule save) | Stop writing a kernel SPSR into `old_thread.context.spsr_el1` while leaving `context.elr_el1` at the user PC. Either (a) store the kernel resume SPSR in a new `inline_schedule_spsr` field so `context.spsr_el1` always matches `context.elr_el1`, or (b) write a matching kernel `elr_el1`. |
| `context_switch.rs:2797 setup_idle_return_locked` and `exception.rs:282 set_idle_stack_for_eret` | Set `next_cr3 = kernel_ttbr0` (and clear `saved_process_cr3`) so the assembly can never fall through to `.Lrestore_saved_ttbr` with a retired root. |
| `boot.S` sync/IRQ ERET epilogues + `syscall_entry.S` epilogue | Widen the existing `elr < 0x1000` guard to the real invariant: `(spsr & 0xF) != 0 && elr < KERNEL_VIRT_BASE` → redirect to idle + record into a per-CPU slot (printed by the postmortem). **Branch-only, no UART on the return path.** This is adjacent to a gold-master region (`aarch64_enter_exception_frame` ISB placement) — do **not** touch ISB/ERET ordering; read `docs/planning/cpu0-user-guard-autopsy/README.md` and carry the PR-signoff note. |

### Step 2 — the exiting thread must leave its own stack and address space before either is retired
| File:fn | Change |
|---|---|
| `arch_impl/aarch64/syscall_entry.rs:328 sys_exit_aarch64` | **Before** `handle_thread_exit()`: call the (promoted, shared) `switch_ttbr0_to_kernel()` and clear per-CPU `saved_process_cr3` + `next_cr3`. **Replace the terminal `wfi` loop** with an explicit final schedule: pivot to the per-CPU scheduler stack (existing `scheduler_stack_top(cpu_id)` / `aarch64_inline_schedule_switch` machinery) and only then mark `Terminated` + schedule away, so a thread is never simultaneously `Terminated` and on-CPU/on-its-own-stack. |
| `exception.rs:295-317 switch_ttbr0_to_kernel` | Promote out of `exception.rs` into the arch module so exit/exec/fault paths share one implementation. |
| `task/scheduler.rs:895 reclaim_terminated_threads` | Gate on real liveness, not `cpu_state` name-matching. Add `Thread.on_cpu: Option<usize>` set at dispatch and cleared only after that CPU has architecturally left the stack (post-ERET/post-pivot); refuse reclaim while set. Cheaper interim: refuse to reclaim any thread whose `kernel_stack_top` equals any CPU's `kernel_stack_top`/`user_rsp_scratch`, and require one full scheduling epoch on every CPU (RCU-style grace period) after `Terminated`. |
| `memory/kernel_stack.rs:281-354` | Keep the unconditional 64 KiB scrub, and add a `debug_assert!` that the slot handed out is not any CPU's live `kernel_stack_top`/`user_rsp_scratch` — it must never fire once Step 2 lands. |

### Step 3 — structural: refcounted address space + quiesced reaper
| File | Change |
|---|---|
| `task/process_task.rs:137-160`, `process/manager.rs:1130-1147` | Remove the inline frees. Move `cleanup_cow_frames()` / `drain_old_page_tables()` / `page_table.take()` / `stack.take()` / `pending_old_page_tables.clear()` into a **retire queue**, drained by a reaper at a safe point only when no CPU has that root in `TTBR0_EL1`, `saved_process_cr3`, `next_cr3`, or `Thread.cached_ttbr0`. |
| `process/process.rs:159`, `syscall/clone.rs:201` | Replace `Option<Box<ProcessPageTable>>` + raw `inherited_cr3` with `Arc<AddressSpace>`; CLONE_VM shares the `Arc`; retire on last drop through the same reaper. Kills the "next exec or exit means CR3 definitely switched" assumption (`process.rs:172,523`, `manager.rs:3054`). |
| `process/manager.rs:46` (exec sibling quiescence) | Track *which CPUs have the mm loaded*, not logical thread state; terminated siblings are not architecturally quiesced. |
| new invariant (debug build) | Assert no retired root matches any CPU's live `TTBR0_EL1` / `saved_process_cr3` / `next_cr3` / current-thread `cached_ttbr0` before reclamation. This would have caught the bug at the violation instead of four aborts downstream. |

### Validation plan
1. **Build clean, both arches, zero warnings**
   `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
   `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
2. **QEMU aarch64 ×10 consecutive clean boots** — `./docker/qemu/run-aarch64-boot-test-native.sh`; each must show zero `UNHANDLED_EC`, `INSTRUCTION_ABORT`, `DATA_ABORT`, `EL1_INLINE_ABORT`, `FATAL_POSTMORTEM`, `[BUG] dispatch_thread`, `[TTBR_GONE`, panic.
3. **New targeted regression** (`tests/`, shared-QEMU): parent forks N children that `exit()` immediately while the parent keeps forking — drives `reclaim_terminated_threads()` + `allocate_kernel_stack()` + 64 KiB zero-fill against a still-on-CPU dying thread. ≥1000 iterations, zero aborts, and the Step-2 `debug_assert!` never fires. Do NOT weaken this to "process was created".
4. **Parallels streak** — fresh epoch-named VM per attempt via `./run.sh --parallels` only (never a static VM name, never `deploy-to-vm.sh`); `parallels-launcher-test` skill, sequential, **10 consecutive PASS with `inject_retries=0`, up to 15 attempts**. Each PASS must additionally show zero of the markers in (2). `prlctl stop --kill` every VM afterwards.
5. **90-minute soak** (r9-style) after the streak; watch cpu0 tick-rate parity given the project's CPU0-timer fragility.

---

## 5. If you land nothing else first: the single cheapest record-only probe

**Reorder `dump_fatal_postmortem_once()` (Step 0, row 1).** The probes that decide A vs B — `dump_stack_pivot_alias_history`, `dump_all_idle_redirect_histories`, the stack-half canaries, the save-skew slots — already exist and already recorded during this run; their output was destroyed because the postmortem crashed inside `dump_all_buffers()` before printing them. This is a statement reorder, no hot-path cost, no behavioral change, and on the next reproduction it names the second writer directly.

**Tie-breaker if that comes back clean:** in the `INSTRUCTION_ABORT` arm, when `!from_el0 && frame_ref.elr < 0xFFFF_0000_0000_0000` (EL1 executing at a user PC), dump the frame's `elr/spsr/x29/x30`, the per-CPU `kernel_stack_top` / `user_rsp_scratch` / `saved_process_cr3` / `next_cr3`, and the **owning tid of the pool slot containing `frame`** (needs the Step-0 KSTACK-range fix). Post-fault only; zero hot-path cost.

---

## 6. Confidence

- Mechanism family (retire-without-quiescence of address space + kernel stack from the dying thread's own context): **high** — every element verified in source, and the abort-#1 IFSC=0x05 proves the address space was already dead.
- Frame/stack non-exclusive ownership: **high for aborts #2-#4** (impossible SPSR `0x134598`; FAR ≠ frame.ELR three times), **medium for abort #1** (all producers guarded and silent, but the proximate writer is not named).
- Exact proximate writer of abort #1's frame: **unresolved**. Do not let Step 3 wait on it; Steps 0-2 remove the window regardless.
```

---

## 5. INVARIANTS — distilled from the findings above (deduplicated, stated positively)

### Exception/ERET/hot-path discipline
1. No teardown work (page-table walks, frame frees, heap allocation, blocking locks) is reachable from any aarch64 exception-return tail (`check_need_resched_and_switch_arm64`, `schedule_from_kernel`, `boot.S`'s `bl` sites, `syscall_entry.S`) — teardown runs only from a normal, preemptible, schedulable execution context.
2. No `log::*`/`serial_println!`/string formatting is reachable from any code path invoked on an exception-return tail, a fault handler under the PM lock with IRQs off, or the idle loop.
3. Nothing that can block (heap allocation, a contended spinlock, an unbounded push into a shared collection) runs inside `arch_without_interrupts` / IRQs-off / PM-lock-held sections; where a heavyweight operation must eventually happen, it is deferred to a context where it can be preempted.
4. A function's own early-return gate (e.g. `PREEMPT_ACTIVE`) is checked *before*, not after, any newly added heavyweight work is invoked from that function.
5. No frozen/gold-master region (`idle_loop_arm64`, `aarch64_enter_exception_frame` ISB placement, GICv3 SGI-enable block, timer handler re-arm-at-top, CPU0 regression alarm) is touched by a teardown/reclaim change; any diff review must show these regions byte-for-byte unchanged.

### TTBR0 / address-space ownership and quiescence
6. TTBR0 shadows (`saved_process_cr3`, `next_cr3`) are cleared/republished only *after* the hardware `TTBR0_EL1` register has actually been switched — never before, and never left cleared while the register still names the old root.
7. Any oracle that decides whether a page-table root is "live" (`is_ttbr0_root_live`, `current_cpu_retains_ttbr0_root`) must consult the actual hardware register (`TTBR0_EL1`) as an authoritative source, not only software shadows, and must be updated as a first-class invariant whenever a new call site starts depending on it.
8. A quiesce/clobber operation on a CPU's TTBR0 state (`quiesce_ttbr0_for_exit`) is applied only when that CPU is actually the one currently holding the specific process/root in question — never unconditionally against an arbitrary pid — and this ownership check is applied uniformly at every call site that performs the same clobber (not just the new one).
9. A path that redirects a CPU to idle (`setup_idle_return_locked`, `set_idle_stack_for_eret`) must not leave the CPU's saved TTBR0 shadow pointing at a retired/exited process's root indefinitely; whatever eventually switches TTBR0 for that CPU must actually run on every path that can reach idle, including paths where no user-dispatch site is ever revisited.
10. A dying thread/process quiesces (switches away from) its own address space and kernel stack *before* either resource becomes eligible for retirement/reclaim — a thread is never simultaneously marked `Terminated` (or otherwise reclaim-eligible) and still on-CPU / on its own stack / holding its own root in TTBR0.
11. Reclamation of a page-table root or kernel stack requires proof that no CPU still has it live (in `TTBR0_EL1`, `saved_process_cr3`, `next_cr3`, or per-thread cached state) — not a name-match against scheduler bookkeeping (`cpu_state[].current/previous/idle`) that can lag reality.
12. A retirement/grace-period read of "is this resource still referenced by a peer CPU" must be ordered correctly relative to the epoch/generation read that gates it (the acquire-ordered epoch load happens before the plain/volatile liveness read it's meant to fence), and that ordering dependency should not silently rely on an unenforced/unasserted precondition (e.g. "CPU0 is always online post-boot").

### Double-terminate / exit-path correctness
13. `Process::terminate`'s (or its minimal variant's) guard against re-running cleanup on an already-`Terminated` process is preserved on every code path that can call it, including new/refactored paths — an already-terminated process must never have its CoW refcounts decremented, its FDs closed, or its resources released a second time.
14. Ownership/ready-to-reclaim checks (main-thread-only vs. any-thread, or any other predicate) are applied identically — the same predicate, the same call site pattern — at every quiesce/exit call site for a given process, not bespoke per call site (a check keyed on `main_thread.id` while a sibling call site applies no check at all is an inconsistency, not two independent invariants).
15. Every exit path — including the already-terminated / early-return path — still performs child-reparenting to `init` and still wakes any parent blocked in `waitpid()`/`pause()` (`unblock_for_child_exit`/`unblock_for_signal`); an early return added for double-terminate safety must not also skip these phase-2 side effects.
16. State (e.g. `process.children`) is taken/moved out only after all early-return checks for the current operation have been evaluated — not before — so an early-return path cannot silently drop already-extracted state.
17. A function's return-value contract change (e.g. `terminate_minimal` gaining a `bool`/`Option` return to signal "already terminated") is honored by every caller: a `None`/`false` result must not be treated as equivalent to "proceed as if the operation succeeded" (e.g. skipping a scheduler-thread termination or an idle redirect that used to run unconditionally).

### Locking / lock-order discipline
18. File-descriptor cleanup that can wake blocked readers, release PTYs, or close TCP connections (i.e. can reach the scheduler or take other locks) runs outside the PM lock (`take_fd_entries` + a lock-free close phase) on every exit path that touches FDs — not inline under the PM lock with interrupts disabled, on any architecture.
19. A blocking/broadcast operation with cross-CPU synchronization cost (e.g. a broadcast TLB invalidate: `dsb ishst; tlbi vmalle1is; dsb ish; isb`) is not issued from inside a lock held with interrupts disabled on a hot exception/fault path without accounting for the extended IRQ-off window it creates for every other CPU.
20. Locks that can be reached from both interrupt/fault context and normal thread context must use `try_lock()` with a hardware/deferred fallback, never a plain blocking `lock()`, when the same lock can be held by a preemptible thread that a timer interrupt might interrupt on the same CPU (self-deadlock avoidance).

### Overflow / panic / diagnostics discipline
21. Overflow policies for bounded queues (e.g. a capped pending-reclaim vector) never panic while holding a lock with interrupts disabled — a panic reached in that state cannot itself print/report (the panic handler's own diagnostics take locks), so the kernel dies silently instead of surfacing the failure.
22. A bounded resource whose exhaustion is fatal must not be reachable when a single CPU stalls/wedges (e.g. stops advancing its scheduling epoch) — a wedged CPU must not be able to turn "the cap was reached" into "the kernel died with no diagnostic."
23. Counters/statistics that exist for postmortem diagnostics (queue depth, blocked-sweep count, capacity-failure count, etc.) are actually read somewhere (a postmortem dumper, a `/proc` handler, or a GDB helper) — a counter that is written but never read anywhere in the tree does not satisfy an "exposed for diagnostics" claim.
24. Diagnostic code (stack-range classifiers, postmortem dumpers, etc.) that names ranges/constants (e.g. kernel-stack pool base/end) imports them from the single source of truth rather than re-hardcoding a stale copy, so the diagnostic stays correct as the real ranges change.
25. A fatal postmortem dumper prints its highest-value diagnostic sections first (or wraps each section so a later crash truncates only what follows), since the dumper itself can crash partway through and destroy exactly the evidence needed to root-cause the fault it's reporting on.

### Cross-architecture / commit-honesty discipline
26. A change scoped to "AArch64 X" that also silently changes shared/generic code, or that leaves the x86_64 twin of the same function behaviorally divergent (e.g. one arch's `cleanup_for_exec` keeps its logging/counters while the other's doesn't), is called out explicitly rather than left as an unstated arch asymmetry.
27. Commit messages state the full reachability/blast radius of a change (e.g. "drain deferred process reclaims during scheduling" must not undersell that the call site is reached from every exception return, not just an explicit scheduling call) and disclose any safety guard silently removed by the same commit (e.g. a double-terminate guard dropped while switching from `terminate()` to `terminate_minimal()`).
28. Any comment justifying a code change asserts something only about control flow that is actually true for every path reaching that code — a rationale like "the shadow will be republished by function X" is invalid if X is not reached from all the paths the change affects (e.g. the idle-redirect path never calls the user-dispatch-only `switch_ttbr0_if_needed`).

