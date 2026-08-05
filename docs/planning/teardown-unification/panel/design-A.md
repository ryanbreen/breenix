# Teardown Unification — Design A: Minimal-Incremental

**Lens:** the smallest sequence of independently-shippable, independently-revertable steps that each
measurably reduce the bypass surface, reusing the merged #417/#418/#494 machinery rather than
introducing new concepts.
**Base:** `main` @ `eebc8868` (re-verified against the live tree, not against PR bodies).
**Scope:** #491 (spine), #464, #471. Acknowledges and does not foreclose #448, #492, #493.

---

## 0. Thesis

There is already exactly one hardened teardown machine in this kernel — `quiesce → quarantine →
grace-stamped defer → liveness-last reclaim` — and it is built from four **separately callable,
already-merged primitives**. The four bypassing death paths do not need a new machine; they need to
call those same four primitives, in the same order, from one place. This design therefore adds
**one small composition module** (`kernel/src/process/teardown.rs`) that is nothing but an ordered
call of the existing primitives, migrates each death path onto it one PR at a time, and spends the
remaining phases removing the three things that make the composition unsafe to apply universally
today: an implicit idempotence key (`is_terminated()` standing in for "resources already handed
off"), a compile-time init identity (a hardcoded `1`), and an un-detached thread group after `exec`.

Nothing here is a rewrite. Every phase is a diff a reviewer can hold in their head, each is
revertable without touching any other, and each leaves the tree shippable.

---

## 1. Architecture

### 1.1 The four primitives (all merged, all reused verbatim)

| # | Primitive | Location (live, `eebc8868`) | Context requirement |
|---|---|---|---|
| P-A | `quiesce_ttbr0_for_exit()` — install kernel root on **this** CPU, zero both per-CPU TTBR0 shadows | `kernel/src/arch_impl/aarch64/ttbr0.rs` (`quiesce_ttbr0_for_exit`, ~:40-47) | local CPU only, no arguments, no locks, no alloc, idempotent |
| P-B | `Scheduler::terminate_process_threads(owner_pid)` — mark every scheduler-owned thread of the victim `Terminated`, drain it from every per-CPU ready queue | `kernel/src/task/scheduler.rs` (~:2598-2614) | **scheduler lock**, must not be nested inside a DAIF-masked PM guard |
| P-C | `ProcessManager::exit_process(pid, code)` — unconditional grace-stamped `defer_process_resources` + `enqueue_process_reclaim` on aarch64, then `terminate()`, reparent, SIGCHLD | `kernel/src/process/manager.rs:1119-1213` | **PM lock**, DAIF masked |
| P-D | `reclaim_deferred_process_resources()` + `Scheduler::reclaim_terminated_threads()` — grace-first, liveness-last reclaim | `kernel/src/task/process_task.rs` (~:374-391), `kernel/src/task/scheduler.rs:952-1017`; drained at `syscall_entry.rs:932-933` (fork) and `context_switch.rs:4459-4462` (`schedule_from_kernel`) | IRQs on, no PM lock (fork site) / caller's mask (schedule site) |

### 1.2 The unified state machine

Every death — normal exit, EL0 fault, SIGKILL, default-fatal signal, init death, thread-group
member death — passes through the same five stages. The only per-path variance is **Stage 0** and
one named, justified variance in Stage 1.

```
 Stage 0  IDENTIFY   path-specific: who is dying, with what status, with certainty
             │
 Stage 1  DETACH     P-A on THIS CPU iff this CPU is executing in the victim's address space
             │       (a remote victim's CPU cannot be reached: see §1.3)
 Stage 2  QUARANTINE P-B under the scheduler lock, then EXPEDITE:
             │       SGI_RESCHEDULE to every peer CPU currently running a victim-owned thread
             │
 Stage 3  COMMIT     P-C under the PM lock: take page_table + pending_old_page_tables into a
             │       grace-stamped PendingProcessReclaim, mark Terminated, reparent, SIGCHLD
             │
 Stage 4  RECLAIM    P-D at the two existing drain sites: two-epoch grace elapsed AND
                     !root_is_live (grace first, short-circuit) → cleanup_cow_page_table + free
             │
 Stage 5  ESCALATE   post-commit, NO locks held, DAIF restored: one-shot init-death policy check
```

`teardown.rs` exposes exactly three entry points, all taking `pid: ProcessId` (never `&mut Process`,
which is what forces every caller to have already dropped the PM guard):

```rust
// kernel/src/process/teardown.rs
pub(crate) enum ExitCause { Exit, Fault, Signal(u32), GroupMember }

/// Stages 1-3 + 5 for a victim that IS the thread running on this CPU.
pub(crate) fn exit_current(pid: ProcessId, code: i32, cause: ExitCause) -> ExitDisposition;

/// Stages 1(no-op)-3 + 5 for a victim that may be running on a PEER CPU.
pub(crate) fn kill_remote(pid: ProcessId, code: i32, cause: ExitCause) -> ExitDisposition;

/// Stage 2 only — for callers that must quarantine before they can commit (drain sites).
pub(crate) fn quarantine_and_expedite(pid: ProcessId);
```

`ExitDisposition` is `#[must_use]` and reports `Committed | AlreadyCommitted | NotFound` for
diagnostics only. **No caller branches on it for control flow** — this is grave-spec R8's rule
adopted, because two of the parked branch's blocking findings (r20 #1, "five silent early-return
paths", and R2-11, "an `Option` return silently disabled the idle redirect") were both "a mandatory
tail got skipped based on a return value". Here the mandatory tails (frame redirect, `switch_to_idle`)
stay at the call site and run unconditionally, exactly as they do on `main` today.

### 1.3 The one justified variance: you cannot quiesce a remote CPU

`quiesce_ttbr0_for_exit()` is local-only by construction, and the parked spec's conflict resolution
R2 (adopted on `main` already) is explicit that giving it a pid argument is **strictly worse** —
it reintroduces a refusal path that can leave a retired root installed. AArch64 has no remote
`mrs`/`msr` of another CPU's `TTBR0_EL1`.

So for a remote victim, Stage 1 is a no-op and its safety obligation is discharged by Stages 2+4
instead:

- Stage 2 makes the victim non-runnable **before** Stage 3 hands its resources off, and the SGI
  forces any peer currently executing that victim off EL0 promptly rather than at its next natural
  tick.
- Stage 4 will not free anything until (a) every online CPU has advanced two scheduling epochs past
  the stamp — which requires each CPU to have re-entered the scheduler twice, i.e. to have left
  both whatever it was doing at stamp time and whatever it picked up next — **and** (b) no online
  CPU's TTBR0 shadow still names the root.

That is the same proof the fault path already relies on for *peer* CPUs (the fault path's local
quiesce only covers the faulting CPU; peers are covered by grace+liveness). The remote-kill case is
therefore not a weaker mechanism — it is **the identical mechanism minus a local optimisation that
is inapplicable**. This is the "structural equivalent, named" the charter asks for.

### 1.4 What this replaces, per path

| Path | Today | After |
|---|---|---|
| normal exit (`sys_exit`/`exit_group`) | own Stage 1 inline; **liveness-conditional** defer (`defer_live_process_resources`); no Stage 2 | Stage 1 inline (unchanged, already correct); Stage 3 defer made **unconditional** (P5) so Stage 3 is byte-identical across paths |
| EL0 fault (4 sites) | full machine, hand-inlined 4× | calls `teardown::exit_current` (P1) — 4 copies → 1 |
| SIGKILL | `Process::terminate(-9)` under PM+mask: eager `close_all_fds` + eager `cleanup_cow_frames` while the victim may be at EL0 on a peer | `teardown::kill_remote` (P3) |
| default-fatal signal (SIGSEGV/SIGTERM/… via `deliver_default_action`) | `Process::terminate(code)` under PM+mask, **with the victim's root installed in TTBR0 on this CPU** | `terminate_minimal` under the lock + `teardown::exit_current` after the guard drops (P4) |
| init death | indistinguishable from any other exit; three hardcoded `ProcessId::new(1)` reparent literals | designated-init runtime flag (P7) + one-shot lock-free escalation at Stage 5 (P8) |
| CLONE_VM group | no group concept at teardown; `exec` never detaches `thread_group_id`/`inherited_cr3` | `exec` detach (P9), group seal + atomic snapshot (P10), optional sweep built from `teardown::kill_remote` (P11) |

---

## 2. Per-issue mechanism

### 2.1 #491 — SIGKILL routing (the spine)

**Defect, re-confirmed live** at `kernel/src/syscall/signal.rs:157-170`: the SIGKILL arm calls
`process.terminate(-9)` while holding the PM guard (`manager()`, all DAIF bits masked). That reaches
`close_all_fds()` (pipe/PTY/TCP locks under mask) and `cleanup_cow_frames()` →
`frame_decref`/`deallocate_frame` (blocking `FRAME_METADATA`) while the victim may still be executing
at EL0 on a peer CPU. No quiesce, no quarantine, no grace deferral, no SGI.

**Mechanism (Phase 3).** Replace the arm's body with:

1. **Under the existing PM guard**: validate only — the row exists and is not terminated (this check
   already exists at `signal.rs:150-154`). Capture `pid`. Perform **no** mutation of the victim's
   resources. `drop(manager_guard)` explicitly (the function already contains a precedent for this
   drop-before-scheduler pattern at `signal.rs:220`).
2. **`teardown::kill_remote(pid, -9, ExitCause::Signal(SIGKILL))`**:
   - Stage 1: no-op (`kill_remote` never touches TTBR0 — the victim is by definition not this CPU's
     current process; if it *is* — `kill(getpid(), SIGKILL)` — the caller path is the syscall return
     path, which reinstalls the root from the shadows, and Stage 3's defer plus Stage 4's liveness
     gate cover it; see Residual R-4).
   - Stage 2: `with_scheduler(|s| { s.terminate_process_threads(pid.as_u64()); s.expedite_process_threads(pid.as_u64()); })`.
   - Stage 3: `with_process_manager(|pm| pm.exit_process(pid, -9))`.
   - Stage 5: init-death escalation check (after P8).
3. Keep the existing `set_need_resched()` tail.

**What each of #491's named corners gets:**

- *UAF class*: Stage 3 takes the page table into a `PendingProcessReclaim` **before** `terminate()`
  runs, so `terminate()`'s `cleanup_cow_frames()` walks a `None` page table — a no-op, exactly as the
  comment at `manager.rs:1156-1160` already documents for the fault path. The CoW decref now happens
  in `PendingProcessReclaim::reclaim()`, behind grace + `!root_is_live()`. **This is a strict
  reduction in work done under PM+mask, not an addition**: today's SIGKILL does the whole CoW walk
  there.
- *Retained clone kernel stack*: Stage 2 marks the scheduler-owned thread `Terminated`, so
  `reclaim_terminated_threads()` picks it up under grace+liveness. Before this, a SIGKILL'd clone's
  scheduler thread never reached `Terminated`, so PR #494's `a2aa4359` stack transfer had no
  reclaimer.
- *No SIGCHLD at kill time*: `exit_process` sends SIGCHLD unconditionally (`manager.rs:1201-1212`),
  so routing through it fixes this with no new code.
- *Expedite*: new `Scheduler::expedite_process_threads(owner_pid)` — under the scheduler lock, for
  each `cpu != current` that is online, if `cpu_state[cpu].current_thread` names a thread whose
  `owner_pid == Some(owner_pid)`, `gic::send_sgi(SGI_RESCHEDULE, cpu)`. This is a copy of the
  existing, proven shape at `scheduler.rs:1867-1889` (`send_resched_ipi_to_cpu`) with a different
  predicate — no new IPI, no new GIC configuration, no touch of the frozen `init_gicv3_redistributor`
  SGI-enable block.

`send_signal_to_all_processes` / `send_signal_to_caller_process_group` reach SIGKILL only through
`send_signal_to_process`, so they are fixed by the same change (verified: `grep -n SIGKILL
kernel/src/syscall/signal.rs` shows one termination site, `:157`).

### 2.2 #464 — init identity + death policy

Split deliberately into two shippable phases, because four prior attempts died by bundling
"who is init" with "what happens when init dies".

**P7 — identity only (no fatal behavior).** New `kernel/src/process/init_identity.rs`:

```rust
static DESIGNATED_INIT: AtomicU64 = AtomicU64::new(0);   // 0 == none designated

pub fn designate_init(pid: ProcessId);   // CAS 0 -> pid, one-shot; ignores a second call
pub fn init_pid() -> Option<ProcessId>;  // None before designation
pub fn is_designated_init(pid: ProcessId) -> bool;
pub fn reparent_target() -> Option<ProcessId>;  // == init_pid(), named for intent
```

- **Set once, after success only** (AC #1): the single call site is the boot init spawn — on aarch64
  `kernel/src/main_aarch64.rs:~115-122`, immediately after
  `manager.create_process_with_argv(...)?` returns `Ok(pid)` and after the PM guard is dropped.
  `create_process_with_argv` allocates the pid from `next_pid` *before* the fallible page-table and
  ELF steps (`manager.rs:~612` vs `:622-640`), so designating anywhere earlier could name a pid that
  never gets a row. Designation happens with interrupts still disabled around the boot spawn and
  before the first ERET to userspace, so no process can observe the un-designated window.
- **One source of truth** (AC #4): the three production literals — `manager.rs:1178`,
  `process_task.rs:226`, `process_task.rs:285` — become `init_identity::reparent_target()`.
  When it returns `None` (no init designated, e.g. a pure boot-test build), reparenting is skipped
  and orphans keep `parent = None`; `sys_getppid` already returns 1 as its fallback
  (`handlers.rs:2691-2699`), so userspace-visible behavior for that case is unchanged. The three
  `test_userspace.rs` sites are test-only setup, not teardown, and stay literal (explicitly
  allowlisted by the P0 ratchet test rather than silently ignored).
- **Kernel/userspace agreement** (AC #5): the Linux analogue is exact — Linux's kernel-side test is
  a runtime pointer (`task_pid_ns(p)->child_reaper == p`) while userspace keys on `getpid() == 1`,
  and they agree because init *is* pid 1. We adopt that: `init_shell.rs:1028`'s `getpid() == 1`
  guard is **left unchanged**, and `designate_init` emits a one-time coherence report at the boot
  site (no locks held, IRQs disabled by the boot path, `log::info!` legal there) stating the
  designated pid; if the designated pid is ever `!= 1`, that log line is the loud signal that the
  userspace contract has drifted. See OQ-2 for the stronger variant.

**P8 — death policy, without a panic under a lock (AC #2, AC #3).**

- At Stage 3, inside `exit_process`, when the committed pid `is_designated_init` and this pass took
  the *live* branch (i.e. the row existed and was not already terminated — attribution is certain by
  construction, not heuristic), do exactly one thing: `INIT_DIED.store(code, Relaxed)` plus
  `trace_count!(TEARDOWN_INIT_DEATH)`. One relaxed atomic store. No allocation, no logging, no lock,
  no panic under the DAIF-masked PM guard.
- At Stage 5 — in `teardown.rs`, *after* the PM guard and scheduler guard are both out of scope and
  DAIF is restored — `escalate_init_death_if_flagged()` reads the flag once and panics with the
  recorded status. Because every death path now funnels through `teardown.rs`, this check has exactly
  one home; the panic handler can take its own locks safely because none of ours are held.
- **No `#[cfg]` gate anywhere.** The four prior attempts all died on `interactive = ["testing"]`
  making feature-based scoping backwards. Designation is *data*: a test build that never calls
  `designate_init` can never trip the policy, and a build that does designate an init *should* trip
  it. This is the "real runtime flag" #464 asks for, and it removes the landmine rather than
  re-scoping around it.
- The fault path's `find_process_by_cr3_mut` heuristic can miss (returns `None` → no commit → no
  flag). A miss therefore **cannot** escalate. This is the AC #3 direction that PR #494's
  "misattributed panic escalation" finding named.

### 2.3 #471 — group seal + exec detach

**P9 — exec detach (AC #6).** At each exec commit point — the statement block that assigns
`process.page_table = Some(new_page_table)` after every fallible operation has succeeded:
`manager.rs:3215` (aarch64 `exec_process_with_argv`), and the sibling paths at `:2621`, `:2944`,
`:3492` — add two lines immediately adjacent:

```rust
process.page_table = Some(new_page_table);
// exec starts a fresh thread group: this row no longer shares any address space.
process.thread_group_id = None;
process.inherited_cr3 = None;
```

Safety at that instant is already established by the pre-existing guard: exec refuses outright while
`find_live_clone_vm_sibling_holding_cr3` finds a live sibling naming the old root
(`manager.rs:46-77`, called at `:3065-3076`). Two directions to state explicitly:

- The exec'ing row *owned* the old root (`page_table` was `Some`): the guard proved no live sibling
  names it; `page_table.take()` at `manager.rs:~3193` retires it into `pending_old_page_tables` on
  the normal path. Detaching the group id afterwards cannot orphan anyone.
- The exec'ing row was a *CLONE_VM child* (`page_table == None`, root from `inherited_cr3`):
  `take()` yields `None`, the owner keeps its root, and clearing `inherited_cr3` detaches this row
  from an address space it never owned. No free, no refcount change.

`thread_group_id`'s only other consumers are futex keying (`futex.rs:29-35`, `handlers.rs:149`) and
`sys_exit_aarch64`'s `clear_child_tid` wake (`syscall_entry.rs:338`) — all of which fall back to
`pid` when it is `None`, which is the correct post-exec semantics (a single-member group of one).

**P10 — seal + atomic snapshot (AC #7, AC #9).** Two pieces:

- **Seal**: `sys_clone` (`clone.rs:73-90`) refuses (`-ESRCH`) to join a group whose leader row is
  terminated or whose resources have already been handed off (the P2 `ResourceState`). Membership
  therefore cannot grow after any member's Stage 3 commit — which is precisely the race PR #418's
  round-4 review named ("the `owner_pids` snapshot could go stale across a lock drop mid-sweep").
- **Atomic snapshot**: `ProcessManager::snapshot_thread_group(tg_id, &mut [ProcessId; MAX_TG]) -> usize`
  fills a caller-provided fixed-capacity array in **one** PM hold, no allocation, no lock drop, no
  logging. Overflow past `MAX_TG` is reported by the return value (saturating) and bumps a counter —
  it is never silently truncated.

The **sweep itself** (P11) is then a loop over the snapshot calling `teardown::kill_remote(member,
code, ExitCause::GroupMember)` — one member per short, separate PM transaction, so no N-member FD or
resource loop ever runs inside a single masked critical section (AC #9), and sibling kernel stacks are
never dropped by the sweep: it only marks scheduler threads `Terminated` (P-B) and lets
`reclaim_terminated_threads` free them behind grace+liveness (AC #8). **P11 is explicitly optional
for this round** — P9 + P10 are what #471 asks for, and they are what make a future sweep safe.

### 2.4 Two enabling changes the routing depends on

**(a) `ResourceState` — make the idempotence key explicit (P2).** Both convergence points key
their "already torn down" branch on `process.is_terminated()`:

- `manager.rs:1137-1143` — raw-drops `page_table`/`stack`/`pending_old_page_tables` with no CoW walk.
- `process_task.rs:234-240` — same.

That is correct **today** only because the sole way to become `Terminated` before reaching them is
`Process::terminate()`, which already walked CoW. The moment any path marks `Terminated` *without*
walking CoW — which is exactly what P4 needs to do — the same branch silently leaks every CoW
refcount in the address space. So P2 adds one field and makes the invariant explicit:

```rust
// kernel/src/process/process.rs
pub enum ResourceState { Held, HandedOff }   // HandedOff == page table has left this row
pub resources: ResourceState,               // starts Held
```

`HandedOff` is set at exactly three places, each of which is where the page table actually leaves the
row: `release_process_resources`, `defer_process_resources`, and `Process::terminate` (whose
`cleanup_cow_frames` is the walk). Both already-terminated branches then key on
`resources == HandedOff` instead of `is_terminated()`. **Behavior is identical on the day it lands**
(the two predicates agree for every reachable state on `main`), which is what makes it a safe,
independently-shippable, revertable phase — and it converts a latent trap into a compiler-visible
one for every later phase.

**(b) Fault-victim attribution by faulting thread, not by CR3 owner (P1).** `find_process_by_cr3_mut`
(`manager.rs:1313-1335`) matches only rows whose **own** `page_table` root equals the CR3 — a
CLONE_VM sibling row has `page_table == None` and holds the root in `inherited_cr3`, so it is never
matched. A clone thread faulting at EL0 therefore resolves to the *owner* row: the kernel kills the
parent and leaves the actual faulter runnable, which refaults — the exact livelock the parked
branch's r20 review found (finding #2) and which is **live on `main` today**, independent of that
branch.

`teardown::exit_current` resolves the victim as:

1. `scheduler::current_thread_id()` → that thread's `owner_pid` (authoritative for "who was
   executing"; sound at all four EL0 sites, where the current thread *is* the faulter — unlike the
   EL1-fault fallback path, which is why `defer_current_user_thread_sigsegv_exit` correctly uses
   stack-slot resolution there and keeps doing so).
2. Cross-check against `find_process_by_cr3_mut(ttbr0)`. On disagreement: prefer the thread-derived
   pid, `trace_count!(TEARDOWN_VICTIM_DIVERGENCE)`, and **never** escalate fatally on the
   disagreement (AC #3).
3. If (1) yields nothing, fall back to (2). If both fail, return `NotFound` — and the call site's
   mandatory tail (frame redirect to `idle_loop_arm64`, `set_idle_stack_for_eret`, `switch_to_idle`)
   still runs unconditionally, so a resolution miss can never produce the r20 "ERET back into the
   faulting instruction forever" livelock.

---

## 3. Numbered traceability — all 13 acceptance criteria

| # | Criterion | Mechanism in this design | Phase | Evidence it holds |
|---|---|---|---|---|
| **1** | Init designation only after creation fully succeeds — no phantom PIDs | `designate_init(pid)` is called at **one** site, the boot init spawn, after `create_process_with_argv` returns `Ok(pid)` (which means `insert_process` has already run) and after the PM guard drops. `AtomicU64` CAS from 0, one-shot. Nothing in `create_*` touches it, so the fallible page-table (`manager.rs:~622-631`) and ELF (`:637-640`) `?`-returns cannot leave a designation behind. | P7 | Boot test asserts `init_pid() == Some(1)` after init spawn and `init_pid() == None` in a build with no init (boot-test-only kernel). |
| **2** | No panic/fatal action while the PM lock is held with DAIF masked | Stage 3 does **one relaxed atomic store** for init death and nothing else. The panic lives in `escalate_init_death_if_flagged()`, called from Stage 5 in `teardown.rs` where no PM guard and no scheduler guard are in scope and DAIF is restored. No `#[cfg]`, so no build carries a differently-scoped variant. | P8 | P0 ratchet test: `panic!`/`unwrap()`/`expect(` must not appear inside `exit_process`, `handle_thread_exit`, or any `teardown.rs` function that takes a guard. Reviewed by reading the single escalation call site. |
| **3** | Victim attribution certain before fatal escalation; a heuristic CR3 miss must not panic | Escalation triggers only on a pid that `exit_process` **committed** (row found in `self.processes`, live branch taken) — never on a resolution attempt. Fault-site resolution moves to faulting-thread → `owner_pid`, with CR3 as cross-check and a divergence counter; disagreement is never fatal. A total resolution miss returns `NotFound` and the call site's mandatory tail still runs. | P1 (attribution), P8 (escalation) | Regression test: CLONE_VM child faults at EL0 → the *child* row dies, the parent survives, no refault loop (this is broken on `main` today, so the test is meaningful, not a tautology). |
| **4** | Reparent-target coherence — one source of truth, no hardcoded PID 1 alongside runtime designation | All three production literals (`manager.rs:1178`, `process_task.rs:226`, `process_task.rs:285`) become `init_identity::reparent_target()`. `None` ⇒ skip reparenting (orphans keep `parent = None`; `sys_getppid` already answers 1 for that case). The three `test_userspace.rs` literals are process-*setup*, not teardown, and are explicitly allowlisted by name in the ratchet test. | P7 | P0 ratchet test fails on any new `ProcessId::new(1)` in `manager.rs`, `process_task.rs`, `signal.rs`, `wait.rs`, `teardown.rs`; the `INIT_PID: u64 = 1` constant at `signal.rs:26` is migrated in the same phase. |
| **5** | Kernel and userspace init guards must agree (`init_shell` keys on `getpid()==1`) | Adopt the Linux shape exactly: kernel keeps a runtime designation (`child_reaper` analogue), userspace keeps `getpid() == 1`, and they agree because the designated process *is* pid 1. `designate_init` logs the designated pid once at the boot site (no locks held) so a drift is loud, and the boot test asserts the two agree. `init_shell.rs:1028` is **not** changed — no second contract is introduced that could drift. Stronger variant in OQ-2. | P7 | Boot test: designated pid == 1 == the pid `init_shell` observes via `getpid()` (asserted from the shell's own startup line in serial output). |
| **6** | exec detaches `thread_group_id` **and** `inherited_cr3` | Two assignments adjacent to `process.page_table = Some(new_page_table)` at all four exec commit points (`manager.rs:3215` aarch64, plus `:2621`, `:2944`, `:3492`), i.e. after every fallible step succeeded. Both CLONE_VM directions analysed in §2.3. Both arches changed in the same commit (no cross-arch divergence). | P9 | `clonevm_exec_test` extended: after a CLONE_VM child execs, its `thread_group_id` is its own pid and a group snapshot of the *old* tg id no longer contains it. |
| **7** | Group membership examined atomically — no snapshot stale across a PM-lock drop | `snapshot_thread_group(tg, &mut [ProcessId; MAX_TG]) -> usize` fills a caller-owned fixed array inside **one** PM hold: no allocation, no lock drop, no logging inside the walk. Independently, `sys_clone` refuses to join a group whose leader has committed (the seal), so membership cannot grow after teardown begins — the snapshot is stable by construction, not merely by timing. | P10 | Stress test: N clones racing a group kill; assert every member either never existed or reached `Terminated`, and `GROUP_SNAPSHOT_OVERFLOW == 0`. |
| **8** | Sibling kernel stacks freed ONLY behind the two-epoch grace via scheduler ownership | fork (`syscall_entry.rs:961`) and clone (`clone.rs:247-254`) already transfer `kernel_stack_allocation` to the scheduler-owned copy. P6 applies the **same transfer to the spawn path** — `creation.rs:85`, `creation.rs:202`, `syscall_entry.rs:1630`, `boot/test_disk.rs:263` switch from `get_process(pid)` + `main_thread.clone()` to `get_process_mut(pid)` + `kernel_stack_allocation.take()` — closing the third case, where the stack is today freed ungated by `remove_process` (`manager.rs:1102`) at `waitpid` reap (`wait.rs:386`). No teardown path (including any future sweep) ever drops a stack directly; all of them only mark `Terminated` and let `reclaim_terminated_threads` free under grace + `!is_kernel_stack_slot_live`. | P6 (spawn), P3/P4/P10 (never drop directly) | P0 ratchet: no `kernel_stack_allocation` mutation outside creation paths and `reclaim_terminated_threads`. Soak test watches for stack-pool exhaustion and for the reuse-while-live signature. |
| **9** | No N-member FD/resource teardown loop inside one PM-locked, IRQ-masked section in fault context | `teardown.rs`'s contract: every entry point takes `pid` (never `&mut Process`) and commits **one** victim per PM transaction. The group sweep loops *outside* the lock, re-acquiring PM per member. Nothing in Stage 2 or Stage 5 touches PM. **Net reduction, measured:** SIGKILL today performs a full CoW walk *plus* `close_all_fds` under PM+mask; after P3 the CoW walk is a `None`-walk (zero frames) because the defer took the page table first. | P3, P4, P10 | Counter `TEARDOWN_MASKED_FRAMES_WALKED` (incremented in `cleanup_cow_frames` when reached under a PM guard) must be 0 for aarch64 kill paths in the boot test. |
| **10** | No eager `cleanup_cow_frames` while the victim may run on another CPU — all kill paths grace-defer | The two remaining production `Process::terminate()` callers outside `exit_process` — `signal.rs:162` (P3) and `delivery.rs:224`/`:258` (P4) — are routed through `teardown`, which commits via `exit_process`, which takes the page table into a grace-stamped `PendingProcessReclaim` *before* `terminate()` runs. `interrupts/context_switch.rs:1021` is x86_64-only and is migrated in P4 for arch parity (x86 keeps `release_process_resources`, its existing synchronous path). Reclaim then requires grace ∧ `!root_is_live()`. | P3, P4 | P0 ratchet allowlist of `\.terminate\(` call sites shrinks phase-by-phase to `{manager.rs:1161}` only; the test asserts the exact set, so a new bypass fails CI. |
| **11** | Killed threads quiesced in the scheduler AND expedited with the existing `SGI_RESCHEDULE` | Stage 2: `terminate_process_threads(pid)` (existing, proven on the fault path) followed by new `expedite_process_threads(pid)`, which sends `SGI_RESCHEDULE` (`constants.rs:85`) via `gic::send_sgi` to each peer CPU whose `cpu_state[cpu].current_thread` is victim-owned. Same shape as the existing `send_resched_ipi_to_cpu` (`scheduler.rs:1867-1889`); the frozen `init_gicv3_redistributor` SGI-enable block is untouched. | P3 | Test: victim spinning at EL0 pinned on a peer CPU; assert time-from-`kill`-to-`Terminated` is bounded by one SGI round trip, not by the victim's next timer tick (counter `TEARDOWN_EXPEDITE_SGI_SENT` > 0). |
| **12** | Exactly-once SIGCHLD/wake/report with **first-recorded** exit status, idempotent under repeat passes | Status: `terminate`/`terminate_minimal` both early-return on already-`Terminated` (`process.rs:284-291`, `:320-323`), so `exit_code` is first-write-wins; `handle_thread_exit` already reports `exit_code.unwrap_or(param)` (PR #494 `6c3cf1be`) and `exit_process`'s already-terminated branch never writes a status. P2's `ResourceState` makes the resource half one-shot for the same reason (`HandedOff` is never unset). SIGCHLD is a pending-bit set (idempotent); the parent wake is a no-op on a non-blocked thread. `btrt::on_process_exit` keeps firing on every pass — the lessons register records that suppressing it was *disproven* (single call site; a repeat pass can be the parent's only notification). **"Idempotent" here means the reported value is stable, not that later firings are suppressed.** | P2 (+ argument) | Test: SIGKILL a process, then force a fault against the same pid; assert exactly one `-9` reaped by `waitpid` and no `-11` anywhere, across repeat passes. |
| **13** | New reclaim/drain work respects lock ordering and is bounded on idle paths without throttling fork's drain | This design **adds no drain and changes no cap**. The two existing drain sites (`syscall_entry.rs:932-933` fork, IRQs on/no PM; `context_switch.rs:4459-4462` schedule) keep their exact current unbounded-vs-caller-mask behavior, so fork's full-drain guarantee is untouched and no cap is shared across sweeps. The one new scheduler critical section (Stage 2) is entered with **no PM guard live** — the reverse of the r23 finding's `sys_clone` pre-drain inversion. No `FRAME_METADATA` acquisition and no `log::*` is added under any mask. Full analysis: §4. | all | P0 counter `TEARDOWN_LOCK_ORDER_SUSPECT` (Stage 2 entered while `PROCESS_MANAGER_OWNER_TID` names this thread) asserted 0 by every boot test; see §4.4 on why this is a detector, not a guarantee. |

---

## 4. Lock-ordering analysis

### 4.1 The lock inventory and the established order

| Lock | Acquisition discipline on aarch64 | Notes |
|---|---|---|
| PM (`PROCESS_MANAGER`) | `manager()` masks **all** DAIF (`msr daifset, #0xf`) then takes the spin mutex (`process/mod.rs:125-140`) | `with_process_manager` uses `without_interrupts` |
| SCHEDULER | `with_scheduler` / `lock_for_context_switch` | |
| `PENDING_PROCESS_RECLAIMS` | inside `arch_without_interrupts` | leaf |
| `FRAME_METADATA` | blocking `spin::Mutex`, reached from CoW-fault context with IRQs masked **and** from teardown | leaf-ish, but see R-1 |
| FRAME_ALLOCATOR | under `FRAME_METADATA` in decref paths | leaf |
| SERIAL / framebuffer | any `log::*` | forbidden under PM+mask |
| pipe / PTY / TCP | `close_all_fds` | today reached under PM+mask (pre-existing) |
| heap | any `Vec`/`String` alloc | today reached under PM+mask (pre-existing, F12/R5) |

**Established order:** `PM → (drop) → SCHEDULER`. Never SCHEDULER nested inside a DAIF-masked PM
guard — this is the exact inversion that got three bounded-drain designs reverted in the r23 round.

### 4.2 Every new or moved critical section

| Site | Locks taken | Order relative to established | Alloc? | Log? | Bounded? |
|---|---|---|---|---|---|
| `teardown::quarantine_and_expedite` (Stage 2) — **new** | SCHEDULER only | Entered with **no PM guard live**, by construction: all `teardown` entry points take `pid: ProcessId`, so no caller can hold a `&mut Process` borrow (and therefore the guard) across the call | none — `terminate_process_threads` iterates in place; `expedite` writes GIC MMIO | none | O(threads) mark + O(queues×threads) retain — identical to the fault path's existing cost, no new bound needed |
| `Scheduler::expedite_process_threads` — **new**, inside Stage 2's scheduler lock | none beyond SCHEDULER (GIC MMIO is register writes) | Same nesting as the existing `send_resched_ipi_to_cpu`/`send_resched_ipi`, which are already called with the scheduler lock held | none | none | ≤ `MAX_CPUS` iterations, ≤ 8 MMIO writes |
| `teardown::commit` (Stage 3) → `exit_process` — **moved caller, unchanged body** | PM (DAIF masked) | Unchanged; SIGKILL and fatal-signal callers now *drop* their guard first and re-enter via `with_process_manager`, which strictly shortens the previous single long hold | unchanged (pre-existing `children.clone()`, `name.clone()`) | unchanged (pre-existing `log::info!` at `:1121`) | one victim per transaction, always |
| `Process::terminate` reached from Stage 3 | pipe/PTY/TCP (FD close); `FRAME_METADATA` **only if** `page_table` is `Some` | Unchanged from today. On aarch64 the page table is `None` by then (defer ran first), so `cleanup_cow_frames` is a zero-frame walk and `FRAME_METADATA` is never taken — a **removal** of a masked `FRAME_METADATA` acquisition on the SIGKILL and fatal-signal paths | unchanged | unchanged | FD count, unchanged |
| `escalate_init_death_if_flagged` (Stage 5) — **new** | none | Runs after both guards are out of scope, DAIF restored — the same sanctioned region as `handle_thread_exit`'s phase 2 | none (panic path allocates, which is legal here) | panic only | one atomic load |
| `snapshot_thread_group` (P10) — **new** | PM (DAIF masked) | Single hold, no nesting, no drop mid-walk | none — caller-provided `[ProcessId; MAX_TG]` | none | ≤ `MAX_TG`, saturating with an overflow counter |
| `sys_clone` seal check (P10) — **moved predicate** | PM, already held by `sys_clone` | No new acquisition; a field read inside an existing hold | none | none | O(1) |
| P2 `ResourceState` writes/reads | inside existing PM holds | No new acquisition | none | none | O(1) |

### 4.3 Ordering hazards explicitly avoided

- **SCHEDULER inside DAIF-masked PM** (the r23 revert class): impossible for Stage 2 because the API
  is pid-based. The SIGKILL path's `drop(manager_guard)` before `with_scheduler` mirrors the pattern
  already present in the same function at `signal.rs:220`.
- **PM inside SCHEDULER**: `teardown` never calls Stage 3 from inside a `with_scheduler` closure;
  Stage 2 and Stage 3 are sequential statements, not nested.
- **`FRAME_METADATA` under PM+mask**: not added anywhere; removed on two paths (§4.2 row 4).
- **New heap allocation under PM+mask**: none. `snapshot_thread_group` is deliberately array-based
  precisely to avoid the `Vec` that PR #418's stripped sweep used.
- **`log::*` under mask**: none added. Stage 2 and Stage 5 diagnostics use `trace_count!` /
  `trace_event!` from the lock-free tracing framework (`kernel/src/tracing/`), which is the
  sanctioned instrument per the project's debugging discipline; `raw_serial_char()` remains
  available for the fault sites that already use it.

### 4.4 Honest limit of the runtime detector

`TEARDOWN_LOCK_ORDER_SUSPECT` compares against `PROCESS_MANAGER_OWNER_TID`
(`process/mod.rs`, maintained by `note_process_manager_lock_acquired`). That bookkeeping is
**not updated by `try_manager()`**, which was blocking finding #8 in the parked branch's r20 review.
So the counter is a *detector for the common case*, not a proof. The actual guarantee is structural
(pid-based API ⇒ no live `&mut Process` borrow ⇒ no live guard) plus the P0 ratchet test that pins
`terminate_process_threads`'s call sites to `teardown.rs`. The counter is asserted zero in CI so a
regression is loud; it is not load-bearing for correctness, and this design does not claim otherwise.

---

## 5. Phased implementation plan

Every phase: builds clean on both arches with **zero warnings**, ships on its own, is revertable by
reverting exactly that PR (no phase depends on a later one), and passes the standard gate. Sizes are
stated against PR #418 (5 commits, fault-path teardown machinery) as the ceiling.

**Standard gate (every phase, no exceptions):**

1. `cargo build --release --features testing,external_test_bins --bin qemu-uefi` — zero warnings.
2. `cargo build --release --target aarch64-breenix.json -Z build-std=... -p kernel --bin kernel-aarch64` — zero warnings.
3. `./docker/qemu/run-boot-parallel.sh 5` (x86) + `./docker/qemu/run-aarch64-boot-test-strict.sh` — all green.
4. Parallels: `./run.sh --parallels`, **10 consecutive green boots** (or 15 attempts, whichever
   first) per the launcher-test protocol, plus a soak on the phases that change kill timing (P3, P4,
   P5, P10).
5. Phase-specific assertions listed below, each an observable outcome (counter equality, actual exit
   status, zero fault markers) — never "the process was created".

### P0 — Teardown observability + call-site ratchet  *(no behavior change)*

- Add `kernel/src/tracing/providers/teardown.rs`: counters `TEARDOWN_ENTRY{exit,fault,signal,group}`,
  `TEARDOWN_QUARANTINE`, `TEARDOWN_EXPEDITE_SGI_SENT`, `TEARDOWN_DEFER`, `TEARDOWN_RECLAIM`,
  `TEARDOWN_VICTIM_DIVERGENCE`, `TEARDOWN_CR3_MISS`, `TEARDOWN_MASKED_FRAMES_WALKED`,
  `TEARDOWN_LOCK_ORDER_SUSPECT`, `DEFERRED_FAULT_RING_DROPPED` (#492's currently-invisible overflow).
- Add `tests/teardown_structure.rs`: a source-structure ratchet asserting the **exact current set**
  of `\.terminate\(` / `terminate_minimal\(` call sites, `ProcessId::new(1)` sites, and
  `terminate_process_threads` call sites. It passes on `main` as-is; later phases shrink the
  allowlist, and any *new* bypass fails immediately.
- **Gate extras:** boot test asserts `TEARDOWN_DEFER == TEARDOWN_RECLAIM` at quiesce and
  `TEARDOWN_LOCK_ORDER_SUSPECT == 0`.
- **Size:** ~200 lines, 2 commits. **Revert:** delete two files + their registration.
- **Why first:** without these counters every later phase's evidence is a log-reading exercise, and
  §7.5 forbids weaker proxies.

### P1 — `process/teardown.rs`; four fault sites migrated; attribution fixed

- New `kernel/src/process/teardown.rs` with `exit_current` / `kill_remote` /
  `quarantine_and_expedite` (the last two are dead code in this phase — **not** `#[allow(dead_code)]`;
  they land in P3, so P1 ships only `exit_current` and adds the others in P3).
- `exception.rs`: the four EL0 sites (`:749-801`, `:1117-1170`, `:1200-1244`, `:1249-1347`) replace
  their inlined quiesce→resolve→quarantine→`exit_process` block with one `teardown::exit_current`
  call. The frame redirect, `set_idle_stack_for_eret`, and `switch_to_idle` tails stay at the call
  sites, unconditional, untouched.
- Victim attribution changes to faulting-thread-first with CR3 cross-check (§2.4b).
- **Gate extras:** new `clonevm_fault_test` — a CLONE_VM child faults; assert the *child* dies, the
  parent survives, `TEARDOWN_VICTIM_DIVERGENCE == 1`, no refault loop. This test **fails on `main`**,
  which is the point.
- **Size:** ~180 lines net (mostly deletion), 2-3 commits. **Revert:** restore the four inlined
  blocks (kept intact in the commit message diff).
- **Bypass-surface delta:** 4 hand-maintained copies → 1; one live wrong-victim livelock closed.

### P2 — `ResourceState`: explicit hand-off key  *(behavior-identical)*

- `process.rs`: add `ResourceState` + field; set `HandedOff` in `release_process_resources`,
  `defer_process_resources`, `Process::terminate`.
- `manager.rs:1137` and `process_task.rs:234` key their raw-drop branch on `resources == HandedOff`
  instead of `is_terminated()`.
- **Gate extras:** boot test asserts CoW frame accounting is unchanged vs. P1 (same
  `TEARDOWN_RECLAIM` totals for the same workload).
- **Size:** ~70 lines, 1 commit. **Revert:** trivially isolated.

### P3 — SIGKILL through the shared path  *(#491, the spine)*

- `signal.rs:157-170` rewritten per §2.1; `teardown::kill_remote` +
  `Scheduler::expedite_process_threads` land here.
- **Gate extras:** new `sigkill_teardown_test` (userspace): parent forks a child that spins at EL0;
  parent `kill(child, SIGKILL)`; assert (a) `waitpid` reaps status `-9`, (b) SIGCHLD was delivered
  at kill time (parent's `pause()` returns), (c) `TEARDOWN_QUARANTINE`/`TEARDOWN_DEFER`/
  `TEARDOWN_RECLAIM` all increment for that pid, (d) `TEARDOWN_MASKED_FRAMES_WALKED == 0`,
  (e) `TEARDOWN_EXPEDITE_SGI_SENT > 0` when the child is on a peer CPU, (f) zero fault markers over
  a 10-boot Parallels streak. Repeat with the child inside a CLONE_VM group (sibling must survive —
  this design does **not** sweep the group; see §6).
- **Size:** ~160 lines + test program, 3 commits. **Revert:** restore the four-line
  `process.terminate(-9)` arm.
- **Bypass-surface delta:** the largest single reduction in the round — one of the two remaining
  eager-free UAF classes gone.

### P4 — Default-fatal-signal teardown through the shared path

- `delivery.rs:224`/`:258`: `process.terminate(code)` → `process.terminate_minimal(code)` (which
  leaves `resources == Held`, correctly handled thanks to P2), keep the existing scheduler-thread
  marking, and extend the already-existing `DeliverResult::Terminated(notification)` channel — whose
  documented contract is *"caller MUST call notify after releasing the PM lock"* — to also carry
  `(pid, code)`. The three aarch64/x86 callers (`context_switch.rs:5088`,
  `syscall_entry.rs:264`, `interrupts/context_switch.rs:700/1101/1332`) call
  `teardown::exit_current(pid, code, ExitCause::Signal(sig))` in the region where they already do
  their post-lock notification.
- `interrupts/context_switch.rs:1021` (x86_64) migrated for arch parity; x86 Stage 3 keeps its
  existing synchronous `release_process_resources` — the deferral machinery is aarch64-only by
  design and this phase does not change that.
- **Gate extras:** `sigsegv_default_action_test` — assert exit status `-11` reaped once, CoW
  accounting balanced, `TEARDOWN_MASKED_FRAMES_WALKED == 0`, and x86 behavior byte-identical to
  pre-phase (explicit x86 regression run, since two prior rounds shipped accidental x86 divergences).
- **Size:** ~140 lines, 2-3 commits. **Revert:** restore two `terminate()` calls + the notification
  payload.

### P5 — Normal-exit Stage 3 unification + drain-site quarantine

- `process_task.rs:241-251`: aarch64 live branch calls `defer_process_resources` **unconditionally**;
  `defer_live_process_resources` is **deleted** (not `#[allow(dead_code)]`-ed).
  Cost: a process whose root is provably dead now waits ≥2 epochs before its frames return —
  the same bounded-retention tradeoff #418 already accepted ("retains memory longer, never frees
  earlier").
- `drain_deferred_fault_sigsegv_exits` (`process_task.rs:363-371`): before each
  `handle_thread_exit(tid, -11)`, resolve the owner pid and call `teardown::quarantine_and_expedite`
  — the drained victim is not the current thread, so it is exactly the case that needs Stage 2.
- **Gate extras:** memory-retention watch (peak free-frame dip over a fork/exit soak stays within a
  stated bound); `TEARDOWN_DEFER == TEARDOWN_ENTRY{exit}+TEARDOWN_ENTRY{fault}+…` for aarch64.
- **Size:** ~90 lines net (net deletion), 2 commits. **Revert:** restore the helper.
- After this phase, Stage 3 is **literally the same code** for every aarch64 death path.

### P6 — Kernel-stack ownership parity for the spawn path *(AC #8's third case)*

- `creation.rs:85`, `creation.rs:202`, `syscall_entry.rs:1630`, `boot/test_disk.rs:263`:
  `get_process(pid)` + `main_thread.clone()` → `get_process_mut(pid)` +
  `scheduler_thread.kernel_stack_allocation = thread.kernel_stack_allocation.take()`, matching
  fork (`syscall_entry.rs:961`) and clone (`clone.rs:247-254`).
- Effect: an original thread's kernel stack is no longer freed synchronously and ungated by
  `remove_process` at `waitpid` reap (`manager.rs:1102`, `wait.rs:386`); it goes through
  `reclaim_terminated_threads`'s grace + `!is_kernel_stack_slot_live` gate like every other stack.
- **Gate extras:** stack-pool accounting test (allocated == freed after N spawn/exit cycles) plus a
  stress run that previously could hand a recycled stack to a new thread while a peer was still on it.
- **Size:** ~60 lines across 4 sites, 1-2 commits. **Revert:** isolated per site.
- **Honesty note:** §3.7 of the input package reports this asymmetry as fact, not as a diagnosed bug.
  This phase closes it because uniformity is cheap here, **not** because a UAF has been demonstrated.

### P7 — Designated-init runtime flag *(#464 part 1, no fatal behavior)*

- New `kernel/src/process/init_identity.rs` (§2.2). One designation call site. Three production
  literals migrated. `signal.rs:26`'s `INIT_PID` constant migrated. Ratchet allowlist shrinks.
- **Gate extras:** `init_pid() == Some(1)` in the interactive build; `None` in a boot-test-only
  build with no init; reparenting still lands orphans on init (existing orphan test).
- **Size:** ~120 lines, 2 commits. **Revert:** restore the literals.

### P8 — Init-death policy *(#464 part 2)*

- Stage 3 sets `INIT_DIED` (one relaxed store); Stage 5 escalates with no locks held (§2.2).
  No `#[cfg]`, no feature scoping.
- **Gate extras:** a deliberate init-kill test asserts the panic message and that the panic *reports*
  (i.e. serial output is complete, proving no lock was held). A normal boot test asserts the flag
  stays 0 across the whole run, including the `smoke_hello_time` harness — the exact build the four
  prior attempts broke.
- **Size:** ~70 lines, 1-2 commits. **Revert:** delete the flag + check.

### P9 — exec detaches `thread_group_id` + `inherited_cr3` *(#471 part 1)*

- Four exec commit points, two assignments each (§2.3).
- **Gate extras:** extended `clonevm_exec_test`; futex behavior across an exec (group id falls back
  to pid) verified explicitly, since `futex.rs:29-35` is the main consumer.
- **Size:** ~40 lines, 1 commit. **Revert:** delete eight lines.

### P10 — Group seal + atomic snapshot *(#471 part 2)*

- `sys_clone` refuses to join a committed group; `snapshot_thread_group` (fixed array, one PM hold,
  overflow counter).
- **Gate extras:** the racing-clone stress test from AC #7's row; `GROUP_SNAPSHOT_OVERFLOW == 0`.
- **Size:** ~130 lines, 2 commits. **Revert:** isolated.

### P11 *(optional, deliberately deferrable)* — group-wide sweep

- `for member in snapshot { teardown::kill_remote(member, code, ExitCause::GroupMember) }` — one
  member per PM transaction, outside any lock, stacks only via scheduler grace.
- **This round can stop after P10 and still fully close #471 as written.** P11 is listed so the
  design shows the sweep is a three-line consequence of the preceding phases, not a separate machine.

### Ordering constraints (the only hard dependencies)

`P0 → everything` (evidence). `P1 → P3, P4, P5` (the helper). `P2 → P4` (the `terminate_minimal`
hand-off would otherwise leak CoW refcounts). `P7 → P8` (identity before policy). `P9 → P10 → P11`.
Everything else is independent and can be reordered or dropped.

---

## 6. What this design deliberately does NOT solve

Stated plainly, with the argument that none is foreclosed:

1. **#492 — unbounded fault-exit drain.** `drain_deferred_fault_sigsegv_exits` keeps replaying up to
   8×16 sequential `handle_thread_exit` passes under the caller's mask. P5 adds a Stage-2 quarantine
   *per drained victim*, which adds a bounded scheduler-lock section per pass but changes no cap and
   adds no unbounded work. P0 makes the ring's currently-invisible overflow drops observable
   (`DEFERRED_FAULT_RING_DROPPED`), which is a prerequisite for #492's "policy for what happens to
   overflow between passes" — so this design **advances** #492's tractability without claiming it.
2. **#448 — idle-path CoW-walk latency under IRQ mask.** No drain site is moved, no cap is added, no
   cap is shared with fork's full drain (the exact failure of the r23 round's first bounded design).
   P0's counters give the first per-pass measurement hook. P5 slightly *increases* the number of
   deferred entries (unconditional defer), which is the honest cost — stated in Residual R-3.
3. **#493 — `check_signals_for_eintr` disposition.** Untouched. P4 changes *what happens when a fatal
   default action fires*, not *which signals are considered deliverable*; `has_deliverable_signals()`
   (`signal/types.rs:190-192`) is not modified, so the GPU-DMA class is neither fixed nor worsened.
4. **The `is_ttbr0_root_live` local-`TTBR0_EL1` gap.** The predicate consults only the two per-CPU
   software shadows, never the local hardware register. This design keeps that as-is and leans on the
   two-epoch grace to cover it (grace is checked *first*, so a stale shadow cannot short-circuit the
   epoch requirement). Explicitly accepted, not fixed — see Residual R-1 and OQ-4.
5. **The `retirement_grace_elapsed` all-zero-target short-circuit** (`scheduler.rs:563-568`, F13):
   a target of 0 for a CPU passes with zero atomic loads. Unchanged. This design does not rely on
   grace alone for any *new* path — every path also carries the `!root_is_live()` liveness clause —
   but it does not close the hole either. OQ-5.
6. **Allocation and FD-close under the PM lock with DAIF masked** (F12/R5): `children.clone()`,
   `name.clone()`, `take_fd_entries() -> Vec`, `close_all_fds`'s pipe locks. Pre-existing on both
   convergence points. This design does not add to it and removes one `FRAME_METADATA` acquisition
   on two paths, but the parked spec's allocation-free-commit ambition (R5/R6: delete the `children`
   mirror, one-FD-at-a-time close) is **not** attempted here. Residual R-2.
7. **The grave / `kreclaimd` structural rewrite.** Not resurrected. Its good ideas are reused as
   *properties* rather than as its type system: pid-based APIs instead of a `ReclaimContext` token
   (whose r20 failure was release-mode no-op assertions and a `try_manager()` blind spot); source-
   structure ratchet tests instead of a state-machine rewrite; `#[must_use]` disposition with no
   control-flow branching (R8) instead of `ExitPending` (whose r20 failure was a finalization
   deadlock — an `ExitPending` thread is dequeued everywhere and can never quiesce its own stack).
   No dedicated reclaim kernel thread is introduced, so the Tier-2 `kthread.rs`/`workqueue.rs`
   surface and the r20 logging-in-the-reaper failure mode are both avoided entirely.

**Frozen/gold-master and Tier-1 files touched by this design: none.** Specifically untouched:
`context_switch.rs`'s EL0 dispatch site, `idle_loop_arm64`, `aarch64_enter_exception_frame`,
`gic.rs::init_gicv3_redistributor`, both `timer_interrupt.rs` regions, and all five Tier-1 files
(`syscall/handler.rs`, `syscall/time.rs`, `syscall/entry.asm`, `interrupts/timer.rs`,
`interrupts/timer_entry.asm`). `interrupts/context_switch.rs` (Tier 2) is touched in P4 for the
x86_64 `terminate()` caller only — a call-site swap with no logging, no page-table work, no new lock.

---

## 7. Residuals — risks this design accepts rather than closes

**R-1. Hardware TTBR0 can outlive the software shadows on the local CPU.** `is_ttbr0_root_live`
reads only shadows. After `set_idle_stack_for_eret` writes `next_cr3 = kernel_ttbr0` and zeroes
`saved_process_cr3` without performing a hardware switch (the idle redirect does no `msr`), the
hardware register may still hold a root that the liveness predicate reports dead. The two-epoch
grace is the only barrier for that window, and it is a *time* barrier, not a *proof*. This design
keeps the grace-first ordering that makes the window as narrow as `main` already makes it, and adds
no new path that shortens it — but it does not eliminate it.

**R-2. Stage 3 still allocates, logs, and takes pipe/PTY/TCP locks under PM with all DAIF masked.**
`exit_process` keeps `log::info!` at `:1121`, `children.clone()` at `:1183`, and `terminate()`'s
`close_all_fds`. Routing SIGKILL and fatal signals through it means those paths now hit this section
too — they previously hit an *equivalent-or-worse* one (`terminate()` under the same guard, plus a
real CoW walk), so this is not a regression, but it is not a fix either. The parked spec's R5/R6
(allocation-free commit) remains the right eventual answer.

**R-3. Unconditional deferral (P5) increases peak deferred-entry count and peak retained frames.**
Every normal exit now enqueues a `PendingProcessReclaim` even when its root is provably dead, so the
`PENDING_PROCESS_RECLAIMS` vector and the frames it pins grow relative to today under fork-heavy
loads. The reclaim happens at the next drain (fork or `schedule_from_kernel`), so steady-state is
bounded — but a burst is measurably worse than today, and this interacts with #448/#492 rather than
helping them. The P5 gate includes an explicit retention measurement; if it is material, P5 is the
one phase whose revert is genuinely expected to be considered.

**R-4. `kill(getpid(), SIGKILL)` — self-kill — takes the `kill_remote` path with Stage 1 as a
no-op.** The caller is at EL1 in a syscall with its own root still installed and its shadows still
naming it, so `root_is_live()` is `true` and reclaim is correctly blocked; the process is
`Terminated`, so the syscall-return path will reschedule rather than ERET back. This is believed
sound but is a *reasoned* claim, not one this design proves — it is on the P3 test matrix explicitly
(self-SIGKILL test), and if it fails the fix is to have `kill_remote` compare against
`current_thread_id()`'s owner and delegate to `exit_current`.

**R-5. `expedite_process_threads` reads `cpu_state[].current_thread` under the scheduler lock, which
can be stale relative to a peer mid-dispatch.** The consequence of staleness is a *missed* SGI (the
victim leaves at its next natural reschedule, i.e. today's behavior) or a *spurious* SGI (harmless).
It cannot cause an incorrect free, because freeing is gated by grace+liveness, not by the SGI. The
expedite is an optimisation with a correctness floor equal to `main`.

**R-6. The P0 ratchet is a source-text test.** It pins call sites by grep, so a bypass introduced via
a rename, a macro, or a trait method escapes it — the same class of evasion the parked branch's r20
review found (finding #12: `set_terminated()` as "a different spelling of the same forbidden
transition" passing a string-match test). It raises the cost of a regression; it does not make one
impossible.

**R-7. Nothing here proves the two-epoch grace is *sufficient*.** It is the mechanism `main` already
ships and this design generalises it to more paths on the argument that "the same barrier that is
trusted for the fault path's peer CPUs is trusted for a remote kill's peer CPUs." If two epochs is
ever shown insufficient, every path this design unifies is affected simultaneously — which is a
feature for fixing it and a risk for shipping it.

---

## 8. Open questions for the operator

**OQ-1 — Is P5 (unconditional defer on normal exit) worth its retention cost?**
It is the change that makes Stage 3 *literally identical* across all paths, which is the charter's
"one mental model" goal. But it is also the only phase with a measurable steady-state cost (R-3).
The alternative is to keep `defer_live_process_resources` and document the divergence as a
justified optimisation. Recommendation: ship P5, measure, revert if retention is material.

**OQ-2 — How strong should the kernel/userspace init contract be (AC #5)?**
This design keeps `init_shell.rs`'s `getpid() == 1` unchanged and makes the kernel's designation
coherent with it (the Linux shape). A stronger option exists: inject `BREENIX_INIT=1` into the
designated init's environment at `setup_argv_on_stack` (`manager.rs:714` create, `:2885`/`:3158`
exec) and have `init_shell` key on that instead — structurally impossible to disagree, at the cost
of one extra parameter threaded through creation. Which contract do you want?

**OQ-3 — Should P8's init-death policy panic, or halt-and-report?**
`panic!` is Linux's semantic ("Attempted to kill init!") and #464 asks for it. But the panic handler
in this kernel takes SERIAL/framebuffer locks; Stage 5 holds none, so it is safe — yet a panic in an
interactive build ends the session where a loud, non-fatal report plus a deliberate halt might be
more debuggable on Parallels. Default in this design: panic. Say the word to change it.

**OQ-4 — Do you want `is_ttbr0_root_live` to also read the local `TTBR0_EL1` (R-1)?**
It is a two-line addition (`mrs` on the calling CPU, masked-compare) and would close the local half
of the shadow/hardware gap. It is out of this round's charter as written and would touch a helper
every path depends on, so it is not in any phase. Worth its own issue?

**OQ-5 — `retirement_grace_elapsed`'s all-zero-target short-circuit (F13/§3.8).**
Reachable only if `is_cpu_online` was false for every CPU at capture time. Should a phase assert
`target != [0; MAX_CPUS]` at capture (a counter, not a panic), or is this genuinely unreachable on
current boot paths and best left alone?

**OQ-6 — P11 (the group sweep): this round or the next?**
#471 as written asks for the detach + seal that make a sweep safe, not the sweep. Shipping P9+P10
and stopping closes the issue as filed. Shipping P11 too closes the charter's "CLONE_VM group path"
line item completely. P11 is small *given* P9/P10, but it is the same shape as the mechanism PR #418
stripped after four blocking findings, so it deserves its own review round.

**OQ-7 — P6 (spawn-path stack ownership) touches four creation call sites for a hazard nobody has
demonstrated.** The input package explicitly reports it as fact, not diagnosed bug. Ship it for
uniformity, or leave it and record the asymmetry as a known-and-accepted difference?

---

## 9. Compliance with the lessons register

| Previously-rejected mechanism | Is it reopened? | If yes, what is different |
|---|---|---|
| `#[cfg(not(feature = "testing"))]`-scoped init panic (×4 reverted) | **No** | Designation is runtime data; no feature gate exists anywhere in P7/P8, so `interactive = ["testing"]` cannot invert it |
| Panic under a DAIF-masked PM guard | **No** | Stage 3 stores an atomic; Stage 5 panics with no guard live |
| Fatal escalation on heuristic victim resolution | **No** | Escalation keys on a *committed* pid; resolution misses are counted, never fatal |
| CLONE_VM group-wide sweep (4 blocking defects) | **Only as optional P11**, after P9 (exec detach, defect 1), P10 (atomic snapshot + seal, defect 2), P6/P3 (stacks only via scheduler grace, defect 3), and the one-victim-per-PM-transaction rule (defect 4) each land and ship **separately and reviewably first** | The four named defects are closed as prerequisites, not as part of the sweep commit |
| Bounded drains (×3 reverted: shared cap broke fork's full drain; incomplete coverage; scheduler-inside-PM inversion) | **No** | No cap is added, no drain is moved, no new pre-drain call is added to `sys_clone`, and Stage 2's scheduler lock is never entered with a PM guard live |
| Suppressing repeat-pass `btrt::on_process_exit` (disproven) | **No** | Every pass keeps reporting; only the *value* is first-recorded |
| `ReclaimContext` capability token (r20: release-mode no-op assertions, `try_manager()` blind spot) | **No** | Replaced by a pid-based API (structural) + source ratchet; the runtime detector's limits are stated in §4.4 rather than claimed as proof |
| `ThreadState::ExitPending` (r20: finalization deadlock) | **No** | No new thread state; quarantine reuses `terminate_process_threads`, which is proven on the fault path |
| `kreclaimd` worker thread (r20: logging in the reaper, Tier-2 surface) | **No** | Reclaim stays at the two existing drain sites |
