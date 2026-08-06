# Teardown Unification — FINAL DESIGN v2 (panel synthesis, revised against ratification)

**Status:** design-only proposal for operator ratification. No implementation performed, no runtime
evidence claimed. Every build/QEMU/Parallels statement in this document is a *gate to be run*, never
a result already obtained.

**Base:** `main` @ `eebc8868` (anchors re-verified against the live tree during synthesis — see §0.3;
re-verified again during the v2 revision at `main` @ `c9efdcc7`, which is v1 of these documents merged
and no kernel change).
**Scope:** #491 (spine), #464, #471. Acknowledges and does not foreclose #448, #492, #493.
**Inputs:** design A (minimal-incremental, Opus), design B (Linux-fidelity, Codex Sol), design C
(invariant-first, Codex Sol), two adversarial judge verdicts (Opus; Codex Sol), and the **Codex Sol
ratification refusal** (ENDORSE: NO — 2 fatal seams, 7 majors, 1 minor, 7 conditions).

---

## CHANGELOG — v2 deltas against the ratification conditions

v1 was refused ratification (`ENDORSE: NO`) with two FATAL seam findings, seven MAJOR, one MINOR, and
seven explicit conditions. This revision is **targeted**: everything the ratification did not
criticise is preserved verbatim. Every change below is traceable to a numbered condition.

| Cond | Condition (verbatim intent) | v2 closure | Where |
|---|---|---|---|
| **1** | Reorder so every wait-family PR has a real producer: request-only scheduler publication + victim-owned SIGKILL commit suppression move **before** the killable-wait subphases; re-derive the dependency graph honestly incl. the missing P3→P8 edge | Old P10 becomes **P9**; old P9a/b/c become **P10a/b/c**. P9 is independently implementable against P2 via the named `exit_request_is_boundary_reachable()` predicate with a **live, counted legacy remote-mark arm** for not-yet-migrated wait families; P10a/b/c each move one family into the reachable set; **P10c deletes the arm**. `#491 complete` therefore moves from P9 to **P10c** | §1.5, §2.1 Increment 2, §3 AC-10/AC-11, §6 R-2/R-16; PLAN §0 graph, P9, P10a/b/c |
| **2** | Explicit `Pending→Claimed→Completed` state per obligation (or one exclusive redeemer), restoring what design C's single-worker serialization discharged; add the reap/tombstone retention gate **before** any phase ships row-resident resource bits | New **§1.6** defines the four-state obligation machine (`Absent/Pending/Claimed{claimer}/Completed`), PM as the sole serializer, the sole-redeemer invariant, and orphaned-claim recovery. New **retention rule**: `waitpid` no longer removes the row; it tombstones it. Shipped as **P6a (retention gate) before P6b (ledger)** | §1.4, **§1.6** (new), §3 AC-12, §4.3, §6 R-11/R-13/R-17; PLAN P6a/P6b |
| **3** | P1's proof reads never hold the reclaim queue while acquiring scheduler or PM locks; the SIGKILL phase's retirement receipt is returned/enqueued only **after** the PM guard drops — no interim shape violating the no-overlapping-lock rule even temporarily | P1's drain becomes **detach → drop queue lock → prove → free-or-reinsert**; the under-queue-lock predicate stays epoch/shadow-only (lock-free atomics). P2 changes `exit_process` to **return** a `#[must_use] RetirementReceipt`; both existing PM-nested enqueue sites (`manager.rs:1152`, `process_task.rs:244`) are converted in P2 | §4.1, §4.3 (two new rows), §4.4; PLAN P1, P2 |
| **4** | P8 must state explicitly how normal exit exercises the return-boundary hook in its own PR (no dormant hook) | §2.5 (new) gives the control flow: `sys_exit` publishes a self-request and returns; the **hook is the only entry to `do_exit_current`**, so every normal exit exercises it in the introducing PR. Also fills Codex's named P8 gaps: tombstone control flow and one-at-a-time FD acquisition | **§2.5** (new); PLAN P8 |
| **5** | The fatal-signal convergence phase makes the delivery result **intent-only** and deletes the legacy Terminated-channel parent-notification action in the **same** PR | `DeliverResult::Terminated` (and its documented "caller MUST call notify") is **deleted**, replaced by `DeliverResult::FatalIntent{pid,tid,sig,code}` which is documented as performing no notification, no status write and no wake; notification is discharged solely by the P6b ledger | §2.1, §2.6 (new), §3 AC-12; PLAN P11 |
| **6** | P0's counters wire into **named** existing teardown write-sites with at least one causally-paired nonzero defer/reclaim test; state the honest PR count; delete the false P2/P3/P4/P5 file-disjointness claim | PLAN P0 now names every write site with file:line and marks which counters are legitimately zero until a later phase; adds `fork_exit_defer_reclaim_pairing_test` (nonzero, per-pid paired). Honest count: **13 numbered phases / 16 PRs**. The parallel-merge claim is deleted; all phases merge sequentially | §7 OQ-8; PLAN P0, §0 graph, §0 PR ledger |
| **7** | OQ-1 decided (coordinator-adopted): protected-init goes **Linux-faithful** — user-originated signals to designated init with no handler are **silently dropped, send returns success**, NOT `EPERM`; only init's own exit/exit_group or an unhandleable fatal fault is kernel-fatal | §2.2 death policy rewritten; §1.3 init row rewritten; §7 OQ-1 marked **DECIDED**; the `EPERM` claim (and its false "Linux-fidelity" label) is removed everywhere | §1.3, §2.2, §3 AC-2, §7 OQ-1; PLAN P12 |

**Also folded in (Codex per-phase notes that named gaps, not conditions):** P8's hook-activation /
tombstone / FD-acquisition control flow (§2.5); P7's statement that it does not supply P6's missing
prerequisite (that is P6a's job); P2's UAF reduction restated with the receipt discipline.

**Explicitly unchanged from v1** (not criticised, preserved verbatim): §0 adjudication in full, §1.1,
§1.2, §1.5's rejection of `ExitPending`, §2.3 (#471 detach + seal), §2.4 (fault-victim attribution),
§5 (scope cuts), §8 (lessons register) except for two added rows, and every residual not listed above.

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
   progressively upgraded to B's victim-owned exit in Phases 8–10c. We deliberately pay for the SIGKILL
   arm twice rather than leave a confirmed UAF live behind ten PRs. This is the direct answer to
   Judge 1's fatal flaw against B, and it is why A's contribution is real even though A lost.
2. **No phase ships dormant code.** Every new API has a live caller in the PR that introduces it
   (`do_exit_current` ships with normal `exit` as its first and *only* entry — §2.5; the request/wake
   mechanism ships with the SIGKILL path as its live producer — §2.1). No `#[allow(dead_code)]`, no
   "activated later" phase. *(v2 refinement, condition 1: where a phase ships a predicate with two
   live arms — boundary-reachable vs legacy — **both** arms are exercised in that PR, and the legacy
   arm is a ratcheted allowlist that shrinks to empty and is then deleted.)*

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

**v2 additions to the anchor set** (verified during the revision, needed by conditions 3 and 6):

- `PENDING_PROCESS_RECLAIMS` is a `spin::Mutex<Vec<PendingProcessReclaim>>` at `process_task.rs:97`.
  Its drain, `reclaim_deferred_process_resources` (`process_task.rs:375-392`), evaluates
  `retirement_grace_elapsed(&reclaim.after_epoch) && !reclaim.root_is_live()` **while holding the
  queue lock**. Both predicates are lock-free atomic reads today; P1's richer `RootProof` would add
  scheduler-cached-root and live-row blockers, which take SCHEDULER and PM — that is the interim
  violation condition 3 forbids, and §4.3 restructures the drain to prevent it.
- There are exactly **two** `enqueue_process_reclaim` call sites, and **both are inside a live PM
  guard**: `manager.rs:1152` (in `exit_process`) and `process_task.rs:244` (in `handle_thread_exit`
  phase 1). Both take the reclaim-queue lock nested inside PM+DAIF-masked. Both are converted to
  return-a-receipt in P2 (condition 3).
- `terminate_process_threads` has five call sites: the four EL0 fault sites
  (`arch_impl/aarch64/exception.rs:768,1135,1230,1333`) and, after P2, `syscall/signal.rs`.
- `SGI_RESCHEDULE` is sent from `scheduler.rs:1857` and `:1886`; `SGI_RESCHEDULE = 0`
  (`arch_impl/aarch64/constants.rs:85`); the handler dispatches at `exception.rs:1761`.
- The `#492` overflow is `DeferredFaultExitBuffer::push` returning `false` at `process_task.rs:43`
  (caller `defer_fault_sigsegv_exit`, `process_task.rs:352-360`) — dropped silently today.
- `waitpid`'s reap physically removes the row: `syscall/wait.rs:385` calls
  `manager.remove_process(child_pid)` → `manager.rs:1101-1104`. This is the row-lifetime seam
  condition 2 closes with the tombstone gate.
- `Process::terminate` (`process.rs:284`) calls `close_all_fds()` (`process.rs:294`, impl at `:347`);
  `terminate_minimal` (`process.rs:320`) early-returns on a repeat pass;
  `take_fd_entries` (`process.rs:335`) is the allocating extractor used at `process_task.rs:236`.
- `retirement_grace_target`/`retirement_grace_elapsed` are `scheduler.rs:550`/`:563`;
  `is_kernel_stack_slot_live` is `memory/kernel_stack.rs:283`.

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
       │        scope with one batch id; creates DURABLE work obligations in each row
       │        (Absent -> Pending, §1.6); seals the group against clone/exec admission.
       │        Moves nothing, frees nothing, allocates nothing, logs nothing, takes no
       │        second lock. Returns #[must_use] ExitRequestResult + a preallocated
       │        fixed receipt.
       ▼
 S2  KICK       PM dropped. Scheduler transaction publishes the exit request onto each
       │        member's scheduler thread and readies proven-killable blocked victims
       │        WITH their continuation intact. It NEVER sets a remote thread Terminated
       │        once that member's wait family is migrated (§2.1 Increment 2). Returns
       │        ExitKickPlan. Then, holding NO lock: local need_resched + BROADCAST
       │        SGI_RESCHEDULE to other online CPUs when the plan asks for it.
       ▼
 S3  SELF-EXIT  Each victim runs do_exit_current() on ITSELF at its next safe boundary,
       │        entered ONLY from the return-boundary hook (§2.5): claim (atomic) →
       │        local TTBR0 leave (hardware first, shadows after) → short PM commit
       │        (mark zombie, record first status, claim own obligations, decide
       │        last-reference, move root into a retirement receipt) → DROP PM → all slow
       │        work unlocked (one FD at a time, futex/clear_child_tid, bounded reparent
       │        cursor, redeem each claimed obligation and mark it Completed under a
       │        fresh PM acquisition, enqueue receipt with PM dropped) → pivot to neutral
       │        stack → mark ONLY SELF Terminated → schedule away.
       ▼
 S4  RETIRE     GRACE FIRST: RetirementFence (two epochs on every CPU in the captured
       │        online mask, acquire-ordered; an empty mask is INVALID, never "elapsed").
       │        THEN RootProof + StackProof, with NO queue lock held (§4.3). Refusal
       │        records a per-blocker counter and rotates. Physical free runs with NO PM
       │        and NO scheduler lock held. This gate also removes tombstoned rows (§1.6).
       ▼
 S5  ESCALATE   Init-death latch read in ordinary kernel context, all guards dropped,
                DAIF restored, pre-panic lock/IRQ snapshot recorded, THEN panic.
```

### 1.3 Per-path variance — every deviation named

| Path | S0 | S1 scope | S2 | S3 | Named variance |
|---|---|---|---|---|---|
| normal `exit(2)` | self TID | Member | no-op (self) | self | S3 is entered from the return-boundary hook, never inline in the syscall body (§2.5) |
| `exit_group` | self TID | ThreadGroup | kick siblings | each member | none |
| EL0 fatal fault | faulting TID (CR3 cross-check) | ThreadGroup | kick siblings | **this** thread, via the fault site's existing mandatory redirect tail | S3 is entered from a redirect the fault site already performs unconditionally; the tail never branches on the teardown result |
| SIGKILL / default-fatal signal | target PID | ThreadGroup | kick all | each member | sender never touches victim resources; `deliver_default_action` **returns an intent only** and performs no notification (§2.6) |
| init death | committed PID only | as above | as above | as above | **v2 (cond. 7):** user-originated fatal signals never reach S1 for the designated init — they are dropped at send and the send returns success. Only init's own `exit`/`exit_group` or an unhandleable fatal fault reaches S1, latches, and escalates at S5 |
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
                    first_status, live_members, mm_owner_pid }

// Row-resident. The process row IS the work node — no collection grows during exit.
// v2 (condition 2): each obligation carries an explicit four-state lifecycle, not a bool.
enum Obligation { Sigchld, ParentWake, Report, Reparent, Fds, Resources }   // 6 slots, fixed
enum ObligationState {
    Absent,                                   // never created for this row
    Pending,                                  // created at first request; unowned
    Claimed { claimer: ThreadId, at: RetirementFence },  // exactly one owner, in flight
    Completed,                                // discharged; terminal
}
ExitLedger { state: [ObligationState; 6],     // fixed-size array; never allocates
             first_status: Option<i32>, batch: GroupBatchId }

// v2 (condition 2): row lifetime must outlive every obligation.
RowState { Live, Zombie, Tombstone { reaped_by: ProcessId, status: i32 }, /* then removed */ }
teardown_next: Option<ProcessId>              // intrusive link; no collection grows

// Scheduler/boundary mirrors. Release/acquire. Carry NO ownership, authorize NO free.
GroupExitWord { generation, cause, active }
ThreadExitRequest { generation, reason, state }

// Proof values (from C).
RetirementFence { epochs: [u64; MAX_CPUS], online_mask }
RetirementSnapshot                       // proves Acquire loads + fence ran before liveness reads
RootProof { blocked_epoch, blocked_hw, blocked_shadow, blocked_cached, blocked_live_row }
```

`ExitLedger`'s `Resources` obligation subsumes design A's proposed `ResourceState{Held,HandedOff}` —
"the page table has left this row" is just another durable obligation, not a second parallel key.
This is a real simplification the panel surfaced: A needed a separate field only because it had no
ledger.

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
**uninterruptible**.

> **v2 (condition 1) — what "uninterruptible" actually does, and why the sequence works.**
> v1 said an uninterruptible victim "stays latched and dies at its next natural safe boundary". Under
> the corrected phase order the request/wake mechanism (P9) ships *before* any wait family is
> migrated (P10a/b/c), so at P9 **every** family is unmigrated — and "latch and hope" would be a kill-latency
> regression against the P2 behaviour already merged. It is therefore replaced by an explicit,
> named, two-armed predicate that ships with both arms live:
>
> ```rust
> /// True iff this victim will demonstrably reach the return-boundary hook (§2.5)
> /// without external help: it is runnable/running (EL0 or a preemptible kernel path),
> /// or it is blocked in a wait family whose victim-owned cancellation has been
> /// audited and tested (P10a/b/c).
> fn exit_request_is_boundary_reachable(t: &SchedThread) -> bool
> ```
>
> - **true** → publish `ThreadExitRequest`, **never** mark `Terminated` remotely; the victim commits
>   its own exit at the hook.
> - **false** → publish the request *and* fall back to the legacy remote mark + `exit_process` route,
>   which is exactly the already-merged, already-gated P2 behaviour — no new mechanism, no new
>   hazard, no latency regression. Counted per family as `EXIT_LEGACY_REMOTE_MARK{family}`.
>
> Each of P10a/b/c moves one family from the false set to the true set; the false set is a ratcheted
> allowlist that **P10c drives to empty and then deletes along with the fallback arm**. This is the
> same shrink-to-empty discipline the `\.terminate\(` allowlist already uses, and it is what makes
> every wait-family PR a *consumer of a producer that already exists* rather than the reverse.
> Residual R-2 is restated accordingly, and the new interim is disclosed as R-16.

### 1.6 The exactly-once ledger — claim protocol and row retention *(v2, condition 2)*

Design C discharged exactly-once notification with a **single-worker serialization**: one kthread was
the only redeemer, so two producers could not both redeem. The synthesis dropped the worker (OQ-6,
Tier-2 blast radius) and v1 did not replace what the worker was proving. Concurrent redeemers — the
victim's own commit path and `handle_thread_exit` (which the fault-drain at `process_task.rs:369`
and the syscall path at `handlers.rs:169` both reach) — were left mechanically unserialized. That was
the second FATAL finding. This section supplies the replacement.

**The serializer is the PM lock. There is no worker and no second lock.**

Every state transition below happens **under the PM guard and nowhere else**, and no transition takes
a second lock, allocates, or logs. Reading-then-writing the state within one PM acquisition is what
makes the claim atomic; no CAS is required because PM already excludes.

| # | Transition | Who may perform it | Rule |
|---|---|---|---|
| T1 | `Absent → Pending` | S1's request transaction only | Only on the **first** request for that row. A repeat request observes non-`Absent`, creates nothing, writes no second status, and returns the stored batch/status |
| T2 | `Pending → Claimed{me, fence}` | any control path that will discharge it **immediately** | Claim and the start of work are in the same control path; `fence` is the retirement fence captured at claim time (used only by T4) |
| T3 | `Claimed{me} → Completed` | **only** the claimer | Performed under a *fresh* PM acquisition after the work completed outside PM. `claimer == current_tid` is asserted; a mismatch bumps `LEDGER_CLAIM_MISMATCH`, which CI asserts is 0 |
| T4 | `Claimed{dead} → Pending` | **only** the S4 retire/reap gate | Permitted only when the claimer is proven not live by P1's machinery (scheduler thread absent or `Terminated`) **and** the claim-time fence has elapsed. Bumps `LEDGER_CLAIM_ORPHANED` |

**Sole-redeemer invariant (this is what replaces C's single worker):** *at most one control path is
ever in `Claimed` for a given obligation, because every entry into `Claimed` reads the current state
and writes the new one inside one PM acquisition.* Two producers racing to notify no longer both
notify: whichever acquires PM first claims; the other observes `Claimed` or `Completed` and does
nothing. The obligation is still **shared** — a later pass redeems it if the first never claimed it —
so this is exactly-once, not suppression (the suppression theory was disproven in review and is not
reopened; see §8).

**Why a boolean is not sufficient.** A single "done" bit set at claim time is exactly-once but
(a) cannot distinguish "in flight" from "discharged", which makes the AC-12 equality
`SIGCHLD_FIRST_SET == PARENT_WAKE_COMPLETED == BTRT_EXIT_REPORTED == parented_first_commits`
unassertable at any instant, and (b) cannot recover an abandoned claim, so a claimer that dies
between claim and discharge loses the notification **permanently and silently**. The tri-state (plus
`Absent`) costs one extra bit per obligation and makes both properties checkable. T4 is the recovery
path; without it, exactly-once would be purchased with at-most-once.

**Row retention — the reap/tombstone gate.** Obligations are row-resident, so the row must outlive
every obligation. Today `waitpid` physically removes the row (`syscall/wait.rs:385` →
`manager.rs:1101-1104 remove_process`), and the `Resources` obligation **by construction outlives
reap** — grace and RootProof are not complete when the parent collects the status (that is R-13, and
it is a property of the design, not an accident). Shipping row-resident resource bits before fixing
row lifetime is the P6-before-P7 seam the ratification flagged. Therefore:

> **Retention rule.** A row is removed from the process table only when (a) every obligation in its
> ledger is `Completed` or `Absent`, **and** (b) its retirement receipt has been retired (grace
> elapsed + RootProof passed) or was never created. `waitpid` no longer removes the row: it records
> the reap and transitions `Zombie → Tombstone{reaped_by, status}`. A `Tombstone` row is invisible to
> every lookup that means "a live process" — `find_process_by_pid/thread/cr3`, signal delivery, wait
> scanning, procfs enumeration, and PID-reuse allocation — and visible only to the ledger and the
> retire gate. The gate that finally removes it is the same S4 drain that retires resources.

Two consequences, stated so they are not discovered later: PID reuse is delayed until the tombstone
is removed (strictly safer than today, and bounded by the same grace the resources already wait on),
and a leaked tombstone is a *visible* leak — `TOMBSTONE_RESIDENT` is a counter with a reader, asserted
to return to zero at quiesce in every boot gate.

**Phase consequence.** The retention gate ships as **P6a** and the tri-state ledger as **P6b**, in
that order, and no phase before P6a makes any obligation row-resident that can outlive reap. P2's
seed obligations (`Report`, `Sigchld`) are exempt and may ship first, because both are discharged no
later than the zombie transition and a parent cannot reap a process that has not reached zombie —
so they cannot outlive the row even under today's removal semantics. `Resources` is not exempt, and
is the reason P6a exists.

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
grace-stamped `defer_process_resources` on aarch64 **before** `terminate()` runs, so
`cleanup_cow_frames()` walks a `None` page table and the CoW decref moves behind grace + RootProof.
Plus the durable `report`/`sigchld` obligation seed (below).

> **v2 (condition 3) — the receipt leaves PM before it is enqueued.** v1 relied on `exit_process`'s
> existing body, which calls `enqueue_process_reclaim` at `manager.rs:1152` **inside the PM guard**,
> nesting the reclaim-queue lock under PM+DAIF-masked. That is the A-style interim shape the adopted
> no-overlapping-lock rule (§4.1) forbids and the B end-state must rip out — an interim violation is
> still a violation. Phase 2 therefore changes the signature:
>
> ```rust
> #[must_use]
> pub fn exit_process(&mut self, pid: ProcessId, exit_code: i32) -> Option<RetirementReceipt>;
> ```
>
> `exit_process` *stamps and takes* the root into the receipt under PM (as today) but **returns** it;
> `with_process_manager(...)` yields the receipt to the caller, the PM guard drops, and only then does
> the caller `enqueue_process_reclaim(receipt)` with no other lock live. The move is allocation-free:
> `PendingProcessReclaim` is a fixed-size value and `core::mem::take(&mut process.pending_old_page_tables)`
> leaves an empty `Vec` behind without allocating. The **other** PM-nested enqueue,
> `handle_thread_exit` phase 1 at `process_task.rs:244`, is converted in the same PR by riding the
> receipt out through the existing `phase1_result` value and enqueuing it in phase 2, where PM is
> already dropped. After Phase 2, `RECLAIM_ENQUEUE_UNDER_PM == 0` is assertable and ratcheted.

What Increment 1 does and does not buy, stated exactly:
- **Removes** the eager CoW walk while the victim may be remote — the confirmed UAF class. *Strict
  reduction of work under PM+mask, not an addition:* today's SIGKILL does the full walk there.
- **Adds** quarantine, expedite, and the missing SIGCHLD.
- **Removes** (v2) both pre-existing reclaim-queue-under-PM nestings.
- **Does not** yet make the exit victim-owned (still remote-marks), and **does not** yet move FD
  closure out of PM (`exit_process` → `terminate()` → `close_all_fds` is pre-existing at that
  convergence point; unchanged, not worsened; fixed in Phase 7).

Two defects the panel found in this exact mechanism are fixed *in Increment 1*, not deferred:
- *Missed SGI from a stale `cpu_state[]` read* (Judge 2, fatal against A's AC-11): the expedite
  **broadcasts** to other online CPUs rather than deriving a residency mask. B's argument is
  decisive at Breenix's CPU count — a constant number of extra SGIs beats a fallible mask, and the
  SGI handler already does nothing but set `need_resched`. There is no stale read to be wrong about.
- *Lost `btrt` report / parent wake* (Judge 2, fatal against A's AC-12): `btrt::on_process_exit` has
  exactly one call site, inside `handle_thread_exit` (`process_task.rs:267`), which a remotely-marked
  victim may never run. Increment 1 therefore installs the **durable report obligation** at commit: a
  row obligation moved `Absent → Pending` once at first commit and redeemed exactly once, outside PM,
  by whichever of {commit path, `handle_thread_exit`} **claims** it first (§1.6 T2/T3 — the seed
  already uses the four-state machine, it is not a bool that P6b later upgrades). This is the seed
  that Phase 6b generalizes into the full ledger. It is small, and it must land with the remote-kill
  path, not six phases later.

**Increment 2 (Phases 8 → 9 → 10a/b/c) — make it victim-owned.** *(v2, condition 1: the phase order
here is the corrected one — the request/wake producer at P9 precedes every wait-family consumer.)*

1. **P8** introduces `do_exit_current` and the return-boundary hook, with normal `exit(2)` as its
   first and only entry (§2.5).
2. **P9** redefines `terminate_process_threads` to *request and wake*: publish `ThreadExitRequest`,
   return `ExitKickPlan`, and **suppress the remote `Terminated` commit for every boundary-reachable
   victim** (§1.5). SIGKILL becomes a group-scoped `ExitIntent` and the group seal ships. Victims
   blocked in a not-yet-migrated wait family take the counted legacy arm — the merged P2 behaviour,
   unchanged.
3. **P10a/b/c** migrate the wait families one per PR, each consuming the request/wake mechanism P9
   already publishes and each proving deregistration-before-commit for its family. **P10c** empties
   the legacy allowlist and deletes the remote-marking body; only then is #491 complete.
4. **P11** deletes the last direct `Process::terminate` callers (§2.6), at which point
   `Process::terminate` itself is deleted.

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

**Death policy (Phase 12) — v2 (condition 7): Linux-faithful protected init, silent drop, no `EPERM`.**

v1 recommended rejecting user-originated fatal signals to the designated init with `EPERM` and called
that "Linux's protected-init intent". The ratification is correct that this is **not** what Linux
does: Linux drops signals whose action would be the default fatal one when the target is protected
(`SIGNAL_UNKILLABLE` / init in its namespace), and the **send path returns success** — `kill(1, SIGKILL)`
from userspace returns 0 and nothing happens (`kernel/signal.c`, `prepare_signal`/`sig_task_ignore`
region ~L79-117 and the `__send_signal`/`complete_signal` region ~L977-1083). The coordinator has
**adopted the Linux-faithful behaviour**:

> **Adopted policy.** A user-originated signal delivered to the **designated init** whose effective
> disposition is the *default fatal action* and for which init has installed **no handler** is
> **silently dropped**: it is never queued, never made pending, never delivered, and the sending
> syscall **returns success (0)**. A signal init *has* a handler for is delivered and handled
> normally — protection is disposition-scoped, not signal-scoped, exactly as in Linux. Only two
> things are kernel-fatal: init's **own** `exit`/`exit_group`, and an **unhandleable** synchronous
> fatal fault taken by init (or a nonviability invariant). Kernel-originated forced fatal signals are
> not user-originated and are out of scope — Breenix has none targeting init today, and if one is
> ever added it must be an explicit `force_sig`-equivalent that states its intent.

Consequences that must not be discovered later, so they are stated here: `EPERM` is **not** returned,
so no ABI divergence has to be documented and no test may assert `EPERM`; the drop is **counted**
(`INIT_FATAL_SIGNAL_DROPPED`) so the behaviour is observable rather than invisible; and because the
drop happens at send time, init's group is never sealed and `INIT_DEATH_LATCH` is never set by an
external kill — the latch's only producers are init's own exit commit and the unhandleable-fault
path, which is a strictly smaller and more auditable set than v1's.

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

> **v2 (condition 4, second half) — why P3 is a hard prerequisite of P8, not just of P9.** P8's
> victim commit makes the **last-reference decision** and builds the `RetirementReceipt` from the
> row's own root plus `inherited_cr3`. A row that exec'd while still naming a stale `inherited_cr3`
> would present a root it does not own to `RootProof`, which either blocks retirement forever
> (`blocked_live_row` against a root nobody owns) or, worse, contributes a second claimant for one
> root. Exec detach must therefore precede victim-owned exit, not merely precede group-scoped kill.
> The dependency graph in the PLAN carries the `P3 → P8` edge explicitly; v1 omitted it.

**The seal is the mark (Phase 3 admission + Phase 9 cutover).** There is no separate "snapshot then
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

### 2.5 The return-boundary hook — activation, tombstone, and FD acquisition *(v2, condition 4)*

The ratification's fourth condition and its P8 note ("hook activation/tombstone/FD-acquisition need
explicit control flow") are answered here rather than left to the implementation.

**Activation — the hook is the ONLY entry into `do_exit_current`, so normal exit exercises it in the
same PR that introduces it.** There is no second, inline call path that could let the hook lie
dormant:

```
sys_exit(code)                                  [ordinary syscall context, no guard held]
  └─ S1: one PM transaction on SELF — record first status, create obligations
         (Absent -> Pending), mark row ExitRequested                      … PM dropped
  └─ release-store ThreadExitRequest{state: Latched} on OWN scheduler thread
  └─ return normally to the syscall return path            (no teardown work done here)

<syscall/exception return path>
  └─ existing PREEMPT_ACTIVE / nested-return gate                     (unchanged, first)
  └─ HOOK: acquire-load of the per-thread exit-request word            (one load + branch)
        └─ if Latched -> do_exit_current()   [preemptible kernel context, no guard held]
```

The hook is placed **after** the `PREEMPT_ACTIVE`/nested-return gate; it allocates nothing, locks
nothing, walks nothing, drains nothing, and logs nothing. Because `sys_exit` performs *no* teardown
inline, every normal `exit(2)` in the introducing PR — i.e. essentially every process the boot runs —
traverses the hook. The gate asserts `EXIT_HOOK_ENTRIES > 0` and
`EXIT_HOOK_ENTRIES == EXIT_COMMITS` on a normal boot; a dormant hook fails the phase.

**Tombstone control flow at commit.** The victim never removes its own row and never decides its own
reaping:

```
do_exit_current():
  claim(atomic)                       -- one-shot; a second entry returns immediately
  local TTBR0 leave                   -- hardware first, shadows after
  PM #1 (short):  Live -> Zombie; first_status recorded; T2-claim {Fds, Resources,
                  Sigchld, ParentWake, Report, Reparent} that THIS path will discharge;
                  last-reference decision; root moved into RetirementReceipt      … drop PM
  slow work, no guard held:  FD closes (below), futex / clear_child_tid,
                  bounded reparent cursor (PM re-acquired per fixed batch),
                  notification redemption; each obligation T3 -> Completed under its
                  own fresh PM acquisition                                        … no overlap
  enqueue RetirementReceipt           -- reclaim-queue lock only, PM already dropped
  pivot to neutral stack; mark ONLY SELF Terminated; schedule away
```

The row then sits `Zombie` until a parent reaps it (`Zombie → Tombstone`, §1.6) and is removed by the
S4 retire/reap gate once the ledger is fully `Completed` and the receipt has retired. Auto-reap cases
(no parent, or the parent itself exiting) are handled by the reparent cursor re-parenting the child
to the designated init before the parent's own row tombstones — the victim never short-circuits the
gate.

**FD acquisition control flow (P7's API, used here).** Exactly one descriptor crosses each PM
acquisition, and the obligation states bracket the loop:

```
T2: Fds -> Claimed{me}                            [inside PM #1]
loop {
    let entry = with_process_manager(|pm| pm.take_next_for_exit(pid));   // PM held: move ONE
    match entry { None => break, Some(e) => { /* PM dropped */ close(e); } }
}                                                  // endpoint locks only, never with PM live
T3: Fds -> Completed                              [fresh PM acquisition; table proven empty]
```

`take_next_for_exit` moves one `(fd, FileDescriptor)` out of the table into a fixed-size single-slot
receipt; it never builds a `Vec`, which is why the allocating `Process::take_fd_entries()`
(`process.rs:335`) is retired rather than reused. If the victim is preempted mid-loop the ledger
still reads `Claimed{me}`, and T4 (orphan recovery) can only fire if `me` is proven dead — which
cannot happen while `me` is the running victim.

### 2.6 Fatal-signal delivery is intent-only *(v2, condition 5)*

v1's Phase 11 carried the fatal outcome out of `deliver_default_action` through the **existing**
`DeliverResult::Terminated` channel, whose documented contract is "caller MUST call notify after
releasing the PM lock". But §1.6/P6b assigns notification to the ledger. Leaving both alive makes a
duplicate notify reachable in exactly the window the round is trying to close. So:

- `DeliverResult::Terminated` **and its caller-side notification action are deleted in the same PR**
  that introduces the replacement. No overlap window exists, not even one phase wide.
- The replacement is `DeliverResult::FatalIntent { pid, tid, sig, code }`, documented as
  **intent-only**: *it performs no notification, no parent wake, no status write, no scheduler
  mutation, and no resource work.* Its only legal use is to be handed to the S1 request transaction
  after the PM borrow ends.
- All notification for that death is discharged by the ledger obligations created in S1 and redeemed
  by the victim's own commit — one producer, one claim, one redemption.
- The P0 ratchet gains a rule for this: no notification/wake call may appear in any
  `deliver_*`/`DeliverResult` consumer. The `Terminated` variant's absence is asserted by name so it
  cannot be reintroduced by a later phase.

This also preserves the three v1 wins of that phase, which the ratification did not dispute: the FD-closure
loss that design A's `terminate_minimal` hand-off would have caused, the SCHEDULER-under-DAIF-masked-PM
inversion at `scheduler.rs:3101-3109`, and a `log::info!` under mask all disappear with the two
mutating blocks at `signal/delivery.rs:224-239` and `:258-269`.

---

## 3. Numbered traceability — all 13 acceptance criteria

*(Rows whose mechanism changed in v2 are marked; unmarked rows are v1 verbatim. Phase numbers reflect
the corrected order: old P10 → **P9**, old P9a/b/c → **P10a/b/c**, old P6 → **P6a + P6b**.)*

| # | Criterion | Mechanism | Phase | Evidence that must be produced |
|---|---|---|---|---|
| **1** | Init designation only after creation fully succeeds; no phantom PIDs | Held-publication ticket: row inserted **and** non-runnable scheduler thread created before `designated_init` is committed under PM; PID 1 reserved off-table so a failed attempt leaves no row, no designation, and PID 1 retryable. Nothing in `create_*` touches designation. | 5 | Failure injection after PID selection at each fallible stage (page table, ELF, stack, publication): `designated_init() == None`, no row; retry succeeds as PID 1 |
| **2** *(v2 — cond. 7)* | No panic/fatal action while PM held with DAIF masked | S1 does **one relaxed store** to `INIT_DEATH_LATCH`. All fatal escalation is a receipt redeemed at S5 with PM and scheduler guards out of scope and DAIF restored; a pre-panic lock/IRQ snapshot is recorded first. No `#[cfg]`, so no build carries a differently-scoped variant. **The latch's producer set shrinks under the adopted Linux-faithful policy: external fatal signals to init are dropped at send and can never latch, so only init's own exit commit and an unhandleable fault remain.** | 12 | `INIT_PANIC_WITH_LOCK == 0`; injected init death records PM owner `None`, scheduler owner `None`, IRQ state normal immediately before panic; the panic's serial output is **complete** (proving no lock was held). **`kill(1, SIGKILL)` from userspace returns 0, `INIT_FATAL_SIGNAL_DROPPED` increments, init survives, `INIT_DEATH_LATCH == 0`; no test asserts `EPERM`** |
| **3** | Victim attribution certain before fatal escalation; a heuristic CR3 miss must not panic | §2.4: TID-first, stack-slot cross-check, CR3 as root-consistency only, divergence counted and never fatal, `AttributionUncertain` → safe redirect + deferred intent. The latch keys on a **committed** victim, never on a resolution attempt. | 11 (attribution), 12 (escalation) | `clonevm_fault_test`: a CLONE_VM child faults → the **child** row dies, the parent survives, no refault loop. *This test fails on `main` today* — it is a live bug fixed, not a tautology. TID/CR3 mismatch injection bumps `EXIT_ATTRIBUTION_UNCERTAIN`, latches nothing, kills nobody |
| **4** | One source of truth for init identity; no hardcoded PID 1 beside runtime designation | `ProcessManager::designated_init` is the sole authority; the three production literals (`manager.rs:1178`, `process_task.rs:226`, `:285`) and `signal.rs`'s `INIT_PID` become the accessor. `ProcessId::INIT` survives only as the ABI *validation* constant, never a lookup. | 5 | P0 ratchet fails on any new `ProcessId::new(1)` in production teardown/wait/signal/reparent code; the three `test_userspace.rs` sites are allowlisted **by name** |
| **5** | Kernel and userspace init guards must agree (`init_shell` keys on `getpid()==1`) | Structural, not conventional: PID 1 is **reserved** for the explicit init constructor and production designation is **validated == PID 1 and refuses otherwise**. `init_shell.rs:1028` is not changed, so no second contract exists to drift. A non-PID-1 init would require an explicit userspace ABI change and is not silently supported. | 5 | Cross-tree source assertion + boot test: designated pid == 1 == the pid `init_shell` observes; a build with no real init leaves designation unset and does **not** treat whichever process got the low PID as init |
| **6** | exec detaches `thread_group_id` **and** `inherited_cr3` | Both assignments at every exec commit point, after all fallible work, before PM release; both preserved on every failure; existing live-sibling guard retained. Both arches in one commit. **(v2: also a hard prerequisite of P8's last-reference decision — §2.3.)** | 3 | `clonevm_exec_test` extended: successful exec → both `None`, fresh root, effective TGID == pid, and a kill aimed at the **old** group cannot reach it; failed exec → both preserved byte-identical |
| **7** | Group membership examined atomically; no snapshot stale across a PM drop | **No snapshot exists.** The group-exit PM transaction *is* the seal: mark every live effective-TGID member with one batch id inside one guard; `sys_clone` publication validates group lifecycle under the same lock; scheduler threads carry group id + generation and re-check before first dispatch; threads publish non-runnable until the row is published. | 3 (admission), **9** (cutover) | Deterministic clone-vs-seal barrier test: the child is either included in the batch or `sys_clone` returns `EAGAIN` — never a runnable unrequested member. Ratchet rejects any group PID `Vec` snapshot in teardown code |
| **8** | Sibling kernel stacks freed ONLY behind two-epoch grace via scheduler ownership | All three creation paths — fork, CLONE_VM clone, **and spawn/direct-init/test-disk** — transfer `kernel_stack_allocation` to the scheduler-owned copy before the thread can run; a `Process` row can no longer synchronously drop a published stack. **(v2: the ungated free this closes was via `remove_process` at `waitpid` reap — that call site is itself replaced by the tombstone gate in P6a, so the two fixes meet rather than overlap.)** | 4 | Ownership assertion after every creation path (exactly one owner, and it is the scheduler copy); 1000-iteration fork/clone/spawn exit stress with stack-pool accounting (allocated == freed) and an allocator assertion that never selects a live slot |
| **9** | No N-member FD/resource teardown loop in one PM-locked, IRQ-masked section | Each victim takes and closes **only its own** descriptors, **one at a time**: `take_next_for_exit()` under PM → drop PM → close (explicit control flow: §2.5). The existing allocating `take_fd_entries() -> Vec` (`process.rs:335`) is retired, not reused. Group work under PM is bitmap/flag stores only. No sweep loops over members' FD tables. | 7 | `FD_CLOSES_UNDER_PM == 0`; 256-FD × large-group test measures bounded PM hold; ratchet forbids close/reclaim calls inside any request/commit transaction body |
| **10** *(v2 — cond. 1, 3)* | No eager `cleanup_cow_frames` while the victim may run elsewhere; all kill paths grace-defer | Phase 2 routes SIGKILL through `exit_process`, whose aarch64 defer takes the page table into a grace-stamped **receipt returned to the caller and enqueued only after PM drops** (§2.1), so the CoW walk becomes a `None`-walk and no queue lock nests under PM. Phases 9–11 remove every direct `terminate` caller; resources stay in the row until the victim's own commit, and release requires grace + RootProof. `Process::terminate` is deleted with its last caller. | 2, 7, **9**, 11 | Ratchet allowlist of `\.terminate\(` shrinks phase-by-phase to **empty**, asserted as an exact set; **`RECLAIM_ENQUEUE_UNDER_PM == 0` from P2 onward**; peer-CPU SIGKILL stress asserts zero reclaim before the fence elapses and a complete RootProof; `TEARDOWN_MASKED_FRAMES_WALKED == 0` for kill paths |
| **11** *(v2 — cond. 1)* | Killed threads quiesced in the scheduler AND expedited with the existing `SGI_RESCHEDULE` | Phase 2: quarantine via the proven `terminate_process_threads` + **broadcast** `SGI_RESCHEDULE` to other online CPUs (no residency predicate to go stale). **Phase 9 (was 10): the scheduler *requests* instead of remote-marking for every boundary-reachable victim, returns `ExitKickPlan`, and takes the counted legacy arm only for not-yet-migrated wait families. Phases 10a/b/c migrate the families; P10c empties the legacy allowlist and deletes the remote-marking body — that is where AC-11 is fully discharged.** The SGI handler keeps doing only `need_resched`; the victim can never be re-dispatched to EL0 once the generation is observed. **No `ExitPending`.** | 2, **9, 10a/b/c** | Victim spinning at EL0 pinned to a peer: `EXIT_SGI_SENT > 0`, target observes it, time-to-`Terminated` bounded by an SGI round trip rather than the victim's next natural tick; per-wait-family blocked-victim kill tests; **`EXIT_LEGACY_REMOTE_MARK{family}` reported per family and asserted to reach 0 with the allowlist empty at P10c**; unmigrated families explicitly reported, never silently advertised as killable |
| **12** *(v2 — cond. 2, 5)* | Exactly-once SIGCHLD/wake/report with **first-recorded** status, idempotent under repeat passes | **Four-state obligations (`Absent/Pending/Claimed{claimer}/Completed`, §1.6), all transitions under the PM lock, which is the sole serializer — this restores what design C's single-worker serialization discharged after the worker was dropped.** Created at the **first** request; a repeat request returns the stored batch/status and creates no second obligation and no second status. At most one control path is ever `Claimed`, so the commit path and `handle_thread_exit` cannot both redeem; an abandoned claim is recoverable (T4) rather than a silent permanent loss. **Row lifetime is extended by the tombstone gate (P6a) so no obligation can outlive its row.** **The legacy `DeliverResult::Terminated` notification action is deleted in P11, so no second notifier exists.** Still not suppression: the obligation is shared, so a later pass redeems it rather than skipping it. | 2 (seed), **6a (retention), 6b (ledger)**, 11 (legacy notifier deleted) | Matrix — exit→fault, SIGKILL→fault, fault→SIGKILL, repeat request/wait: exactly one SIGCHLD, one parent wake, one `btrt` report, and `waitpid` returns the **first** status; equalities `SIGCHLD_FIRST_SET == PARENT_WAKE_COMPLETED == BTRT_EXIT_REPORTED == parented_first_commits`; **`LEDGER_CLAIM_MISMATCH == 0`, `LEDGER_CLAIM_ORPHANED == 0` on a healthy boot, `TOMBSTONE_RESIDENT == 0` at quiesce**; forced-orphan injection proves T4 recovers the notification instead of losing it |
| **13** *(v2 — cond. 3)* | New reclaim/drain respects lock ordering and is bounded on idle paths without throttling fork's drain | **No cap is added, no drain is moved, no drain is shared.** Fork's pre-allocation drain stays full/unbounded. The only addition to a return tail is an acquire-load + branch, placed **after** the `PREEMPT_ACTIVE`/nested-return gate. Every scheduler critical section is entered with **no PM guard live**, structurally: request/commit APIs take `pid`/`tid`, never `&mut Process`. **v2 adds two absolute rules: (i) P1's proof reads never hold `PENDING_PROCESS_RECLAIMS` while acquiring SCHEDULER or PM — the drain detaches a candidate under the queue lock, drops it, proves, then frees or re-inserts (§4.3); (ii) every retirement receipt is enqueued only after the PM guard drops, including in P2's interim shape.** No `FRAME_METADATA` and no `log::*` is added under any mask. Full analysis: §4. | all | Lock-depth/owner counters (with `try_manager()` instrumented — the r20 blind spot is not repeated); `RECLAIM_CONTEXT_VIOLATIONS == 0`; **`RECLAIM_ENQUEUE_UNDER_PM == 0` and `PROOF_UNDER_QUEUE_LOCK == 0`**; ratchet rejects scheduler-under-PM, queue-lock-then-PM/SCHEDULER, and any reclaim/close/walk in idle or exception tails; fork-pressure test proves a *full* eligible drain, not a capped one. **Declared partial:** this design does not fix #448's or #492's pre-existing boundedness; it must not make them worse, and §5 states the argument |

---

## 4. Lock-ordering analysis

### 4.1 The rule

Documented today: `SCHEDULER → PROCESS_MANAGER → endpoint locks`. Live teardown/signal/exec code
does not honour it consistently (signal delivery and exec update scheduler state under PM; reclaim
enqueue nests a queue op under PM at both `manager.rs:1152` and `process_task.rs:244`).

**This design adopts a strictly stronger rule for all death-path code:**

> PM, SCHEDULER, reclaim queues, FD/endpoint locks, `FRAME_METADATA`, the stack-pool bitmap, and
> SERIAL are **never held simultaneously** by teardown, signal, exec, wait, or reparent code. State
> moves between domains through fixed-size receipts and release/acquire atomics.

**v2 (condition 3): the rule binds interim shapes too.** A phase may not adopt an overlapping-lock
shape "temporarily" on the way to the end state. Two v1 shapes violated it and are corrected:

1. **P1's proof under the queue lock.** `reclaim_deferred_process_resources` (`process_task.rs:375-392`)
   evaluates its readiness predicate while holding `PENDING_PROCESS_RECLAIMS`. Today both terms are
   lock-free atomic reads, so `main` is legal; P1's `RootProof` adds scheduler-cached-root and
   live-row blockers, which would take SCHEDULER and PM **under the queue lock**. P1 restructures the
   drain instead (§4.3, "P1 retire cycle"): the under-queue-lock predicate stays **epoch + shadow
   only** (lock-free), the candidate is detached, the queue lock is dropped, and only then is the full
   proof run.
2. **P2's enqueue under PM.** `exit_process` returns a `#[must_use] RetirementReceipt` and the caller
   enqueues it after the PM guard drops; the `handle_thread_exit` enqueue is moved into phase 2 of
   that function, where PM is already released. Both conversions are in P2's PR.

This makes both nesting directions impossible for teardown, so no ordering *cycle* can exist —
rather than relying on everyone remembering which order is legal. It is enforced structurally
(pid/tid-based APIs ⇒ no live `&mut Process` borrow ⇒ no live guard; receipts returned rather than
consumed in place), by the P0 ratchet, and observed by counters — in that order of authority.

### 4.2 Lock inventory (aarch64)

| Lock | Discipline | Note |
|---|---|---|
| PM (`PROCESS_MANAGER`) | `manager()` masks **all** DAIF, then spin mutex | the hard one: everything under it is masked |
| SCHEDULER | `with_scheduler` / `lock_for_context_switch` | |
| `PENDING_PROCESS_RECLAIMS` | leaf; staging only | insertion moves a receipt; **no proof and no walk under it (v2)** |
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
| **S3 ledger claim (T2)** *(v2)* | **PM only** | inside the commit transaction; read-then-write in one acquisition; no second lock | none | none | O(1) per obligation |
| **S3 ledger completion (T3)** *(v2)* | **PM only**, fresh acquisition after the work | never overlaps the work it completes; no second lock | none | none | O(1) per obligation |
| S3 notification redemption | PM only to claim → drop → SCHEDULER only to wake → PM only to mark `Completed` | no overlap at any point; serialized by the claim, not by lock nesting | none | none | O(1) |
| **S3 receipt enqueue** *(v2 — cond. 3)* | reclaim queue only, **after** PM drop, including in P2's interim shape | leaf staging lock; no walk and no proof under it | none | none | O(1) |
| **P1 retire cycle — detach** *(v2 — cond. 3)* | **reclaim queue only** | predicate under the queue lock is **epoch + shadow atomics only**; candidate is removed into a local fixed slot; lock dropped | none | none | O(queue length) scalar scan |
| **P1 retire cycle — prove** *(v2 — cond. 3)* | fence/hardware/shadows: **no lock**; cached roots: **SCHEDULER only**; live rows: **PM only** | three sequential, non-overlapping acquisitions, **queue lock already dropped**; never nested | none | none | O(MAX_CPUS) + O(threads) + O(rows) |
| **P1 retire cycle — free or re-insert** *(v2)* | free: **no lock**; re-insert: **reclaim queue only** | refusal bumps its blocker counter and rotates | frees only | none | per-victim |
| S4 resource claim + physical free | PM only to `take` → **drop** → TLBI/CoW walk/frame free with no PM or scheduler held; preemption pinned across `FRAME_METADATA` | no heavy destructor under PM; no broadcast TLBI under PM/scheduler/IRQ-off | frees only | none | per-victim |
| S4 stack release | SCHEDULER only to prove/detach → drop → stack-pool bitmap | allocator never acquired under scheduler | none | none | one thread per pass |
| **S4 tombstone removal** *(v2 — cond. 2)* | **PM only**, after the ledger is proven `Completed` and the receipt retired | entered with no queue lock and no scheduler lock live | frees the row only | none | one row per pass |
| S5 init escalation | **none** | all guards out of scope, DAIF restored | panic path may allocate — legal here | panic only | one atomic load |
| Boundary hook (Tier-2) | **none** | acquire-load + branch, placed **after** the `PREEMPT_ACTIVE`/nested-return gate | none | none | O(1); no drain, no walk, no lock |

### 4.4 Hazards explicitly foreclosed — and one honest limit

Foreclosed: **SCHEDULER inside DAIF-masked PM** (the r23 revert class, three designs) — impossible
because every request/quarantine API is pid/tid-based. **PM inside SCHEDULER** — S1/S2 are sequential
statements, never nested closures. **Reclaim queue inside PM** *(v2)* — receipts are returned, never
enqueued in place; both pre-existing sites are converted in P2 and `RECLAIM_ENQUEUE_UNDER_PM` is
asserted 0 thereafter. **SCHEDULER or PM inside the reclaim queue** *(v2)* — the drain detaches before
proving; `PROOF_UNDER_QUEUE_LOCK` is asserted 0. **`FRAME_METADATA` under PM+mask** — removed on the
SIGKILL and fatal-signal paths in Phase 2/11, never added. **New heap allocation under PM+mask** —
receipts are fixed-size and preallocated; the allocating `take_fd_entries()` is retired in Phase 7.
**`log::*` under mask** — none added anywhere; all new diagnostics are `trace_count!`/`trace_event!`
from the lock-free framework, with `raw_serial_char()` remaining the last resort at fault sites that
already use it. **Heavy work before a safety gate** — the boundary hook goes after the
`PREEMPT_ACTIVE` gate, and no drain is added to any tail.

**Honest limit.** The runtime lock-order detector compares against `PROCESS_MANAGER_OWNER_TID`, and
that bookkeeping was *not* updated by `try_manager()` — blocking finding #8 in the parked branch's
r20 review. This design requires `try_manager()` to participate in the same instrumentation, so the
blind spot is closed for the detector — but the detector is still only a detector. **The actual
guarantee is structural** (pid-based APIs + returned receipts + the ratchet), and the counters are
asserted zero in CI so a regression is loud. No safety claim in this document rests on a
release-stripped assertion, and no correctness argument rests on a counter.

---

## 5. What this design deliberately does NOT solve

1. **#492 — unbounded fault-exit deferred drain.** The 8×16 producer and its replay are untouched.
   This design adds a readable `DEFERRED_FAULT_RING_DROPPED` counter at the silent-drop site
   (`process_task.rs:43`, `push()` returning `false`) and gives ring items a stable TID/generation,
   which makes a cursor/backpressure fix easier later. **Not made worse:** certainly-attributed fatal
   faults do not consume the ring, and no SIGKILL or group request depends on ring capacity.
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
   disposition fix sits *before* this machinery and needs no teardown change. *(v2 note: the adopted
   protected-init policy is also a pure disposition-time rule — the drop happens where the effective
   action is computed, so it composes with a future #493 fix instead of colliding with it.)*
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
   the reaper loop). OQ-6. *(v2: the worker's **serialization** role is not deferred — §1.6 replaces
   it with the PM-serialized claim protocol, so dropping the worker no longer drops a proof.)*
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

**R-2. Uninterruptible waits delay death — bounded differently in v2.** *(revised, condition 1.)* A
victim in a wait family whose victim-owned cancellation path has not yet been audited does **not**
simply latch: from P9 it takes the explicit legacy remote-mark arm (§1.5), i.e. exactly the merged P2
behaviour, so kill latency for those victims is unchanged from what has already shipped. What remains
residual is that such a victim is torn down **remotely** rather than by itself until its family lands
in P10a/b/c, and that a family with a genuinely stuck wait still blocks *prompt* victim-owned death.
The group stays sealed and its resources pinned rather than freed underneath it — a *safe* failure
(leak/stall, never early free) — and it is counted per family. Breenix has no killable-wait taxonomy
today; building one is P10's whole job and it is honestly incomplete until every family lands.

**R-3. O(group-size) fatal marking under PM with DAIF masked.** Bitmap/scalar stores only — no
allocation, no FD, no resource work — but interrupt masking scales with group size. Accepted at
current scale; a fixed-batch representation would need a second "marking in progress" state while
preserving atomic sealing.

**R-4. O(process-count) PM scans.** The group seal and the reparent cursor both scan the process map
because the `children` mirror is retained (see §5.4). Scalar, non-allocating, and batched, but not
O(1). Removing the mirror later fixes it. *(v2: tombstone rows are skipped by the "live process"
predicate but still occupy map entries until the retire gate removes them, so the scan constant grows
slightly under burst — see R-13.)*

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

**R-11. Receipt durability is an implementation obligation, not a proof.** *(revised, condition 2.)*
Receipts must be fixed-size, preallocated, non-droppable, and either redeemed synchronously outside PM
or stored in authoritative row state. An implementation that routed notification through the bounded
fault ring would violate this design. Likewise, group-control/receipt destruction must never occur
under PM or the scheduler lock — invariant plus source test, not a proof. **The claim protocol (§1.6)
narrows but does not eliminate this residual: it makes a lost obligation *detectable* (a `Claimed`
state with a dead claimer, recovered by T4 and counted) rather than silent, but the guarantee that
every claimed obligation is eventually completed still rests on the claimer being a kernel control
path that runs to completion, not on a scheduler-enforced property.**

**R-12. aarch64 panic is not stop-the-world.** Init-death panic parks the panicking CPU; peers are
not actively stopped by today's panic handler. Recorded, not fixed (OQ-1b).

**R-13. `waitpid` returns before physical retirement, and the row now outlives the reap.**
*(revised, condition 2.)* The logical corpse is reaped while the retirement receipt awaits grace; in
v2 the row itself is retained as a `Tombstone` until the ledger is `Completed` and the receipt has
retired. Consequences: PID reuse is delayed by the same grace (strictly safer than today), the process
map holds a bounded number of extra entries under burst (R-4), and a stuck obligation manifests as a
resident tombstone rather than as a freed-too-early row. `TOMBSTONE_RESIDENT` has a reader and is
asserted to return to zero at quiesce, so the failure is loud rather than silent.

**R-14. Nonleader exec keeps its PID (Linux transplants the leader's).** Observably different for
programs comparing thread IDs across exec. Transplant would require a registry-wide rekey audit
(scheduler owner id, wait identity, procfs, TTY) unrelated to resource safety.

**R-15. This is a design-only artifact.** No build, QEMU, or Parallels evidence exists yet. Every
gate below is a requirement on the implementation, not a result.

**R-16. The legacy remote-mark arm is live from P9 until P10c.** *(new, condition 1.)* Between the
request/wake cutover and the last wait-family migration, two teardown shapes coexist for SIGKILL:
victim-owned for boundary-reachable victims, and the merged P2 remote-mark for the rest. This is a
deliberate, counted, ratcheted interim — both arms are exercised in every PR that ships them, the
false set is an explicit allowlist that only shrinks, and P10c deletes the arm — but during that
window two paths must both be correct, and a review of any P10x PR must check both. The alternative
(migrate every family in one PR) is the 15-phase big bang Judge 1 found fatal.

**R-17. Orphaned-claim recovery depends on the same liveness machinery it protects.** *(new,
condition 2.)* T4 (`Claimed{dead} → Pending`) uses P1's proof (`scheduler thread absent or
Terminated` + claim-time fence elapsed) to decide that a claimer is dead. If that proof is wrong in
the *unsafe* direction — declaring a live claimer dead — two paths could redeem one obligation. The
mitigation is that T4 is gated on the *same* two-epoch fence that gates physical free, so a claimer
that could still run cannot be declared dead without also making a use-after-free reachable, which the
existing gates already test for. It is a shared dependency, not an independent proof, and is recorded
as such.

---

## 7. Open questions — operator decisions only

**OQ-1. Init death policy — DECIDED (coordinator-adopted at v2, condition 7).** **Linux-faithful
protected init:** a user-originated signal to the designated init whose effective disposition is the
default fatal action and for which init has no handler is **silently dropped and the send returns
success (0)** — not `EPERM`. Handled signals are delivered normally. Only init's own `exit`/`exit_group`
or an unhandleable synchronous fatal fault (or a nonviability invariant) is kernel-fatal. v1's `EPERM`
recommendation is withdrawn: the ratification is right that `EPERM` is a deliberate ABI divergence and
was mislabelled as fidelity, and rather than document a divergence we take the faithful behaviour.
**OQ-1b (still open):** accept today's non-stop-the-world panic (recommended; file `smp_send_stop`
separately), or add an SMP stop broadcast in this round?

**OQ-2. PID-1 reservation.** Reserve PID 1 for the explicit init constructor and start ordinary/test
allocation at 2 (all four `next_pid.fetch_add` sites)? Recommended **yes** — it is what converts AC-5
from convention into structure and makes a failed init creation deterministically retryable.
*(Ratification: AGREE.)*

**OQ-3. Exit scope split.** Confirm `exit(2)` = member, and `exit_group`/SIGKILL/default-fatal
signal/fatal user fault = thread group (Linux). Recommended **yes**; today's collapsed
`Exit | ExitGroup` would undermine #471's group semantics. *(Ratification: AGREE.)*

**OQ-4. Tier-1 approval — `kernel/src/syscall/time.rs`.** Approve routing its raw TTBR0 writer
through the constrained helper? Recommended **yes eventually, not required by any phase here**.
Without it the conservative-shadow invariant cannot honestly be declared closed kernel-wide (R-1). It
is the one Tier-1 file this work would ever want, and the r20 record notes the grave branch's
two-line edit there lacked signoff.

**OQ-5. Tier-2 approval — the exception-return boundary hook.** Approve an acquire-load + branch in
the aarch64 exception/syscall return path and `interrupts/context_switch.rs`, placed after the
`PREEMPT_ACTIVE`/nested-return gate, with all real work in a normal preemptible context? **Without
this approval the design stops at Phase 7** — victim-owned exit is impossible, and #491 ships only at
Increment-1 strength (still a real UAF fix, but the remote-mark model survives). **v2 correction to
v1's blast radius: refusal parks P8 *through P12*, not just P8.** Under the corrected order the group
seal lives in P9 (which needs P8's hook) and the init-death latch's only producers are P8's own-exit
commit and P11's unhandleable-fault path, so a refusal leaves #464 identity-complete but death-policy
parked and #471 with exec detach (P3) but no seal. v1's claim that "#464 and #471 ship complete"
without OQ-5 was wrong and is withdrawn. *(Ratification:
AGREE, strictly under the stated constraints: post-gate, acquire-load + branch only, preemptible-context
work, no logging. §2.5 now states the activation control flow those constraints apply to.)*

**OQ-6. Bounded reclaim worker.** In this round, or deferred with #448/#492? Recommended **deferred**
— the receipt/queue shape makes it a drop-in, it touches Tier-2 kthread surface, and the r20 attempt
failed there. *(v2: deferring the worker no longer defers a proof — §1.6 supplies the serialization the
worker used to provide.)*

**OQ-7. Scope cuts.** Confirm that group-wide exec cull, `children`-mirror removal / subreaper
selection, and nonleader PID transplant are all **out** of this round. Recommended **yes** — each is a
separate blast radius, and #471 as filed is satisfied by detach + seal. *(Ratification: AGREE.)*

**OQ-8. Phase count — CORRECTED (condition 6).** v1 claimed "13 phases / 13 PRs", which the
ratification correctly called a contradiction with the split phases. The honest figure is
**13 numbered phases / 16 PRs**: P6 splits into P6a + P6b (condition 2) and P10 into P10a + P10b + P10c
(the wait families), while every other phase is one PR. Each PR targets ≤ ~230 changed non-generated
lines across ≤ 5 production files (PR #418's 236-line, 5-file diff is the ceiling). The full PR ledger
is enumerated in PLAN §0. That is a lot of review cycles, and it is the deliberate price of Judge 1's
fatal finding against a 15-phase big bang. **Open decision: accept 16 PRs, or batch adjacent phases and
accept larger review surface per PR?**

**OQ-9. x86_64 rollout.** Land shared semantics now with no SMP proof (recommended), or gate the
victim-owned path to aarch64 and keep an explicitly audited synchronous x86 hook? *(Ratification:
AGREE, conditioned on proving every x86 user-return path reaches the common hook and halting for
approval if one is found to bypass it — that proof is a named P8 gate item.)*

---

## 8. Compliance with the lessons register

| Previously-rejected mechanism | Reopened? | What is different |
|---|---|---|
| `#[cfg(not(feature = "testing"))]`-scoped init panic (×4 reverted) | **No** | Designation is runtime data. No feature gate exists anywhere, so `interactive = ["testing"]` cannot invert anything |
| Panic under a DAIF-masked PM guard | **No** | S1 stores one atomic; S5 panics with no guard live and records a pre-panic lock/IRQ snapshot first |
| Fatal escalation on heuristic victim resolution | **No** | The latch keys on a **committed** victim; TID/CR3 divergence is counted and never fatal; `AttributionUncertain` kills nobody. *(v2: the latch's producer set shrinks further — an external kill of init can no longer reach S1 at all)* |
| CLONE_VM group sweep (4 blocking defects) | **Reshaped, not reopened** | All four defects are structurally unrepresentable: exec detach lands first (Phase 3); no snapshot ever leaves PM (the mark *is* the seal); stacks are only ever self-marked and grace-released; no N-member FD loop exists because each victim closes its own FDs one at a time outside PM. Group-wide **exec cull** is still out (OQ-7) |
| Bounded drains (×3 reverted) | **No** | No cap added, no drain moved, no cap shared with fork, no new pre-drain in `sys_clone`, and no scheduler lock is ever entered with a PM guard live |
| Suppressing repeat-pass `btrt::on_process_exit` (disproven) | **No** | The obligation is created once and *shared by every producer*; a later pass **claims and redeems** the same obligation rather than being skipped. `btrt`'s single call site (`process_task.rs:267`) is exactly why Phase 2 carries the report obligation on the remote-kill path |
| `ReclaimContext` capability token (r20) | **No** | Replaced by pid/tid-based APIs and returned receipts (structural) + the source ratchet; `try_manager()` is instrumented so the r20 detector blind spot is closed, and §4.4 states the detector's limits instead of claiming proof |
| `ThreadState::ExitPending` (r20 finalization deadlock) | **No — explicitly rejected from design C** | A victim is made *runnable* so it can run its own continuation, unregister its own wait, and terminate itself; it is never dequeued while its own liveness evidence stands |
| `kreclaimd` worker (r20: logging in the reaper, Tier-2 surface) | **No** | Reclaim stays at the two existing drain sites; a worker is OQ-6, not a phase. *(v2: the worker's serialization role is replaced by the PM-serialized claim protocol, §1.6 — it is not silently missing)* |
| Large coherent design accumulating unreviewed surface before validation (the r20 grave failure, and Judge 1's fatal finding against B) | **No** | The spine's live UAF closes in Phase 2 of 13; every phase ships, is revertable alone, and has a live caller for its new code; stopping the round at *any* phase boundary leaves the tree strictly better than `main` |
| **Interim shapes that violate the end-state lock rule "temporarily"** *(new, v2 — condition 3)* | **No** | v1 let P1 prove under the reclaim-queue lock and P2 enqueue under PM. Both are corrected at the phase that introduces them: the drain detaches before proving, and receipts are returned and enqueued after PM drops. `PROOF_UNDER_QUEUE_LOCK` and `RECLAIM_ENQUEUE_UNDER_PM` are asserted 0 from the phase that creates them onward |
| **A wait-family contract shipping before its producer exists** *(new, v2 — condition 1)* | **No** | The request/wake publication and victim-owned commit suppression now land in P9, before every P10x consumer; each family PR is a consumer of an already-merged producer, and the interim legacy arm is explicit, counted and ratcheted rather than implied |
| **A second notification path surviving alongside the ledger** *(new, v2 — condition 5)* | **No** | `DeliverResult::Terminated` and its caller-side notify action are deleted in the same PR that introduces the intent-only `FatalIntent`; the ratchet asserts the variant's absence by name |
