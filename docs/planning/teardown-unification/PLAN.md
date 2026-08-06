# Teardown Unification — PHASED IMPLEMENTATION PLAN v2 (revised against ratification)

Companion to `teardown-unification-DESIGN-v2.md`. Design-only: nothing below has been implemented and
no gate result is claimed.

**Base:** `main` @ `eebc8868` (docs re-verified at `main` @ `c9efdcc7`). **Issues:** #491 (spine), #464, #471.

---

## CHANGELOG — v2 deltas against the ratification conditions

The Codex Sol ratification refused endorsement (`ENDORSE: NO`) with 2 FATAL seams, 7 MAJOR, 1 MINOR and
7 conditions. This is a **targeted** revision: phases and text the ratification did not criticise are
preserved verbatim.

| Cond | What changed in this PLAN | Where |
|---|---|---|
| **1** | **Phase reorder.** Old P10 (request-only scheduler publication + victim-owned SIGKILL commit suppression + group cutover) becomes **P9**; old P9a/b/c (killable-wait families) become **P10a/b/c**. P9 is independently implementable against P2 via the named `exit_request_is_boundary_reachable()` predicate with a live, counted legacy remote-mark arm; each P10x migrates one family; **P10c empties the allowlist and deletes the arm**, which is where #491 becomes complete. Dependency graph fully re-derived, including the **missing `P3 → P8` edge** | §0 graph, P9, P10a/b/c, §1 |
| **2** | **New P6a** (reap/tombstone retention gate) ships **before** **P6b** (the ledger), because the `Resources` obligation outlives reap while today's `waitpid` physically removes the row. P6b's bits become the four-state `Absent/Pending/Claimed{claimer}/Completed` machine with PM as sole serializer | P6a, P6b |
| **3** | **P1** restructures the reclaim drain to *detach → drop queue lock → prove → free-or-reinsert*; the under-queue-lock predicate stays lock-free. **P2** changes `exit_process` to return a `#[must_use] RetirementReceipt` and converts **both** existing PM-nested `enqueue_process_reclaim` sites | P1, P2 |
| **4** | **P8** now states the hook-activation control flow explicitly (the hook is the *only* entry to `do_exit_current`, so normal exit exercises it in this PR), plus the tombstone and one-at-a-time FD-acquisition control flow Codex's P8 note called for | P8 |
| **5** | **P11** deletes `DeliverResult::Terminated` **and** its caller-side parent-notification action in the same PR, replacing it with intent-only `DeliverResult::FatalIntent` | P11 |
| **6** | **P0** names every existing teardown write-site (file:line) each counter attaches to, marks which counters are legitimately zero until a later phase, and adds a **nonzero causally-paired** defer/reclaim test. The **honest PR count is 13 numbered phases / 16 PRs**, enumerated below. The false "P3/P4/P5 are file-disjoint from P2, so they can merge in parallel" claim is **deleted** — the file lists overlap and all phases merge sequentially | P0, §0 PR ledger, §0 graph |
| **7** | **P12** implements the coordinator-adopted Linux-faithful protected-init policy: user-originated default-fatal signals to designated init with no handler are **silently dropped and the send returns 0**. All `EPERM` language and its test assertion are removed | P12 |

**Unchanged from v1:** the five phasing rules (with one refinement noted in rule 2), the standard gate,
P3, P4, P5, P7 (except an honesty note), the merge discipline, and the round-level stop rule.

---

## 0. Phasing contract (this is the part that differs from every prior attempt)

Five rules bind every phase. They exist because the two failure modes that killed prior rounds were
(a) a large coherent design accumulating unreviewed surface before validation (grave branch: fully
green at r18, 27 findings / 15 blocking at r20) and (b) point fixes that each introduced a new defect
class (r23: fixed 3, introduced 4).

1. **Strictly better, always.** Stopping the round at *any* phase boundary must leave the tree
   strictly better than `main`. No phase depends on a later phase for its safety argument.
2. **No dormant code.** Every new API has a live caller in the PR that introduces it. No
   `#[allow(dead_code)]`, no "activated in a later phase". *(v2 refinement, condition 1: where a phase
   ships a predicate with two arms — e.g. P9's boundary-reachable vs legacy remote-mark — **both arms
   must be exercised by that PR's own tests**, and the arm destined for deletion must be a ratcheted
   allowlist that only shrinks. A branch that no test in its own PR can reach is dormant code wearing
   a different hat.)*
3. **Spine first.** #491's confirmed-live UAF closes in **Phase 2 of 13**, using only already-merged
   primitives — not behind ten prerequisite PRs.
4. **Hard size ceiling.** PR #418 measured **5 files / 166 insertions / 70 deletions = 236 lines**.
   Each PR targets **≤ ~230 changed non-generated lines across ≤ 5 production files**. Crossing it
   means splitting at the named seam *before* review, not merging anyway. The ceiling is a gate.
5. **One revert story per phase**, written in the PR body before merge, and the phase's code must
   actually be revertable alone (verified by `git revert` dry run on the merge commit).

### The honest PR ledger — 13 numbered phases, **16 PRs** *(condition 6)*

v1 claimed "13 phases / 13 PRs" while splitting P9 into three; the ratification counted 15 and called
the contradiction a MAJOR. With P6 also split (condition 2) the true figure is **16 PRs**. Every one
is listed; there are no others.

| # | PR | Phase | Ceiling estimate |
|---|---|---|---|
| 1 | Teardown observability + call-site ratchet | P0 | ~200 lines / 3 files |
| 2 | Retirement fence + RootProof taxonomy + drain restructure | P1 | ~190 lines / 5 files |
| 3 | SPINE-1: SIGKILL stops eager-freeing (+ receipt-after-PM-drop) | P2 | ~210 lines / 5 files |
| 4 | exec detach + clone/exec admission | P3 | ~150 lines / 4 files |
| 5 | Kernel-stack ownership parity | P4 | ~150 lines / 5 files |
| 6 | Runtime init designation (identity only) | P5 | ~180 lines / 5 files |
| 7 | Reap/tombstone retention gate | **P6a** | ~170 lines / 5 files |
| 8 | Exactly-once ledger (four-state obligations) | **P6b** | ~190 lines / 5 files |
| 9 | FD closure leaves the PM lock | P7 | ~160 lines / 5 files |
| 10 | Victim-owned `do_exit_current` + boundary hook | P8 | ~220 lines / 5 files |
| 11 | Request-only scheduler termination + group cutover | **P9** (was P10) | ~220 lines / 5 files |
| 12 | Killable wait: futex | **P10a** (was P9a) | ~150 lines / 4 files |
| 13 | Killable wait: `WaitQueueHead` + stdin/TTY readers | **P10b** (was P9b) | ~150 lines / 4 files |
| 14 | Killable wait: child-wait + timer/nanosleep + completion/I-O; **delete the legacy arm** | **P10c** (was P9c) | ~180 lines / 5 files |
| 15 | Fatal-signal + fault convergence (intent-only delivery) | P11 | ~220 lines / 5 files |
| 16 | Init death policy | P12 | ~120 lines / 4 files |

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
   `waitpid` status, zero fault markers). "The process was created" is never evidence, and **a
   counter equality that holds at zero is never evidence** (condition 6): every equality gate names
   the workload that drives it nonzero, or declares explicitly why that counter is legitimately zero
   until a named later phase.
4. **Parallels launcher streak:** 10 consecutive PASS with `inject_retries=0`, ≤15 attempts, fresh
   epoch-named VM via `./run.sh --parallels`, `prlctl stop --kill` after each.
5. **Soak** on any phase that changes kill timing or retention (2, 6a, 6b, 7, 8, 9, 10c, 11): 30-min
   minimum, plus the retention measurement where noted.
6. **Frozen-region hash gate:** all six gold-master regions byte-identical vs `main`; all five Tier-1
   files byte-identical unless OQ-4 has been granted in writing.
7. **Cleanup:** all Parallels VMs stopped, all stray QEMU killed, before reporting the phase done.

### Dependency graph — re-derived *(condition 1)*

Every edge below is a **hard** edge: the target phase's safety argument or its "no dormant code"
obligation is unsatisfiable without the source.

```
P0  ──> everything                      (evidence + ratchet; every later gate cites its counters)

P1  ──> P2                              (grace-stamped receipt must be provably ordered before
    ──> P6a                              the tombstone-removal gate reuses grace+RootProof)
    ──> P8, P9, P11                     (wider grace reliance)

P2  ──> P6b                             (P2's seed obligation is what the ledger generalizes)
    ──> P7                              (P2's SIGKILL commit is one of P7's FD convergence points)
    ──> P9                              (the live SIGKILL call site is the thing P9 upgrades, and
                                         P9's legacy arm IS P2's behaviour)

P3  ──> P8   *** MISSING IN v1 ***      (P8's last-reference decision and RootProof read the row's
                                         own root + inherited_cr3; a stale inherited_cr3 after exec
                                         presents a root the row does not own — DESIGN §2.3)
    ──> P9                              (exec detach before any group-scoped kill)

P4  ──> P8, P9                          (all stacks scheduler-owned before any victim-owned exit or
                                         request-only termination)

P5  ──> P12                             (identity before policy — never bundled)

P6a ──> P6b   *** NEW ***               (row must outlive obligations before Resources becomes
                                         row-resident — DESIGN §1.6)
P6b ──> P8                              (the victim commit claims ledger obligations)
    ──> P11                             (P11 deletes the legacy notifier and relies on the ledger)

P7  ──> P8                              (one-at-a-time FD acquisition is what the commit uses)

P8  ──> P9                              (the hook + do_exit_current are what a published request
                                         resolves to)
    ──> P12                             (init's own exit commit is a latch producer)

P9  ──> P10a, P10b, P10c   *** REORDERED ***
                                        (every wait-family PR consumes the request/wake mechanism;
                                         v1 had this edge backwards)
    ──> P11                             (the terminate() allowlist cannot reach empty until SIGKILL
                                         has left the exit_process/terminate route)

P10c ──> (no successor is blocked; it is the completion point for #491 and AC-11)

P11 ──> P12                             (the unhandleable-fault latch producer lives here)
```

**No parallel-merge path is claimed.** *(condition 6 — v1's "P3, P4, P5 can run in parallel with P2
(disjoint files)" was false and is deleted.)* The production file lists overlap materially:
`process/manager.rs` appears in P2, P3, P5, P6a, P6b, P7, P8, P9; `task/scheduler.rs` in P1, P2, P3,
P4, P9, P10a/b/c; `syscall/signal.rs` in P2, P5, P9, P12; `task/process_task.rs` in P1, P2, P4, P6a,
P6b, P7, P8. There is no pair of phases in this plan whose production files are disjoint enough to
merge concurrently without rebasing the other, so **phases merge strictly sequentially in the order
listed**. Reordering is permitted only where the graph above has no edge, and only one phase is in
flight at a time.

---

## Phase 0 — Teardown observability + call-site ratchet *(no behaviour change)*

**Scope.** Lock-free counters with a normal-context reader, and a source-structure ratchet that pins
the *exact current* bypass surface so any regression fails CI immediately.

> **v2 (condition 6): every counter names the existing write-site it attaches to.** v1 listed counter
> names only, so the equality gate could pass vacuously at zero. The table below is the complete
> wiring plan against `main`'s live code; a counter with no site named here is not in P0.

| Counter | Existing write-site(s) wired in P0 | Nonzero on a normal boot? |
|---|---|---|
| `TEARDOWN_ENTRY{exit}` | `task/process_task.rs:218 handle_thread_exit` (callers `syscall/handlers.rs:169`, `arch_impl/aarch64/syscall_entry.rs:377`) | **yes** — every process exit |
| `TEARDOWN_ENTRY{fault}` | `task/process_task.rs:369` (`handle_thread_exit(tid, -11)` inside `drain_deferred_fault_sigsegv_exits`, `:363`) + the four EL0 fault sites `arch_impl/aarch64/exception.rs:768,1135,1230,1333` | yes under the fault tests |
| `TEARDOWN_ENTRY{signal}` | `syscall/signal.rs:162`, `signal/delivery.rs:224`, `:258` | yes under the signal tests |
| `TEARDOWN_ENTRY{group}` | *(no group path exists on `main`)* | **declared zero until P9** — not part of any equality gate before then |
| `EXIT_FIRST_REQUESTS` / `EXIT_REPEAT_REQUESTS` | the `already_terminated` reads at `process/manager.rs:1121` and `task/process_task.rs:222`, plus `Process::terminate_minimal`'s repeat early-return (`process/process.rs:320`) | yes (first); repeat is exercised by the P0 repeat test |
| `TEARDOWN_QUARANTINE` | `task/scheduler.rs:2599 terminate_process_threads` (five call sites: `exception.rs:768,1135,1230,1333`, and `signal.rs` from P2) | yes under the fault tests |
| `EXIT_SGI_SENT` | the `SGI_RESCHEDULE` sends at `task/scheduler.rs:1857` and `:1886` | yes |
| `TEARDOWN_DEFER` | `task/process_task.rs:129 defer_process_resources` (reached from `:125 defer_live_process_resources` and `process/manager.rs:1151`) | **yes** — every aarch64 exit |
| `TEARDOWN_RECLAIM` | `task/process_task.rs:388` (`reclaim.reclaim()` inside `reclaim_deferred_process_resources`, `:375`) | **yes** |
| `TEARDOWN_MASKED_FRAMES_WALKED` | `task/process_task.rs:100 release_process_resources` → `cleanup_cow_frames()`, incremented only when the PM-owner instrumentation says a PM guard is live (call sites `manager.rs:1156`, `process_task.rs:247`, `:250`) | yes on `main` (that is the defect P2/P7/P11 drive to 0) |
| `FD_CLOSES_UNDER_PM` | `process/process.rs:347 close_all_fds` reached under PM via `Process::terminate` (`:284`, `:294`) from `manager.rs:1161`, `signal.rs:162`, `delivery.rs:224`, `:258`, `interrupts/context_switch.rs:1021`; plus `take_fd_entries` (`process.rs:335`) at `process_task.rs:236` | yes on `main` (driven to 0 by P7) |
| `TEARDOWN_VICTIM_DIVERGENCE` / `TEARDOWN_CR3_MISS` | `process/manager.rs:1313-1335 find_process_by_cr3_mut` vs the TID-derived owner at the four EL0 sites | zero on a clean boot; **driven nonzero by the P11 `clonevm_fault_test`**, declared until then |
| `EXIT_ATTRIBUTION_UNCERTAIN` | the total-resolution-failure branch at the four EL0 sites | declared zero until injected |
| `DEFERRED_FAULT_RING_DROPPED` | `task/process_task.rs:43` — `DeferredFaultExitBuffer::push` returning `false` (caller `defer_fault_sigsegv_exit`, `:352`) | zero normally; **P0 ships a 17-deep injection that drives it nonzero**, proving the counter is real (#492's overflow is invisible today) |
| `RECLAIM_ENQUEUE_UNDER_PM` *(new, cond. 3)* | `task/process_task.rs:140 enqueue_process_reclaim`, incremented when the PM-owner instrumentation says PM is live — true today at **both** call sites (`manager.rs:1152`, `process_task.rs:244`) | **yes on `main`** — this is the pre-existing violation P2 drives to 0 |
| `PROOF_UNDER_QUEUE_LOCK` *(new, cond. 3)* | `task/process_task.rs:375-392`, incremented if SCHEDULER or PM is acquired while `PENDING_PROCESS_RECLAIMS` is held | zero on `main` (both predicates are lock-free); P1 must keep it zero |
| `RECLAIM_CONTEXT_VIOLATIONS`, `TEARDOWN_LOCK_ORDER_SUSPECT` | the lock-depth/owner instrumentation, **including `try_manager()`** (the r20 detector blind spot) | zero; asserted |

Ratchet (`tests/teardown_structure.rs`) asserts the exact current sets of: `\.terminate\(` /
`terminate_minimal\(` call sites; production `ProcessId::new(1)` sites (with the three
`test_userspace.rs` sites allowlisted **by name**); `terminate_process_threads` call sites;
`kernel_stack_allocation` mutation sites; **`enqueue_process_reclaim` call sites** (v2). It passes on
`main` unchanged; later phases shrink the allowlists, and any *new* bypass fails on arrival.

**Files.** `kernel/src/tracing/providers/teardown.rs` (new), tracing registration,
`tests/teardown_structure.rs` (new). **~200 lines, 2 commits.**

**Gate extras.** *(v2 — condition 6: no vacuous zero-baseline gate.)*
1. **`fork_exit_defer_reclaim_pairing_test` (the causally-paired nonzero gate).** Fork and exit **64**
   children in a loop, then quiesce. Assert (a) `TEARDOWN_ENTRY{exit} >= 64`; (b) `TEARDOWN_DEFER >= 64`;
   (c) after the drain, `TEARDOWN_DEFER == TEARDOWN_RECLAIM` **at a value ≥ 64** — the equality is only
   accepted at a nonzero, workload-explained value; (d) **per-pid pairing**: each deferred pid recorded
   by a `trace_event!` payload appears exactly once in a later reclaim event, so the equality cannot be
   satisfied by two unrelated streams that happen to have equal totals.
2. Ring-overflow injection drives `DEFERRED_FAULT_RING_DROPPED` nonzero and back to a quiescent read.
3. Baseline snapshot of `TEARDOWN_MASKED_FRAMES_WALKED`, `FD_CLOSES_UNDER_PM` and
   `RECLAIM_ENQUEUE_UNDER_PM` on `main` — these are **expected nonzero** and are the pre-existing
   defects later phases drive to zero; recording the baseline is what makes those later gates meaningful.
4. `TEARDOWN_LOCK_ORDER_SUSPECT == 0`, `PROOF_UNDER_QUEUE_LOCK == 0`, and every counter has a reader
   (no write-only counters).

**Strictly better.** Every later phase's evidence becomes a counter equality instead of a log-reading
exercise, and the bypass surface can only shrink. #492's silent drops become visible, and two
pre-existing lock violations become measurable before anyone tries to fix them.

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

> **v2 (condition 3) — the drain is restructured so no proof ever runs under the reclaim-queue lock.**
> `reclaim_deferred_process_resources` (`task/process_task.rs:375-392`) currently evaluates
> `retirement_grace_elapsed(&reclaim.after_epoch) && !reclaim.root_is_live()` **while holding**
> `PENDING_PROCESS_RECLAIMS`. Both terms are lock-free atomic reads today, so `main` is legal — but
> `RootProof` adds *scheduler cached roots* and *live rows*, which take SCHEDULER and PM. Acquiring
> either under the queue lock is the overlapping-lock violation the design forbids, and "it's only
> until P8" is not an exemption. P1 therefore ships the cycle as:
>
> ```
> 1. QUEUE LOCK ONLY:  scan for a candidate whose fence has elapsed AND whose
>                      lock-free blockers (hardware/shadow) are clear; swap_remove it
>                      into a local fixed slot.                        -> DROP QUEUE LOCK
> 2. NO LOCK:          RetirementSnapshot (acquire fence) + hardware/shadow re-read.
> 3. SCHEDULER ONLY:   cached-root blocker.                            -> DROP
> 4. PM ONLY:          live/creating-row blocker.                      -> DROP
> 5. proof passed  -> free with NO lock held.
>    proof refused -> bump that blocker's counter, re-acquire the QUEUE LOCK ALONE,
>                     re-insert, rotate.
> ```
>
> The under-queue-lock predicate is permanently restricted to lock-free reads; `PROOF_UNDER_QUEUE_LOCK`
> (P0) asserts it. A candidate detached and then refused is re-inserted, never dropped — the
> re-insertion path is part of this PR's tests, not a later phase's.

**Files.** `kernel/src/task/scheduler.rs`, `kernel/src/task/process_task.rs`,
`kernel/src/arch_impl/aarch64/ttbr0.rs`, tracing provider, targeted tests. **~190 lines, 2 commits.**

**Gate extras.** Unit injection with a zero online mask refuses reclaim; wrap-safe epoch comparison
test; the existing epoch-before-stack-liveness ordering becomes a structural test; every refusal in a
normal boot is attributable to exactly one blocker. **v2:** `PROOF_UNDER_QUEUE_LOCK == 0` with the
richer proof live; a forced refusal at each of the four blocker classes proves the detach/re-insert
cycle preserves the entry (queue length returns to its prior value, the same receipt is retried and
eventually retires) — asserted at a **nonzero** refusal count, not at zero.

**Strictly better.** Grace can no longer elapse on an unordered or empty observation; reclaim
refusals stop being a single opaque boolean; the local hardware register enters the proof (closing the
local half of the shadow/hardware gap `main` has today); the drain stops being a place where a future
proof could nest locks.

**Revert.** Restore the bare arrays, the boolean predicate and the in-lock scan; counters in P0 go
unused but harmless (they are read by the reader, not dead).

---

## Phase 2 — **SPINE-1: SIGKILL stops eager-freeing** *(#491's live UAF)*

**Scope.** Rewrite the SIGKILL arm at `syscall/signal.rs:162` to: validate under the existing PM
guard, capture `pid`, mutate **nothing**, `drop(guard)`; then
`with_scheduler(|s| s.terminate_process_threads(pid))`; then **broadcast** `SGI_RESCHEDULE` to every
other online CPU (no `cpu_state[].current_thread` residency predicate — that read is stale-prone and
was a fatal panel finding); then `with_process_manager(|pm| pm.exit_process(pid, -9))`, whose
merged aarch64 path already grace-defers the page table **before** `terminate()` runs. Keep the
existing `set_need_resched()` tail. Additionally install the **durable report/SIGCHLD obligation seed**:
one row obligation moved `Absent → Pending` at first commit and redeemed exactly once outside PM by
whichever of {commit path, `handle_thread_exit`} **claims** it first (the four-state machine of
DESIGN §1.6 — the seed uses it from day one; P6b generalizes it, it does not upgrade a bool) — because
`btrt::on_process_exit` has exactly one call site at `task/process_task.rs:267` inside
`handle_thread_exit`, which a remotely-marked victim may never run.

> **v2 (condition 3) — the retirement receipt is enqueued only after the PM guard drops.**
> `exit_process` becomes:
>
> ```rust
> #[must_use]
> pub fn exit_process(&mut self, pid: ProcessId, exit_code: i32) -> Option<RetirementReceipt>
> ```
>
> It still stamps the grace epoch and takes the root under PM, but **returns** the receipt instead of
> calling `enqueue_process_reclaim` at `manager.rs:1152`. The caller enqueues after
> `with_process_manager(...)` returns, with no guard live. The move allocates nothing
> (`core::mem::take` on the existing `pending_old_page_tables` `Vec` leaves it empty without
> allocating). The **second** PM-nested enqueue, `handle_thread_exit` phase 1 at
> `task/process_task.rs:244`, is converted in the same PR: the receipt rides out through the existing
> `phase1_result` value and is enqueued in phase 2, where PM is already dropped. Both `#[allow(dead_code)]`
> markers on `exit_process` come off — P2 is its first live caller.

**Files.** `kernel/src/syscall/signal.rs`, `kernel/src/process/manager.rs`,
`kernel/src/task/scheduler.rs`, `kernel/src/task/process_task.rs`, tests. **~210 lines, 3 commits —
seam for splitting if it crosses the ceiling is {receipt-return refactor} / {SIGKILL arm + seed}.**

**Gate extras.** New `sigkill_teardown_test` (userspace): parent forks a child spinning at EL0;
parent `kill(child, SIGKILL)`. Assert (a) `waitpid` reaps **-9**; (b) SIGCHLD arrived at kill time
(parent's `pause()` returns); (c) `TEARDOWN_QUARANTINE`/`TEARDOWN_DEFER`/`TEARDOWN_RECLAIM` all
increment for that pid; (d) `TEARDOWN_MASKED_FRAMES_WALKED == 0` **for kill paths** (the baseline
recorded in P0 must drop, not merely stay flat); (e) `EXIT_SGI_SENT > 0` and the peer observes it;
(f) exactly one `btrt` report; (g) zero fault markers over the 10-boot Parallels streak; **(h)
`RECLAIM_ENQUEUE_UNDER_PM == 0` — the P0 baseline was nonzero at both sites, so this is a measured
drop, not a vacuous zero.** Repeat with the child inside a CLONE_VM group — the **sibling must
survive** (this phase does not sweep the group). Repeat as self-kill `kill(getpid(), SIGKILL)`.
**Soak + retention measurement.**

**Strictly better.** The confirmed-live eager `cleanup_cow_frames`-while-remote-runs UAF class is
gone; quarantine, expedited reschedule, and SIGCHLD arrive for the first time on this path; and both
pre-existing reclaim-queue-under-PM nestings are removed. *Honest bound:* the exit is still remotely
marked (upgraded in P8/P9/P10a-c), and FD closure still happens inside `exit_process` under PM —
unchanged from today's SIGKILL, which does that **plus** the full CoW walk. Not a regression; not yet
a fix. Fixed in P7.

**Revert.** Restore the four-line `process.terminate(-9)` arm, the in-PM enqueues and the `void`
return; drop the obligation. The exact pre-image is preserved in the commit body.

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

**Dependency note (v2, condition 4).** This phase is a hard prerequisite of **P8**, not only of P9:
P8's last-reference decision and `RootProof` read the row's own root *and* `inherited_cr3`, so a row
carrying a stale `inherited_cr3` past an exec would present a root it does not own (DESIGN §2.3). The
`P3 → P8` edge is now in the graph; v1 omitted it.

**Revert.** Delete the two assignments + the admission check; ~15 lines.

---

## Phase 4 — Kernel-stack ownership parity for all three creation paths *(AC-8)*

**Scope.** Centralize "clone the process-side thread and take `kernel_stack_allocation` into the
scheduler copy", then apply it to the paths that never got it: fresh spawn (`creation.rs` ×2),
direct init, `boot/test_disk.rs`, alongside the already-fixed fork and clone. A `Process` row can no
longer synchronously drop a published stack — today the original thread's stack is freed ungated by
`remove_process` at `waitpid` reap (`syscall/wait.rs:385` → `manager.rs:1101-1104`). Drop PM before
every scheduler registration (removing the existing PM→scheduler nesting in the spawn/test-disk paths).

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
*(v2:* the `remove_process` call site this phase protects against is itself replaced by P6a's
tombstone gate; the two fixes meet at the same call site and P6a's gate extras re-run this phase's
stack-accounting assertion.*)*

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

## Phase 6a — Reap/tombstone retention gate *(NEW in v2 — condition 2)*

**Scope.** Make a process row outlive its reap so that row-resident obligations cannot be destroyed
before they are discharged. Today `waitpid`'s `complete_wait` physically removes the row
(`kernel/src/syscall/wait.rs:385` → `ProcessManager::remove_process`, `manager.rs:1101-1104`), which
is exactly the seam the ratification flagged: P6b's `Resources` obligation **by construction outlives
reap** (grace + RootProof are still pending when the parent collects the status — R-13).

- Add `RowState { Live, Zombie, Tombstone { reaped_by, status } }` to the process row.
- `complete_wait` no longer calls `remove_process`; it records the reap and transitions
  `Zombie → Tombstone`. The parent's `children` retain is unchanged.
- A `Tombstone` row is **invisible** to every "live process" query: `find_process_by_pid/thread/cr3`,
  signal delivery, wait scanning, procfs enumeration, and PID allocation reuse. Each of those call
  sites is enumerated in the PR body and covered by a ratchet rule (`no raw self.processes.get*` in
  teardown/wait/signal code without the liveness predicate).
- Removal moves to the S4 retire gate: a tombstone is removed only when its retirement receipt has
  retired (grace elapsed + RootProof passed) or was never created. In P6a there are no ledger bits
  yet, so the gate's ledger term is vacuously true and is asserted to become live in P6b.
- `TOMBSTONE_RESIDENT` (gauge with a reader) and `TOMBSTONE_REMOVED` counters.

**Files.** `kernel/src/syscall/wait.rs`, `kernel/src/process/manager.rs`,
`kernel/src/process/process.rs`, `kernel/src/task/process_task.rs`, tests. **~170 lines, 2 commits.**

**Gate extras.** (a) `waitpid` still returns the correct status and the parent's `children` list is
still pruned — existing wait/orphan tests green unchanged. (b) `TOMBSTONE_RESIDENT` returns to **0**
at quiesce after a 64-child fork/exit/reap workload, having been **observably nonzero mid-run** (both
halves asserted — a gauge that is always zero proves nothing). (c) A pid whose row is tombstoned is
not returned by any live-process lookup: negative tests for `kill(pid)` → `ESRCH`, `waitpid` repeat →
`ECHILD`, and procfs absence. (d) PID reuse does not hand out a tombstoned pid. (e) P4's stack-pool
accounting re-run (allocated == freed) now that reap no longer drops the row. **Soak + retention
measurement** (this phase changes retention by construction).

**Strictly better.** Row lifetime stops being shorter than the lifetime of the work the row owns — the
precondition every later phase's row-resident state depends on. It also converts today's "reap frees
the row and whatever is still attached to it" into a proof-gated removal, which is the same discipline
already applied to page tables and stacks.

**Revert.** Restore the `remove_process` call in `complete_wait` and delete `RowState::Tombstone`;
P6b is not yet merged, so nothing depends on it.

---

## Phase 6b — Exactly-once ledger: four-state obligations + first status *(AC-12)*

**Scope.** Generalize Phase 2's obligation seed into the row-resident `ExitLedger` of DESIGN §1.6:
six obligations (`Sigchld, ParentWake, Report, Reparent, Fds, Resources`), each carrying
`Absent | Pending | Claimed{claimer, at} | Completed` in a fixed-size array — **not booleans**.

> **v2 (condition 2) — this is the replacement for design C's single-worker serialization.** All four
> transitions happen under the PM guard and nowhere else, take no second lock, allocate nothing and
> log nothing: **T1** `Absent→Pending` in S1's first request only; **T2** `Pending→Claimed{me}` by a
> control path that will discharge it immediately; **T3** `Claimed{me}→Completed` by the claimer only,
> under a fresh PM acquisition after the work finished outside PM; **T4** `Claimed{dead}→Pending` by
> the S4 retire gate only, when the claimer is proven not live and the claim-time fence has elapsed.
> The sole-redeemer invariant — at most one control path is ever `Claimed` — falls out of read-then-write
> inside one PM acquisition, so the commit path and `handle_thread_exit` can no longer both redeem.

A repeat request returns the stored batch/status and creates no second obligation and no second status
write. `ExitLedger`'s `Resources` obligation subsumes design A's proposed separate
`ResourceState{Held,HandedOff}` field. Both already-terminated branches (`manager.rs:1137`,
`process_task.rs:234`) re-key onto the ledger state.

**Files.** `kernel/src/process/process.rs`, `kernel/src/process/manager.rs`,
`kernel/src/task/process_task.rs`, tracing provider, tests. **~190 lines, 2 commits.**

**Gate extras.** Repeat matrix — exit→fault, SIGKILL→fault, fault→SIGKILL, repeated request/wait:
exactly one SIGCHLD, one parent wake, one `btrt` report, and `waitpid` returns the **first** status.
Equalities: `SIGCHLD_FIRST_SET == PARENT_WAKE_COMPLETED == BTRT_EXIT_REPORTED == parented_first_commits`,
**asserted at the nonzero value the 64-child workload produces**. `LEDGER_CLAIM_MISMATCH == 0`.
**Two-producer race test:** force `handle_thread_exit` and the commit path to contend for the same
row's `Report` obligation; assert exactly one `btrt` report and that the loser observed
`Claimed`/`Completed` (counter `LEDGER_CLAIM_LOST_RACE > 0` — the race must actually be exercised,
not assumed). **Orphan-recovery test:** inject a claimer that dies between T2 and T3; assert
`LEDGER_CLAIM_ORPHANED == 1`, the obligation returns to `Pending`, and the notification is ultimately
delivered exactly once rather than lost. `TOMBSTONE_RESIDENT` still returns to 0 at quiesce with the
ledger term of the removal gate now live. CoW frame accounting unchanged vs P5 for the same workload.

**Strictly better.** Closes PR #418's own declared follow-up ("duplicate SIGCHLD / stale exit code on
the already-terminated path") **with a mechanism, not a convention**. Explicitly **not** notification
suppression (disproven in review): the obligation is shared by every producer, so a later pass claims
and redeems it rather than being skipped.

**Revert.** Re-key the two branches onto `is_terminated()` and drop the ledger; P6a's tombstone gate
survives independently (its ledger term becomes vacuously true again).

---

## Phase 7 — FD closure leaves the PM lock *(AC-9)*

**Scope.** Add `FdTable::take_next_for_exit()` — take exactly **one** descriptor under PM, drop PM,
close it, repeat. Retire the existing allocating `Process::take_fd_entries() -> alloc::vec::Vec`
(`process.rs:335`) rather than reusing it. Apply at both convergence points and at Phase 2's SIGKILL
commit. The `Fds` obligation brackets the loop (`T2` before the first take, `T3` when the table is
proven empty under PM) — the explicit control flow is in DESIGN §2.5.

**Files.** `kernel/src/ipc/fd.rs`, `kernel/src/process/process.rs`,
`kernel/src/process/manager.rs`, `kernel/src/task/process_task.rs`, tests.
**~160 lines, 2 commits.**

**Gate extras.** `FD_CLOSES_UNDER_PM == 0` — a **measured drop** from the nonzero P0 baseline, not a
zero that was always zero. 256-FD test with a large process set: measure and assert a bounded PM hold
(one descriptor per acquisition). P0 ratchet forbids any close/reclaim call inside a request/commit
transaction body. Soak.

**Strictly better.** Removes the pipe/PTY/TCP endpoint locks *and* a heap allocation from under
PM+DAIF-masked on **every** exit path — a pre-existing violation on `main` that predates all of this
work, and the last one Phase 2 knowingly left standing.

*Honest scope note (v2, folding the ratification's P7 remark):* P7 matches the end state but does
**not** supply P6b's row-lifetime prerequisite — that is P6a's job, and P6a is sequenced before both.

**Revert.** Restore the `Vec` path; the one-at-a-time API is additive.

---

## Phase 8 — Victim-owned `do_exit_current`, normal exit as first consumer *(Tier-2; needs OQ-5)*

**Scope.** The exit trampoline and one-shot victim transaction: claim (atomic) → local TTBR0 leave
(hardware first, shadows after) → short PM commit (mark zombie, first status, T2-claim the ledger
obligations this path will discharge, take own FDs via P7, decide last-reference, move root into a
retirement receipt) → drop PM → all slow work unlocked (one-FD closes, futex/`clear_child_tid`,
bounded reparent cursor with PM dropped and DAIF restored between batches, redeem each obligation and
T3-complete it under its own fresh PM acquisition, enqueue the receipt **with PM already dropped**) →
pivot to neutral stack → mark **only self** `Terminated` → schedule away.

> **v2 (condition 4) — how normal exit exercises the return-boundary hook in THIS PR.** The hook is
> not an optional accelerator that P9 later starts using; it is the **only** entry into
> `do_exit_current`, so it is exercised by every process that exits in this PR:
>
> ```
> sys_exit(code)
>   └─ S1 on SELF under PM (first status, obligations Absent->Pending, row ExitRequested)  … PM dropped
>   └─ release-store ThreadExitRequest{Latched} on OWN scheduler thread
>   └─ return normally to the syscall return path      (NO teardown work inline — this is the point)
> <syscall/exception return path>
>   └─ existing PREEMPT_ACTIVE / nested-return gate                              (unchanged, first)
>   └─ HOOK: acquire-load of the exit-request word -> if Latched, call do_exit_current()
> ```
>
> `sys_exit` performing no teardown inline is what makes the hook load-bearing rather than dormant.
> **Tombstone control flow:** the victim marks its row `Zombie` and never removes it; `Zombie →
> Tombstone` is the parent's reap (P6a) and removal is the retire gate's. **FD-acquisition control
> flow:** `T2: Fds→Claimed` inside the commit, then `take_next_for_exit()` one descriptor per PM
> acquisition with every close performed PM-dropped, then `T3: Fds→Completed` under a fresh
> acquisition once the table is proven empty (DESIGN §2.5).

**Files.** new `kernel/src/task/teardown.rs`; `kernel/src/task/process_task.rs`,
`kernel/src/arch_impl/aarch64/context_switch.rs` (Tier-2), `kernel/src/arch_impl/aarch64/syscall_entry.rs`,
`kernel/src/process/manager.rs`. **~220 lines, 3 commits — at the ceiling; seam for splitting is
{trampoline + commit} / {boundary hook + normal-exit routing}.**

**Gate extras.** **Hook liveness (the anti-dormancy gate): `EXIT_HOOK_ENTRIES > 0` and
`EXIT_HOOK_ENTRIES == EXIT_COMMITS` on a normal boot** — if any exit reached `do_exit_current` without
traversing the hook, or the hook never fired, the phase fails. Repeated/nested exit injection produces
one status, one SIGCHLD, one parent wake, one report; FD closes and reclaim enqueue observed with PM
unlocked (`FD_CLOSES_UNDER_PM == 0`, `RECLAIM_ENQUEUE_UNDER_PM == 0` still). Disassembly review of the
hook proving no logging, allocation, page-table walk, or contended lock on the return tail (Tier-2
requirement). **x86_64 user-return audit (OQ-9): enumerate every user-return path and prove each
reaches the common hook; if one bypasses it, halt and escalate for operator approval rather than
patching Tier-1 syscall entry.** Soak. Frozen-region hashes unchanged (the hook is *outside* every
frozen block).

**Strictly better.** The exit path becomes victim-owned for the most common death; a `Process` row
never drops a published stack; slow work leaves the masked section on every normal exit.

**Revert.** Route normal exit back to `handle_thread_exit`'s current two-phase body and delete the
hook; single-file revert plus the hook removal.

**Blocked on OQ-5.** Without Tier-2 approval the plan **stops at Phase 7**, and — correcting v1 —
that parks **P8 through P12 entirely**, not just P8. Concretely: #491 ships at Increment-1 strength
(a real UAF fix, remote-mark model surviving); #464 ships **identity only**, because under the
corrected order P12's only latch producers are P8's own-exit commit and P11's unhandleable-fault
path; #471 ships **exec detach only** (P3), because the group seal now lives in P9, which is
unreachable without P8's hook. v1's claim that "#464 and #471 ship complete" without OQ-5 was wrong
and is withdrawn.

---

## Phase 9 — Request-only scheduler termination + group-scope cutover *(was Phase 10; #491 near-complete, #471 part 2)*

> **v2 (condition 1): this phase moved AHEAD of the killable-wait families.** The ratification's first
> FATAL was that v1's wait-family PRs (old P9a/b/c) consumed a request/wake mechanism that did not
> exist until old P10 — no producer. The order is inverted here: P9 publishes the mechanism and
> suppresses the victim-owned commit's remote counterpart; P10a/b/c consume it, one family per PR.

**Scope.** `terminate_process_threads` is redefined to **request and wake**: publish
`ThreadExitRequest` on each member's scheduler thread, return `ExitKickPlan`, and **never set a remote
thread `Terminated` for a boundary-reachable victim**. SIGKILL and `exit_group` become group-scoped
`ExitIntent`s; the group-exit PM transaction marks every live effective-TGID member with one batch id
and *is* the seal (no snapshot ever leaves the lock); `sys_clone` fails `EAGAIN` into a non-`Open`
group. SGIs are sent after all locks are dropped.

**The two-armed predicate (both arms live in this PR).**

```rust
/// True iff the victim will demonstrably reach the P8 return-boundary hook without help:
/// runnable/running (EL0 or a preemptible kernel path), or blocked in a wait family whose
/// victim-owned cancellation has been audited and tested (P10a/b/c).
fn exit_request_is_boundary_reachable(t: &SchedThread) -> bool
```

- **true** → publish the request only; the victim runs `do_exit_current` on itself. Counted
  `EXIT_VICTIM_OWNED`.
- **false** → publish the request **and** take the legacy remote-mark + `exit_process` route, which is
  byte-for-byte the behaviour merged in P2 — no new mechanism, no new hazard, and no kill-latency
  regression for blocked victims. Counted `EXIT_LEGACY_REMOTE_MARK{family}`.

At P9 the "false" set is **every wait family** (none is migrated yet); it is a named allowlist in the
P0 ratchet that may only shrink. P10a/b/c shrink it; **P10c empties it and deletes the arm.** Both
arms are exercised by this PR's own tests (rule 2), so nothing dormant lands.

**Files.** `kernel/src/task/scheduler.rs`, `kernel/src/syscall/signal.rs`,
`kernel/src/task/teardown.rs`, `kernel/src/process/manager.rs`, `kernel/src/syscall/clone.rs`.
**~220 lines, 3 commits — at the ceiling; seam is {request API + kick plan + predicate} / {group
scope + seal}.**

**Gate extras.** Two-CPU aarch64: kill a thread running remotely at EL0 and prove **its own TID**
executes the exit commit, with zero post-request EL0 trace for the victim and `EXIT_VICTIM_OWNED > 0`.
Kill a thread blocked in an (unmigrated) futex wait and prove it still dies with the correct status
via the legacy arm, `EXIT_LEGACY_REMOTE_MARK{futex} > 0` — **the fallback is tested, not assumed**.
Deterministic clone-vs-seal barrier: the child is either in the batch or `sys_clone` returns `EAGAIN`
— never a runnable unrequested member. No resource claim before the batch commits. Soak.

**Strictly better.** Nothing that can reach a return boundary is torn down remotely any more; group
membership is atomic by construction; #471's seal ships. *Honest bound (R-16):* victims blocked in
unmigrated wait families are still torn down remotely — that is P10's job, it is counted per family,
and it is unchanged from what P2 already shipped rather than a new regression.

**Revert.** Restore the remote-marking body and pid-scoped SIGKILL — i.e. fall back to Phase 2's
already-safe behaviour, not to `main`.

---

## Phase 10 (a/b/c) — Killable-wait contract, one family per PR *(was Phase 9)*

**Scope.** 10a futex; 10b `WaitQueueHead` + stdin/TTY readers; 10c child-wait + timer/nanosleep +
completion/I-O. Each sub-phase: on a fatal request the victim is made **runnable with its saved
continuation and `blocked_in_syscall` intact**; the resumed continuation gives the latched fatal
request priority over ordinary success, unregisters itself through the existing `finish_wait`/
unregister path, and only then branches to the trampoline. **No `ExitPending` state** — dequeuing a
victim whose saved resume SP is still live evidence is the banked r20 finalization deadlock, and
design C's version of it was the panel's fatal finding against C.

**Each sub-phase consumes the request/wake mechanism P9 already publishes** — it moves its family from
the `exit_request_is_boundary_reachable() == false` allowlist into the `true` set, so the concrete,
already-live acceptance test "SIGKILL a victim blocked in *this* family" changes from *legacy remote
mark* to *victim-owned exit* in the same PR. The live futex loop is the named hazard: today an
unrelated wake can take the success branch before the signal check and leave the TID registered; the
resumed loop must be reordered.

**10c additionally deletes the legacy arm**: with the allowlist empty, `exit_request_is_boundary_reachable`
becomes total, the remote-marking body of `terminate_process_threads` is deleted, and
`EXIT_LEGACY_REMOTE_MARK` must read 0 for a full run. **This is where #491 is complete and AC-11 is
fully discharged** — v1 claimed that at the cutover phase, which was premature.

**Files (per sub-phase).** the family's own file(s) + `kernel/src/task/scheduler.rs` +
`kernel/src/task/teardown.rs` + its test. **~150 lines each (10c ~180), 1-2 commits each.**

**Gate extras.** Per family: inject an exit request and prove the family's registry/heap **no longer
contains the TID before the exit commit runs** (deregistration-before-commit is the load-bearing
assertion the ratification asked for); SIGKILL of a victim blocked in that family reaps the right
status; `EXIT_VICTIM_OWNED` increments for that family and `EXIT_LEGACY_REMOTE_MARK{family}` drops to
0 while remaining nonzero for families not yet migrated (both halves asserted). Families not yet
migrated are **reported by counter** — never silently advertised as killable. **10c only:** allowlist
empty asserted as an exact set, `EXIT_LEGACY_REMOTE_MARK == 0` over a full run including the group and
CLONE_VM stress, and the ratchet asserts the remote-marking body is gone by name. Soak on 10c.

**Strictly better.** Each family converts from "torn down remotely by the legacy arm" to "dies
promptly, on its own thread, with its wait registration cleanly removed". Unmigrated families are
unchanged and visible.

**Revert.** Per family, independently: return the family to the allowlist (10a/10b), or restore the
legacy arm plus the family (10c).

---

## Phase 11 — Fatal-signal and fault convergence *(the last direct `terminate` callers)*

**Scope.** `deliver_default_action` **returns a fatal intent** instead of mutating anything under the
PM borrow — delete both `process.terminate(...)` + `with_thread_mut(...)` blocks at
`signal/delivery.rs:224-239` and `:258-269`.

> **v2 (condition 5) — the result is INTENT-ONLY and the legacy notifier dies in this same PR.**
> v1 carried `(pid, code, sig)` out through the **existing** `DeliverResult::Terminated` channel,
> whose documented contract is "caller MUST call notify after releasing the PM lock" — while P6b
> assigns notification to the ledger. That is a duplicate-notify window, so:
> - **Delete `DeliverResult::Terminated` and its caller-side parent-notification action** in this PR.
> - Introduce `DeliverResult::FatalIntent { pid, tid, sig, code }`, documented as performing **no
>   notification, no parent wake, no status write, no scheduler mutation and no resource work**; its
>   only legal use is to be handed to the S1 request transaction after the PM borrow ends.
> - Notification is discharged solely by the ledger obligations created in S1 and redeemed by the
>   victim's own commit.
> - The P0 ratchet gains: no notify/wake call in any `deliver_*`/`DeliverResult` consumer, and the
>   `Terminated` variant is asserted absent **by name** so a later phase cannot reintroduce it.

This single change also kills the three problems v1 named and the ratification did not dispute: the
FD-closure loss that design A's `terminate_minimal` hand-off would have caused, the retained
SCHEDULER-under-DAIF-masked-PM inversion (`with_thread_mut` takes SCHEDULER at `scheduler.rs:3101-3109`
while the PM guard is live), and a `log::info!` under mask. The four EL0 fault sites
(`exception.rs:768,1135,1230,1333`) collapse to one TID-attributed adapter (DESIGN §2.4): TID first,
stack-slot cross-check, CR3 as root-consistency only, divergence counted and never fatal,
`AttributionUncertain` → safe redirect, mandatory tails always unconditional.
`interrupts/context_switch.rs:1021` (x86_64) converges. **`Process::terminate` is deleted** with its
last caller.

**Files.** `kernel/src/signal/delivery.rs`, `kernel/src/arch_impl/aarch64/exception.rs`,
`kernel/src/arch_impl/aarch64/context_switch.rs`, `kernel/src/interrupts/context_switch.rs` (Tier-2),
`kernel/src/process/process.rs`. **~220 lines, 3 commits — split by architecture if it crosses.**

**Gate extras.** New `clonevm_fault_test`: a CLONE_VM child faults at EL0 → the **child** dies, the
parent survives, `TEARDOWN_VICTIM_DIVERGENCE == 1`, no refault loop. **This test fails on `main`
today** — it is a live wrong-victim livelock, so the test is meaningful rather than tautological.
`sigsegv_default_action_test`: status `-11` reaped exactly once, CoW accounting balanced,
`TEARDOWN_MASKED_FRAMES_WALKED == 0`. **Duplicate-notify negative test: a fatal signal delivered to a
process that is concurrently faulting produces exactly one SIGCHLD and one `btrt` report** (the
window the deleted `Terminated` action would have opened). Explicit x86 regression run (two prior
rounds shipped accidental x86 divergences). P0 ratchet's `\.terminate\(` allowlist must now be
**empty**, asserted as an exact set, and `DeliverResult::Terminated` asserted absent by name. Soak.

**Strictly better.** Every death path in the kernel now runs the same state machine; the last eager
teardown-under-PM-borrow is gone; a live wrong-victim livelock is fixed; and there is exactly one
notifier in the kernel.

**Revert.** Restore the two delivery blocks, the `Terminated` variant with its notify action, and the
four inlined fault bodies (preserved in the commit bodies); `Process::terminate` returns.

---

## Phase 12 — Init death policy *(#464 part 2 — separate PR by construction)*

**Scope — v2 (condition 7): Linux-faithful protected init. No `EPERM`.**

- A **user-originated** signal to the designated init whose effective disposition is the default fatal
  action, and for which init has installed **no handler**, is **silently dropped**: never queued,
  never made pending, never delivered, and the sending syscall **returns success (0)**. This is what
  Linux does for a protected init (`kernel/signal.c`, `prepare_signal`/`sig_task_ignore` region
  ~L79-117 and `__send_signal`/`complete_signal` ~L977-1083). Counted `INIT_FATAL_SIGNAL_DROPPED`.
- A signal init **has a handler for** is delivered and handled normally — protection is
  disposition-scoped, not signal-scoped.
- Kernel-fatal, and only these: init's **own** `exit`/`exit_group`, an **unhandleable** synchronous
  fatal fault taken by init, or a nonviability invariant.
- S1 sets `INIT_DEATH_LATCH` with one relaxed store, **only** for a committed, certainly-attributed
  victim. Because external kills are dropped at send, the latch's only producers are init's own exit
  commit (P8) and the unhandleable-fault path (P11) — a strictly smaller producer set than v1's.
- S5 reads the latch in ordinary kernel context with all guards out of scope and DAIF restored,
  records a pre-panic lock/IRQ snapshot, then panics. **No `#[cfg]` gate** — designation is runtime
  data, so a build that never designates an init can never trip the policy, and
  `interactive = ["testing"]` cannot invert anything.

**Files.** `kernel/src/syscall/signal.rs`, `kernel/src/signal/delivery.rs`,
`kernel/src/task/teardown.rs`, `kernel/src/process/manager.rs`, tracing reader, tests.
**~120 lines, 2 commits.**

**Gate extras.** `kill(1, SIGKILL)` from userspace **returns 0**, init survives, its pending signal
set is unchanged, `INIT_FATAL_SIGNAL_DROPPED == 1`, and `INIT_DEATH_LATCH == 0`. **No test asserts
`EPERM`** — the v1 assertion is deleted, not inverted. A signal init *does* handle is delivered
normally (handler runs, counter does not increment). Deliberate init-death test (init calls
`exit_group`) asserts the panic message **and that the panic reports completely** on serial (proving
no lock was held), with the snapshot showing PM owner `None`, scheduler owner `None`, normal IRQ
state. `INIT_PANIC_WITH_LOCK == 0`. A normal boot asserts the latch stays 0 for the whole run
**including the `smoke_hello_time` harness** — the exact build all four prior attempts broke. A
test-build PID-1 process that is *not* designated exits normally.

**Strictly better.** Closes #464 with the runtime flag the issue asked for, on the fifth attempt,
without the cfg landmine that killed the first four — and with Linux's actual semantics rather than an
undocumented ABI divergence.

**Revert.** Delete the latch, the drop rule and the escalation call; identity (P5) survives
independently.

---

## 1. Cumulative outcome at each stop point

| Stop after | #491 | #464 | #471 | Net vs `main` |
|---|---|---|---|---|
| P0 | — | — | — | Bypass surface pinned; #492 overflow visible; two pre-existing lock violations measured |
| P1 | — | — | — | + grace cannot elapse unordered/empty; refusals attributable; no proof under the queue lock |
| **P2** | **live UAF closed** (remote-mark strength) | — | — | + quarantine, expedite, SIGCHLD on kill; + no reclaim enqueue under PM |
| P3 | ↑ | — | **detach done** | + wrong-victim-after-exec impossible |
| P4 | ↑ | — | ↑ | + all three creation paths grace-protected |
| P5 | ↑ | **identity done** | ↑ | + AC-1/4/5 structural |
| **P6a** | ↑ | ↑ | ↑ | + a row outlives its reap; removal is proof-gated |
| P6b | ↑ | ↑ | ↑ | + exactly-once notification with a claim protocol (#418's own follow-up) |
| P7 | ↑ | ↑ | ↑ | + no FD/endpoint lock or alloc under PM on any exit |
| P8 (needs OQ-5) | ↑ | ↑ | ↑ | + normal exit is victim-owned, through the boundary hook |
| **P9** | **nothing boundary-reachable dies remotely** | ↑ | **seal done** | + atomic group membership; legacy arm counted per family |
| P10a/b/c | **complete at 10c** | ↑ | ↑ | + each wait family becomes promptly killable; legacy arm deleted at 10c |
| P11 | ↑ | ↑ | ↑ | + one machine for all five paths; one notifier; live livelock fixed |
| P12 | ↑ | **complete** | ↑ | + Linux-faithful init protection without the cfg landmine |

Stopping anywhere is a shippable, defensible state. **P2, P3, P5, P9, P10c, P12 are the six points
where an issue's headline claim actually changes** — those are the PR bodies that must be written most
carefully, and the ones where an overstated commit message would be a blocking finding. *(v2: #491's
"complete" milestone moved from the cutover phase to P10c, because the legacy remote-mark arm is still
live until then. Claiming completion at P9 would be exactly the kind of overstatement the round-level
stop rule exists to catch.)*

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

**Ratification gate (condition 7 of the refusal).** These revised documents must obtain a **new
ratification pass** before implementation begins. No phase — including P0 and P1, which the reviewer
called "the correct first two behaviour-preserving PRs" once the conditions are incorporated — is
cleared for build until that pass returns.
