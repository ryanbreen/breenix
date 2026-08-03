# AArch64 Process Teardown / Reclaim Lifecycle — Ground-Up Design (Designer A)

Target tree: `/Users/wrb/fun/code/breenix` @ `main` (31126c2a). Supersedes
`fix/teardown-followups` (5 commits). Satisfies all 28 findings and all 28 distilled
invariants in `teardown-invariant-corpus.md` **structurally** — by ownership, typestate,
and single-writer protocols — not by guards at call sites.

**No code in this document. It is a specification.**

---

## 0. Core idea in three sentences

A dying process's heavy resources are moved — in one allocation-free `Option::take` under
the PM lock — into a **pre-allocated grave** that was allocated when the process was born,
and the grave is published onto a lock-free intrusive stack that only a dedicated
preemptible kernel thread (`kreclaimd`) ever drains. TTBR0 root liveness stops being a
best-effort scan of two overloaded mechanism words and becomes a **per-CPU occupancy
record with a single-writer superset invariant**: the record names a root strictly before
the hardware install and is cleared strictly after the hardware leave, so "no CPU records
this root" is a *proof* that no CPU can be using it. Every function that walks page
tables, frees frames, or logs requires a `ReclaimContext` capability token that only the
reaper (and, on x86_64, an explicitly-checked syscall-context mint) can produce, which
makes "teardown on an exception-return tail" a compile error rather than a review finding.

---

## 1. Why the two prior attempts failed (structural diagnosis, not blame)

Both rounds fixed the flagged set and created a new one. The corpus shows three repeating
generators of new violations. The design has to kill the generators, not the symptoms.

| Generator | Evidence in the corpus | Structural kill |
|---|---|---|
| **G1. "Is this root live?" is answered by scanning words that several unrelated writers mutate for unrelated reasons.** `saved_process_cr3` / `next_cr3` are simultaneously (a) the ERET mechanism the assembly consumes and (b) the liveness record teardown reads. Any change to one meaning breaks the other. | R1-7 (shadows cleared without a hardware switch), R2-1 & R2-2 (idle redirect leaves shadows stale forever; the justifying comments were false), branch's `switch_ttbr0_if_needed` fix that closed one window and opened another | **Separate record from mechanism.** New per-CPU word `occupied_root`, single writer (the local CPU), written before every install and cleared after every leave. Mechanism words stay untouched and are folded in only as extra conservatism. §5 |
| **G2. The "already terminated" guard and the "release the resources" action are two separate statements, so refactors keep separating them further.** | R1-5 (guard dropped when `terminate()` → `terminate_minimal()`), R2-5 (children taken one statement before the guard), R2-6 (guard skipped the waitpid wakeup), R2-11 (`Option` return silently disabled the idle redirect) | **Make the guard *be* the action.** `Process::grave: Option<Box<ProcessGrave>>`; `grave.take()` is simultaneously the one-shot test and the resource move. A second exit gets `None` and cannot double-free because there is nothing left to free. §4, §6 (C2) |
| **G3. Teardown's execution context is decided per call site, so every new call site re-litigates "is this a legal place to free a page table?"** | R1-1/2/3/4 (teardown reached the ERET tail), R2-3/4 (panic + heap `Vec::push` under PM lock + IRQs off), R2-8 (broadcast TLBI under PM lock) | **Make the context a type.** `ReclaimContext` token required by every teardown/logging function; exactly one mint site on aarch64 (the reaper). §3 (P0), §7 |

---

## 2. Scope boundary (deliberate non-goals)

The r10 reconciliation's Step-3 proposal (`Arc<AddressSpace>`, CLONE_VM sharing, replacing
`Option<Box<ProcessPageTable>>` everywhere) is **out of scope**. It is a correct long-term
direction but it is a multi-hundred-line refactor across fork/exec/clone and is not needed
to satisfy any of the 28 invariants. This design keeps `Option<Box<ProcessPageTable>>` and
achieves the same safety with a grave + occupancy record. Called out so a later reviewer
does not read its absence as an oversight.

Also out of scope: any change to `syscall_entry.S` or `boot.S`. **Zero assembly diff.** §10.

---

## 3. Design pillars

### P0 — `ReclaimContext`: teardown context becomes a type (kills invariants 1–4, 20 at compile time)

A zero-sized capability token `ReclaimContext(())` in a new module `kernel/src/task/reclaim.rs`.
It has no public constructor; the only way to obtain one is `ReclaimContext::assert_preemptible()`,
which `debug_assert!`s: interrupts enabled, `preempt_count` has no PREEMPT_ACTIVE bit, and the
per-CPU PM-lock-depth counter is zero.

Every function that does any of the following takes `&ReclaimContext`:

- walks or frees page-table structure frames (`ProcessPageTable::cleanup_for_exec`,
  `cleanup_cow_page_table`, `Process::cleanup_cow_frames`)
- calls `frame_decref` / `deallocate_frame` in bulk
- emits `log::*` from within a teardown routine

Mint sites, exhaustively:

- **aarch64: exactly one** — the `kreclaimd` loop body.
- **x86_64: exactly one** — `arch_retire_address_space()`, which on x86 frees inline
  because x86 exits already run in preemptible syscall/fault context with no PM lock held
  at that point (see §8 arch hooks).

Consequence: a future agent adding `reclaim_deferred_process_resources()` to
`check_need_resched_and_switch_arm64` cannot compile without visibly writing
`ReclaimContext::assert_preemptible()` on an exception-return tail — a diff that reviews
itself. This is the single highest-leverage element of the design; R1-1/2/3/4 and R2-4 all
required a human to notice a call graph.

### P1 — TTBR0 occupancy record: one writer, superset invariant (kills 6–9, 11, 14)

Full protocol in §5. Summary: a new per-CPU `occupied_root` word is the *record*;
`saved_process_cr3`/`next_cr3` stay exactly as they are and remain the *mechanism* the
assembly consumes. The record is written before every hardware install and cleared only
after a hardware leave, giving:

> **OCC:** for every CPU *c* and every root *R*, at every instant, if `TTBR0_EL1(c)` names
> *R* then `occupied_root(c) == R ∨ saved_process_cr3(c) == R ∨ next_cr3(c) == R`.

The oracle is the union over online CPUs of those three words (plus a direct
`mrs ttbr0_el1` for the *local* CPU, which is exact and free). Over-reporting delays a
free; under-reporting is impossible. §5 proves each transition.

### P2 — The pre-allocated grave (kills 3, 12, 13, 16, 21, 22)

`Process` gains `grave: Option<Box<ProcessGrave>>`, allocated at process construction
(same place and same failure mode as the page table: creation returns `ENOMEM`).
At exit, the commit is a sequence of moves:

```
grave.page_table        ← process.page_table.take()
swap(grave.old_tables, process.pending_old_page_tables)   // buffer swap, no alloc, no free
grave.stack             ← process.stack.take()
grave.after_epoch       ← retirement_grace_target()
```

`swap` (not `extend`, not `take`) is load-bearing: it moves the populated buffer into the
grave and hands the grave's empty buffer back to the `Process`, so **no heap allocation and
no heap free occurs at the commit point**. The `Process` row keeps that empty buffer until
`waitpid` reaps it in normal context.

Publication is a Treiber push (`compare_exchange` Release on an `AtomicPtr` head) using an
intrusive `next` pointer inside the grave. Therefore:

- **no allocation** at exit → R2-4 dissolves (no heap lock under PM lock/IRQs-off)
- **no lock** at exit → R1-2's global-spinlock contention dissolves
- **no capacity cap** → R2-3 dissolves (nothing to overflow, nothing to panic about);
  the queue is bounded by the number of live processes, which is already bounded by memory
- the grave's presence/absence **is** the double-terminate guard → R1-5, R2-5 dissolve

### P3 — `ExitReceipt`: phase 2 cannot be skipped (kills 15, 17)

`ProcessManager::retire_process(pid, code)` **always** returns an `ExitReceipt` — never
`Option<ExitReceipt>`, never `bool`. The receipt is `#[must_use]`, has no public fields, and
its only method is `complete(self)`. With the project's zero-warnings policy, dropping a
receipt is a build failure. Phase-2 side effects (FD close, window cleanup, waitpid wakeup,
reaper wake, logging) live inside `complete()` and therefore run on **every** path,
including the already-terminated path, where they simply have nothing to do.

R2-6 (waitpid wakeup skipped) and R2-11 (`None` return disabled the idle redirect) are both
"an early return skipped a mandatory tail". A non-optional must-use receipt makes that
shape unrepresentable.

### P4 — One retirement implementation, four callers (kills 14, 26)

Today `ProcessManager::exit_process` (fault paths, x86 interrupts) and
`ProcessScheduler::handle_thread_exit` (syscall path) are two divergent implementations of
the same lifecycle; every fix to one drifts the other. They merge into
`ProcessManager::retire_process(pid, code) -> ExitReceipt`.
`handle_thread_exit(tid, code)` becomes a five-line wrapper: resolve tid→pid under the PM
lock, call `retire_process`, drop the lock, `receipt.complete()`.

### P5 — Thread retirement and process retirement are two independent one-shots (kills 15, 17; handles multithreaded processes)

- **Thread retirement** (`retire_thread`): always runs, per thread. Marks the `Thread`
  `Terminated`, removes it from the ready queue, stamps a `retirement_grace` entry,
  decrements `Process::live_thread_count`.
- **Process retirement** (`retire_process`): one-shot per process. Fires when
  `live_thread_count` reaches 0, or immediately on a process-fatal event (SIGSEGV /
  `exit_group` / `SIGKILL`), in which case it first marks every sibling thread terminated
  and each sibling's own `retire_thread` still decrements the count.

The address space is committed to the grave by `retire_process` only. The commit does not
have to wait for siblings on other CPUs to notice — their `occupied_root` still names the
root, so the reaper's liveness test blocks the free until each of them leaves. **No IPI is
required for correctness**; cross-CPU quiescence falls out of P1.

This is the structural answer to R2-7. The branch tried to fix "which thread drives the
exit" with a `main_thread.id` predicate at one call site and no predicate at the sibling
call site. The correct answer is that the question is wrong: the exit driver never touches
another CPU's state at all (P6), so there is no ownership predicate to get wrong.

### P6 — TTBR0 state is only ever mutated by the CPU that owns it, via a no-argument primitive (kills 6, 8, 14, 19)

`quiesce_ttbr0_for_exit()` and `current_cpu_retains_ttbr0_root(root)` are **deleted**.
They are replaced by `leave_process_ttbr0()` — no arguments, no pid, no root. It means
exactly one thing: *"I, this CPU, am no longer using whatever process root I have."* A
function with no arguments cannot be misapplied to an unrelated pid (R1-6), and there is no
per-call-site predicate to apply inconsistently (R2-7 / invariant 14).

`retire_process` performs **no TTBR0 operation whatsoever**. Therefore no broadcast TLBI is
ever issued under the PM lock with interrupts disabled (R2-8 / invariant 19).

### P7 — The broadcast TLB invalidate moves to the reaper (kills 19; reduces fault-path cost vs. main)

`leave_process_ttbr0()` is `msr ttbr0_el1, kernel; isb` plus three per-CPU stores — **no
TLBI**. A stale user TLB entry under the kernel root is harmless *until the frames are
reused*, and frame reuse happens at exactly one place: the reaper's free step. So the
reaper issues one `tlbi vmalle1is; dsb ish; isb` per batch, immediately before freeing, in
a fully preemptible context. This is strictly cheaper than main, which issues the broadcast
form at four fault-arm sites.

`install_process_ttbr0()` keeps its existing TLBI (unchanged behavior).

### P8 — No caps, no panics; a stall watchdog instead (kills 21, 22, 23)

There is no bounded queue to exhaust, so there is no overflow policy and no panic-under-lock
(R2-3). A wedged CPU produces a *pinned, attributed, observable* address space, not a silent
death: the reaper's watchdog emits one `log::warn!` per grave whose age exceeds a threshold,
naming the pid, the root, and the **bitmask of CPUs whose record still names it**. The
counters are read by a new `dump_reclaim_state()` section printed early in
`dump_fatal_postmortem_once()` (invariant 25), using `raw_uart_*` only.

The one genuinely bounded structure that remains is the 16-slot-per-CPU deferred-fault-exit
ring, which already exists on main. Its overflow policy is: `push` returns false → increment
`DEFERRED_FAULT_EXIT_DROPPED`, still terminate the thread and redirect to idle, never panic.

---

## 4. Lifecycle state machine

### 4.1 Process states

```
                 exit(2) / exit_group / fatal fault / SIGKILL
   RUNNING ───────────────────────────────────────────────▶ EXITING
                                                               │
                                        live_thread_count == 0 │  (C2: grave.take())
                                                               ▼
                                                            RETIRED
                                                               │  (C3: Treiber push)
                                                               ▼
                                                    QUEUED (in GRAVEYARD)
                                                               │
                        grace_elapsed(after_epoch) ∧ ¬root_is_live(all roots)
                                                               │  (C4: reaper frees)
                                                               ▼
                                                             FREED
   RETIRED/QUEUED/FREED ──── waitpid() reaps row ────▶ GONE  (C6, independent axis)
```

`RETIRED → QUEUED` is instantaneous (same statement sequence). They are listed separately
because the *owner* differs: between them the grave is owned by the exiting CPU's stack
frame; after C3 it is owned by the graveyard and the exit path may not touch it.

The `waitpid` reap axis (C6) is independent: the `Process` row survives as a zombie holding
only `exit_code`, `pid`, `parent`, `name`, and an empty grave-vec buffer. It holds no page
tables, no stack, no FDs.

### 4.2 Thread states (unchanged shape, tightened predicate)

```
   RUNNING ──(retire_thread)──▶ TERMINATED ──(¬stack_live ∧ grace_elapsed)──▶ dropped
                                     │                                          (C5)
                                     └─ stamped with RetirementTarget at transition
```

`retire_thread` must run **after** the CPU has architecturally left the thread's kernel
stack (syscall path: inside `exit_schedule_trampoline`, already correct on main; fault
path: after `set_idle_stack_for_eret()` has repointed SP and the frame has been redirected).
This is invariant 10 and main already satisfies it on the syscall path; the fault path is
covered by the grace + `is_kernel_stack_slot_live` predicate at C5.

### 4.3 Per-CPU TTBR0 states

```
   OCCUPIED(R) ──leave_process_ttbr0()──▶ KERNEL ──install/arm_process_ttbr0(R')──▶ OCCUPIED(R')
        │                                                                              ▲
        └──────────── install/arm_process_ttbr0(R') (direct handover) ─────────────────┘
```

Both edges out of `OCCUPIED(R)` are safe under OCC (§5). A CPU in `KERNEL` records
`occupied_root == 0`, `saved_process_cr3 == kernel_ttbr0`, `next_cr3 == 0`.

---

## 5. The TTBR0 occupancy protocol (proof of the OCC invariant)

### 5.1 The three words

| Word | Offset | Role | Writers |
|---|---|---|---|
| `next_cr3` | 64 | **mechanism**: "assembly, install this on the way out" | Rust arm/install/leave; `syscall_entry.S` clears it |
| `saved_process_cr3` | 80 | **mechanism**: "assembly, re-install this if nothing pending" | `syscall_entry.S` stamps it at syscall entry (`mrs ttbr0_el1`); Rust install/leave |
| `occupied_root` | **144 (new, from existing padding)** | **record**: "this CPU may still be using this root" | **local CPU only**, via exactly three helpers |

`PerCpuData` has 48 bytes of tail padding (`_pad3: [u8; 48]`, total size asserted at 192).
`occupied_root` takes offset 144 and `_pad3` shrinks to 40. **No existing assembly offset
moves**, so §2's zero-assembly-diff constraint holds.

### 5.2 The three helpers (the only TTBR0 writers in the tree)

**`leave_process_ttbr0()`** — the only way a CPU stops using a process root.
```
if read_ttbr0_el1() == kernel_ttbr0 && occupied_root == 0 { return }   // idempotent fast path
msr ttbr0_el1, kernel_ttbr0 ; isb
saved_process_cr3 = kernel_ttbr0      // NEVER 0 — see §5.5
next_cr3          = 0
occupied_root     = 0                 // Release, LAST
```

**`install_process_ttbr0(R)`** — Rust performs the hardware switch (dispatch path).
```
occupied_root = R                     // Release, FIRST
dsb ishst ; msr ttbr0_el1, R ; isb ; tlbi vmalle1is ; dsb ish ; isb
saved_process_cr3 = R
next_cr3          = 0
```

**`arm_process_ttbr0(R)`** — assembly will perform the hardware switch (first-entry / armed paths).
```
occupied_root = R                     // Release, FIRST
next_cr3      = R
```

### 5.3 Proof that OCC holds across every transition

Let *O* be the previously-occupied root and *R* the new one. Reads are Acquire; the local
writes are volatile and ordered by the `dsb`/`isb` already present.

| Transition | Instant | Hardware | Record set {occ, saved, next} | OCC? |
|---|---|---|---|---|
| **install(R)** | before `occupied_root = R` | O | {O, O, 0} | O covered |
| | after `occupied_root = R`, before `msr` | O | {R, **O**, 0} | O covered by `saved` |
| | after `msr`, before `saved = R` | R | {**R**, O, 0} | R covered by `occ`; O over-reported (safe) |
| | after `saved = R`, `next = 0` | R | {R, R, 0} | R covered |
| **arm(R)** + asm `str xzr,[x0,#64]` then `msr` | after `occupied_root = R`, before asm | O | {R, **O**, R} | O covered |
| | asm cleared `next`, before `msr` | O | {R, **O**, 0} | O covered |
| | after asm `msr` | R | {**R**, O, 0} | R covered by `occ`; O over-reported |
| **leave** | before `msr` | O | {O, O, 0} | O covered |
| | after `msr`, before stores | kernel | {**O**, O, 0} | O over-reported (safe) |
| | after all three stores | kernel | {0, kernel, 0} | nothing occupied |

**No row under-reports.** This is exactly the property the branch's
`switch_ttbr0_if_needed` change was reaching for and missed on the idle path (R2-1/R2-2),
and it is why `occupied_root` must exist: at `arm(R)` time the CPU has *not* left *O*, so a
two-word record whose words are both overloaded by the assembly cannot express
"{O and R are both possibly in use}".

### 5.4 Bounding the over-report (so graves are not pinned forever)

Over-reporting delays a free. Each stale over-report has a bounded clearing event:

| Stale word | Cleared by |
|---|---|
| `occupied_root` stale after the CPU logically stops running the process | the next `install_process_ttbr0`, `arm_process_ttbr0`, or `leave_process_ttbr0` on that CPU |
| `saved_process_cr3` stale after an asm-performed install | the next syscall entry (`mrs ttbr0_el1` → `saved`), the next Rust install, or the next leave |

The load-bearing addition is that **redirect-to-idle now performs a real leave**
(`setup_idle_return_locked` and `set_idle_stack_for_eret`, §11). Before this design, an
idle CPU had no clearing event at all — that is R2-1/R2-2, and it is the reason the
"blocked sweeps climb forever" failure mode existed. After it, the closure is total: a CPU
is always in exactly one of {running a user thread with a fresh record, in the kernel with
the record of the thread it will return to, idle with an empty record}.

### 5.5 Why `saved_process_cr3` is never set to 0

`syscall_entry.S:.Lrestore_saved_ttbr` does `cbz x1, .Lafter_ttbr_check`. Writing 0 makes
the assembly *skip* the restore, so an ERET to EL0 runs with the kernel root and takes an
immediate instruction abort — R1-6's failure. Writing `kernel_ttbr0` instead costs the same
and produces the correct behavior on every path: an idle redirect gets the kernel root
(correct), and a hypothetical ERET-to-EL0 after a leave faults loudly at EL0 rather than
running EL1 code on a freed root. **Rule: the mechanism words never carry 0-as-"unknown";
`next_cr3 == 0` retains only its existing meaning of "nothing pending".**

### 5.6 The oracle

`root_is_live(R) -> bool` and `root_live_mask(R) -> u32` (for diagnostics):

```
local CPU:  read_ttbr0_el1() & MASK == R          // exact, authoritative, free
any online CPU c: occupied_root[c] | saved_process_cr3[c] | next_cr3[c] matches R
```

Invariant 7 asks the oracle to consult the hardware register as an authoritative source.
It does so for the local CPU, where that is possible. For remote CPUs there is **no
architectural way to read another CPU's `TTBR0_EL1`** — the honest statement the branch's
comment fumbled. The record is a *proven* superset (§5.3), which is strictly stronger than
a hardware read would need to be. This reasoning belongs in the doc comment verbatim so the
next agent does not "fix" it back.

Offline CPUs are skipped. Breenix does not hot-unplug CPUs post-boot; a `debug_assert!` in
`retirement_grace_target()` records that assumption where it is depended on (invariant 12).

---

## 6. Commit points (exactly one per transition)

| # | Transition | Exact commit statement | Context | Idempotence mechanism |
|---|---|---|---|---|
| **C1** | thread RUNNING → TERMINATED | `Thread::set_terminated()` + `remove_from_ready_queue` + `retirement_grace.push(RetirementTarget)` inside `Scheduler::retire_thread(tid)` | syscall path: `exit_schedule_trampoline`, already pivoted to the per-CPU scheduler stack. fault path: after `set_idle_stack_for_eret()` | state test on `ThreadState` |
| **C2** | process EXITING → RETIRED | `Process::commit_grave()` — `self.grave.take()?` followed by the move/swap block (§P2) | `ProcessManager::retire_process`, PM lock held, IRQs off. Pure moves; no alloc, no free, no lock, no logging | **`Option::take` — the guard *is* the action** |
| **C3** | RETIRED → QUEUED | `GRAVEYARD.compare_exchange(head, grave_ptr, Release, Relaxed)` in `push_grave` | same critical section as C2 or immediately after; allocation-free, lock-free | CAS; a grave is pushed once because it is moved by value |
| **C4** | QUEUED → FREED | in `reclaim_pass`: `invalidate_user_tlb_broadcast()` **once per batch**, then per-grave `cleanup_cow_page_table` / `cleanup_for_exec` / drop | `kreclaimd` only; `&ReclaimContext` required | grave popped by `swap(null, Acquire)`; a grave is examined by one reaper |
| **C5** | thread TERMINATED → dropped (kernel stack freed) | `Scheduler::threads.retain(...)` drop in `reclaim_terminated_threads` | `sys_fork_aarch64` (IRQs on, no PM lock) and `kreclaimd` | predicate `¬is_kernel_stack_slot_live(top) ∧ grace_elapsed(target)` |
| **C6** | zombie row → GONE | `waitpid` removes the row from `ProcessManager::processes` | syscall context | map removal |

**Ordering constraints between commit points — deliberately minimal.** C1 and C2 are
independent; neither orders the other. Safety comes entirely from the C4/C5 *predicates*,
not from sequencing. That is the point: the prior attempts tried to buy safety with
ordering ("quiesce before publishing"), which requires every future call site to preserve
the order. Predicate-based safety is order-free.

The one real ordering requirement: **C4's predicate must be evaluated with the epoch read
ordered before the liveness read.** Handled in §9.

---

## 7. Reclaim execution context

### 7.1 `kreclaimd` — the single aarch64 mint site

A kernel thread started from `main_aarch64.rs` after `init_workqueue()` /
`init_softirq()`, via the existing `kthread_run(..., "kreclaimd")`.

Loop body, in order:

1. `let ctx = ReclaimContext::assert_preemptible();`
2. `drain_deferred_fault_exits()` — pop the per-CPU tid rings, run the unified
   `handle_thread_exit` for each. *(This is the only drain site; the two calls on
   `check_need_resched_and_switch_arm64` and `schedule_from_kernel` are deleted — invariant 1.)*
3. `let stats = reclaim_pass(&ctx);`
4. stall watchdog: for any grave older than `RECLAIM_STALL_WARN_NS` (5 s), emit **one**
   `log::warn!` naming pid / root / `root_live_mask` / age, and set a "already warned" bit
   on that grave so it warns once, not every pass.
5. sleep, adaptively (§7.3).

`reclaim_pass` itself:

- `let mut list = GRAVEYARD.swap(null, Acquire);` — takes the whole list in one atomic op,
  so the reaper never holds a lock while walking and enqueuers never block.
- partition into `ready` (grace elapsed ∧ ¬root_is_live) and `blocked`.
- if `ready` is non-empty: `invalidate_user_tlb_broadcast()` **once**, then free each ready
  grave (page-table walks, `frame_decref`, `deallocate_frame`, `GuardedStack` drop, and the
  `Box`/`Vec` frees — all legal here).
- re-push `blocked` onto the graveyard (order does not matter).
- update `GRAVES_PENDING`, `GRAVES_RECLAIMED`, `GRAVE_BLOCKED_PASSES`, `GRAVE_OLDEST_AGE_NS`.

### 7.2 The opportunistic drain at the allocation point (backpressure without a cap)

Keep main's existing `sys_fork_aarch64` call (IRQs on, no PM lock, before allocating the
child page table), but change it to `reclaim_pass` **once, not in a loop**, so fork latency
is bounded. This is the "reclaim before you allocate" pressure valve that makes queue growth
self-limiting; combined with P8 it is why no cap is needed. Keep the adjacent
`reclaim_terminated_threads()` call for the same reason.

### 7.3 Sleep policy (kills R2-10)

The branch's fixed 10 ms `block_current_for_timer` + `schedule_from_kernel` had three
defects: it spun at full CPU if `with_scheduler` returned `None` or `schedule_from_kernel`
early-returned; it pushed an undeduped `timer_heap` entry every iteration; and it never
parked. Replacement:

- **at most one blocking call per pass** (so at most one `timer_heap` entry per pass);
- if graves or deferred exits are pending → `block_current_for_timer(now + 10 ms)`;
- if the graveyard is empty and the rings are empty → `kthread_park()` (indefinite,
  woken by `kreclaim_wake()`);
- `kreclaim_wake()` is called from `ExitReceipt::complete()` — phase 2, **no locks held**,
  which is why the wake can safely take the kthread registry / scheduler locks;
- if `with_scheduler` returns `None` **or** the block call reports it did not take effect →
  fall back to `kthread_park()`. Never a bare `continue`. The loop has no path that
  re-executes step 3 without an intervening block or park.

### 7.4 x86_64

`arch_retire_address_space(grave)` on x86 mints a `ReclaimContext::assert_preemptible()`
and frees inline, preserving today's behavior exactly. The grave/receipt/one-shot structure
is shared, so the two arches cannot drift in the *lifecycle*; only the reclaim timing
differs, and that difference is one named function (invariant 26).

---

## 8. Ownership: who owns each resource at each stage

| Resource | RUNNING | EXITING (pre-C2) | RETIRED/QUEUED | FREED / reaped |
|---|---|---|---|---|
| page-table root (`Box<ProcessPageTable>`) | `Process.page_table` | `Process.page_table` | `ProcessGrave.page_table` (graveyard) | frame allocator (C4) |
| old exec tables (`Vec<Box<…>>`) | `Process.pending_old_page_tables` | same | `ProcessGrave.old_page_tables` (buffer swapped in at C2) | frame allocator (C4) |
| user stack (`GuardedStack`) | `Process.stack` | `Process.stack` | `ProcessGrave.stack` | dropped by reaper (C4) |
| FD entries | `Process.fd_table` | moved into `ExitReceipt` at C2 | — | closed in `complete()` |
| children list | `Process.children` | reparented to init **inside** the C2 one-shot | — | — |
| parent wakeup obligation | — | recorded as `parent_tid` in the receipt | — | discharged in `complete()` |
| window buffers | graphics subsystem | graphics subsystem | — | freed in `complete()` |
| kernel stack (per thread) | `Thread.kernel_stack` | `Thread` (still architecturally live) | scheduler `retirement_grace` | pool (C5) |
| `Thread` object | `Scheduler.threads` | `Scheduler.threads` (Terminated) | `Scheduler.threads` (grace pending) | dropped (C5) |
| TTBR0 occupancy | per-CPU `occupied_root` of every CPU running a thread of this process | cleared per-CPU by `leave_process_ttbr0` as each CPU leaves | must read 0 on all CPUs before C4 | — |
| `Process` row (zombie) | PM map | PM map | PM map | removed by `waitpid` (C6) |
| the grave `Box` itself | `Process.grave` (pre-allocated at birth) | taken at C2 | graveyard | dropped at C4 |

The critical read of this table: **at no stage are two owners listed for the same resource.**
The prior attempts had the page-table root owned by both `Process` and
`PendingProcessReclaim` in different `#[cfg]` branches of the same function, and the user
stack dropped by the exit path while the grave owned the address space (R1-9, R2-12).

`GuardedStack::drop` remains an unimplemented TODO stub. Because the stack now lives in the
grave until after quiescence, **implementing it later is safe by construction** — the prose
comment the branch relied on ("safe only while drop is a no-op") is deleted, not reworded.

---

## 9. Memory ordering for the grace period (kills 12)

Three fixes to `kernel/src/task/scheduler.rs`:

1. `retirement_grace_target()` returns a `RetirementTarget { epochs: [u64; MAX_CPUS],
   online_mask: u32 }`. A target with `online_mask == 0` is *invalid* and
   `retirement_grace_elapsed` returns `false` for it, forever. This removes the
   "zero atomic loads, therefore no barrier, therefore true" hole (R1-8) instead of relying
   on the unstated `is_cpu_online(0)` precondition.
2. `retirement_grace_elapsed()` executes an unconditional
   `core::sync::atomic::fence(Ordering::Acquire)` before returning, on every path including
   the short-circuit path. The dependent plain/volatile reads
   (`is_kernel_stack_slot_live`, `root_is_live`) are then ordered after the epoch reads by
   construction, not by luck.
3. Keep the branch's epoch-before-liveness statement reorder in
   `reclaim_terminated_threads` (commit 867ce0c6 — correct, and now actually backed by a
   fence).

`root_is_live` reads each per-CPU word with `Acquire`; the writers use `Release`. Combined
with (2), C4's predicate is sound.

---

## 10. Frozen-region and hot-path audit (kills 1, 5, 19)

**Frozen regions — byte-for-byte unchanged. None is referenced by any change below.**

| Frozen region | Touched? |
|---|---|
| `context_switch.rs` EL0 dispatch site (no CPU0-specific guard) | No |
| `context_switch.rs::idle_loop_arm64` (sleep gate + `dsb sy; wfi; daifclr`) | No |
| `context_switch.rs::aarch64_enter_exception_frame` (ISB before dispatch ERET) | No |
| `gic.rs::init_gicv3_redistributor` SGI-enable block | No |
| `timer_interrupt.rs` handler arm-at-top | No |
| `timer_interrupt.rs` CPU0 regression alarm | No |

**Assembly — zero diff.** `boot.S` and `syscall_entry.S` are not modified. `occupied_root`
occupies existing tail padding so no consumed offset moves; the 192-byte `PerCpuData`
size assertion is preserved.

**Exception-return tails — net negative work.**

| Path | Delta |
|---|---|
| `check_need_resched_and_switch_arm64` | **−1 call** (`drain_deferred_fault_sigsegv_exits` removed). Nothing added. Also removes R1-3's "heavyweight work before the PREEMPT_ACTIVE early return" entirely, since there is no work before it. |
| `schedule_from_kernel` | **−1 call** (same). |
| `boot.S` sync/IRQ ERET epilogues | unchanged |
| `syscall_entry.S` epilogue | unchanged |
| `aarch64_enter_exception_frame` | unchanged |
| the four fault arms | `switch_ttbr0_to_kernel()` (`msr` + `isb` + `tlbi vmalle1is` + 2 × `dsb`) → `leave_process_ttbr0()` (`msr` + `isb`) — **cheaper**; and `retire_process` no longer performs any TTBR0 op, removing the second and third quiesce (R1-11) |
| `setup_idle_return_locked` | **+1 `msr` + 1 `isb`**, −0 stores (three stores replace two). This is the design's only added instruction on any path reachable from an exception return. |
| `set_idle_stack_for_eret` | same, `+1 msr + 1 isb` |

**Justification for the one addition.** `setup_idle_return_locked` is the redirect-to-idle
path; the added `msr ttbr0_el1, kernel; isb` is the *architectural act* that its current
shadow write only pretends to perform. Without it a CPU ERETs into `idle_loop_arm64` with a
retired user root still in `TTBR0_EL1` — the exact state proven at abort #1 in the r10
reconciliation (IFSC=0x05, EL1 under a dead user root). It is unconditional-cost ~tens of
cycles, no lock, no memory traffic, no TLBI, on idle transitions only, and it is what makes
§5.4's over-report closure total. If measurement shows an idle-transition-rate regression,
the fallback is to gate it on `occupied_root != 0` (a single per-CPU load), which makes it
free for a CPU that is already on the kernel root.

**Logging audit.** No `log::*` / `serial_println!` / `format!` is reachable from: any ERET
tail, any fault handler while the PM lock is held, `idle_loop_arm64`, or `retire_process`.
`retire_process` has no logging at all (main's `log::info!("Process … exiting")` moves to
`ExitReceipt::complete()`). All teardown logging is gated behind `&ReclaimContext`, whose
only aarch64 mint site is the reaper. This is invariant 2 made structural.

---

## 11. Per-path walkthroughs

### 11.1 `exit(2)` from a syscall (the common path)

1. `sys_exit_aarch64` — futex/clear_child_tid work, logging (IRQs on, no PM lock) — unchanged.
2. `leave_process_ttbr0()` (replaces main's `switch_ttbr0_to_kernel()` + two shadow zeroes).
   The CPU is now on the kernel root and records nothing.
3. `handle_thread_exit(tid, code)` → PM lock → `retire_thread` + `retire_process` →
   receipt. Under the lock: `mark_terminated`, `commit_grave` (C2), `push_grave` (C3),
   `take_fd_entries`, reparent children, SIGCHLD, record `parent_tid`. Lock dropped.
4. `receipt.complete()` — `close_extracted_fds`, `cleanup_windows_for_pid`,
   `unblock_for_child_exit` / `unblock_for_signal`, `kreclaim_wake()`, `log::debug!`.
5. `schedule_terminated_from_exit(tid)` — unchanged; pivots to the per-CPU scheduler stack,
   *then* publishes `Terminated` (C1). Invariant 10 already satisfied on main.

### 11.2 EL0 fault (SIGSEGV kill) — the four `handle_sync_exception` arms

1. `leave_process_ttbr0()` at the existing `super::switch_ttbr0_to_kernel()` site.
2. `let receipt = with_process_manager(|pm| pm.retire_process(pid, -11))` — one statement,
   no `Option`, no `terminated`/`already_terminated` booleans.
3. Lock dropped; `receipt.complete()`.
4. `terminate_current_scheduler_thread()` + `set_need_resched()` + frame redirect +
   `set_idle_stack_for_eret()` + `switch_to_idle()` — **unconditional for `from_el0`**.
   These depend only on "this CPU must not resume the faulting user thread", which is true
   regardless of who terminated the process. R2-11 dissolves: there is no return value to
   mis-branch on.
5. The four arms become textually identical modulo their diagnostic prints. Specify them as
   one shared helper `fn kill_current_user_process_and_redirect(frame, pid_root) -> !` so
   there is one body, not four (this is where R2-7's "check at one site, not the other"
   came from).

### 11.3 EL1 fault that kills a user thread (`defer_current_user_thread_sigsegv_exit`)

1. `leave_process_ttbr0()` (idempotent; the fast path makes a repeat call ~free).
2. Resolve the victim tid from the stack-slot owner (main's existing logic — keep).
3. Ring `push`; on failure increment `DEFERRED_FAULT_EXIT_DROPPED` and still redirect.
4. The reaper drains the ring and runs 11.1's steps 3–4 for that tid in a legal context.

### 11.4 Already-terminated re-entry

`retire_process` on an already-`Terminated` process: `mark_terminated` returns false, so
the one-shot body is skipped — no second `commit_grave` (there is no grave), no second FD
extraction, no second reparent, no second SIGCHLD. The receipt is still returned and
`complete()` still runs with empty work. Reparenting and the waitpid wakeup already
happened on the first pass, which is the correct semantics (R2-5, R2-6, invariants 13/15/16).

`children` is taken **inside** the committed branch, after the one-shot test — invariant 16
is satisfied by statement placement that the one-shot structure forces (there is no code
path that reaches the take without having committed).

### 11.5 Multithreaded process, fatal fault on a non-main thread

1. The faulting CPU leaves the root (11.2 step 1) and calls `retire_process`.
2. `retire_process` marks the process `Terminated`, marks **all** sibling threads
   terminated (scheduler call is not needed under the PM lock — record the sibling tid list
   in the receipt and let `complete()` do the scheduler work, keeping invariant 18's shape).
3. `commit_grave` runs immediately — the grave is *published*, not *freed*.
4. Siblings on other CPUs still record the root in `occupied_root`; `root_is_live` is true;
   the reaper waits. Each sibling CPU clears its record on its next dispatch or idle
   redirect. No IPI, no cross-CPU state mutation, no ownership predicate.
5. Optional stage-2 hardening (not required for correctness): if a grave stays blocked past
   the watchdog threshold, the reaper sends `SGI_RESCHEDULE` to the CPUs in
   `root_live_mask`. Sending an SGI is outside the frozen `gic.rs` init block.

---

## 12. Observability and overflow policy

| Counter | Written | Read |
|---|---|---|
| `GRAVES_PENDING` | `reclaim_pass`, `push_grave` | `dump_reclaim_state()`, `/proc/reclaim` |
| `GRAVES_RECLAIMED` | `reclaim_pass` | same |
| `GRAVE_BLOCKED_PASSES` | `reclaim_pass` | same |
| `GRAVE_OLDEST_AGE_NS` | `reclaim_pass` | same |
| `GRAVE_STALL_WARNINGS` | watchdog | same |
| `DEFERRED_FAULT_EXIT_DROPPED` | ring `push` failure | same |

`dump_reclaim_state()` is `raw_uart_*`-only (no locks, no formatting) and is printed by
`dump_fatal_postmortem_once()` **before** `crate::tracing::dump_all_buffers()`, along with
the other high-value sections, per invariant 25 and the r10 reconciliation's Step-0 finding
that the postmortem died inside `dump_all_buffers()` and destroyed every probe that
mattered. Each section is wrapped so a nested abort truncates only what follows.

`/proc/reclaim` is the second reader; either alone satisfies invariant 23, both are cheap.
No counter in this design is write-only (R2-9).

**No panic exists anywhere in the reclaim path.** Invariants 21 and 22 are satisfied by
construction because there is no bounded resource whose exhaustion is fatal.

---

## 13. Invariant coverage (all 28, structurally)

| # | Invariant | Satisfied structurally by |
|---|---|---|
| 1 | no teardown from ERET tails | **P0** token (compile-time) + drain calls deleted from `check_need_resched_and_switch_arm64` / `schedule_from_kernel` |
| 2 | no logging from tails / PM-lock-held / idle | P0 token gates all teardown logging; `retire_process` has zero logging; all logging lives in `complete()` and the reaper |
| 3 | nothing blocking inside IRQs-off / PM lock | C2/C3 are pure moves + one CAS: no alloc (pre-allocated grave + `swap` not `extend`), no lock, no free |
| 4 | early-return gate checked before new heavy work | there is no work before `PREEMPT_ACTIVE` in `check_need_resched_and_switch_arm64` — the call was removed, not reordered |
| 5 | frozen regions untouched | §10 table; zero assembly diff |
| 6 | shadows cleared/republished only after the hardware switch | §5.2 helper bodies; §5.3 proof; `saved_process_cr3` never 0 (§5.5) |
| 7 | liveness oracle consults hardware authoritatively | §5.6: exact local `mrs`; proven-superset record for remote CPUs, with the honest "no architectural remote read exists" rationale in the doc comment |
| 8 | quiesce applied only by the owning CPU, uniformly | **P6**: `leave_process_ttbr0()` takes no arguments; `quiesce_ttbr0_for_exit` / `current_cpu_retains_ttbr0_root` deleted |
| 9 | idle redirect must not leave a stale shadow | §5.4 + §11: both idle-redirect sites perform a real leave |
| 10 | dying thread leaves its stack/root before either is reclaimable | `leave_process_ttbr0()` first in every exit path; `exit_schedule_trampoline` pivot (already on main); C5 predicate |
| 11 | reclamation requires proof no CPU has it live | C4 predicate = `grace_elapsed ∧ ¬root_is_live` over the proven-superset record; C5 predicate = `¬is_kernel_stack_slot_live ∧ grace_elapsed` — neither is a `cpu_state` name-match |
| 12 | epoch read ordered before liveness read | §9: unconditional `fence(Acquire)`; invalid targets never elapse (`online_mask`) |
| 13 | double-terminate guard preserved on every path | **G2 kill**: `grave.take()` is the guard and the action; `mark_terminated` returns the transition bit |
| 14 | same predicate at every call site | there is no per-site predicate left — P6 removed the question |
| 15 | every exit path reparents + wakes waitpid | **P3**: `ExitReceipt` is non-optional and `#[must_use]`; `complete()` is the only tail |
| 16 | state moved out only after early-return checks | the take is inside the committed one-shot branch |
| 17 | return-contract changes honored by every caller | receipt has no `Option`, no `bool`; the fault arms' redirect is unconditional for `from_el0` |
| 18 | FD cleanup outside the PM lock, every path, every arch | one implementation (P4) using `take_fd_entries` + `close_extracted_fds`; `Process::terminate()` and both `close_all_fds()` copies are deleted |
| 19 | no broadcast TLBI under an IRQs-off lock | **P7**: `retire_process` does no TTBR0 work; leave has no TLBI; the broadcast moves to one per-batch call in the reaper |
| 20 | dual-context locks use try_lock or are unreachable | `frame_decref`'s blocking `FRAME_METADATA.lock()` becomes unreachable from interrupt context because P0 confines all teardown to the reaper; documented reachability set on the function |
| 21 | no panic while holding a lock with IRQs off | no panic in the reclaim path at all (P8) |
| 22 | a wedged CPU cannot make a cap fatal | there is no cap (P2/P8); a wedged CPU produces an attributed warning + a pinned grave |
| 23 | diagnostic counters are actually read | §12 table; `dump_reclaim_state()` in the postmortem + `/proc/reclaim` |
| 24 | diagnostics import range constants | `dump_stack_classification` imports `ARM64_KERNEL_STACK_BASE/END` from `memory/kernel_stack.rs` |
| 25 | postmortem prints high-value sections first | `dump_fatal_postmortem_once()` reordered; each section wrapped |
| 26 | no unstated arch asymmetry | shared lifecycle with two named arch hooks (§7.4); `cleanup_for_exec` keeps counters **and** logging on *both* arches (legal on aarch64 now that it only runs in the reaper) |
| 27 | commit messages state full blast radius | §16 commit contract |
| 28 | justifying comments true for every reaching path | the two false comments are deleted along with the code they justified; §5.6's rationale is the replacement and is provable |

---

## 14. File:function-level change list

### New

**`kernel/src/task/reclaim.rs`** (new module, ~280 lines)
- `pub struct ReclaimContext(())`, `ReclaimContext::assert_preemptible()`
- `pub struct ProcessGrave { pid, exit_code, page_table, old_page_tables, stack, after_epoch: RetirementTarget, queued_at_ns, warned: bool, next: *mut ProcessGrave }`
- `static GRAVEYARD: AtomicPtr<ProcessGrave>`
- `pub(crate) fn push_grave(Box<ProcessGrave>)` — Treiber, Release CAS, allocation-free
- `fn take_all_graves() -> *mut ProcessGrave` — `swap(null, Acquire)`
- `pub fn reclaim_pass(&ReclaimContext) -> ReclaimStats`
- `fn kreclaimd_main()`, `pub fn init_reclaim_thread() -> Result<(), KthreadError>`
- `pub fn kreclaim_wake()`
- counters + `pub fn dump_reclaim_state()` (raw-UART, lock-free)
- `#[cfg(not(target_arch = "aarch64"))] pub fn arch_retire_address_space(Box<ProcessGrave>)` — inline free

### Modified

**`kernel/src/per_cpu_aarch64.rs`**
- `PerCpuData`: add `pub occupied_root: u64` at offset 144; `_pad3: [u8; 48]` → `[u8; 40]`; keep the 192-byte assert
- `PerCpuData::new`: initialize it
- add `pub fn occupied_root_snapshot(cpu_id) -> Option<u64>` (remote, Acquire)

**`kernel/src/arch_impl/aarch64/percpu.rs`**
- add `pub fn occupied_root() -> u64`, `pub unsafe fn set_occupied_root(u64)` (offset 144)

**`kernel/src/arch_impl/aarch64/ttbr0.rs`** — the protocol core
- add `fn read_ttbr0_el1()`
- **delete** `switch_ttbr0_to_kernel()`; add `pub fn leave_process_ttbr0()` (§5.2)
- add `pub fn install_process_ttbr0(root)`, `pub fn arm_process_ttbr0(root)`
- add `pub fn invalidate_user_tlb_broadcast()`
- **delete** `quiesce_ttbr0_for_exit()`
- rename `is_ttbr0_root_live` → `pub fn root_is_live(root)`; add `pub fn root_live_mask(root) -> u32`; both per §5.6

**`kernel/src/arch_impl/aarch64/mod.rs`** — re-export set updated to the five new names

**`kernel/src/arch_impl/aarch64/context_switch.rs`**
- `switch_ttbr0_if_needed` → delegates to `install_process_ttbr0`
- the `set_next_cr3(tagged_ttbr0)` site (~4842) → `arm_process_ttbr0(tagged_ttbr0)`
- `setup_idle_return_locked` → replace `set_next_cr3(kernel)/set_saved_process_cr3(0)` with `leave_process_ttbr0()`
- **delete** `drain_deferred_fault_sigsegv_exits()` at ~3438 and ~4459
- `dump_all_eret_frame_anomaly_snapshots` → keep the branch's `decode_last_dispatched` fix (R1-13, correct as-is)
- *unchanged:* `idle_loop_arm64`, `aarch64_enter_exception_frame`, EL0 dispatch site, `schedule_terminated_from_exit`, `exit_schedule_trampoline`

**`kernel/src/arch_impl/aarch64/exception.rs`**
- `set_idle_stack_for_eret` → `leave_process_ttbr0()` in place of the two shadow writes
- new `fn kill_current_user_process_and_redirect(frame_ref, page_table_phys)` — the single shared body for all four `from_el0` fault arms (§11.2); the four arms call it after their own diagnostics
- `defer_current_user_thread_sigsegv_exit` → `leave_process_ttbr0()`; count ring-push failures
- `dump_fatal_postmortem_once` → section reorder + wrapping + `dump_reclaim_state()` early
- `dump_stack_classification` → import `ARM64_KERNEL_STACK_BASE/END`; print slot index + recorded owner tid

**`kernel/src/process/process.rs`**
- add `pub grave: Option<Box<ProcessGrave>>`, `pub live_thread_count: usize`
- `terminate_minimal(code) -> bool` → rename `mark_terminated(code) -> bool` (same body, same guard)
- add `pub fn commit_grave(&mut self) -> Option<Box<ProcessGrave>>` (C2, §P2 — `take` + `swap`)
- **delete** `terminate()`, `close_all_fds()` (both `#[cfg]` copies), `cleanup_cow_frames`'s direct call site
- `cleanup_cow_frames(&mut self, &ReclaimContext)`

**`kernel/src/process/manager.rs`**
- `exit_process(pid, code)` → `#[must_use] pub fn retire_process(&mut self, pid, code) -> ExitReceipt`
  (one-shot body: `mark_terminated`, sibling-thread marking, `commit_grave` + `push_grave`,
  `take_fd_entries`, ready-queue/`current_pid` cleanup, reparent to init, SIGCHLD, record
  `parent_tid`). No logging, no TTBR0, no FD closing, no frame frees.

**`kernel/src/process/mod.rs`**
- add `#[must_use] pub struct ExitReceipt { … }` + `pub fn complete(self)` (phase 2)
- **delete** `exit_current()` (zero callers, `#[allow(dead_code)]` — R2-13; removal, not refactor, per the project's dead-code policy)

**`kernel/src/task/process_task.rs`**
- **delete** `PendingProcessReclaim`, `PENDING_PROCESS_RECLAIMS`, `defer_live_process_resources`, `enqueue_process_reclaim`, `release_process_resources`, `reclaim_deferred_process_resources`
- `ProcessScheduler::handle_thread_exit` → thin wrapper over `retire_thread` + `retire_process` + `receipt.complete()`
- keep `close_extracted_fds` (now called from `ExitReceipt::complete`)
- keep the deferred-fault-exit ring; add the drop counter; the drain moves to the reaper

**`kernel/src/task/scheduler.rs`**
- `retirement_grace_target()` → `RetirementTarget { epochs, online_mask }`
- `retirement_grace_elapsed()` → unconditional `fence(Acquire)`; invalid target ⇒ false
- add `pub fn retire_thread(&mut self, tid)` (C1 in one place)
- `reclaim_terminated_threads` — keep the epoch-first ordering; use the new target type

**`kernel/src/memory/process_memory.rs`**
- `ProcessPageTable::cleanup_for_exec(&self, &ReclaimContext)`; **restore** the counters and
  `log::info!` on aarch64 so both arches report identically (R2-14 / invariant 26)
- `cleanup_cow_page_table(&…, &ReclaimContext)`

**`kernel/src/memory/stack.rs`**
- `GuardedStack::drop` — keep the TODO stub; delete the "safe only because this is a no-op"
  coupling comment from the exit path (the coupling no longer exists)

**`kernel/src/memory/frame_metadata.rs`**
- `frame_decref` — doc comment stating the (now thread-context-only) reachability set;
  no behavior change

**`kernel/src/arch_impl/aarch64/syscall_entry.rs`**
- `sys_exit_aarch64` → `leave_process_ttbr0()` replaces `switch_ttbr0_to_kernel()` + two zeroes
- `sys_fork_aarch64` → `reclaim_pass` once (not a loop); keep `reclaim_terminated_threads()`
- the `set_saved_process_cr3(new_ttbr0)` exec site (~1279) → `install_process_ttbr0(new_ttbr0)`

**`kernel/src/main_aarch64.rs`**
- start `kreclaimd` after `init_workqueue()` / `init_softirq()`

**`kernel/src/interrupts.rs`** (x86_64)
- the two `pm.exit_process(pid, -11)` sites → `retire_process` receipt, `complete()` after
  the PM guard is dropped

**Process creation sites** (`process/creation.rs`, `manager.rs::fork_process_aarch64`,
`syscall/clone.rs`)
- allocate the grave alongside the page table (outside the PM lock, same `ENOMEM` failure
  mode); `live_thread_count` init/increment

### Deleted from the branch (not carried forward)

`current_cpu_retains_ttbr0_root`, `quiesce_ttbr0_for_exit`, `defer_process_resources`,
`enqueue_process_reclaim`, `MAX_PENDING_PROCESS_RECLAIMS` + its panic, the three
write-only counters, `finish_extracted_process_exit`, the `main_thread.id` ownership gate,
the `exit_process -> Option<Vec<…>>` signature, the two false justifying comments, and
`init_process_reclaim_worker`'s fixed-10 ms loop.

---

## 15. Delta from the current branch: what survives, what is replaced

**Survives (adopt as-is):**
- `5781442b` — `decode_last_dispatched` in the ERET-anomaly dumper. Correct, postmortem-only,
  no frozen region. Keep verbatim.
- `867ce0c6` — epoch-read-before-stack-liveness reorder in `reclaim_terminated_threads`.
  Correct; now actually backed by §9's fence.
- The *direction* of `28a7933e` (retirement off the ERET paths, a dedicated reaper kthread)
  and of `c0be17e7` (fault exits share the defer machinery). Both are right; their
  mechanisms are replaced.
- The x86 `let _ = pm.exit_process(...)` call-site adjustments are subsumed by the receipt.

**Replaced:**
- `defer_process_resources` + `PENDING_PROCESS_RECLAIMS` `Vec` + cap + panic → pre-allocated
  grave + Treiber stack, no cap, no panic (P2/P8).
- `quiesce_ttbr0_for_exit` + `current_cpu_retains_ttbr0_root` + the `main_thread.id` gate →
  `leave_process_ttbr0()` with no arguments (P6).
- `is_ttbr0_root_live`'s local-hardware-read patch and the two comment-only "keep the shadow
  visible" changes → the `occupied_root` record with a proved superset invariant (P1/§5).
- `exit_process -> Option<Vec<…>>` + `finish_extracted_process_exit` → `retire_process ->
  ExitReceipt` + `complete()` (P3/P4).
- `terminate_minimal -> bool` used as a *caller-checked* guard → `grave.take()` as the guard
  itself (G2 kill). The `bool` survives, but nothing depends on a caller honoring it.
- `init_process_reclaim_worker`'s fixed 10 ms `block_current_for_timer` loop →
  adaptive park/timed-block with a `None`-safe fallback (§7.3).
- Removal of aarch64 `cleanup_for_exec` counters/logging → restored, both arches identical.

**Net line count** vs. main: roughly +300 (new `reclaim.rs`, occupancy helpers, receipt)
and −250 (deleted `terminate()`, both `close_all_fds()` copies, `exit_current`, the
`PendingProcessReclaim` machinery, and the four duplicated fault arms collapsing into one
helper). The delta is comparable to the branch's, but every line is on the invariant side
of the ledger.

---

## 16. Commit contract (invariant 27)

Land in this order; each commit builds clean on both arches with zero warnings and states
its full reachability.

1. **`fix(aarch64): reorder fatal postmortem and correct kernel-stack ranges`** — Step 0 from
   the r10 reconciliation. Zero hot-path cost. Must state that `dump_stack_classification`
   previously mis-ranged *every* pool frame as `STACK=unknown`.
2. **`fix(aarch64): fence retirement grace reads and invalidate empty targets`** — §9. Must
   state that `retirement_grace_elapsed` previously returned true with zero atomic loads when
   no CPU was recorded online.
3. **`feat(aarch64): per-CPU TTBR0 occupancy record`** — §5, plus the two idle-redirect leaves.
   Must state: adds `msr ttbr0_el1 + isb` to `setup_idle_return_locked` and
   `set_idle_stack_for_eret`, which are reachable from `check_need_resched_and_switch_arm64`;
   removes the broadcast TLBI from four fault arms; zero assembly diff; `PerCpuData` stays
   192 bytes.
4. **`refactor(process): single retirement path with a must-use exit receipt`** — P3/P4/P5.
   Must state that it deletes `Process::terminate()` and both `close_all_fds()` copies and
   moves FD closing off the PM lock on x86_64 as well as aarch64 — a **behavior change on
   x86_64**, disclosed, not silent.
5. **`feat(process): pre-allocated grave + lock-free graveyard`** — P2. Must state that the
   grave allocation moves process-creation failure earlier and that exit becomes infallible.
6. **`feat(aarch64): kreclaimd owns all address-space teardown`** — P0/P7/§7. Must state that
   `drain_deferred_fault_sigsegv_exits` is removed from `check_need_resched_and_switch_arm64`
   and `schedule_from_kernel` (i.e. from every exception return), that `ReclaimContext` is
   required by every teardown function, and that aarch64 `cleanup_for_exec` regains the
   logging x86_64 never lost.
7. **`feat(aarch64): reclaim observability and stall watchdog`** — §12.

Each message carries `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

## 17. Validation gates

1. Both arches build clean, zero warnings (`-D warnings` makes the `#[must_use]` receipt a
   hard gate).
2. QEMU aarch64 × 10 consecutive clean boots: zero `UNHANDLED_EC`, `INSTRUCTION_ABORT`,
   `DATA_ABORT`, `EL1_INLINE_ABORT`, `FATAL_POSTMORTEM`, `[BUG] dispatch_thread`,
   `TTBR_GONE`, panic.
3. **New regression test** (`tests/`, shared QEMU): a parent forks N children that exit
   immediately while continuing to fork, driving `reclaim_terminated_threads` +
   `allocate_kernel_stack` + the 64 KiB scrub against still-on-CPU dying threads. ≥1000
   iterations. Assert: `GRAVES_RECLAIMED == processes_exited`, `GRAVE_STALL_WARNINGS == 0`,
   `DEFERRED_FAULT_EXIT_DROPPED == 0`, zero aborts. Do **not** weaken to "process was created".
4. **New multithreaded regression**: a thread group where a non-main thread faults; assert
   the grave is reclaimed within one watchdog window and `GRAVE_BLOCKED_PASSES` returns to
   a steady state (this is the R2-7 / R2-1 combination that the branch made systemic).
5. **New debug assertion, must never fire**: before C4's free, assert no online CPU's
   `occupied_root` / `saved_process_cr3` / `next_cr3` and no local `TTBR0_EL1` names any
   root in the grave. This is the assertion the r10 reconciliation asked for — it catches
   the bug at the violation instead of four aborts downstream.
6. Parallels streak: 10 consecutive PASS with `inject_retries=0`, up to 15 attempts, fresh
   epoch-named VM per attempt via `./run.sh --parallels`; `prlctl stop --kill` after each.
7. 90-minute soak, watching CPU0 tick-rate parity given the project's CPU0-timer fragility
   (the frozen CPU0 regression alarm is the tripwire).
8. Diff review must show the six frozen regions byte-for-byte unchanged and
   `git diff --stat -- '*.S'` empty.

---

## 18. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| `PerCpuData` field addition breaks an offset assumption not visible in Rust | High if wrong | Field goes in existing tail padding; the 192-byte `const _: () = assert!` is preserved; audited: the highest per-CPU offset any assembly touches is **96** (`eret_scratch`), well below 144, and the Rust-declared fields end at 136 (`eret_guard_source`) |
| The added `msr + isb` in `setup_idle_return_locked` regresses idle-transition latency or perturbs the CPU0 timer path | Medium | Fallback: gate on `occupied_root != 0` (one per-CPU load). Gate 7 (soak + CPU0 tick parity) is the detector |
| `ReclaimContext` propagates into more call sites than expected, ballooning the diff | Medium | Confine the token to the four teardown entry points (`cleanup_for_exec`, `cleanup_cow_page_table`, `cleanup_cow_frames`, `GuardedStack::drop`-adjacent free helper); do not thread it through `frame_decref` itself |
| Pre-allocating the grave adds a heap allocation to every process creation | Low | It is one small `Box` alongside an already-allocated `ProcessPageTable`; same failure mode (`ENOMEM`), same code site |
| The x86_64 FD-closing behavior change (moving off the PM lock) regresses x86 | Medium | It is the shape aarch64 already runs and that the `handle_thread_exit` doc comment mandates; explicitly disclosed in commit 4; x86 boot + fork/exit tests are the gate |
| Treiber-stack ABA | Low | Nodes are moved by value and never re-pushed while owned by the reaper; a grave is pushed at most twice (once at C3, once on re-push after a blocked pass) and never concurrently |
| A permanently wedged CPU pins a grave forever | Low (by design) | Not fatal by construction (no cap, no panic); attributed by the stall watchdog with `root_live_mask`; optional SGI escalation |
| `kreclaimd` fails to start (kthread infra not ready) | Medium | `init_reclaim_thread()` returns `Result`; boot fails loudly at init rather than silently leaking. The `sys_fork_aarch64` opportunistic pass is a partial safety net |

