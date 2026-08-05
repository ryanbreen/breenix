# Teardown Unification — PHASED IMPLEMENTATION PLAN

Companion to `teardown-unification-DESIGN.md`. Design-only: nothing below has been implemented and no
gate result is claimed.

**Base:** `main` @ `eebc8868`. **Issues:** #491 (spine), #464, #471.

---

## 0. Phasing contract (this is the part that differs from every prior attempt)

Five rules bind every phase. They exist because the two failure modes that killed prior rounds were
(a) a large coherent design accumulating unreviewed surface before validation (grave branch: fully
green at r18, 27 findings / 15 blocking at r20) and (b) point fixes that each introduced a new defect
class (r23: fixed 3, introduced 4).

1. **Strictly better, always.** Stopping the round at *any* phase boundary must leave the tree
   strictly better than `main`. No phase depends on a later phase for its safety argument.
2. **No dormant code.** Every new API has a live caller in the PR that introduces it. No
   `#[allow(dead_code)]`, no "activated in a later phase". *(This is Judge 1's second fatal finding
   against design B, cured structurally.)*
3. **Spine first.** #491's confirmed-live UAF closes in **Phase 2 of 13**, using only already-merged
   primitives — not behind ten prerequisite PRs. *(Judge 1's first fatal finding against B.)*
4. **Hard size ceiling.** PR #418 measured **5 files / 166 insertions / 70 deletions = 236 lines**.
   Each phase targets **≤ ~230 changed non-generated lines across ≤ 5 production files**. Crossing it
   means splitting at the named seam *before* review, not merging anyway. The ceiling is a gate.
5. **One revert story per phase**, written in the PR body before merge, and the phase's code must
   actually be revertable alone (verified by `git revert` dry run on the merge commit).

### Standard gate — run on EVERY phase, no exceptions

1. **Zero-warning builds** (all three configs; the grep must produce no output):
   - `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
   - `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
   - the aarch64 `ec0_fault_inject` config
2. **QEMU boot/regression:** `./docker/qemu/run-boot-parallel.sh` (x86) + the aarch64 native/strict
   boot test. Tests must reach the real `KERNEL_POST_TESTS_COMPLETE` marker — a marker printed before
   the behaviour under test is never accepted as evidence. **QEMU concurrency capped at 4** per
   standing operator rule (batch 4 and 4, never 8+).
3. **Phase-specific assertions** below — every one an observed outcome (counter equality, actual
   `waitpid` status, zero fault markers). "The process was created" is never evidence.
4. **Parallels launcher streak:** 10 consecutive PASS with `inject_retries=0`, ≤15 attempts, fresh
   epoch-named VM via `./run.sh --parallels`, `prlctl stop --kill` after each.
5. **Soak** on any phase that changes kill timing or retention (2, 6, 7, 8, 10, 11): 30-min minimum,
   plus the retention measurement where noted.
6. **Frozen-region hash gate:** all six gold-master regions byte-identical vs `main`; all five Tier-1
   files byte-identical unless OQ-4 has been granted in writing.
7. **Cleanup:** all Parallels VMs stopped, all stray QEMU killed, before reporting the phase done.

### Dependency graph (the only hard edges)

```
P0 ──> everything (evidence)
P1 ──> P2, P8, P10, P11        (fence/proof must precede wider grace reliance)
P2 ──> P6, P7, P10             (the live SIGKILL call site is the thing later phases upgrade)
P3 ──> P10                     (exec detach before any group-scoped kill)
P4 ──> P8, P10                 (all stacks scheduler-owned before victim-owned exit)
P5 ──> P12                     (identity before policy — never bundled)
P6 ──> P8, P11                 (ledger before more producers exist)
P7 ──> P8
P8 ──> P9a/b/c ──> P10 ──> P11 ──> P12
```

Everything else is independent and can be reordered or dropped. **P3, P4, P5 can run in parallel with
P2** (disjoint files) if review bandwidth allows.

---

## Phase 0 — Teardown observability + call-site ratchet *(no behaviour change)*

**Scope.** Lock-free counters with a normal-context reader, and a source-structure ratchet that pins
the *exact current* bypass surface so any regression fails CI immediately.

Counters (all `trace_count!`, all with a reader — no write-only counters):
`TEARDOWN_ENTRY{exit,fault,signal,group}`, `EXIT_FIRST_REQUESTS`, `EXIT_REPEAT_REQUESTS`,
`EXIT_ATTRIBUTION_UNCERTAIN`, `TEARDOWN_QUARANTINE`, `EXIT_SGI_SENT`, `TEARDOWN_DEFER`,
`TEARDOWN_RECLAIM`, `TEARDOWN_VICTIM_DIVERGENCE`, `TEARDOWN_CR3_MISS`,
`TEARDOWN_MASKED_FRAMES_WALKED`, `FD_CLOSES_UNDER_PM`, `RECLAIM_CONTEXT_VIOLATIONS`,
`TEARDOWN_LOCK_ORDER_SUSPECT`, `DEFERRED_FAULT_RING_DROPPED` (#492's overflow is invisible today).

Ratchet (`tests/teardown_structure.rs`) asserts the exact current sets of: `\.terminate\(` /
`terminate_minimal\(` call sites; production `ProcessId::new(1)` sites (with the three
`test_userspace.rs` sites allowlisted **by name**); `terminate_process_threads` call sites;
`kernel_stack_allocation` mutation sites. It passes on `main` unchanged; later phases shrink the
allowlist, and any *new* bypass fails on arrival.

**Files.** `kernel/src/tracing/providers/teardown.rs` (new), tracing registration,
`tests/teardown_structure.rs` (new). **~200 lines, 2 commits.**

**Gate extras.** Boot test asserts `TEARDOWN_DEFER == TEARDOWN_RECLAIM` at quiesce,
`TEARDOWN_LOCK_ORDER_SUSPECT == 0`, and that every counter has a reader (no write-only counters).

**Strictly better.** Every later phase's evidence becomes a counter equality instead of a log-reading
exercise, and the bypass surface can only shrink. #492's silent drops become visible.

**Revert.** Delete two files + their registration. Zero coupling.

---

## Phase 1 — Retirement fence + RootProof blocker taxonomy *(behaviour-preserving hardening)*

**Scope.** Replace the bare grace arrays with `RetirementFence { epochs, online_mask }` and
`RetirementSnapshot` (unconditional Acquire fence before any liveness read). **An empty/all-zero
online mask is INVALID** — it refuses and increments `RETIRE_EMPTY_ONLINE_MASK` rather than passing
with zero atomic loads (this closes the banked `retirement_grace_elapsed` short-circuit, F13).
Replace the boolean root-liveness answer with `RootProof`'s blocker taxonomy: local **hardware**
`TTBR0_EL1` on the proving CPU, every captured CPU's `saved_process_cr3`/`next_cr3` shadows,
scheduler cached roots, live/creating rows — each with its own counter. Adapt today's two reclaim
users. Grace is still checked **first**.

**Files.** `kernel/src/task/scheduler.rs`, `kernel/src/task/process_task.rs`,
`kernel/src/arch_impl/aarch64/ttbr0.rs`, tracing provider, targeted tests. **~170 lines, 2 commits.**

**Gate extras.** Unit injection with a zero online mask refuses reclaim; wrap-safe epoch comparison
test; the existing epoch-before-stack-liveness ordering becomes a structural test; every refusal in a
normal boot is attributable to exactly one blocker.

**Strictly better.** Grace can no longer elapse on an unordered or empty observation; reclaim
refusals stop being a single opaque boolean; the local hardware register enters the proof (closing the
local half of the shadow/hardware gap `main` has today).

**Revert.** Restore the bare arrays and the boolean predicate; counters in P0 go unused but harmless
(they are read by the reader, not dead).

---

## Phase 2 — **SPINE-1: SIGKILL stops eager-freeing** *(#491's live UAF)*

**Scope.** Rewrite the SIGKILL arm at `syscall/signal.rs:162` to: validate under the existing PM
guard, capture `pid`, mutate **nothing**, `drop(guard)`; then
`with_scheduler(|s| s.terminate_process_threads(pid))`; then **broadcast** `SGI_RESCHEDULE` to every
other online CPU (no `cpu_state[].current_thread` residency predicate — that read is stale-prone and
was a fatal panel finding); then `with_process_manager(|pm| pm.exit_process(pid, -9))`, whose
merged aarch64 path already grace-defers the page table **before** `terminate()` runs. Keep the
existing `set_need_resched()` tail. Additionally install the **durable report/SIGCHLD obligation seed**:
one row work bit set at first commit, redeemed exactly once outside PM by whichever of
{commit path, `handle_thread_exit`} reaches it first — because `btrt::on_process_exit` has exactly one
call site inside `handle_thread_exit`, which a remotely-marked victim may never run.

**Files.** `kernel/src/syscall/signal.rs`, `kernel/src/process/manager.rs`,
`kernel/src/task/scheduler.rs`, `kernel/src/task/process_task.rs`, tests. **~180 lines, 3 commits.**

**Gate extras.** New `sigkill_teardown_test` (userspace): parent forks a child spinning at EL0;
parent `kill(child, SIGKILL)`. Assert (a) `waitpid` reaps **-9**; (b) SIGCHLD arrived at kill time
(parent's `pause()` returns); (c) `TEARDOWN_QUARANTINE`/`TEARDOWN_DEFER`/`TEARDOWN_RECLAIM` all
increment for that pid; (d) `TEARDOWN_MASKED_FRAMES_WALKED == 0`; (e) `EXIT_SGI_SENT > 0` and the
peer observes it; (f) exactly one `btrt` report; (g) zero fault markers over the 10-boot Parallels
streak. Repeat with the child inside a CLONE_VM group — the **sibling must survive** (this phase does
not sweep the group). Repeat as self-kill `kill(getpid(), SIGKILL)`. **Soak + retention measurement.**

**Strictly better.** The confirmed-live eager `cleanup_cow_frames`-while-remote-runs UAF class is
gone; quarantine, expedited reschedule, and SIGCHLD arrive for the first time on this path. *Honest
bound:* the exit is still remotely marked (upgraded in P8–P10), and FD closure still happens inside
`exit_process` under PM — unchanged from today's SIGKILL, which does that **plus** the full CoW walk.
Not a regression; not yet a fix. Fixed in P7.

**Revert.** Restore the four-line `process.terminate(-9)` arm and drop the work bit. The exact
pre-image is preserved in the commit body.

---

## Phase 3 — exec detach + clone/exec admission *(#471 part 1)*

**Scope.** At every exec commit point (after all fallible work, before PM release), set
`process.inherited_cr3 = None` and `process.thread_group_id = None` alongside
`process.page_table = Some(new_page_table)`; preserve both on **every** exec failure. Keep the
existing live-sibling guard. `sys_clone` validates the parent row is `Live` under the same PM
transaction that publishes the child row; user-thread creation publishes the scheduler thread
**non-runnable** until the row is published, and dispatch refuses `Creating` rows before arming TTBR0.

**Files.** `kernel/src/process/manager.rs`, `kernel/src/syscall/clone.rs`,
`kernel/src/task/scheduler.rs` (dispatch gate), `userspace/programs/src/clonevm_exec_test.rs`.
**~150 lines, 2 commits.**

**Gate extras.** Extended `clonevm_exec_test`: successful exec → both fields `None`, fresh root,
effective TGID == pid, and a kill aimed at the **old** group cannot reach it; failed exec → both
fields byte-identical to pre-exec. Futex behaviour across an exec verified explicitly (the group id
falls back to pid — `futex.rs` is the main consumer). Deterministic clone-vs-exec race.

**Strictly better.** Closes the wrong-victim-after-exec defect that was one of the four blockers
which killed PR #418's group sweep — *before* any group-scoped kill exists to trip over it.

**Revert.** Delete the two assignments + the admission check; ~15 lines.

---

## Phase 4 — Kernel-stack ownership parity for all three creation paths *(AC-8)*

**Scope.** Centralize "clone the process-side thread and take `kernel_stack_allocation` into the
scheduler copy", then apply it to the paths that never got it: fresh spawn (`creation.rs` ×2),
direct init, `boot/test_disk.rs`, alongside the already-fixed fork and clone. A `Process` row can no
longer synchronously drop a published stack — today the original thread's stack is freed ungated by
`remove_process` at `waitpid` reap. Drop PM before every scheduler registration (removing the existing
PM→scheduler nesting in the spawn/test-disk paths).

**Files.** `kernel/src/process/creation.rs`, `kernel/src/main_aarch64.rs`,
`kernel/src/boot/test_disk.rs`, `kernel/src/arch_impl/aarch64/syscall_entry.rs`,
`kernel/src/task/scheduler.rs`. **~150 lines, 2 commits.**

**Gate extras.** After **each** creation path, assert exactly one owner and that it is the scheduler
copy. 1000-iteration fork/clone/spawn exit stress with stack-pool accounting (allocated == freed) and
an allocator assertion that never selects a live slot. P0 ratchet extended: no
`kernel_stack_allocation` mutation outside creation paths and `reclaim_terminated_threads`.

**Strictly better.** Closes the third and last un-graced stack case; removes a PM→scheduler nesting.
*Honesty note:* the input package reports the spawn asymmetry as **fact, not a diagnosed bug** — this
phase closes it because uniformity is cheap and it is an AC, not because a UAF was demonstrated.

**Revert.** Per-site; each call site is independent.

---

## Phase 5 — Runtime init designation, identity ONLY *(#464 part 1 — no fatal behaviour)*

**Scope.** `ProcessManager::designated_init: Option<ProcessId>` as the single authority. **Reserve
PID 1** for the explicit init constructor; ordinary/test allocation starts at 2 (all four
`next_pid.fetch_add` sites). Init is built off-table with provisional PID 1 through a
**held-publication ticket**: fallible image → row inserted → scheduler thread created **not-yet-runnable**
→ ticket returned → PM validates the ticket names a live PID 1 → `designated_init` set → thread
published to the run queue. Production designation is **validated == PID 1 and refuses otherwise**.
Migrate the three production literals (`manager.rs:1178`, `process_task.rs:226`, `:285`) and
`signal.rs`'s `INIT_PID` to the accessor. **No `#[cfg]` anywhere.** `init_shell.rs:1028` is not
touched.

**Files.** `kernel/src/process/manager.rs`, `kernel/src/process/mod.rs`,
`kernel/src/main_aarch64.rs`, `kernel/src/syscall/signal.rs`, `kernel/src/task/process_task.rs`.
**~180 lines, 2 commits.**

**Gate extras.** Failure injection at **each** fallible stage after provisional PID selection:
`designated_init() == None`, no row, and a retry succeeds as PID 1. Boot test: designated pid == 1 ==
the pid `init_shell` observes via `getpid()`. A build with no real init leaves designation unset and
does **not** treat whichever process got a low PID as init. Existing orphan-reparent test still green.
P0 ratchet allowlist shrinks to the three named test sites.

**Strictly better.** Converts AC-5 from "convention plus a boot log line" into a structural guarantee,
and makes a failed init creation deterministically retryable. Ships **no** behaviour change on init
death — deliberately, because bundling identity with policy is what killed four prior attempts.

**Revert.** Restore the literals and the `next_pid` base; the ticket is additive.

---

## Phase 6 — Exactly-once ledger: durable work bits + first status *(AC-12)*

**Scope.** Generalize Phase 2's report-obligation seed into row-resident
`ExitWorkBits { sigchld, parent_wake, report, reparent, fds, resources }`, created at the **first**
request and cleared only on completion. A repeat request returns the stored batch/status and creates
no second obligation and no second status write. `ExitWorkBits.resources` subsumes design A's proposed
separate `ResourceState{Held,HandedOff}` field — "the page table has left this row" is one more durable
obligation, not a parallel key. Both already-terminated branches (`manager.rs:1137`,
`process_task.rs:234`) re-key onto the work bit.

**Files.** `kernel/src/process/process.rs`, `kernel/src/process/manager.rs`,
`kernel/src/task/process_task.rs`, tracing provider, tests. **~180 lines, 2 commits.**

**Gate extras.** Repeat matrix — exit→fault, SIGKILL→fault, fault→SIGKILL, repeated request/wait:
exactly one SIGCHLD, one parent wake, one `btrt` report, and `waitpid` returns the **first** status.
Equalities: `SIGCHLD_FIRST_SET == PARENT_WAKE_COMPLETED == BTRT_EXIT_REPORTED == parented_first_commits`.
CoW frame accounting unchanged vs P5 for the same workload.

**Strictly better.** Closes PR #418's own declared follow-up ("duplicate SIGCHLD / stale exit code on
the already-terminated path"). Explicitly **not** notification suppression (disproven in review): the
obligation is shared by every producer, so a later pass redeems it rather than being skipped.

**Revert.** Re-key the two branches onto `is_terminated()` and drop the bits.

---

## Phase 7 — FD closure leaves the PM lock *(AC-9)*

**Scope.** Add `FdTable::take_next_for_exit()` — take exactly **one** descriptor under PM, drop PM,
close it, repeat. Retire the existing allocating `Process::take_fd_entries() -> alloc::vec::Vec`
(`process.rs:335`) rather than reusing it. Apply at both convergence points and at Phase 2's SIGKILL
commit.

**Files.** `kernel/src/ipc/fd.rs`, `kernel/src/process/process.rs`,
`kernel/src/process/manager.rs`, `kernel/src/task/process_task.rs`, tests.
**~160 lines, 2 commits.**

**Gate extras.** `FD_CLOSES_UNDER_PM == 0`. 256-FD test with a large process set: measure and assert a
bounded PM hold. P0 ratchet forbids any close/reclaim call inside a request/commit transaction body.
Soak.

**Strictly better.** Removes the pipe/PTY/TCP endpoint locks *and* a heap allocation from under
PM+DAIF-masked on **every** exit path — a pre-existing violation on `main` that predates all of this
work, and the last one Phase 2 knowingly left standing.

**Revert.** Restore the `Vec` path; the one-at-a-time API is additive.

---

## Phase 8 — Victim-owned `do_exit_current`, normal exit as first consumer *(Tier-2; needs OQ-5)*

**Scope.** The exit trampoline and one-shot victim transaction: claim (atomic) → local TTBR0 leave
(hardware first, shadows after) → short PM commit (mark zombie, first status, take own FDs via P7,
decide last-reference, move root into a retirement receipt) → drop PM → all slow work unlocked
(one-FD closes, futex/`clear_child_tid`, bounded reparent cursor with PM dropped and DAIF restored
between batches, redeem the P6 notification receipt, enqueue the receipt) → pivot to neutral stack →
mark **only self** `Terminated` → schedule away. **Normal `exit(2)` is routed through it in this same
PR** — the API is never dormant. The boundary hook is an acquire-load + branch **after** the
`PREEMPT_ACTIVE`/nested-return gate; it allocates nothing, locks nothing, walks nothing, drains
nothing.

**Files.** new `kernel/src/task/teardown.rs`; `kernel/src/task/process_task.rs`,
`kernel/src/arch_impl/aarch64/context_switch.rs` (Tier-2), `kernel/src/arch_impl/aarch64/syscall_entry.rs`,
`kernel/src/process/manager.rs`. **~220 lines, 3 commits — at the ceiling; seam for splitting is
{trampoline + commit} / {boundary hook + normal-exit routing}.**

**Gate extras.** Repeated/nested exit injection produces one status, one SIGCHLD, one parent wake,
one report; FD closes and reclaim enqueue observed with PM unlocked. Disassembly review of the hook
proving no logging, allocation, page-table walk, or contended lock on the return tail (Tier-2
requirement). Soak. Frozen-region hashes unchanged (the hook is *outside* every frozen block).

**Strictly better.** The exit path becomes victim-owned for the most common death; a `Process` row
never drops a published stack; slow work leaves the masked section on every normal exit.

**Revert.** Route normal exit back to `handle_thread_exit`'s current two-phase body and delete the
hook; single-file revert plus the hook removal.

**Blocked on OQ-5.** Without Tier-2 approval the plan **stops at Phase 7**: #491 ships at
Increment-1 strength (a real UAF fix), #464 and #471 ship complete, and P8–P12 park.

---

## Phase 9 (a/b/c) — Killable-wait contract, one family per PR

**Scope.** 9a futex; 9b `WaitQueueHead` + stdin/TTY readers; 9c child-wait + timer/nanosleep +
completion/I-O. Each sub-phase: on a fatal request the victim is made **runnable with its saved
continuation and `blocked_in_syscall` intact**; the resumed continuation gives the latched fatal
request priority over ordinary success, unregisters itself through the existing `finish_wait`/
unregister path, then branches to the trampoline. **No `ExitPending` state** — dequeuing a victim
whose saved resume SP is still live evidence is the banked r20 finalization deadlock, and design C's
version of it was the panel's fatal finding against C.

**Each sub-phase immediately upgrades the already-live SIGKILL path for victims in that family** — so
nothing dormant lands, and the concrete acceptance test is "SIGKILL a victim blocked in *this*
family". The live futex loop is the named hazard: today an unrelated wake can take the success branch
before the signal check and leave the TID registered; the resumed loop must be reordered.

**Files (per sub-phase).** the family's own file(s) + `kernel/src/task/scheduler.rs` +
`kernel/src/task/teardown.rs` + its test. **~150 lines each, 1-2 commits each.**

**Gate extras.** Per family: inject an exit request and prove the registry/heap no longer contains the
TID **before** the exit commit; SIGKILL of a victim blocked in that family reaps the right status.
Families not yet migrated are **reported as uninterruptible** by a counter — never silently advertised
as killable.

**Strictly better.** Each family converts from "dies at its next natural boundary, whenever that is"
to "dies promptly, with its wait registration cleanly removed". Unmigrated families are unchanged, and
are visible.

**Revert.** Per family, independently.

---

## Phase 10 — Request-only scheduler termination + group-scope cutover *(#491 complete, #471 part 2)*

**Scope.** `terminate_process_threads` is redefined to **request and wake**: publish
`ThreadExitRequest` on each member's scheduler thread, ready proven-killable blockers (P9) with
continuations intact, **never** set a remote thread `Terminated`, return `ExitKickPlan`. Its old
remote-marking body is **deleted in this same PR** — no parallel API is left behind. SIGKILL and
`exit_group` become group-scoped `ExitIntent`s; the group-exit PM transaction marks every live
effective-TGID member with one batch id and *is* the seal (no snapshot ever leaves the lock);
`sys_clone` fails `EAGAIN` into a non-`Open` group. SGIs are sent after all locks are dropped.

**Files.** `kernel/src/task/scheduler.rs`, `kernel/src/syscall/signal.rs`,
`kernel/src/task/teardown.rs`, `kernel/src/process/manager.rs`, `kernel/src/syscall/clone.rs`.
**~220 lines, 3 commits — at the ceiling; seam is {request API + kick plan} / {group scope + seal}.**

**Gate extras.** Two-CPU aarch64: kill a thread running remotely at EL0 and prove **its own TID**
executes the exit commit, with zero post-request EL0 trace for the victim. Deterministic clone-vs-seal
barrier: the child is either in the batch or `sys_clone` returns `EAGAIN` — never a runnable
unrequested member. No resource claim before the batch commits. Blocked and userspace victims both
die. Soak.

**Strictly better.** #491 reaches full strength: nothing is torn down remotely; group membership is
atomic by construction. #471's seal ships.

**Revert.** Restore the remote-marking body and pid-scoped SIGKILL — i.e. fall back to Phase 2's
already-safe behaviour, not to `main`.

---

## Phase 11 — Fatal-signal and fault convergence *(the last direct `terminate` callers)*

**Scope.** `deliver_default_action` **returns a fatal intent** instead of mutating anything under the
PM borrow — delete both `process.terminate(...)` + `with_thread_mut(...)` blocks at
`signal/delivery.rs:224-239` and `:258-269` and carry `(pid, code, sig)` out through the existing
`DeliverResult::Terminated` channel, whose documented contract is already "caller MUST call notify
after releasing the PM lock". This single change kills three problems at once: the FD-closure loss
that design A's `terminate_minimal` hand-off would have caused, the retained
SCHEDULER-under-DAIF-masked-PM inversion (`with_thread_mut` takes SCHEDULER at `scheduler.rs:3101-3109`
while the PM guard is live), and a `log::info!` under mask. The four EL0 fault sites collapse to one
TID-attributed adapter (§2.4 of the design): TID first, stack-slot cross-check, CR3 as
root-consistency only, divergence counted and never fatal, `AttributionUncertain` → safe redirect,
mandatory tails always unconditional. `interrupts/context_switch.rs:1021` (x86_64) converges.
**`Process::terminate` is deleted** with its last caller.

**Files.** `kernel/src/signal/delivery.rs`, `kernel/src/arch_impl/aarch64/exception.rs`,
`kernel/src/arch_impl/aarch64/context_switch.rs`, `kernel/src/interrupts/context_switch.rs` (Tier-2),
`kernel/src/process/process.rs`. **~220 lines, 3 commits — split by architecture if it crosses.**

**Gate extras.** New `clonevm_fault_test`: a CLONE_VM child faults at EL0 → the **child** dies, the
parent survives, `TEARDOWN_VICTIM_DIVERGENCE == 1`, no refault loop. **This test fails on `main`
today** — it is a live wrong-victim livelock, so the test is meaningful rather than tautological.
`sigsegv_default_action_test`: status `-11` reaped exactly once, CoW accounting balanced,
`TEARDOWN_MASKED_FRAMES_WALKED == 0`. Explicit x86 regression run (two prior rounds shipped accidental
x86 divergences). P0 ratchet's `\.terminate\(` allowlist must now be **empty**, asserted as an exact
set. Soak.

**Strictly better.** Every death path in the kernel now runs the same state machine; the last eager
teardown-under-PM-borrow is gone; a live wrong-victim livelock is fixed.

**Revert.** Restore the two delivery blocks and the four inlined fault bodies (preserved in the commit
bodies); `Process::terminate` returns.

---

## Phase 12 — Init death policy *(#464 part 2 — separate PR by construction)*

**Scope.** Policy per OQ-1 (recommended: user-originated default-fatal signals to designated init →
`EPERM`, no group exit; init's own `exit`/`exit_group` or an unhandleable fatal fault → kernel-fatal).
S1 sets `INIT_DEATH_LATCH` with one relaxed store, **only** for a committed, certainly-attributed
victim. S5 reads it in ordinary kernel context with all guards out of scope and DAIF restored, records
a pre-panic lock/IRQ snapshot, then panics. **No `#[cfg]` gate** — designation is runtime data, so a
build that never designates an init can never trip the policy, and `interactive = ["testing"]` cannot
invert anything.

**Files.** `kernel/src/task/teardown.rs`, `kernel/src/process/manager.rs`,
`kernel/src/syscall/signal.rs`, tracing reader, tests. **~120 lines, 2 commits.**

**Gate extras.** Deliberate init-kill test asserts the panic message **and that the panic reports
completely** on serial (proving no lock was held), with the snapshot showing PM owner `None`,
scheduler owner `None`, normal IRQ state. `INIT_PANIC_WITH_LOCK == 0`. A normal boot asserts the latch
stays 0 for the whole run **including the `smoke_hello_time` harness** — the exact build all four
prior attempts broke. A test-build PID-1 process that is *not* designated exits normally.
`kill(1, SIGKILL)` from userspace returns `EPERM` and init survives.

**Strictly better.** Closes #464 with the runtime flag the issue asked for, on the fifth attempt,
without the cfg landmine that killed the first four.

**Revert.** Delete the latch + the escalation call; identity (P5) survives independently.

---

## 1. Cumulative outcome at each stop point

| Stop after | #491 | #464 | #471 | Net vs `main` |
|---|---|---|---|---|
| P0 | — | — | — | Bypass surface pinned; #492 overflow visible |
| P1 | — | — | — | + grace cannot elapse unordered/empty; refusals attributable |
| **P2** | **live UAF closed** (remote-mark strength) | — | — | + quarantine, expedite, SIGCHLD on kill |
| P3 | ↑ | — | **detach done** | + wrong-victim-after-exec impossible |
| P4 | ↑ | — | ↑ | + all three creation paths grace-protected |
| P5 | ↑ | **identity done** | ↑ | + AC-1/4/5 structural |
| P6 | ↑ | ↑ | ↑ | + exactly-once notification (#418's own follow-up) |
| P7 | ↑ | ↑ | ↑ | + no FD/endpoint lock or alloc under PM on any exit |
| P8 (needs OQ-5) | ↑ | ↑ | ↑ | + normal exit is victim-owned |
| P9a/b/c | ↑ | ↑ | ↑ | + each wait family becomes promptly killable |
| **P10** | **complete** | ↑ | **seal done** | + nothing torn down remotely; atomic membership |
| P11 | ↑ | ↑ | ↑ | + one machine for all five paths; live livelock fixed |
| P12 | ↑ | **complete** | ↑ | + init death policy without the cfg landmine |

Stopping anywhere is a shippable, defensible state. **P2, P3, P5, P10, P12 are the five points where
an issue's headline claim actually changes** — those are the PR bodies that must be written most
carefully, and the ones where an overstated commit message would be a blocking finding.

## 2. Merge discipline

Per operator standing rules: feature branch per phase → PR → **merge commit, never squash/rebase**
(preserve every SHA both directions) → verify `git log main..branch` empty post-merge → local back to
`main` → delete the merged branch → branch fresh for the next phase. Any reviewer note — minors and
nits included — is closed out before merge; approval alone is not merge clearance. Any defect the work
surfaces is this work's problem, including pre-existing ones, unless the operator explicitly rules it
held.

**Round-level stop rule (from the r22/r23 history):** if any phase draws blocking findings in **two
consecutive** fix rounds, or a fix round is net-negative (closes N, introduces ≥N), that is a
divergence signal — **hard stop and escalate to the operator** rather than a third round. The r23
round proved a third round on a diverging phase costs more than it closes.
