# Teardown Unification — FINAL DESIGN (panel synthesis)

**Status:** design-only proposal for operator ratification. No implementation performed, no runtime
evidence claimed. Every build/QEMU/Parallels statement in this document is a *gate to be run*, never
a result already obtained.

**Base:** `main` @ `eebc8868` (anchors re-verified against the live tree during synthesis — see §0.3).
**Scope:** #491 (spine), #464, #471. Acknowledges and does not foreclose #448, #492, #493.
**Inputs:** design A (minimal-incremental, Opus), design B (Linux-fidelity, Codex Sol), design C
(invariant-first, Codex Sol), two adversarial judge verdicts (Opus; Codex Sol).

---

## 0. Adjudication — how the winner was ruled

### 0.1 The judges disagreed

| | Judge 1 (Opus) | Judge 2 (Codex Sol) |
|---|---|---|
| Ranking | **A** > C > B | **B** > C > A |
| Coverage | A 11.5/13 (with AC-2 overclaimed, AC-5 weakest form, AC-12 partial, AC-13 material omission) | A **7/13**, B **12/13** (AC-13 honestly flagged partial), C **12/13** |
| Fatal flaws | B ×2, A ×1, C ×0 | A ×5, C ×1, B ×0 |
| Lock-safety | A "sound-for-new-work, incomplete disclosure"; B/C not scored in the excerpt | A "HAS ISSUES" (3 named); B "CLEAN as described" (2 implementation conditions); C "CLEAN, but carries a separate fatal deadlock" |

### 0.2 The ruling: **B wins the architecture; A wins the phase plan; C wins the proof machinery.**

Applying the two tiebreak instruments the brief specifies:

**Criteria-coverage count.** B and C tie at 12/13; A is 11.5 from one judge and 7 from the other.
No reading puts A above B. B's single gap (AC-13) is *declared* rather than argued away, which is
the behaviour the package's honesty policy rewards. **→ B or C, not A.**

**Fatal-flaw absence — and, decisively, fatal-flaw CLASS.** Counting is not enough here, because the
three designs failed in structurally different places:

- **A's flaws are mechanism-level and plural.** Five of Judge 2's six are defects in what the code
  would *do*: a designated init whose PID may not be 1 while userspace hard-checks `getpid()==1`
  (verified live at `userspace/programs/src/init_shell.rs:1028`); a leader-only seal that lets a
  clone join after the snapshot; an accepted stale `cpu_state[].current_thread` read that drops the
  SGI AC-11 forbids relying on the fallback for; a remote SIGKILL that marks the scheduler thread
  `Terminated` before `handle_thread_exit` — the only `btrt::on_process_exit` call site — can run;
  and new quarantine work inserted into the exception-return drain *ahead of* the `PREEMPT_ACTIVE`
  gate. Judge 1 independently found a sixth (P4's `terminate_minimal` hand-off silently loses FD
  closure). Mechanism defects at this density are not curable by re-phasing.
- **C's single fatal flaw is one *named mechanism*: `ExitPending`.** A victim blocked mid-syscall is
  removed from every runnable queue while its saved resume SP is still treated as live evidence, so
  it can never clear that evidence — the exact finalization deadlock that killed the grave branch at
  r20. Judge 2's own graft list says: take everything else from C, do **not** take `ExitPending`.
- **B's two fatal flaws are both PLAN-level, not mechanism-level.** (i) #491 — the round's declared
  spine and a confirmed-live UAF at `signal.rs:162` — cannot land until Phase 11 of 15, behind
  ~2300 lines of prerequisite; that is precisely the "large coherent design accumulates unreviewed
  surface before validation" pattern that killed the grave branch at r20 with 27 findings.
  (ii) Phases 9 and 10 land production code explicitly "dormant until Phase 11", which the repo's
  zero-tolerance dead-code rule forbids and for which §7.5 removes the `#[allow(dead_code)]` escape.
  **Both are cured by re-sequencing. Neither touches what the code does.** Judge 2, reviewing B's
  mechanism directly, found *no* lock-safety or correctness fatal.

A synthesis is exactly the instrument that can keep B's mechanism and throw away B's sequence. So:

> **Winner: B (Linux-fidelity) as the architectural target — "senders mark, victims exit themselves,
> reapers own corpses, retirement is proof-gated" — re-phased under A's shippability discipline
> (spine first, every phase strictly better, no dormant code, hard line budget, call-site ratchet),
> with C's proof machinery grafted wholesale and C's `ExitPending` state explicitly rejected.**

Two consequences of the ruling are load-bearing and are honoured throughout §4:

1. **The spine lands third, not eleventh.** #491's live eager-free UAF is closed in Phase 2 using
   only already-merged primitives (A's mechanism, hardened), and the *same call site* is then
   progressively upgraded to B's victim-owned exit in Phases 8–10. We deliberately pay for the SIGKILL
   arm twice rather than leave a confirmed UAF live behind ten PRs. This is the direct answer to
   Judge 1's fatal flaw against B, and it is why A's contribution is real even though A lost.
2. **No phase ships dormant code.** Every new API has a live caller in the PR that introduces it
   (`do_exit_current` ships with normal `exit` as its first consumer; each wait-family contract ships
   with the SIGKILL-of-a-blocked-victim path it makes killable). No `#[allow(dead_code)]`, no
   "activated later" phase.

### 0.3 Anchors re-verified in the live tree during synthesis (not taken from the designs)

`main` = `eebc8868`. Four direct `Process::terminate` callers outside `exit_process`:
`syscall/signal.rs:162` (`terminate(-9)`), `signal/delivery.rs:224` and `:258`,
`interrupts/context_switch.rs:1021` (x86_64). Three *production* `ProcessId::new(1)` literals
(`process/manager.rs:1178`, `task/process_task.rs:226`, `:285`) plus three test-only in
`test_userspace.rs`. `next_pid` is `AtomicU64::new(1)` with four `fetch_add` allocation sites
(`manager.rs:138`, `:388`, `:612`, `:1092`). `Scheduler::terminate_process_threads(owner_pid: u64)`
at `scheduler.rs:2599`. `exit_process` at `manager.rs:1120` carries `#[allow(dead_code)]` and already
performs the unconditional aarch64 grace-stamped defer. `Process::take_fd_entries` (`process.rs:335`)
**returns an allocating `alloc::vec::Vec`** — confirming Judge 2's warning that it cannot be reused
unchanged under PM. `init_shell.rs:1028` reads `getpid().map(|p| p.raw()).unwrap_or(0) != 1`.

---

## 1. Architecture

### 1.1 One sentence

**Senders mark; victims exit themselves; reapers own corpses; retirement is proof-gated.**

No path ever tears down a process that is executing somewhere else. A killer's entire job is to
record an intent and kick a CPU. The victim performs its own teardown, on its own stack, in ordinary
kernel context, with no lock held for anything slow. Physical release happens only after a fenced
two-epoch grace *and* a positive proof that no CPU, no scheduler cache, and no live row still names
the resource.

### 1.2 The single state machine (all five death paths)

```
 S0  IDENTIFY   exact victim: PID or TID. Never CR3-only. CR3 is a cross-check that
       │        counts disagreement and NEVER escalates fatally.
       ▼
 S1  REQUEST    ONE PM transaction. First status wins. Marks every live member of the
       │        scope with one batch id; creates DURABLE work obligations in each row;
       │        seals the group against clone/exec admission. Moves nothing, frees
       │        nothing, allocates nothing, logs nothing, takes no second lock.
       │        Returns #[must_use] ExitRequestResult + a preallocated fixed receipt.
       ▼
 S2  KICK       PM dropped. Scheduler transaction publishes the exit request onto each
       │        member's scheduler thread and readies proven-killable blocked victims
       │        WITH their continuation intact. It NEVER sets a remote thread Terminated.
       │        Returns ExitKickPlan. Then, holding NO lock: local need_resched +
       │        BROADCAST SGI_RESCHEDULE to other online CPUs when the plan asks for it.
       ▼
 S3  SELF-EXIT  Each victim runs do_exit_current() on ITSELF at its next safe boundary:
       │        claim (atomic) → local TTBR0 leave (hardware first, shadows after) →
       │        short PM commit (mark zombie, record first status, take own FDs, decide
       │        last-reference, move root into a retirement receipt) → DROP PM → all slow
       │        work unlocked (one FD at a time, futex/clear_child_tid, bounded reparent
       │        cursor, redeem the exactly-once notification receipt, enqueue receipt) →
       │        pivot to neutral stack → mark ONLY SELF Terminated → schedule away.
       ▼
 S4  RETIRE     GRACE FIRST: RetirementFence (two epochs on every CPU in the captured
       │        online mask, acquire-ordered; an empty mask is INVALID, never "elapsed").
       │        THEN RootProof + StackProof. Refusal records a per-blocker counter and
       │        rotates. Physical free runs with NO PM and NO scheduler lock held.
       ▼
 S5  ESCALATE   Init-death latch read in ordinary kernel context, all guards dropped,
                DAIF restored, pre-panic lock/IRQ snapshot recorded, THEN panic.
```

### 1.3 Per-path variance — every deviation named

| Path | S0 | S1 scope | S2 | S3 | Named variance |
|---|---|---|---|---|---|
| normal `exit(2)` | self TID | Member | no-op (self) | self | none |
| `exit_group` | self TID | ThreadGroup | kick siblings | each member | none |
| EL0 fatal fault | faulting TID (CR3 cross-check) | ThreadGroup | kick siblings | **this** thread, via the fault site's existing mandatory redirect tail | S3 is entered from a redirect the fault site already performs unconditionally; the tail never branches on the teardown result |
| SIGKILL / default-fatal signal | target PID | ThreadGroup | kick all | each member | sender never touches victim resources; `deliver_default_action` **returns** an intent instead of mutating under the PM borrow |
| init death | committed PID only | as above | as above | as above | S1 latches; S5 escalates in ordinary context |
| CLONE_VM group member | member TID/PID | ThreadGroup (seal) | kick all | each member | seal is the same PM transaction as the mark — no snapshot ever leaves the lock |

**The only structural asymmetry in the whole design** is that a killer cannot switch a remote CPU's
`TTBR0_EL1` (no architectural remote `mrs`/`msr`). This design does not paper over it with a
"remote quiesce" that would be a refusal path in disguise: the local leave is argument-free and
local by construction, and the remote half of the obligation is discharged by S2 (the victim is
forced to a boundary and can never be re-dispatched to EL0) plus S4 (grace + RootProof). That is
the *same* discharge the merged fault path already relies on for peer CPUs.

### 1.4 New state (deliberately small)

```rust
// PM-owned. One lock, no second lock, no new global.
ThreadGroupRecord { id, leader_pid, parent_pid, control: /*fixed, preallocated*/,
                    lifecycle: Open | ExecSealed{owner_tid, gen} | GroupExit{cause, gen},
                    first_status, live_members, mm_owner_pid,
                    notification: NotReady | Ready | Delivered | Reaped }

// Row-resident. The process row IS the work node — no collection grows during exit.
ExitWorkBits { sigchld, parent_wake, report, reparent, fds, resources }  // durable obligations
teardown_next: Option<ProcessId>                                        // intrusive link

// Scheduler/boundary mirrors. Release/acquire. Carry NO ownership, authorize NO free.
GroupExitWord { generation, cause, active }
ThreadExitRequest { generation, reason, state }

// Proof values (from C).
RetirementFence { epochs: [u64; MAX_CPUS], online_mask }
RetirementSnapshot                       // proves Acquire loads + fence ran before liveness reads
RootProof { blocked_epoch, blocked_hw, blocked_shadow, blocked_cached, blocked_live_row }
```

`ExitWorkBits.resources` subsumes design A's proposed `ResourceState{Held,HandedOff}` — "the page
table has left this row" is just another durable obligation, not a second parallel key. This is a
real simplification the panel surfaced: A needed a separate field only because it had no ledger.

**Explicitly NOT added:** no `ExitPending` thread state (C's fatal flaw), no `ProcessGrave`, no
graveyard stack, no `Arc<AddressSpace>` / mm-refcount conversion, no `ReclaimContext` capability
token (r20: release-mode no-op assertions + `try_manager()` blind spot), no new always-running
kernel worker in the core round (see OQ-6), no `#[cfg]` gate on any init behaviour.

### 1.5 Why not `ExitPending` — and what replaces it

C's `ExitPending` makes a victim non-runnable *before* it has cleared the evidence of its own
liveness. A thread blocked mid-syscall is off-CPU with a saved resume SP that `kernel_stack.rs:277`
treats as live; having been dequeued from everywhere, it has no path left to clear that. That is the
banked r20 finalization deadlock verbatim.

Replacement (B's model, endorsed by Judge 2's graft list): **an exit-requested victim is made
RUNNABLE precisely so it can execute its own exit continuation.** Its `blocked_in_syscall` state and
saved continuation are preserved; the continuation gives the latched fatal request priority over
ordinary success, unregisters itself from its futex/waitqueue/reader/timer registry through the
existing `finish_wait`/unregister path, and only then branches to the exit trampoline. The scheduler
never takes an external wait-queue lock and never discards a continuation.

A wait primitive whose victim-owned cancellation path has not been audited and tested is classified
**uninterruptible**: the request stays latched and the victim dies at its next natural safe boundary.
This is a declared, counted, bounded-by-nothing delay (Residual R-2) — never a silent claim of
killability, and never a free underneath a live stack.

---

## 2. Per-issue mechanism

### 2.1 #491 — SIGKILL routing (the spine)

**Confirmed live defect** (`syscall/signal.rs:162`): the SIGKILL arm calls `process.terminate(-9)`
while holding the PM guard (`manager()` masks all DAIF). That synchronously reaches
`close_all_fds()` (pipe/PTY/TCP locks under mask) and `cleanup_cow_frames()` →
`frame_decref`/`deallocate_frame` (blocking `FRAME_METADATA`) **while the victim may be executing at
EL0 on a peer CPU**. No quiesce, no quarantine, no grace deferral, no SGI, no SIGCHLD.

This closes in **two increments**, both on the same call site:

**Increment 1 (Phase 2) — stop the bleeding with merged primitives only.**
Under the existing PM guard: validate only, capture `pid`, mutate nothing, `drop(guard)` (the same
function already contains a drop-before-scheduler precedent). Then:
`with_scheduler(|s| s.terminate_process_threads(pid))` → **broadcast** `SGI_RESCHEDULE` to every
other online CPU (no `cpu_state[].current_thread` predicate — see below) →
`with_process_manager(|pm| pm.exit_process(pid, -9))`, which already performs the unconditional
grace-stamped `defer_process_resources` + `enqueue_process_reclaim` on aarch64 **before**
`terminate()` runs, so `cleanup_cow_frames()` walks a `None` page table and the CoW decref moves
behind grace + RootProof. Plus the durable `report`/`sigchld` obligation seed (below).

What Increment 1 does and does not buy, stated exactly:
- **Removes** the eager CoW walk while the victim may be remote — the confirmed UAF class. *Strict
  reduction of work under PM+mask, not an addition:* today's SIGKILL does the full walk there.
- **Adds** quarantine, expedite, and the missing SIGCHLD.
- **Does not** yet make the exit victim-owned (still remote-marks), and **does not** yet move FD
  closure out of PM (`exit_process` → `terminate()` → `close_all_fds` is pre-existing at that
  convergence point; unchanged, not worsened; fixed in Phase 7).

Two defects the panel found in this exact mechanism are fixed *in Increment 1*, not deferred:
- *Missed SGI from a stale `cpu_state[]` read* (Judge 2, fatal against A's AC-11): the expedite
  **broadcasts** to other online CPUs rather than deriving a residency mask. B's argument is
  decisive at Breenix's CPU count — a constant number of extra SGIs beats a fallible mask, and the
  SGI handler already does nothing but set `need_resched`. There is no stale read to be wrong about.
- *Lost `btrt` report / parent wake* (Judge 2, fatal against A's AC-12): `btrt::on_process_exit` has
  exactly one call site, inside `handle_thread_exit`, which a remotely-marked victim may never run.
  Increment 1 therefore installs the **durable report obligation** at commit: a row work bit set once
  at first commit and redeemed exactly once, outside PM, by whichever of {commit path,
  `handle_thread_exit`} reaches it first. This is the seed that Phase 6 generalizes into the full
  ledger. It is small, and it must land with the remote-kill path, not six phases later.

**Increment 2 (Phases 8–10) — make it victim-owned.** `terminate_process_threads` is redefined to
*request and wake* (publish `ThreadExitRequest`, ready proven-killable blockers with continuations
intact, return `ExitKickPlan`) and its remote-marking body is deleted in the same PR that removes
its last remote-marking caller. SIGKILL then becomes a group-scoped `ExitIntent`, and every victim
runs `do_exit_current` on itself. `Process::terminate` is deleted when its last caller goes.

`send_signal_to_all_processes` / `send_signal_to_caller_process_group` reach SIGKILL only through
`send_signal_to_process`, so they are fixed by the same change.

### 2.2 #464 — init identity, then (separately) init death policy

Four prior attempts died by bundling "who is init" with "what happens when init dies", and three of
them died specifically on `interactive = ["testing"]` inverting a `#[cfg]` gate. This design ships
them as **two separate PRs** and uses **no `#[cfg]` gate anywhere** — designation is runtime *data*,
so a build that never designates an init can never trip the policy.

**Identity (Phase 5).** `ProcessManager` gains exactly one authority: `designated_init: Option<ProcessId>`.

- **PID 1 is reserved** for the explicit init constructor; ordinary/test allocation starts at 2
  (all four `next_pid.fetch_add` sites). Init is built off-table with provisional PID 1. (C's graft,
  endorsed by Judge 1.)
- **Held-publication ticket** (B): build the fallible image → insert the live row → create the
  scheduler thread **not-yet-runnable** → only then return a ticket → only then, under PM, validate
  the ticket names a live PID 1 and set `designated_init` → only then publish to the run queue.
  Failure anywhere before designation leaves no row and no designation, and PID 1 is retryable.
  This satisfies AC-1 structurally: `create_process_with_argv` allocates the PID at `manager.rs:612`
  *before* the fallible page-table (`:622-631`) and ELF (`:637-640`) steps, so any designation
  earlier could name a PID that never gets a row.
- **Production designation is validated to be PID 1 and refuses otherwise.** This is what makes AC-5
  structural rather than a log line: `init_shell.rs:1028`'s `getpid() != 1` guard is left completely
  unchanged, and the kernel cannot designate an init that would disagree with it. Tests that boot no
  real init leave designation unset — whichever process happens to get a low PID is *not* init.
- All three production `ProcessId::new(1)` literals and `signal.rs`'s `INIT_PID` constant become the
  accessor. The three `test_userspace.rs` literals are creation-time setup, not teardown, and are
  named explicitly in the ratchet allowlist rather than silently ignored.

**Death policy (Phase 12).** Recommended (Linux's protected-init intent, OQ-1):
user-originated default-fatal signals — including SIGKILL — to the designated init are rejected with
`EPERM` and set no group exit; a caught signal is handled normally; init's **own** `exit`/`exit_group`,
an unhandleable synchronous fatal fault, or a nonviability invariant is kernel-fatal.

The fatal action never happens under a guard. S1 sets `INIT_DEATH_LATCH` (one relaxed store) **only
for a committed, certainly-attributed victim** — an attribution miss or a TID/CR3 divergence can
never latch (AC-3, both directions). S5 reads the latch in ordinary kernel context with PM and
scheduler guards out of scope and DAIF restored, records a pre-panic lock/IRQ snapshot (Judge 1's
graft — this is what makes the panic *reportable*, since the panic handler takes SERIAL/framebuffer
locks), then panics. Note honestly: aarch64's panic handler parks the panicking CPU only; peers are
not actively stopped (Residual R-12, OQ-1b).

### 2.3 #471 — group seal + exec detach

**Exec detach (Phase 3).** At each exec commit point — after every fallible step has succeeded and
the new page table is installed — clear both fields:

```rust
process.page_table = Some(new_page_table);
process.inherited_cr3 = None;
process.thread_group_id = None;   // effective TGID falls back to pid: a fresh singleton
```

On **any** exec failure both fields are preserved unchanged. The existing live-sibling guard
(`find_live_clone_vm_sibling_holding_cr3`, `manager.rs:46-77`) stays. Both directions analysed:
if the row owned the old root, the guard proved no live sibling names it and `take()` retires it
normally; if the row was a CLONE_VM child (`page_table == None`, root via `inherited_cr3`), `take()`
yields `None`, the owner keeps its root, and clearing `inherited_cr3` detaches a row from an address
space it never owned — no free, no refcount change. `thread_group_id`'s other consumers (futex
keying, `clear_child_tid` wake) all fall back to `pid` when `None`, which is exactly correct
post-exec semantics.

**The seal is the mark (Phase 3 admission + Phase 10 cutover).** There is no separate "snapshot then
sweep". The group-exit PM transaction *is* the seal: it walks the process map once, marks every live
row whose effective TGID matches with one batch id, and does nothing else. In the *same* PM lock
discipline, `sys_clone` refuses to publish into a group that is not `Open`, and every user-thread
creation path publishes its scheduler thread **non-runnable** until the row is published, with
dispatch independently refusing `Creating`/`ExitRequested` rows before arming TTBR0.

Therefore **no membership snapshot ever leaves the PM lock** — the stale-snapshot defect that killed
PR #418's sweep is not fixed, it is *unrepresentable*. A racing clone either inserted its row before
the transaction (and is included and cannot become dispatchable) or acquires PM afterwards and sees
a sealed parent (`EAGAIN`). Design A's leader-only seal (Judge 2's fatal flaw) is rejected: the mark
covers every member regardless of which member the kill named.

Group-wide **exec cull** (siblings self-exit so the execer can commit) is explicitly *not* in the
core round — see §5 and OQ-7. #471 as filed asks for the detach and the seal; both ship here.

### 2.4 Fault-victim attribution (AC-3, both directions)

`find_process_by_cr3_mut` (`manager.rs:1313-1335`) matches only rows whose **own** `page_table` root
equals the CR3. A CLONE_VM sibling row has `page_table == None` and holds the root in
`inherited_cr3`, so it is never matched: a clone thread faulting at EL0 resolves to the *owner* row,
the kernel kills the parent, and the real faulter stays runnable and refaults. This is **live on
`main` today**, independent of any branch.

Resolution order at the four EL0 sites:
1. current scheduler TID → its `owner_pid` (authoritative for "who was executing"; at an EL0 fault
   the current thread *is* the faulter);
2. cross-check the scheduler-owned stack slot owner when available — they must agree;
3. `find_process_by_cr3_mut(ttbr0)` is a **root-consistency cross-check only**. Disagreement bumps
   `TEARDOWN_VICTIM_DIVERGENCE`, prefers the TID-derived victim, and **can never escalate fatally**;
4. total resolution failure → `AttributionUncertain`: publish the existing deferred TID intent, do
   the local safe redirect, escalate nothing, kill nothing.

In every branch the fault site's **mandatory tails** (frame redirect to `idle_loop_arm64`,
`set_idle_stack_for_eret`, `switch_to_idle`) run **unconditionally** and never branch on the
teardown result (grave-spec R8, adopted; A's rule, endorsed by Judge 2). Two banked findings were
"a mandatory tail got skipped based on a return value" (r20 #1's five silent early returns; R2-11's
`Option` that disabled the idle redirect); `#[must_use] ExitRequestResult` is for diagnostics and
exhaustive matching, not control flow over mandatory work.

---

## 3. Numbered traceability — all 13 acceptance criteria

| # | Criterion | Mechanism | Phase | Evidence that must be produced |
|---|---|---|---|---|
| **1** | Init designation only after creation fully succeeds; no phantom PIDs | Held-publication ticket: row inserted **and** non-runnable scheduler thread created before `designated_init` is committed under PM; PID 1 reserved off-table so a failed attempt leaves no row, no designation, and PID 1 retryable. Nothing in `create_*` touches designation. | 5 | Failure injection after PID selection at each fallible stage (page table, ELF, stack, publication): `designated_init() == None`, no row; retry succeeds as PID 1 |
| **2** | No panic/fatal action while PM held with DAIF masked | S1 does **one relaxed store** to `INIT_DEATH_LATCH`. All fatal escalation is a receipt redeemed at S5 with PM and scheduler guards out of scope and DAIF restored; a pre-panic lock/IRQ snapshot is recorded first. No `#[cfg]`, so no build carries a differently-scoped variant. | 12 | `INIT_PANIC_WITH_LOCK == 0`; injected init death records PM owner `None`, scheduler owner `None`, IRQ state normal immediately before panic; the panic's serial output is **complete** (proving no lock was held) |
| **3** | Victim attribution certain before fatal escalation; a heuristic CR3 miss must not panic | §2.4: TID-first, stack-slot cross-check, CR3 as root-consistency only, divergence counted and never fatal, `AttributionUncertain` → safe redirect + deferred intent. The latch keys on a **committed** victim, never on a resolution attempt. | 11 (attribution), 12 (escalation) | `clonevm_fault_test`: a CLONE_VM child faults → the **child** row dies, the parent survives, no refault loop. *This test fails on `main` today* — it is a live bug fixed, not a tautology. TID/CR3 mismatch injection bumps `EXIT_ATTRIBUTION_UNCERTAIN`, latches nothing, kills nobody |
| **4** | One source of truth for init identity; no hardcoded PID 1 beside runtime designation | `ProcessManager::designated_init` is the sole authority; the three production literals (`manager.rs:1178`, `process_task.rs:226`, `:285`) and `signal.rs`'s `INIT_PID` become the accessor. `ProcessId::INIT` survives only as the ABI *validation* constant, never a lookup. | 5 | P0 ratchet fails on any new `ProcessId::new(1)` in production teardown/wait/signal/reparent code; the three `test_userspace.rs` sites are allowlisted **by name** |
| **5** | Kernel and userspace init guards must agree (`init_shell` keys on `getpid()==1`) | Structural, not conventional: PID 1 is **reserved** for the explicit init constructor and production designation is **validated == PID 1 and refuses otherwise**. `init_shell.rs:1028` is not changed, so no second contract exists to drift. A non-PID-1 init would require an explicit userspace ABI change and is not silently supported. | 5 | Cross-tree source assertion + boot test: designated pid == 1 == the pid `init_shell` observes; a build with no real init leaves designation unset and does **not** treat whichever process got the low PID as init |
| **6** | exec detaches `thread_group_id` **and** `inherited_cr3` | Both assignments at every exec commit point, after all fallible work, before PM release; both preserved on every failure; existing live-sibling guard retained. Both arches in one commit. | 3 | `clonevm_exec_test` extended: successful exec → both `None`, fresh root, effective TGID == pid, and a kill aimed at the **old** group cannot reach it; failed exec → both preserved byte-identical |
| **7** | Group membership examined atomically; no snapshot stale across a PM drop | **No snapshot exists.** The group-exit PM transaction *is* the seal: mark every live effective-TGID member with one batch id inside one guard; `sys_clone` publication validates group lifecycle under the same lock; scheduler threads carry group id + generation and re-check before first dispatch; threads publish non-runnable until the row is published. | 3 (admission), 10 (cutover) | Deterministic clone-vs-seal barrier test: the child is either included in the batch or `sys_clone` returns `EAGAIN` — never a runnable unrequested member. Ratchet rejects any group PID `Vec` snapshot in teardown code |
| **8** | Sibling kernel stacks freed ONLY behind two-epoch grace via scheduler ownership | All three creation paths — fork, CLONE_VM clone, **and spawn/direct-init/test-disk** — transfer `kernel_stack_allocation` to the scheduler-owned copy before the thread can run; a `Process` row can no longer synchronously drop a published stack (closing today's ungated free via `remove_process` at `waitpid` reap). No teardown path ever drops a stack directly: victims mark only themselves `Terminated`, and release requires grace + `!is_kernel_stack_slot_live`. | 4 | Ownership assertion after every creation path (exactly one owner, and it is the scheduler copy); 1000-iteration fork/clone/spawn exit stress with stack-pool accounting (allocated == freed) and an allocator assertion that never selects a live slot |
| **9** | No N-member FD/resource teardown loop in one PM-locked, IRQ-masked section | Each victim takes and closes **only its own** descriptors, **one at a time**: `take_next_for_exit()` under PM → drop PM → close. The existing allocating `take_fd_entries() -> Vec` (`process.rs:335`) is retired, not reused. Group work under PM is bitmap/flag stores only. No sweep loops over members' FD tables. | 7 | `FD_CLOSES_UNDER_PM == 0`; 256-FD × large-group test measures bounded PM hold; ratchet forbids close/reclaim calls inside any request/commit transaction body |
| **10** | No eager `cleanup_cow_frames` while the victim may run elsewhere; all kill paths grace-defer | Phase 2 routes SIGKILL through `exit_process`, whose aarch64 defer takes the page table into a grace-stamped receipt **before** `terminate()` runs (so the CoW walk becomes a `None`-walk). Phases 10–11 remove every direct `terminate` caller; resources stay in the row until the victim's own commit, and release requires grace + RootProof. `Process::terminate` is deleted with its last caller. | 2, 7, 10, 11 | Ratchet allowlist of `\.terminate\(` shrinks phase-by-phase to **empty**, asserted as an exact set; peer-CPU SIGKILL stress asserts zero reclaim before the fence elapses and a complete RootProof; `TEARDOWN_MASKED_FRAMES_WALKED == 0` for kill paths |
| **11** | Killed threads quiesced in the scheduler AND expedited with the existing `SGI_RESCHEDULE` | Phase 2: quarantine via the proven `terminate_process_threads` + **broadcast** `SGI_RESCHEDULE` to other online CPUs (no residency predicate to go stale — this is the direct fix to the panel's fatal finding). Phases 9–10: scheduler *requests* instead of remote-marking, readies proven-killable blockers with continuations intact, returns `ExitKickPlan`; the SGI handler keeps doing only `need_resched`; the victim can never be re-dispatched to EL0 once the generation is observed. **No `ExitPending`.** | 2, 9, 10 | Victim spinning at EL0 pinned to a peer: `EXIT_SGI_SENT > 0`, target observes it, time-to-`Terminated` bounded by an SGI round trip rather than the victim's next natural tick; per-wait-family blocked-victim kill tests; unmigrated families explicitly reported as uninterruptible, not silently latched |
| **12** | Exactly-once SIGCHLD/wake/report with **first-recorded** status, idempotent under repeat passes | Durable `ExitWorkBits` created at the **first** request and cleared only on completion; a repeat request returns the stored batch/status and creates no second obligation and no second status. Redemption happens once, outside PM, by whichever producer reaches it first — which is why the remote-kill path (Phase 2) carries the report obligation from day one rather than relying on `handle_thread_exit` (the single `btrt::on_process_exit` call site) being reached. **This does not reopen notification suppression** (disproven in review): the first transition creates one obligation *every* producer shares, so a later pass is never the only possible notifier — it redeems the same obligation, it does not skip it. | 2 (seed), 6 (ledger) | Matrix — exit→fault, SIGKILL→fault, fault→SIGKILL, repeat request/wait: exactly one SIGCHLD, one parent wake, one `btrt` report, and `waitpid` returns the **first** status; equality assertions `SIGCHLD_FIRST_SET == PARENT_WAKE_COMPLETED == BTRT_EXIT_REPORTED == parented_first_commits` |
| **13** | New reclaim/drain respects lock ordering and is bounded on idle paths without throttling fork's drain | **No cap is added, no drain is moved, no drain is shared.** Fork's pre-allocation drain stays full/unbounded. The only addition to a return tail is an acquire-load + branch, placed **after** the `PREEMPT_ACTIVE`/nested-return gate — design A's insertion of quarantine work into the pre-gate deferred-fault drain (Judge 2's fatal finding, and a #448 regression) is **rejected and not adopted**. Every scheduler critical section is entered with **no PM guard live**, structurally: request/commit APIs take `pid`/`tid`, never `&mut Process`, so no caller can hold the guard across the call. No `FRAME_METADATA` and no `log::*` is added under any mask. Full analysis: §4. | all | Lock-depth/owner counters (with `try_manager()` instrumented — the r20 blind spot is not repeated); `RECLAIM_CONTEXT_VIOLATIONS == 0`; ratchet rejects scheduler-under-PM and any reclaim/close/walk in idle or exception tails; fork-pressure test proves a *full* eligible drain, not a capped one. **Declared partial:** this design does not fix #448's or #492's pre-existing boundedness; it must not make them worse, and §5 states the argument |

---

## 4. Lock-ordering analysis

### 4.1 The rule

Documented today: `SCHEDULER → PROCESS_MANAGER → endpoint locks`. Live teardown/signal/exec code
does not honour it consistently (signal delivery and exec update scheduler state under PM; reclaim
enqueue nests a queue op under PM).

**This design adopts a strictly stronger rule for all death-path code:**

> PM, SCHEDULER, reclaim queues, FD/endpoint locks, `FRAME_METADATA`, the stack-pool bitmap, and
> SERIAL are **never held simultaneously** by teardown, signal, exec, wait, or reparent code. State
> moves between domains through fixed-size receipts and release/acquire atomics.

This makes both nesting directions impossible for teardown, so no ordering *cycle* can exist —
rather than relying on everyone remembering which order is legal. It is enforced structurally
(pid/tid-based APIs ⇒ no live `&mut Process` borrow ⇒ no live guard), by the P0 ratchet, and observed
by counters — in that order of authority.

### 4.2 Lock inventory (aarch64)

| Lock | Discipline | Note |
|---|---|---|
| PM (`PROCESS_MANAGER`) | `manager()` masks **all** DAIF, then spin mutex | the hard one: everything under it is masked |
| SCHEDULER | `with_scheduler` / `lock_for_context_switch` | |
| `PENDING_PROCESS_RECLAIMS` | leaf; staging only | insertion moves a receipt; no walk under it |
| `FRAME_METADATA` | blocking `spin::Mutex`; reached from CoW-fault context AND teardown | reclaimer pins preemption while held; no guard spans alloc/copy/unmap/map |
| FRAME_ALLOCATOR | under `FRAME_METADATA` in decref paths | leaf |
| SERIAL / framebuffer | any `log::*` | forbidden under any mask in this surface |
| pipe / PTY / TCP | FD close | **moves out of PM in Phase 7** |
| heap | any `Vec`/`String` | forbidden in request/commit bodies |

### 4.3 Every new or moved critical section

| Site | Locks | Order | Alloc? | Log? | Bounded? |
|---|---|---|---|---|---|
| S1 `request_exit` / group seal | **PM only** | entered with no other guard; takes no second lock | none | none | O(process-count) scalar scan, flag stores only (Residual R-4) |
| S2 scheduler request/quarantine + `ExitKickPlan` | **SCHEDULER only** | entered with **no PM guard live** — structural: API takes `pid`/`tid`, never `&mut Process` | none | none | O(threads) mark + O(queues×threads) retain — identical to the merged fault path's existing cost |
| SGI kick | **no lock** | after both guards drop | none | none | ≤ MAX_CPUS MMIO writes; handler does only `need_resched` |
| S3 commit transaction | **PM only** | short; single hold; no drop mid-transaction | none (fixed receipts preallocated) | none | O(1) per victim; one victim per transaction, always |
| S3 FD close | PM only to `take_next_for_exit`; **then endpoint lock with PM dropped** | strict edge: PM released before any endpoint lock | none | none | one descriptor per PM acquisition |
| S3 reparent cursor | **PM only**, fixed-size batch | PM dropped and DAIF restored between batches | none (`children` mirror not extended) | none | fixed batch (Residual R-4) |
| S3 notification redemption | PM only to mark → drop → SCHEDULER only to wake → PM only to clear the bit | no overlap; serialized by the durable work bit | none | none | O(1) |
| S3 receipt enqueue | reclaim queue only, **after** PM drop | leaf staging lock; no walk under it | none | none | O(1) |
| S4 grace + RootProof (epochs/hardware/shadows) | **no lock** | pure ordered observation (`RetirementSnapshot`) | none | none | O(MAX_CPUS) |
| S4 RootProof (cached roots) | SCHEDULER only | guard drops before PM revalidation | none | none | O(threads) |
| S4 resource claim + physical free | PM only to `take` → **drop** → TLBI/CoW walk/frame free with no PM or scheduler held; preemption pinned across `FRAME_METADATA` | no heavy destructor under PM; no broadcast TLBI under PM/scheduler/IRQ-off | frees only | none | per-victim |
| S4 stack release | SCHEDULER only to prove/detach → drop → stack-pool bitmap | allocator never acquired under scheduler | none | none | one thread per pass |
| S5 init escalation | **none** | all guards out of scope, DAIF restored | panic path may allocate — legal here | panic only | one atomic load |
| Boundary hook (Tier-2) | **none** | acquire-load + branch, placed **after** the `PREEMPT_ACTIVE`/nested-return gate | none | none | O(1); no drain, no walk, no lock |

### 4.4 Hazards explicitly foreclosed — and one honest limit

Foreclosed: **SCHEDULER inside DAIF-masked PM** (the r23 revert class, three designs) — impossible
because every request/quarantine API is pid/tid-based. **PM inside SCHEDULER** — S1/S2 are sequential
statements, never nested closures. **`FRAME_METADATA` under PM+mask** — removed on the SIGKILL and
fatal-signal paths in Phase 2/11, never added. **New heap allocation under PM+mask** — receipts are
fixed-size and preallocated; the allocating `take_fd_entries()` is retired in Phase 7. **`log::*`
under mask** — none added anywhere; all new diagnostics are `trace_count!`/`trace_event!` from the
lock-free framework, with `raw_serial_char()` remaining the last resort at fault sites that already
use it. **Heavy work before a safety gate** — the boundary hook goes after the `PREEMPT_ACTIVE` gate,
and no drain is added to any tail.

**Honest limit.** The runtime lock-order detector compares against `PROCESS_MANAGER_OWNER_TID`, and
that bookkeeping was *not* updated by `try_manager()` — blocking finding #8 in the parked branch's
r20 review. This design requires `try_manager()` to participate in the same instrumentation, so the
blind spot is closed for the detector — but the detector is still only a detector. **The actual
guarantee is structural** (pid-based APIs + the ratchet), and the counters are asserted zero in CI so
a regression is loud. No safety claim in this document rests on a release-stripped assertion, and no
correctness argument rests on a counter.

---

## 5. What this design deliberately does NOT solve

1. **#492 — unbounded fault-exit deferred drain.** The 8×16 producer and its replay are untouched.
   This design adds a readable `DEFERRED_FAULT_RING_DROPPED` counter (the overflow is invisible today)
   and gives ring items a stable TID/generation, which makes a cursor/backpressure fix easier later.
   **Not made worse:** certainly-attributed fatal faults do not consume the ring, and no SIGKILL or
   group request depends on ring capacity.
2. **#448 — idle-path CoW-walk latency under IRQ mask.** No drain is moved, no cap is added, no cap
   is shared with fork's full drain (the exact failure of the r23 round's first bounded design). The
   only addition to a return tail is an acquire-load + branch **after** the `PREEMPT_ACTIVE` gate.
   Design A's proposal to insert quarantine work into the pre-gate deferred-fault drain is rejected
   precisely because it would worsen #448. **Honest cost:** routing more paths through unconditional
   grace deferral raises the peak deferred-entry count and peak retained frames under burst
   (Residual R-6); the phase gates include an explicit retention measurement.
3. **#493 — `check_signals_for_eintr` disposition.** Untouched. `has_deliverable_signals()` is not
   modified. Group exit is set only *after* the existing disposition code determines the effective
   action is terminate; the request API takes an already-resolved fatal intent, so a later
   disposition fix sits *before* this machinery and needs no teardown change.
4. **The `children` mirror / parent-authority migration.** B's Phases 6–7 (remove the allocating
   `children` vector, subreaper selection) are **cut from the core round**: they are a wait/procfs/fork
   refactor whose blast radius is unrelated to the three chartered issues. The bounded reparent cursor
   is kept (it is what makes reparenting non-allocating and non-unbounded); nearest-living-subreaper
   selection is deferred. OQ-7.
5. **Group-wide exec cull, PID transplant, `prctl` subreapers, core dumps, mm/VFS refcounting.**
   All out. #471 as filed asks for detach + seal; both ship.
6. **A dedicated reclaim worker.** Heavy reclaim stays at the two existing merged drain sites (fork's
   full drain + `schedule_from_kernel`). The receipt + intrusive-row-queue shape makes a bounded
   normal-context worker a drop-in later, which is where #448/#492 point — but a worker touches
   Tier-2 `kthread.rs`/`workqueue.rs` territory and the r20 attempt failed there (`log::warn!` inside
   the reaper loop). OQ-6.
7. **The `retirement_grace_elapsed` all-zero-target short-circuit is FIXED here**, not deferred:
   Phase 1 makes an empty online mask invalid rather than trivially "elapsed".

**Frozen/gold-master regions touched: none.** Specifically untouched: the EL0 dispatch site,
`idle_loop_arm64`, `aarch64_enter_exception_frame`, `gic.rs::init_gicv3_redistributor`, both
`timer_interrupt.rs` regions. **Tier-1 files touched: none** in the core round — the one Tier-1
question (`syscall/time.rs`'s raw TTBR0 writer) is raised as OQ-4 and is *not* required for any phase
to ship. **Tier-2:** `interrupts/context_switch.rs` and the aarch64 exception/context-switch boundary
are edited in Phases 8 and 11; the justification is functional, not diagnostic — GDB can observe a
failing boundary but cannot make a production victim execute its own exit continuation.

---

## 6. Residuals — risks accepted, not closed

**R-1. Remote TTBR0 liveness is shadow-based.** There is no architectural remote `mrs TTBR0_EL1`.
RootProof reads the **local** hardware register on the proving CPU (closing the local half of the
gap A left open), but remote CPUs are covered by the software shadows, whose conservative-superset
property depends on the install ordering (name before install; publish `saved` after ISB; clear
`next` last). That ordering is enforced for the writers this design touches; kernel-wide closure
needs OQ-4. Until then the absolute claim is a *declared residual, not a stated fact*.

**R-2. Uninterruptible waits delay death by an unbounded amount.** A victim in a wait family whose
victim-owned cancellation path has not yet been audited stays latched and dies at its next natural
safe boundary. The group stays sealed and its resources pinned rather than freed underneath it. This
is a *safe* failure (leak/stall, never early free) and is counted per family, but a genuinely stuck
wait blocks reaping indefinitely. Breenix has no killable-wait taxonomy today; building one is
Phase 9's whole job and it is honestly incomplete until every family lands.

**R-3. O(group-size) fatal marking under PM with DAIF masked.** Bitmap/scalar stores only — no
allocation, no FD, no resource work — but interrupt masking scales with group size. Accepted at
current scale; a fixed-batch representation would need a second "marking in progress" state while
preserving atomic sealing.

**R-4. O(process-count) PM scans.** The group seal and the reparent cursor both scan the process map
because the `children` mirror is retained (see §5.4). Scalar, non-allocating, and batched, but not
O(1). Removing the mirror later fixes it.

**R-5. FD closure is only moved out of PM at Phase 7.** Between Phase 2 and Phase 7 the SIGKILL path
closes descriptors inside `exit_process` → `terminate()` under PM+mask. That is *unchanged from
today's SIGKILL* (which does the same, plus a full CoW walk), so it is not a regression — but it is
not a fix during that window either, and this document does not pretend otherwise.

**R-6. Wider unconditional grace deferral raises peak retention.** More paths enqueue a
`PendingProcessReclaim` under burst. Steady state is bounded by the drains; a burst is measurably
worse than today. Measured explicitly at each gate; the retention numbers, not an argument, decide.

**R-7. Broadcast SGI is coarse.** Every other online CPU may take an unnecessary SGI. At Breenix's
CPU count this is strictly better than a fallible residency mask — that mask is exactly what the
panel found fatal — but it is not Linux's targeted selection.

**R-8. x86_64 is single-CPU and unproven for remote delivery.** It shares the group state, victim-owned
transaction, stack retirement, ledger, and tests, but adds no APIC IPI protocol. A targeted test must
prove no x86_64 user-return path bypasses the common hook; if one does, that is an operator approval
point (OQ-9), not license to touch Tier-1 syscall entry.

**R-9. The two-epoch grace is assumed sufficient, not proven.** It is the mechanism `main` already
ships. This design generalizes it to more paths, so if two epochs is ever shown insufficient, every
unified path is affected simultaneously — a feature for fixing it, a risk for shipping it.

**R-10. The call-site ratchet is a source-text test.** A bypass introduced by rename, macro, or trait
method escapes it — the r20 review found exactly this evasion (`set_terminated()` as "a different
spelling of the same forbidden transition"). It raises the cost of a regression; it does not make one
impossible.

**R-11. Receipt durability is an implementation obligation, not a proof.** Receipts must be
fixed-size, preallocated, non-droppable, and either redeemed synchronously outside PM or stored in
authoritative row state. An implementation that routed notification through the bounded fault ring
would violate this design. Likewise, group-control/receipt destruction must never occur under PM or
the scheduler lock — invariant plus source test, not a proof.

**R-12. aarch64 panic is not stop-the-world.** Init-death panic parks the panicking CPU; peers are
not actively stopped by today's panic handler. Recorded, not fixed (OQ-1b).

**R-13. `waitpid` may return before physical retirement.** The logical corpse can be reaped while a
lightweight `ReapedPendingRetirement` record awaits grace. Unobservable except through kernel
diagnostics and resource pressure.

**R-14. Nonleader exec keeps its PID (Linux transplants the leader's).** Observably different for
programs comparing thread IDs across exec. Transplant would require a registry-wide rekey audit
(scheduler owner id, wait identity, procfs, TTY) unrelated to resource safety.

**R-15. This is a design-only artifact.** No build, QEMU, or Parallels evidence exists yet. Every
gate below is a requirement on the implementation, not a result.

---

## 7. Open questions — operator decisions only

**OQ-1. Init death policy.** Recommended (Linux protected-init): user-originated default-fatal
signals to designated init → `EPERM`, no group exit; init's own `exit`/`exit_group` or an
unhandleable fatal fault → kernel-fatal panic. Alternative: treat an authorized `SIGKILL(1)` as a
deliberate shutdown/panic. **OQ-1b:** accept today's non-stop-the-world panic (recommended; file
`smp_send_stop` separately), or add an SMP stop broadcast in this round?

**OQ-2. PID-1 reservation.** Reserve PID 1 for the explicit init constructor and start ordinary/test
allocation at 2 (all four `next_pid.fetch_add` sites)? Recommended **yes** — it is what converts AC-5
from convention into structure and makes a failed init creation deterministically retryable.

**OQ-3. Exit scope split.** Confirm `exit(2)` = member, and `exit_group`/SIGKILL/default-fatal
signal/fatal user fault = thread group (Linux). Recommended **yes**; today's collapsed
`Exit | ExitGroup` would undermine #471's group semantics.

**OQ-4. Tier-1 approval — `kernel/src/syscall/time.rs`.** Approve routing its raw TTBR0 writer
through the constrained helper? Recommended **yes eventually, not required by any phase here**.
Without it the conservative-shadow invariant cannot honestly be declared closed kernel-wide (R-1). It
is the one Tier-1 file this work would ever want, and the r20 record notes the grave branch's
two-line edit there lacked signoff.

**OQ-5. Tier-2 approval — the exception-return boundary hook.** Approve an acquire-load + branch in
the aarch64 exception/syscall return path and `interrupts/context_switch.rs`, placed after the
`PREEMPT_ACTIVE`/nested-return gate, with all real work in a normal preemptible context? **Without
this approval the design stops at Phase 7** — victim-owned exit is impossible, and #491 ships only at
Increment-1 strength (still a real UAF fix, but the remote-mark model survives).

**OQ-6. Bounded reclaim worker.** In this round, or deferred with #448/#492? Recommended **deferred**
— the receipt/queue shape makes it a drop-in, it touches Tier-2 kthread surface, and the r20 attempt
failed there. Judge 2 endorsed grafting it; this synthesis defers it and says so rather than smuggling
it in.

**OQ-7. Scope cuts.** Confirm that group-wide exec cull, `children`-mirror removal / subreaper
selection, and nonleader PID transplant are all **out** of this round. Recommended **yes** — each is a
separate blast radius, and #471 as filed is satisfied by detach + seal.

**OQ-8. Phase count.** This plan is 13 phases / 13 PRs at ≤ ~230 changed lines and ≤5 production
files each (PR #418's 236-line, 5-file diff is the ceiling). That is a lot of review cycles, and it is
the deliberate price of Judge 1's fatal finding against a 15-phase big bang. Accept, or batch adjacent
phases and accept larger review surface per PR?

**OQ-9. x86_64 rollout.** Land shared semantics now with no SMP proof (recommended), or gate the
victim-owned path to aarch64 and keep an explicitly audited synchronous x86 hook?

---

## 8. Compliance with the lessons register

| Previously-rejected mechanism | Reopened? | What is different |
|---|---|---|
| `#[cfg(not(feature = "testing"))]`-scoped init panic (×4 reverted) | **No** | Designation is runtime data. No feature gate exists anywhere, so `interactive = ["testing"]` cannot invert anything |
| Panic under a DAIF-masked PM guard | **No** | S1 stores one atomic; S5 panics with no guard live and records a pre-panic lock/IRQ snapshot first |
| Fatal escalation on heuristic victim resolution | **No** | The latch keys on a **committed** victim; TID/CR3 divergence is counted and never fatal; `AttributionUncertain` kills nobody |
| CLONE_VM group sweep (4 blocking defects) | **Reshaped, not reopened** | All four defects are structurally unrepresentable: exec detach lands first (Phase 3); no snapshot ever leaves PM (the mark *is* the seal); stacks are only ever self-marked and grace-released; no N-member FD loop exists because each victim closes its own FDs one at a time outside PM. Group-wide **exec cull** is still out (OQ-7) |
| Bounded drains (×3 reverted) | **No** | No cap added, no drain moved, no cap shared with fork, no new pre-drain in `sys_clone`, and no scheduler lock is ever entered with a PM guard live |
| Suppressing repeat-pass `btrt::on_process_exit` (disproven) | **No** | The obligation is created once and *shared by every producer*; a later pass redeems the same obligation rather than being skipped. `btrt`'s single call site is exactly why Phase 2 carries the report obligation on the remote-kill path |
| `ReclaimContext` capability token (r20) | **No** | Replaced by pid/tid-based APIs (structural) + the source ratchet; `try_manager()` is instrumented so the r20 detector blind spot is closed, and §4.4 states the detector's limits instead of claiming proof |
| `ThreadState::ExitPending` (r20 finalization deadlock) | **No — explicitly rejected from design C** | A victim is made *runnable* so it can run its own continuation, unregister its own wait, and terminate itself; it is never dequeued while its own liveness evidence stands |
| `kreclaimd` worker (r20: logging in the reaper, Tier-2 surface) | **No** | Reclaim stays at the two existing drain sites; a worker is OQ-6, not a phase |
| Large coherent design accumulating unreviewed surface before validation (the r20 grave failure, and Judge 1's fatal finding against B) | **No** | The spine's live UAF closes in Phase 2 of 13; every phase ships, is revertable alone, and has a live caller for its new code; stopping the round at *any* phase boundary leaves the tree strictly better than `main` |
