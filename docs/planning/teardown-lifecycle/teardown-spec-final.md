# AArch64 Process Teardown / Reclaim Lifecycle — Final Implementation Spec

Reconciliation of Design A (`teardown-design-A.md`) and Design B (`teardown-design-B.md`) against
`teardown-invariant-corpus.md` and the source tree at `/Users/wrb/fun/code/breenix` @ **`main` = 31126c2a**.
Every conflict is resolved by reading `main`, never by averaging. Line numbers below were re-derived from
`git show main:<path>` so an implementer can re-check each claim independently.

Supersedes branch `fix/teardown-followups` (`5781442b`, `867ce0c6`, `c0be17e7`, `28a7933e`, `5d7ab37c`),
which is based directly on `31126c2a` (verified: `git merge-base main fix/teardown-followups` = `31126c2a`).

**Design name:** *grave + lease + reclaimer*.

**Three sentences.** A dying process's heavy resources move, in one allocation-free `Option::take` under
the PM lock, into a **grave** that was allocated when the process was born, and the grave is published on
a lock-free intrusive stack that only a dedicated preemptible kernel thread (`kreclaimd`) drains. The two
TTBR0 software shadows become a *provable superset* of hardware occupancy by fixing the publication order
at its source — name the root before/through the switch, publish `saved` after the switch, clear `next`
last — including the two assembly sites that today clear `next` **before** `msr` (`syscall_entry.S:365`,
`:467`). A resource is freed only after a fenced retirement snapshot plus a full liveness proof (local
`TTBR0_EL1`, every online CPU's shadows, every scheduler thread's `cached_ttbr0`, and every live process
row that still names the root), which makes "Terminated therefore freeable" unrepresentable.

---

## 0. Source-verified baseline (what `main` actually does today)

| # | Fact | Location (main @31126c2a) |
|---|---|---|
| F1 | Deferred-fault drain runs on **both** exception-return tails | `context_switch.rs:3438` (`check_need_resched_and_switch_arm64`), `:4459` (`schedule_from_kernel`) |
| F2 | Idle redirect writes shadows but performs **no** hardware TTBR0 switch | `context_switch.rs:2967-2968`, `exception.rs:293-294` |
| F3 | `boot.S` ERET epilogues never touch TTBR0 (only boot-time construction / secondary bring-up) | no `ttbr0` reference in any `boot.S` return tail |
| F4 | Both assembly TTBR0 installs clear `next_cr3` **before** `msr ttbr0_el1` and never publish `saved` | `syscall_entry.S:365-369` (base `x0`, root `x1`), `:467-469` (base `x9`, root `x10`) |
| F5 | Assembly re-installs `saved_process_cr3` when `next==0`, and **skips entirely on `saved==0`** (`cbz`) | `syscall_entry.S:377-386`, `:478-487` |
| F6 | Syscall entry stamps `saved = mrs ttbr0_el1` | `syscall_entry.S:163-165` |
| F7 | `switch_ttbr0_if_needed` publishes `saved` **only** in the `current != next` branch, then clears `next` unconditionally | `context_switch.rs:4737-4771` |
| F8 | `is_ttbr0_root_live` reads software shadows only — never `TTBR0_EL1`, never `cached_ttbr0`, never live process rows | `ttbr0.rs:49-71` |
| F9 | `quiesce_ttbr0_for_exit()` unconditionally switches **and** zeroes both shadows regardless of which pid is exiting | `ttbr0.rs:39-46` |
| F10 | Fault handler marks the victim `Terminated` **while still executing on that thread's kernel stack**, and `set_idle_stack_for_eret` has already repointed the per-CPU stack words at the *idle* stack, so `is_kernel_stack_slot_live` returns **false** for the stack the CPU is standing on | `exception.rs:284-298`, `:309-318`; `kernel_stack.rs:283-296` (reads `live_stack_snapshot(cpu)`) |
| F11 | `manager()` masks **all** DAIF bits (`msr daifset, #0xf`) before taking the PM lock; `with_process_manager` uses `without_interrupts` | `process/mod.rs:125-140`, `:167-175` |
| F12 | The PM-locked, IRQ-off exit transaction **allocates and logs**: `log::info!`, `name.clone()`, `take_fd_entries()` → `fd_table.take_all() -> Vec`, `children.clone()`, `init.children.extend()`, `log::debug!`, plus `cleanup_windows_for_pid` | `manager.rs:1125`(log) `:1161-1176`(children) `:1186`(log) `:1154`(graphics); `process_task.rs:217,218,222,259,287`; `process.rs:335-337` → `fd.rs` `take_all` |
| F13 | `retirement_grace_elapsed` returns `true` with **zero atomic loads** when the target is all-zero (`target[cpu]==0 ||` short-circuit) | `scheduler.rs:563-568` |
| F14 | `reclaim_terminated_threads` reads stack liveness **before** the epoch, and allocates three `Vec`s (`terminated_ids`, `idle_ids`, `reclaimed_ids`) | `scheduler.rs:952-1015` |
| F15 | `FRAME_METADATA` is a **blocking** `spin::Mutex` reached from both CoW-fault context and teardown, and `frame_decref` calls `log::error!` / `log::trace!` **while holding it** | `frame_metadata.rs:79-113`; CoW callers `exception.rs:1976` (`handle_cow_fault_arm64`, `frame_decref` at `:2110`), `interrupts.rs` |
| F16 | An AArch64 **synchronous** exception enters with DAIF masked and `handle_sync_exception` never unmasks — the only `daifclr` in `exception.rs` is at `:1874`, inside `handle_irq`. So the CoW path spins on `FRAME_METADATA` with **IRQs masked** | `exception.rs:1874` (only unmask site), `:1976-2110` |
| F17 | CLONE_VM members are **separate `Process` rows** holding the owner's raw root in `inherited_cr3`; dispatch resolves `page_table … .or(process.inherited_cr3)` | `process.rs:161`; `clone.rs:209`; `context_switch.rs:4805-4808` |
| F18 | `exec` refuses while a live CLONE_VM sibling holds the old root — **`exit` has no such check and frees the root** | guard `manager.rs:46-76` + only call site `:3046`; unguarded free `manager.rs:1148-1150` |
| F19 | `PerCpuData` is 192 bytes; declared fields end at **144** (`eret_guard_source` at 136); `_pad3: [u8; 48]` spans 144..192. Assembly addresses at most offset 96 numerically, and names 120/128/136 symbolically via `.equ` | `per_cpu_aarch64.rs:20-104`; `syscall_entry.S:40-43` |
| F20 | The fatal postmortem is **already** sectioned with per-section claim wrappers, high-value-first, trace buffers last (section 7); `dump_stack_classification` **already** imports `ARM64_KERNEL_STACK_BASE/END`; `defer_current_user_thread_sigsegv_exit` **already** resolves the victim by stack slot | `exception.rs:370-421`, `:424-441`, `:329-336` |
| F21 | `kthread_park()` / `kthread_unpark()` exist and re-check `should_stop` after setting the parked flag | `kthread.rs:111-145` |
| F22 | The per-CPU deferred-fault-exit ring is an allocation-free CAS push, 8 CPUs x 16 slots, and `push` already returns `bool` | `process_task.rs:12-61` |
| F23 | `sys_exit_aarch64` already leaves the root (`switch_ttbr0_to_kernel` + zero both shadows) and already pivots to the neutral scheduler stack before publishing `Terminated` | `syscall_entry.rs:369-375`, `:395-397` |
| F24 | **There is no `impl Drop for ProcessPageTable`.** Dropping the `Box` frees nothing; frames are released only by explicit `cleanup_cow_page_table` / `cleanup_for_exec`. `main:manager.rs:1148` therefore *leaks* table frames after `terminate()` already decremented the user-frame CoW refs | `process_memory.rs` (no `Drop` impl); `manager.rs:1131-1150` |
| F25 | `GuardedStack::drop` is a TODO stub that calls `log::debug!`, and it is reached from `exit_process` (`stack.take()`) **under the PM lock with IRQs masked** | `stack.rs:264-267`; `manager.rs:1149` |
| F26 | `allocate_frame()` returns `Option` — exhaustion is `None`, not a panic | `frame_allocator.rs:298-330` |
| F27 | `ISR_WAKEUP_BUFFERS` is 8 buffers x 32 CAS slots and `push` can **fail** when full | `scheduler.rs:75-128` |
| F28 | Direct `Process::terminate()` callers outside `exit_process`: `signal/delivery.rs:224`, `:258`, `syscall/signal.rs:162`, `interrupts/context_switch.rs:1021` | as listed |
| F29 | `sys_fork_aarch64` calls `reclaim_deferred_process_resources()` then `reclaim_terminated_threads()` inline, IRQs on, no PM lock | `syscall_entry.rs:932-933` |

**Corrections the source forces on the two designs:**

- **A §18** claims "the highest per-CPU offset any assembly touches is 96". Numerically true for
  `[reg, #N]` addressing, but `syscall_entry.S:41-43` names 120/128/136 via `.equ`. A's *conclusion*
  (offset 144 is free padding, `PerCpuData` stays 192 bytes) still holds — F19.
- **B §2/§6.1** claims the assembly reorder "uses the same number of tail instructions". **False.**
  Offsets 64 and 80 are not `stp`-adjacent (72 is `kernel_ttbr0`), so publishing `saved` costs **one
  extra store per site** (2 sites, 2 stores). The reorder alone is *insufficient* — moving the clear
  after the `msr` without publishing `saved` leaves the freshly installed root named by nothing.
  State the +1 honestly in the commit message.
- **A §16 commit 1** ("reorder fatal postmortem and correct kernel-stack ranges") is **already landed on
  main** — F20. Do not redo it; only the new teardown-state section remains.
- **A §7.2** keeps a `reclaim_pass` in `sys_fork_aarch64`, which contradicts A §P0's "aarch64: exactly
  one mint site". This spec keeps the fork pass and states the honest count: **two** justified
  `ReclaimContext` mint sites on aarch64 (§5.1).
- **A §5.3** (`install(R)`) writes `occupied_root = R` before the `msr`; the equivalent two-word
  protocol must therefore also name `R` before the `msr`. This spec fixes A's and B's shared omission
  in §6.1 — see the `install_process_ttbr0` body.
- **A §15** "`5781442b` survives" — confirmed: `main:context_switch.rs:1663` still loads the raw
  encoded word into `owner_tid`.

---

## 1. Conflict resolutions

| # | A says | B says | Resolution | Source basis |
|---|---|---|---|---|
| R1 | Add a third per-CPU word `occupied_root` (offset 144); **zero assembly diff** | Fix the assembly to publish `saved` after `msr` | **B.** Fix the violating instruction; no third word. | Invariant 6 *is* "shadows are republished only after the hardware switch"; F4 is the instruction that violates it. Once F4 is fixed, `{saved, next}` is already a proven superset (§6.3), so the third word is redundant state that every future TTBR0 writer must maintain — A's own generator **G1**. `syscall_entry.S` is not a frozen region and the change touches no guard, no ERET/ISB ordering, no banner. See **OQ-A**. |
| R2 | `quiesce_ttbr0_for_exit(pid)` → `leave_process_ttbr0()`, **no arguments** | CP1 takes an `expected_root` and refuses to clobber on mismatch | **A.** No-argument primitive. | Every caller is a "this CPU is going away from user space" path (four fault arms, `sys_exit`, both idle redirects). "I am no longer using whatever root I have" is exactly the operation. An argument reintroduces the misapplication surface (F9 / R1-6) **and** a refusal path that can leave a retired root installed in `TTBR0_EL1` — strictly worse than an unconditional local leave. |
| R3 | Fault handler keeps `terminate_current_scheduler_thread()`; safety comes from the C5 predicate | New `ThreadState::ExitPending`; `Terminated` only off-stack | **B.** Add `ExitPending`. | **F10 is decisive**: `set_idle_stack_for_eret` repoints the per-CPU stack words at the idle stack *before* the mark, so `is_kernel_stack_slot_live(dying_top)` is **false** while the CPU is still standing on that stack. A's predicate therefore does not protect the fault path at all — only the epoch grace does, and grace alone was never designed to be the sole barrier. Invariant 10 demands the state distinction. |
| R4 | `Arc<AddressSpace>` identity refactor is out of scope | Replace `inherited_cr3` with `AddressSpaceRef(owner_pid, generation)` | **Split: adopt B's proof obligation, defer B's type.** | F17/F18: the hole is real and reachable in-tree (`userspace/programs/src/clonevm_exec_test.rs` exists). But a sibling only needs the **raw root value**, which stays valid as long as the frames are pinned — and the grave pins them. So the minimum structural fix is a reclaim-predicate clause ("no live process row names this root"), generalizing the function that already exists at `manager.rs:46`. A's silent hole is closed; B's refactor is deferred. See **OQ-C**. |
| R5 | Keep `take_fd_entries() -> Vec` (moved into the receipt) | `take_next_for_exit()`, one FD at a time, allocation-free | **B.** | F11 + F12: the PM lock is held with **all** interrupts masked and `take_all` allocates. That is precisely the documented heap-lock-under-IRQ-off deadlock (`syscall_entry.rs:900-906`). Invariant 3 is satisfiable only if the transaction is allocation-free. A's own P2 claims an allocation-free commit but its C2 still calls `take_fd_entries` — an internal contradiction the source settles. |
| R6 | Keep `children`, take it *inside* the one-shot | Delete the `children` mirror; `parent` is authoritative | **B.** | Same basis: `children.clone()` (`manager.rs:1163`) and `init.children.extend()` (`manager.rs:1173`, `process_task.rs:259`) allocate under PM + IRQs off. Deleting the mirror also removes the R2-5 take-before-guard bug class outright rather than re-ordering around it. ~22 call sites; `processes` is a `BTreeMap`, so scanning by `parent` is cheap. See **OQ-B**. |
| R7 | `ReclaimContext` ZST capability gating all teardown | No token; source-structure tests instead | **A — plus B's structure tests.** | Not averaging: they are additive and cover different failure modes. R1-1..R1-4 and R2-4 each required a human to trace a call graph; the token turns that into a compile error. B's structure tests cover what a type cannot express (assembly ordering, frozen-region hashes, counter readers). Confine the token to four entry points (A §18's own risk mitigation). |
| R8 | `ExitReceipt` (`#[must_use]`, never `Option`) | Typed outcome `Committed / AlreadyCommitted / Missing` | **Merge — they are orthogonal, not competing.** `retire_process` always returns `#[must_use] ExitReceipt`; the receipt *carries* an `ExitOutcome` for diagnostics only. **No caller ever branches on the outcome for control flow.** | R2-11 (an `Option` return silently disabled the idle redirect) and R2-6 (an early return skipped the waitpid wake) are both "a mandatory tail got skipped". A non-optional must-use receipt makes that unrepresentable; the typed outcome keeps the diagnostic honest. |
| R9 | `frame_decref` needs only a doc comment once teardown is reaper-only | Preemption-pinned normal guard + `try_lock` fault transaction | **B**, with the load-bearing element named. | **F15 + F16 are decisive.** The reaper is *preemptible*; a preempted reaper holding `FRAME_METADATA` is waited on by a CoW fault handler that runs with **IRQs masked** (F16) — on a single-CPU ARM64 system that is a permanent deadlock, exactly the class A's design cites elsewhere. **Load-bearing fix: the preemption-pinned guard** (the holder can never be descheduled, so no same-CPU EL0 CoW fault can find the lock held). The fault-side `try_lock` + retry-without-mutation is cheap SMP hardening (an unchanged PTE re-faults correctly) and is adopted, but is not what closes the hole. Independently required: **delete the two in-lock `log::` calls** (`frame_metadata.rs:90`, `:110`) — they are reachable from an IRQ-masked fault handler and take the SERIAL lock (invariant 2). |
| R10 | Keep the 16-slot/CPU ring + a drop counter | Delete the ring; one fault-intent slot per CPU + dispatch quarantine | **A.** Keep the ring, add the counter. | The ring is per-CPU, CAS-based, allocation-free, and already returns `bool` (F22); overflow needs >=16 unserviced EL1-fault kills on one CPU, which is fatal-class already. B's quarantine adds a gate inside `dispatch_thread_locked`, whose body contains the frozen "DO NOT ADD A CPU0-SPECIFIC EL0 DISPATCH GUARD" region (`context_switch.rs:3261`) — invariant-5 risk for a strictly worse cost/benefit. See **OQ-D**. |
| R11 | `saved_process_cr3` must **never** be 0 (A §5.5) | (silent) | **Reject A's rule.** After a real hardware leave, `saved = 0` is correct. | A's justification is R1-6 (an innocent thread ERETs to EL0 with the kernel root). R2 removes that precondition: the leave is local-only and the dispatcher always arms `next` for a user thread (CP0). Meanwhile `saved = kernel_ttbr0` would make `.Lrestore_saved_ttbr` (F5) stop skipping via `cbz` and instead execute `msr + tlbi vmalle1is + 2x dsb` **on every idle return** — a new broadcast TLBI on a return tail, violating invariant 19 to fix a precondition that no longer exists. |
| R12 | Restore aarch64 `cleanup_for_exec` counters + `log::info!` | Same conclusion (teardown logging is worker-only) | **Agreed.** Keep both arches identical. | R2-14 / invariant 26. On main both copies still log; the branch removed only the aarch64 copy. Logging is legal after this design because the only caller is `kreclaimd`, gated by `&ReclaimContext`. |
| R13 | Broadcast TLBI moves to the reaper, once per batch, immediately before freeing | Same (CP5) | **Agreed.** `leave_process_ttbr0()` performs **no** TLBI. | Breenix has no per-mm ASIDs, so a stale user translation survives a leave — harmless while the grave pins the frames, fatal only at reuse, which is exactly where the reaper's `tlbi vmalle1is` sits. Net effect vs. main: **four** broadcast TLBIs removed from the fault arms. |
| R14 | Park/timed-block with a `None`-safe fallback, one blocking call per pass | Untimed `BlockedOnIO` + a **reserved** ISR wake slot + yield/WFI backoff | **A's shape, B's backoff; reject B's reserved slot.** | `kthread_park` already re-checks after setting the flag (F21). `ISR_WAKEUP_BUFFERS.push` can fail when all 32 slots are full (F27), so a wake built on it is *not* loss-free without new reservation machinery — added complexity for a wake that an atomic generation counter + `kthread_unpark` already makes loss-free. B's yield/WFI fallback is adopted verbatim for the "block reported no effect" path. |
| R15 | Keep both inline reclaim calls in `sys_fork_aarch64` | Delete both | **Split.** Keep **one** `reclaim_pass` (not a loop) at `syscall_entry.rs:932`; **delete** `reclaim_terminated_threads()` at `:933`. | F29: the site is IRQs-on, no PM lock — a legal reclaim context, so keeping the address-space pass costs no invariant and bounds worst-case memory growth. The *kernel-stack* reclaim at `:933` is the exact hazard the r10 reconciliation named (pool free + 64 KiB zero-fill re-handout racing a still-on-CPU dying thread) and is CP9's job. F26 (`allocate_frame` returns `None`, never panics) means the fork pass is an optimization, not a correctness dependency. |

---

## 2. Lifecycle state machines

### 2.1 Process teardown lifecycle

```
             exit(2) / exit_group / fatal fault / SIGKILL / default fatal signal
   LIVE ───────────────────────────────────────────────────────────▶ EXIT_COMMITTED
                                                                          │ CP2 (one statement:
                                                                          │  grave.take() + moves)
                                                                          ▼
                                                                     GRAVE_QUEUED
                                                                          │ CP4 (proof) → CP5 (free)
                                                                          ▼
                                                                      RECLAIMED
   EXIT_COMMITTED / GRAVE_QUEUED / RECLAIMED ── waitpid ──▶ REAPED   (independent axis, CP6)
   REAPED ∧ RECLAIMED ─────────────────────────────────────────────▶ row removed (CP7)
```

- `LIVE → EXIT_COMMITTED` is the **only** one-shot mutation, and the one-shot test *is* the resource
  move: `self.grave.take()`. A second exit observes `None` and cannot double-free because there is
  nothing left to free (kills R1-5 / R2-5 / invariant 13). This is Design A's generator-**G2** kill and
  it is adopted intact.
- `GRAVE_QUEUED → RECLAIMED` is **not** a state any exit caller can set. It is a permit the reaper earns
  by discharging §7's proof obligations — Design B's "`ResourcesClaimable` is not storable" rule.
- Row removal requires **both** `REAPED` and `RECLAIMED`, so `waitpid` never triggers a destructor. This
  is what closes the CLONE_VM owner-exit UAF (F18).

### 2.2 Scheduler-thread / kernel-stack lifecycle

```
   RUNNING/READY/BLOCKED ──(fault, remote kill, exit_group)──▶ EXIT_PENDING
        │                                                           │
        │ (syscall exit: pivot to the neutral stack first)          │ off-stack proof
        ▼                                                           ▼
   ══════════════ TERMINATED  (retirement fence stamped at this edge) ══════════════
                                     │ ¬is_kernel_stack_slot_live ∧ fence elapsed
                                     ▼
                        STACK_CLAIMED (worker owns the detached Thread) ──▶ dropped
```

- `EXIT_PENDING` is **non-runnable and non-reclaimable**. No fault handler may set `TERMINATED`:
  `terminate_current_scheduler_thread` is deleted (F10).
- Two legal `→ TERMINATED` edges: (a) `exit_schedule_trampoline`, already on the neutral per-CPU
  scheduler stack (correct on main — F23); (b) `kreclaimd`, once the thread's fence has elapsed and the
  ordered live-stack snapshot excludes its slot.
- Only the scheduler owns kernel stacks; only `kreclaimd` drops them, outside the scheduler lock.

### 2.3 Per-CPU TTBR0 lease state

```
   OCCUPIED(R) ──leave_process_ttbr0()──▶ KERNEL ──arm/install_process_ttbr0(R')──▶ OCCUPIED(R')
        └──────────────── arm/install_process_ttbr0(R') (direct handover) ─────────────────┘
```

`KERNEL` ⇔ hardware `TTBR0_EL1 == kernel_ttbr0` ∧ `saved == 0` ∧ `next == 0`.
**Every edge is executed only by the CPU it describes.** There is no cross-CPU TTBR0 mutation anywhere
in this design, which is why no ownership predicate is needed at any call site (invariant 14).

### 2.4 Address space

An address space is retired when its owning row commits CP2, or when `exec` supersedes a table. It is
*reclaimable* only when no CPU lease, no thread dispatch lease, and no **live process row** names its
root. `inherited_cr3` (F17) stays a raw `u64`; that stays sound because the grave pins the frames until
the last live sharer is gone (§7.2 clause 4, **OQ-C**).

---

## 3. Resource ownership table

| Resource | LIVE | EXIT_COMMITTED (post-CP2) | GRAVE_QUEUED | Released by / at |
|---|---|---|---|---|
| page-table root `Box<ProcessPageTable>` | `Process.page_table` | `ProcessGrave.page_table` | graveyard | `kreclaimd` CP5 |
| old exec tables | `Process.pending_old_page_tables` | `ProcessGrave.old_page_tables` (buffer **swapped**, never extended) | graveyard | `kreclaimd` CP5 |
| user stack `GuardedStack` | `Process.stack` | `ProcessGrave.stack` | graveyard | dropped by `kreclaimd` CP5, **before** its page table |
| CoW refcounts of user frames | page table | grave | grave | `kreclaimd` CP5 (`cleanup_cow_page_table`) |
| FD entries | `Process.fd_table` | `Process.fd_table` (row still owns; work bit set) | — | `kreclaimd`, one entry at a time, **outside** PM |
| parent relation | child's `Process.parent` (sole source of truth) | same | same | row removal CP7 |
| child enumeration | derived by scanning rows for `parent == pid` | same | same | — (no mirror exists) |
| reparent-to-init obligation | — | durable work bit | work bit | `kreclaimd`, one child per short PM transaction |
| SIGCHLD + parent wake obligation | — | pending bit + durable wake bit | same | `kreclaimd`, scheduler call outside PM |
| window buffers | graphics registry | durable work bit | work bit | `kreclaimd`, outside PM |
| kernel stack | `Thread.kernel_stack` | `Thread` (EXIT_PENDING → TERMINATED) | scheduler + fence | `kreclaimd` after detach, outside the scheduler lock |
| `Thread` object | `Scheduler.threads` | same | same | dropped by `kreclaimd` (CP9) |
| hardware TTBR0 lease | local CPU | cleared by that CPU's `leave_process_ttbr0()` | must be absent | — |
| `saved_process_cr3` / `next_cr3` | per-CPU lease | conservative until the hardware switch completes | must not match | — |
| `Thread.cached_ttbr0` | dispatch lease (`context_switch.rs:2258`) | cleared at the off-stack TERMINATED edge (CP8) | must not match | — |
| `inherited_cr3` of a live sibling | that sibling's row | still live | **blocks CP4** | that sibling's own CP2 |
| the grave `Box` | `Process.grave` (allocated at birth) | moved out at CP2 | graveyard | dropped at CP5 |
| `Process` row (zombie) | PM map | PM map | PM map | CP7 (`REAPED ∧ RECLAIMED`) |

**No row lists two owners for the same resource at the same stage.** That is exactly the property `main`
violates (`manager.rs:1148-1150` frees a root a live CLONE_VM sibling still names — F18) and that both
prior branch attempts reproduced in different `#[cfg]` arms of one function.

Because there is **no `Drop for ProcessPageTable`** (F24), "the grave owns it" is not a formality:
nothing is freed until `kreclaimd` explicitly calls the cleanup routines behind the proof.

---

## 4. Commit points

| # | Transition | Exact commit | Context | Idempotence |
|---|---|---|---|---|
| **CP0** | dispatch lease acquired | `arm_process_ttbr0(R)` writes `next = R`, after the dispatcher resolved the row and confirmed it is not `EXIT_COMMITTED` | scheduler lock; no PM alloc | refusal ⇒ the thread becomes `EXIT_PENDING` and is never sent to EL0 |
| **CP1** | local TTBR0 detach | `leave_process_ttbr0()` — see §6.1 | any local context; **no TLBI, no lock, no alloc, no output** | idempotent fast path (`hw==kernel ∧ saved==0 ∧ next==0` ⇒ return) |
| **CP2** | `LIVE → EXIT_COMMITTED` | `ProcessManager::retire_process(pid, code) -> ExitReceipt`; inside it `Process::commit_grave()` = `self.grave.take()?` then `page_table.take()`, `mem::swap(old_tables)`, `stack.take()`, fence capture | PM lock, IRQs off. **Allocation-free, output-free, no second lock, no TTBR0 op, no frame op, no FD close, no scheduler call** | `Option::take` — the guard *is* the action |
| **CP3** | grave published | `GRAVEYARD.compare_exchange(head, grave, Release, Relaxed)` (Treiber stack, intrusive `next`) | same critical section as CP2 | moved by value; pushed once |
| **CP4** | reclaim permit earned | `RetirementSnapshot::acquire(&fence)?` then the full liveness proof of §7.2 | `kreclaimd` only; requires `&ReclaimContext` | the batch is taken by one `swap(null, Acquire)` |
| **CP5** | physical release | per batch: `tlbi vmalle1is; dsb ish; isb` **once**; then per grave: drop `GuardedStack`, `cleanup_cow_page_table`, `cleanup_for_exec` on old tables, free the root frame | `kreclaimd`, IRQs on, no PM/scheduler lock | grave owned by the reaper's local variable |
| **CP6** | logical reap | `waitpid` consumes status and sets `REAPED` | syscall context | status consumed once |
| **CP7** | row removal | `processes.remove(&pid)` only when `REAPED ∧ RECLAIMED` ∧ no live row names its root | `kreclaimd`, short PM transaction | map removal |
| **CP8** | thread off-stack | `EXIT_PENDING → TERMINATED`, `cached_ttbr0 = 0`, fence stamped | `exit_schedule_trampoline` (neutral stack) or `kreclaimd` | state test |
| **CP9** | kernel stack freed | scheduler detaches the `Thread` by non-allocating swap; the reaper drops it **after** releasing the lock | `kreclaimd` | `¬is_kernel_stack_slot_live ∧ fence elapsed ∧ state == TERMINATED` |

**Ordering between commit points is deliberately minimal.** CP2 and CP8 are independent; neither orders
the other. Safety comes from the CP4/CP9 *predicates*, not from sequencing — the prior attempts bought
safety with ordering ("quiesce before publishing"), which every future call site must then remember to
preserve. The one real ordering requirement lives **inside** CP4: the fenced epoch observation happens
before any liveness read, and that is enforced by the `RetirementSnapshot` capability type (§7.1).

---

## 5. Reclaim execution context

### 5.1 `ReclaimContext` (compile-time enforcement of invariants 1, 2, 4, 20)

New module `kernel/src/task/reclaim.rs`:

```
pub struct ReclaimContext(());              // ZST, no public constructor
impl ReclaimContext {
    pub(crate) fn assert_preemptible() -> Self   // debug_assert!: IRQs enabled,
                                                 // no PREEMPT_ACTIVE, PM-lock depth == 0
}
```

`&ReclaimContext` is required by exactly four entry points — `ProcessPageTable::cleanup_for_exec`,
`cleanup_cow_page_table`, `Process::cleanup_cow_frames`, and the grave's stack-release helper. It is
deliberately **not** threaded through `frame_decref` / `deallocate_frame` (A §18's own token-sprawl risk).

Mint sites, exhaustively and honestly:

- **aarch64: two.** (1) the `kreclaimd` loop body; (2) the single `reclaim_pass` in `sys_fork_aarch64`
  (`syscall_entry.rs:932`), which is IRQs-on and PM-lock-free (F29, R15).
- **x86_64: one** — `arch_retire_address_space()`, called from `ExitReceipt::complete()` (no PM lock,
  preemptible), which frees inline exactly as x86 does today.

The property being bought is not "exactly one site" but "**every** mint is explicit, rare, and visible in
the diff". Re-adding teardown to `check_need_resched_and_switch_arm64` cannot compile without writing
`ReclaimContext::assert_preemptible()` on an exception-return tail — a diff that reviews itself.

### 5.2 `kreclaimd`

Started from `main_aarch64.rs` after workqueue/softirq init (`main_aarch64.rs:804`, `:810`) via
`kthread_run(..., "kreclaimd")`. `init_reclaim_thread() -> Result<_, KthreadError>`; boot fails loudly if
it cannot start.

**One action per pass**, in priority order:

1. drain the per-CPU fault-exit rings and run the common exit for each victim tid;
2. finalize one `EXIT_PENDING` thread whose fence has elapsed (CP8);
3. reparent one orphaned child (short PM transaction);
4. discharge one parent wake / SIGCHLD / graphics-cleanup work bit (outside PM);
5. close one FD (claim under PM, close outside);
6. run one `reclaim_pass` batch (CP4/CP5);
7. detach and drop one reclaimable `Thread` + kernel stack (CP9);
8. remove one `REAPED ∧ RECLAIMED` row (CP7).

PM and scheduler locks are **never held together**; frame-metadata and allocator locks are never taken
while either is held; no heavy destructor runs under any lock.

### 5.3 Sleep / wake (kills R2-10)

- **At most one** blocking call per pass ⇒ at most one `timer_heap` entry per pass (main pushes one per
  iteration with no dedup — `scheduler.rs:2165`).
- Backlog empty ⇒ `kthread_park()` (indefinite; F21 is lost-wake-safe).
- Backlog non-empty but blocked on liveness ⇒ one `block_current_for_timer(now + 10 ms)`.
- If `with_scheduler` returns `None`, or the block reports it did not take effect, or
  `schedule_from_kernel` early-returns ⇒ explicit yield, then `wfi` with IRQs enabled, then retry with
  exponential backoff. **No path re-enters the work loop without an intervening block, park, or WFI.**
- Wake: `ExitReceipt::complete()` bumps an atomic `RECLAIM_WORK_GEN` and calls `kthread_unpark` — phase
  2, no locks held. The park loop re-checks the generation, so a wake cannot be lost. **No reserved
  `ISR_WAKEUP_BUFFERS` slot** (F27, R14).

---

## 6. TTBR0 lease protocol

### 6.1 The three Rust helpers — the only Rust TTBR0 writers

```
leave_process_ttbr0()               // no arguments; local CPU only
    if read_ttbr0_el1()==kernel && saved==0 && next==0 { return }   // idempotent
    dsb ishst ; msr ttbr0_el1, kernel ; isb                          // NO tlbi (R13)
    saved = 0 ; next = 0                                             // AFTER the switch

install_process_ttbr0(R)            // Rust performs the switch (dispatch + exec paths)
    next = R                                                         // name R BEFORE the switch
    dsb ishst ; msr ttbr0_el1, R ; isb
    saved = R                                                        // publish installed root
    next  = 0                                                        // clear the pending lease last
    tlbi vmalle1is ; dsb ish ; isb                                   // unchanged behaviour

arm_process_ttbr0(R)                // assembly will perform the switch
    next = R                                                         // saved keeps naming the old root
```

The `next = R` **before** the `msr` in `install_process_ttbr0` is load-bearing and is missing from both
input designs' two-word formulations: without it, the exec-path install (`syscall_entry.rs:1279`, where
no dispatcher armed `next`) leaves the freshly installed root named by nothing between the `msr` and the
`saved = R` store. It costs one store and closes the last under-report window.

`quiesce_ttbr0_for_exit`, `current_cpu_retains_ttbr0_root` and `switch_ttbr0_to_kernel` are **deleted**.
A source-structure test asserts these three helpers are the only Rust writers of `ttbr0_el1` outside
boot/paging/SMP bring-up.

### 6.2 The two assembly sites (the only assembly TTBR0 installs)

At `syscall_entry.S:365-369` (base `x0`, root `x1`) and `:467-469` (base `x9`, root `x10`), move the
`next` clear to **after** the hardware switch and add the `saved` publication (F4 ⇒ invariant 6). Using
the second site's registers as the example:

```
  (delete)   str  xzr, [x9, #64]        /* was: clear next BEFORE the switch */
             dsb  ishst
             msr  ttbr0_el1, x10
             isb
  (add)      str  x10, [x9, #80]        /* publish saved = installed root */
  (add)      str  xzr, [x9, #64]        /* then clear the pending lease    */
             tlbi vmalle1is
             dsb  ish
             isb
```

Notes an implementer must not get wrong:

- The two new stores go **after the `isb`** but **before the `tlbi`**. Per-CPU data is a TTBR1 (kernel)
  VA, so it is unaffected by the TTBR0 switch; placing the stores before the broadcast invalidate also
  keeps their translations warm. The original "clear before switching (avoid accessing after switch)"
  comment is superseded and must be replaced, not just deleted.
- Net cost is **+1 store per site** (2 sites). `stp` is not usable: offsets 64 and 80 are not adjacent
  (72 is `kernel_ttbr0`). Do **not** claim instruction parity in the commit message.
- Nothing else in either epilogue changes: no guard, no ERET/ISB ordering, no banner, no `boot.S` edit
  (F3 means `boot.S` needs none). `.Lrestore_saved_ttbr` (F5) is untouched — it reinstalls the value
  `saved` already names.

### 6.3 Superset proof (no instant under-reports)

Let *O* be the old root and *R* the new one. The record is `{saved, next}` over online CPUs, plus an
exact local `mrs ttbr0_el1` for the reaper's own CPU.

| Transition | Instant | HW | {saved, next} | Covered? |
|---|---|---|---|---|
| `arm(R)` | after `next = R` | O | {O, R} | O ✓ (saved), R ✓ (next) |
| asm install | after `msr`, before `saved = R` | R | {O, R} | R ✓ (next); O over-reported (safe) |
| | after `saved = R`, before `next = 0` | R | {R, R} | R ✓ |
| | after `next = 0` | R | {R, 0} | R ✓ |
| `install(R)` (Rust) | after `next = R`, before `msr` | O | {O, R} | O ✓, R ✓ |
| | after `msr`, before `saved = R` | R | {O, R} | R ✓ (next) |
| | after both stores | R | {R, 0} | R ✓ |
| `leave` | after `msr`, before stores | kernel | {O, 0} | over-reports O (safe) |
| | after both stores | kernel | {0, 0} | nothing occupied ✓ |

**No row under-reports.** Every over-report is transient and self-clearing at that CPU's next
arm/install/leave. The load-bearing addition versus main is that **both idle-redirect sites now perform
a real hardware leave** (F2) — that is what closes the "blocked sweeps climb forever" mode (R2-1/R2-2)
and, per the r10 reconciliation, the abort-#1 state (EL1 executing under a dead user root).

`install_process_ttbr0` keeps its broadcast TLBI (unchanged); `leave` has none (R13).

---

## 7. Reclaim proof obligations (CP4)

### 7.1 Fenced epoch snapshot

`RetirementFence { epochs: [u64; MAX_CPUS], online_mask: u32 }` replaces the bare `[u64; MAX_CPUS]`.

- `RetirementFence::capture()` records the online mask. A fence with `online_mask == 0` is **invalid**
  and never elapses — this kills F13, where an all-zero target returns `true` after **zero** atomic loads
  and therefore with no barrier at all.
- `RetirementSnapshot::acquire(&fence) -> Option<RetirementSnapshot>` performs the `Acquire` epoch loads
  and then an **unconditional** `core::sync::atomic::fence(Acquire)` on every path, including the
  short-circuit path.
- Every liveness read below takes `&RetirementSnapshot`, so the ordering dependency is expressed in the
  **type** rather than in statement order or a comment (invariant 12). This carries `867ce0c6`'s reorder
  forward as a structural guarantee instead of a fragile one.
- `debug_assert!(is_cpu_online(0))` inside `capture()` records the boot invariant at the point where it
  is actually depended upon.

### 7.2 Liveness proof for a root R (all five clauses must hold)

1. `read_ttbr0_el1() & MASK != R` on the reaper's own CPU — exact and authoritative (invariant 7);
2. for every **online** CPU: `saved & MASK != R` ∧ `next & MASK != R` (proven superset, §6.3);
3. for every scheduler `Thread`: `cached_ttbr0 & MASK != R` (`context_switch.rs:2258`) — covers a
   *descheduled* thread that would be re-dispatched onto R;
4. **no live (`!EXIT_COMMITTED`) process row names R** via `page_table.level_4_frame()` or
   `inherited_cr3` — closes F18, the CLONE_VM owner-exit hole that Design A leaves open;
5. the fence has elapsed under §7.1.

Clause 4 is `manager.rs:46`'s `find_live_clone_vm_sibling_holding_cr3` generalized to
`root_has_live_sharer(root)` and given a second call site. Today it guards only `exec` (`manager.rs:3046`).

A CPU cannot read another CPU's `TTBR0_EL1` architecturally. **Say exactly that in the doc comment**,
with the §6.3 superset as the justification, so the next agent does not "fix" it back into a false claim
(invariant 28 — this is the failure mode of the two comments the branch added).

### 7.3 Kernel-stack proof (CP9)

`¬is_kernel_stack_slot_live(top)` (`kernel_stack.rs:283`, unchanged) **∧** the thread's embedded fence
elapsed under §7.1 **∧** state is `TERMINATED` (never `EXIT_PENDING`). No `cpu_state[].current/previous/
idle` name-match participates (invariant 11). Keep the existing allocator-side debug assertion at
`kernel_stack.rs:338` and the full 64 KiB scrub.

### 7.4 Stall policy (no cap, no panic)

There is no bounded reclaim collection, therefore no overflow policy and no panic under a lock
(invariants 21, 22). A wedged CPU pins **only its own** graves; the reaper keeps servicing unrelated ones
(no head-of-line blocking) and emits **one** rate-limited `log::warn!` per stalled grave past
`RECLAIM_STALL_WARN_NS`, naming pid, root, blocker mask, and age — from worker context only, never from
a producer, a PM critical section, a fault handler, the idle loop, or a return path. Allocator pressure
surfaces as the existing `None`/`ENOMEM` (F26), never as a teardown panic.

---

## 8. Exact file:function change list

### New

**`kernel/src/task/reclaim.rs`**
- `ReclaimContext` + `assert_preemptible()`
- `ProcessGrave { pid, exit_code, page_table, old_page_tables, stack, fence, queued_at_ns, warned, next: *mut ProcessGrave }`
- `static GRAVEYARD: AtomicPtr<ProcessGrave>`; `push_grave` (Treiber, Release CAS); `take_all_graves` (`swap(null, Acquire)`)
- `reclaim_pass(&ReclaimContext) -> ReclaimStats`; `kreclaimd_main()`; `init_reclaim_thread()`; `kreclaim_wake()`
- counters + `dump_reclaim_state()` (raw-UART, lock-free)
- `#[cfg(not(target_arch = "aarch64"))] arch_retire_address_space(Box<ProcessGrave>)` — inline free (x86 parity)

### Modified

**`kernel/src/arch_impl/aarch64/ttbr0.rs`**
- add private `read_ttbr0_el1()` — carries `5d7ab37c`'s helper forward
- **delete** `switch_ttbr0_to_kernel`, `quiesce_ttbr0_for_exit`, `current_cpu_retains_ttbr0_root`
- add `leave_process_ttbr0()`, `install_process_ttbr0(R)`, `arm_process_ttbr0(R)`,
  `invalidate_user_tlb_broadcast()` — bodies exactly as §6.1
- replace `is_ttbr0_root_live` with `root_liveness(&RetirementSnapshot, R) -> RootLiveness`
  (bool + blocker masks for diagnostics), implementing §7.2 clauses 1–3

**`kernel/src/arch_impl/aarch64/mod.rs`** — export only the constrained API; no arbitrary shadow clearing.

**`kernel/src/arch_impl/aarch64/context_switch.rs`**
- `:1663 dump_all_eret_frame_anomaly_snapshots` → `decode_last_dispatched(...)` — **adopt `5781442b` verbatim**
- `:3438` and `:4459` → **delete** both `drain_deferred_fault_sigsegv_exits()` calls; add nothing
- `:2967-2968 setup_idle_return_locked` → `leave_process_ttbr0()` replacing the two shadow writes
- `:4737 switch_ttbr0_if_needed` → delegate to `install_process_ttbr0` (which publishes `saved` in the
  already-equal case too — fixes F7)
- `:4842 set_next_cr3(tagged_ttbr0)` → `arm_process_ttbr0(tagged_ttbr0)`
- `set_next_ttbr0_for_thread` / `:4805-4808` → refuse `EXIT_COMMITTED` rows and `EXIT_PENDING` threads (CP0)
- *unchanged:* `idle_loop_arm64`, `aarch64_enter_exception_frame`, the EL0 dispatch site (`:3261`),
  `schedule_terminated_from_exit`, `exit_schedule_trampoline` (except the CP8 stamp),
  **and `dispatch_thread_locked` — deliberately not modified (R10)**

**`kernel/src/arch_impl/aarch64/exception.rs`**
- `:284 set_idle_stack_for_eret` → `leave_process_ttbr0()` replacing the two shadow writes
- `:309 terminate_current_scheduler_thread` → **delete**; replaced by `request_exit_pending(tid)`
- new `fn kill_current_user_process_and_redirect(frame, page_table_phys)` — **one** body for all four
  `from_el0` terminating arms (today `pm.exit_process` at `:770`, `:1132`, `:1211`, `:1308`):
  CP1 → `retire_process` → drop the guard → `receipt.complete()` → `request_exit_pending` → frame
  redirect → idle. **The redirect is unconditional**; no `terminated` / `already_terminated` booleans and
  no branch on the outcome (kills R2-11)
- `:320 defer_current_user_thread_sigsegv_exit` → `leave_process_ttbr0()`; **keep** the existing
  stack-slot victim resolution (already correct on main — F20); count ring-push failures into
  `FAULT_EXIT_INTENT_DROPPED` and emit a raw-UART marker
- `:370 dump_fatal_postmortem_once` → add `dump_reclaim_state()` as a new section **before** the
  trace-buffer section, using the existing `dump_fatal_postmortem_section` wrapper.
  **Do not redo A's commit 1** — the sectioning and ordering already exist (F20)
- `:424 dump_stack_classification` → add slot index + `stamp_last_dispatched_tid` owner. The range
  constants are already imported correctly (F20); do not re-hardcode
- `:1976 handle_cow_fault_arm64` / `:2110` → `try_lock` frame-metadata transaction; on contention return
  without any PTE mutation so the instruction re-faults (R9)

**`kernel/src/arch_impl/aarch64/syscall_entry.S`** — exactly the four lines of §6.2 at `:365-369` and
`:467-469`, plus the replaced comment. Nothing else in the file.

**`kernel/src/arch_impl/aarch64/syscall_entry.rs`**
- `:369-375 sys_exit_aarch64` → `leave_process_ttbr0()` replaces `switch_ttbr0_to_kernel()` + the two
  zero stores; split `Exit` vs `ExitGroup` dispatcher arms; **keep** the `:397` pivot (CP8 is already
  correct — F23)
- `:932` → one `reclaim_pass` (not a loop); `:933` → **delete** `reclaim_terminated_threads()` (R15)
- `:1279` exec install → `install_process_ttbr0(new_ttbr0)`; exec links a **preallocated** retired-table
  node instead of `Vec::push` / `drain_old_page_tables`

**`kernel/src/process/process.rs`**
- add `grave: Option<Box<ProcessGrave>>`, `exit_stage`, `exit_work_bits`, `live_thread_count`
- **delete** `children` (`:114`, `:522`, `:528`) — `parent` becomes the sole relation
- **delete** `terminate()` (`:284`) and both `close_all_fds()` copies
- `terminate_minimal` (`:320`) → `mark_exit_committed(code) -> ExitOutcome`
- add `commit_grave(&mut self) -> Option<Box<ProcessGrave>>` (CP2: `take` + `take` + `mem::swap` + `take`)
- `cleanup_cow_frames(&mut self, &ReclaimContext)`

**`kernel/src/process/manager.rs`**
- `:1120 exit_process` → `#[must_use] retire_process(pid, code) -> ExitReceipt` (CP2 body:
  allocation-free, output-free, no TTBR0 op, no FD close, no frame op, no graphics call). This removes
  the `log::info!` at `:1125`, the `log::debug!` at `:1186`, the `cleanup_windows_for_pid` at `:1154`
  and the `stack.take()` at `:1149` (whose `Drop` logs — F25) from the IRQ-off critical section
- `:1161-1176` reparent block → replaced by a durable work bit consumed by the reaper
- `:46 find_live_clone_vm_sibling_holding_cr3` → generalized to `root_has_live_sharer(root)`, reused by
  §7.2 clause 4 **and** by the existing exec guard at `:3046`
- `remove_process` → `mark_reaped`; physical removal gated on `REAPED ∧ RECLAIMED` (CP7)
- add allocation-free worker claim/revalidate helpers (one FD, one child, one retired node, one row)

**`kernel/src/process/mod.rs`**
- add `#[must_use] pub struct ExitReceipt` (carries `ExitOutcome` + pid + parent tid + work bits) and
  `pub fn complete(self)` — FD closes, graphics cleanup, waitpid/pause wakeups, `kreclaim_wake()`, logging
- **delete** `exit_current()` (`:258`, zero callers, `#[allow(dead_code)]` — R2-13; deletion, not refactor,
  per the project's dead-code policy)

**`kernel/src/task/process_task.rs`**
- **delete** `PendingProcessReclaim`, `PENDING_PROCESS_RECLAIMS`, `defer_live_process_resources`,
  `enqueue_process_reclaim`, `release_process_resources`, `reclaim_deferred_process_resources`
- **keep** the per-CPU fault-exit ring (F22); add `FAULT_EXIT_INTENT_DROPPED`; the drain moves to `kreclaimd`
- `:210 handle_thread_exit` → thin wrapper: resolve tid→pid under the PM lock, `retire_process`, drop the
  lock, `receipt.complete()`. It no longer takes `children`, no longer clones `name` under the lock, and
  no longer performs phase-2 work itself
- FD close helper takes **one** already-owned `FileDescriptor`

**`kernel/src/ipc/fd.rs`** — replace `take_all() -> Vec` with allocation-free `take_next_for_exit()` +
`is_drained()`; the row destructor must observe an already-drained table.

**`kernel/src/task/thread.rs`** — add `ThreadState::ExitPending` and an embedded
`Option<RetirementFence>`; initialize in **every** constructor (`:598,660,709,757,818,874,955,999` and
the clone at `:531`); a clone never inherits retirement state.

**`kernel/src/task/scheduler.rs`**
- `:550-568` → `RetirementFence` / `RetirementSnapshot` per §7.1
- `:952 reclaim_terminated_threads` → allocation-free single-thread claim (removes the three `Vec`s of
  F14); epoch-before-liveness (**adopt `867ce0c6`'s order**, now backed by the fence); `ExitPending`
  never reclaimable
- add `request_exit_pending(tid)`, `finalize_exit_pending(tid, &RetirementSnapshot)`,
  `detach_reclaimable_thread()`
- **no** reclaim call added to any scheduling entry/return path; no reserved ISR wake slot (R14)

**`kernel/src/memory/frame_metadata.rs`** — preemption-pinned guard for normal context (the load-bearing
fix, R9); `try_lock` transaction for fault context returning `Retry`; **remove the `log::error!` (`:90`)
and `log::trace!` (`:110`) from inside the lock** in favour of atomic counters.

**`kernel/src/memory/process_memory.rs`** — `cleanup_for_exec(self, &ReclaimContext)` and
`cleanup_cow_page_table(&…, &ReclaimContext)` on both arches; the aarch64 copy **keeps** its counters and
`log::info!` so the two arches report identically (R12). Logging is legal here because the only callers
mint a `ReclaimContext`.

**`kernel/src/memory/stack.rs`** — `GuardedStack::drop` stays a TODO stub, but **no correctness comment
couples to it**: the stack lives in the grave until quiescence is proven, so implementing it later is
safe by construction. Delete the branch's "safe only while this is a no-op" prose rather than rewording it.

**`kernel/src/memory/kernel_stack.rs`** — release eligibility requires a `&RetirementSnapshot`; keep the
full 64 KiB scrub and the `:338` debug assertion that a handed-out slot is no CPU's live/resume stack.

**`kernel/src/syscall/{wait.rs,handlers.rs,clone.rs}`**, **`kernel/src/fs/procfs/mod.rs`**,
**`kernel/src/process/creation.rs`** — enumerate children by scanning `parent`; `clone`/`fork` stop
pushing the mirror (`manager.rs:805,1887,2111`, `clone.rs:241`, `process.rs:522`); allocate the grave
alongside the page table (same site, same `ENOMEM` failure mode).

**`kernel/src/signal/delivery.rs`**, **`kernel/src/syscall/signal.rs`** — replace the direct
`process.terminate(...)` calls (`delivery.rs:224`, `:258`, `signal.rs:162`) with `retire_process` +
`receipt.complete()`. A remote killer **never** touches its own TTBR0 (R2, invariant 8).

**`kernel/src/interrupts.rs`**, **`kernel/src/interrupts/context_switch.rs`** (x86_64) — the
`exit_process` sites (`:1429`, `:1735`) and the `terminate` site (`context_switch.rs:1021`) adopt the
receipt; `complete()` mints the x86 `ReclaimContext` and frees inline. **Disclosed x86 behavior change:**
FD closing moves off the PM lock.

**`kernel/src/main_aarch64.rs`** — start `kreclaimd` after workqueue (`:804`) / softirq (`:810`) init;
`expect` on failure so boot fails loudly rather than silently leaking.

### Source-structure tests (B §12, adopted)

Assert that: the two tail functions contain no reclaim/drain call; the assembly never clears `next`
before `msr`; only the three helpers of §6.1 write `ttbr0_el1` in Rust outside boot/paging/SMP; every
teardown counter has an in-tree reader; the six frozen regions hash unchanged; `git diff -- '*.S'` shows
only §6.2's lines.

---

## 9. What survives from `fix/teardown-followups`

| Commit | Verdict | Basis |
|---|---|---|
| `5781442b` decode last-dispatch owner | **SURVIVES verbatim.** Re-verified: `main:context_switch.rs:1663` still loads the raw encoded word into `owner_tid`, so the delta is still real. | R1-13; postmortem-only, no frozen region |
| `867ce0c6` epoch-before-liveness reorder | **SURVIVES**, folded into `RetirementSnapshot` so the ordering is type-enforced rather than statement-ordered. Re-verified: `main:scheduler.rs:994-1005` still reads stack liveness first. | R1-8 / invariant 12 |
| `c0be17e7` owner-safe fault exit (`exit_process -> Option<Vec>`, `finish_extracted_process_exit`, `terminate_minimal -> bool`) | **DIRECTION survives** (all fault exits share one machinery; FD close moves off the PM lock). **Mechanism replaced** by `ExitReceipt` + `ExitOutcome`. | R2-6, R2-11: a caller-checked `Option`/`bool` is the exact shape that skipped phase 2 |
| `28a7933e` retirement off ERET paths | **DIRECTION survives** (drains deleted from both tails; a dedicated worker owns teardown). **Mechanism replaced**: no `Vec` queue, no cap, no panic, no fixed 10 ms poll. Its `switch_ttbr0_if_needed` publish-in-the-equal-case fix survives inside `install_process_ttbr0`. Its aarch64-only `cleanup_for_exec` log/counter removal is **reverted** (R12). Its two "keep the shadow visible" comments die with the code they justified — both were false (R2-1/R2-2; F2/F3 confirm `switch_ttbr0_if_needed` is never reached on the idle path). | invariants 1, 26, 28 |
| `5d7ab37c` pid-ownership before quiesce | **DELETED** — `current_cpu_retains_ttbr0_root`, the `main_thread.id` gate and `quiesce_ttbr0_for_exit` all go (R2-7). Its `read_ttbr0_el1()` helper and its "read the hardware register in the oracle" idea **survive** inside `root_liveness` (§7.2 clause 1, invariant 7). | R2-7 / invariant 14: the correct answer is that **no** per-site ownership predicate should exist |
| x86 `let _ = pm.exit_process(...)` | Subsumed by the receipt. | — |
| The three write-only counters, `MAX_PENDING_PROCESS_RECLAIMS` + its panic, `defer_process_resources`, `init_process_reclaim_worker`'s poll loop, the early `stack.take()` prose coupling | **DELETED.** | R2-3, R2-4, R2-9, R2-10, R2-12 |

**Already on `main`, therefore not part of this work despite appearing in Design A's plan** (F20): the
fatal-postmortem sectioning and high-value-first ordering, the `dump_stack_classification` range import,
and the stack-slot victim-tid resolution in `defer_current_user_thread_sigsegv_exit`.

---

## 10. Invariant-by-invariant audit (structural satisfaction)

| # | Invariant | Design element that satisfies it |
|---|---|---|
| 1 | no teardown from ERET tails | `ReclaimContext` (§5.1) makes it a compile error; both `drain_…` calls deleted (`context_switch.rs:3438`, `:4459`); the only mints are `kreclaimd` and the IRQs-on fork pass |
| 2 | no logging on tails / under the PM lock / in idle | CP2 has zero output by construction (`log::info!`, `log::debug!`, `cleanup_windows_for_pid` and the logging `GuardedStack::drop` all leave the critical section — F12, F25); teardown logging is gated by `&ReclaimContext`; `frame_decref`'s two in-lock logs are removed; postmortem output is raw-UART only |
| 3 | nothing blocking inside IRQs-off / PM-lock sections | CP2/CP3 are `Option::take` + `mem::swap` + one CAS: no alloc (grave pre-allocated at birth, buffer swapped not extended, no `Vec::push`), no second lock, no free. FD claim is one entry at a time; the `children` mirror is deleted; `name.clone()` moves into `complete()` |
| 4 | early-return gate checked before new heavy work | there is **no** work before the `PREEMPT_ACTIVE` check — the call was removed, not reordered |
| 5 | frozen regions untouched | §8 touches none of the six; the assembly diff is §6.2 only, in a non-frozen file; `dispatch_thread_locked` (which contains the frozen EL0 dispatch marker at `:3261`) is deliberately not modified — R10 |
| 6 | shadows republished only **after** the hardware switch | §6.1 helper bodies + the §6.2 assembly fix, which removes F4 — the last writer that cleared a shadow before switching; `install_process_ttbr0` publishes in the already-equal case too (fixes F7) |
| 7 | liveness oracle consults hardware authoritatively | §7.2 clause 1 is an exact local `mrs`; remote CPUs are covered by the §6.3 proven superset, with the "no architectural remote read exists" rationale stated verbatim in the doc comment |
| 8 | quiesce only by the owning CPU, uniformly | `leave_process_ttbr0()` takes **no arguments** — there is no pid to misapply, no refusal path, and no per-site predicate to diverge; `retire_process` performs no TTBR0 operation at all |
| 9 | idle redirect must not strand a shadow | both idle-redirect sites perform a **real hardware leave** (`setup_idle_return_locked`, `set_idle_stack_for_eret`), so the closure is total even on paths that never revisit a dispatch site (F2/F3) |
| 10 | dying thread leaves stack + root before either is reclaimable | `ExitPending` is non-reclaimable; `TERMINATED` is committed only on the neutral stack (F23) or by the reaper after the fence (CP8); `terminate_current_scheduler_thread` is deleted — F10 shows the stack predicate alone does **not** cover the fault path |
| 11 | reclamation requires proof, not scheduler name-matching | CP4 = §7.2 clauses 1–5; CP9 = `¬is_kernel_stack_slot_live ∧ fence ∧ TERMINATED`. No `cpu_state[].current/previous/idle` test participates |
| 12 | epoch read ordered before liveness read | `RetirementSnapshot` is a capability: it cannot be produced without the `Acquire` epoch loads and an unconditional `fence(Acquire)`; an empty `online_mask` never elapses (kills F13); every liveness API takes `&RetirementSnapshot` |
| 13 | double-terminate guard preserved on every path | `grave.take()` **is** the guard and the action; a second exit gets `None` and there is nothing left to double-free |
| 14 | the same predicate at every call site | there is no per-site predicate left: root-based liveness lives in exactly one function, and the leave takes no arguments (kills the `main_thread.id` gate and the unchecked sibling call site) |
| 15 | every exit path reparents and wakes waitpid | `#[must_use] ExitReceipt` is non-optional; `complete()` is the only tail and runs on `Committed` **and** `AlreadyCommitted`; reparent and wake are durable work bits, not one-shot statements |
| 16 | state moved out only after early-return checks | nothing is moved out on the already-committed path — the `children` mirror no longer exists and the grave is already `None`; FD entries stay in the row until the worker claims them |
| 17 | return-contract changes honored by every caller | the receipt has no `Option` and no `bool`; the fault arms' redirect is unconditional; `ExitOutcome` is diagnostics-only and no control flow reads it |
| 18 | FD cleanup outside the PM lock on every path and arch | one shared transaction; the worker claims one entry under PM and closes it outside — on x86_64 too (disclosed change) |
| 19 | no broadcast TLBI under an IRQs-off lock | `retire_process` does no TTBR0 work; `leave_process_ttbr0` has no TLBI; the broadcast is one per batch in `kreclaimd` (CP5). Net: four broadcast TLBIs removed from the fault arms versus main |
| 20 | dual-context locks use try_lock or are unreachable | frame metadata: preemption-pinned in normal context (load-bearing) + `try_lock`/retry-without-mutation in fault context. F16 proves A's "confine it to the reaper" is insufficient — the reaper is preemptible and the CoW waiter has IRQs masked |
| 21 | no panic while holding a lock with IRQs off | no bounded reclaim collection exists, so there is no overflow policy and no panic (§7.4) |
| 22 | a wedged CPU cannot make a cap fatal | there is no cap; a wedged CPU pins only its own graves, unrelated work continues, and allocator pressure surfaces as `None`/`ENOMEM` (F26) |
| 23 | diagnostic counters are actually read | `dump_reclaim_state()` is called from `dump_fatal_postmortem_once` before the trace-buffer section; a source-structure test asserts every teardown counter has an in-tree reader |
| 24 | diagnostics import range constants | already true on main (F20); the new slot-index print reuses the same imports and adds no hardcoded range |
| 25 | postmortem prints high-value sections first | already true on main (F20); the new section is inserted **before** trace buffers using the existing per-section claim wrapper |
| 26 | no unstated arch asymmetry | one shared lifecycle with two named arch hooks; aarch64 `cleanup_for_exec` keeps the counters and logging x86 never lost (the branch's removal is reverted); the x86 FD-close move is disclosed in its own commit |
| 27 | commit messages state the full blast radius | §11 slices each name the reachability they change, the guard they remove, and the +1 store in the assembly |
| 28 | justifying comments true on every reaching path | the two false comments die with the code they justified; the replacement rationale (§6.3 superset table; §7.2's "no remote `mrs` exists") is provable, and a source-structure test enforces the control-flow claims |

---

## 11. Commit slices (each build-clean on both arches, zero warnings)

1. `fix(aarch64): fence retirement grace and reject empty targets` — §7.1 plus `867ce0c6`'s reorder.
   State that `retirement_grace_elapsed` previously returned `true` after **zero** atomic loads.
2. `fix(aarch64): decode last-dispatch owner in the ERET anomaly dump` — `5781442b` verbatim.
3. `fix(aarch64): publish TTBR0 leases after the hardware handoff` — §6, including the `syscall_entry.S`
   change (**+1 store per site — do not claim instruction parity**) and the two real idle-path leaves.
   State that both idle redirects previously wrote shadows without switching hardware, and that four
   broadcast TLBIs leave the fault arms.
4. `refactor(process): one-shot exit transaction with a must-use receipt` — CP2 and §8. State that it
   deletes `Process::terminate()`, both `close_all_fds()` copies, the `children` mirror and
   `exit_current()`, and that FD closing moves off the PM lock **on x86_64 as well**.
5. `fix(aarch64): make off-stack termination a scheduler state transition` — `ExitPending`, CP8, CP9,
   deletion of `terminate_current_scheduler_thread`. State that the previous mark ran while the CPU was
   still on the dying thread's stack and that the stack-liveness predicate did not cover it.
6. `feat(process): pre-allocated grave and lock-free graveyard` — CP3. State that exit becomes infallible
   and creation gains one `Box` with the existing `ENOMEM` failure mode.
7. `feat(aarch64): kreclaimd owns all address-space teardown` — §5, CP4/CP5, `ReclaimContext`. State that
   the drains leave **every exception return**, that the broadcast TLBI moves to the worker, that the
   aarch64 `cleanup_for_exec` counters/logging are retained, and that there are two aarch64 mint sites.
8. `fix(memory): make frame metadata safe across faults and preemption` — R9, both arches. State that the
   CoW fault path runs with IRQs masked.
9. `feat(aarch64): reclaim observability and stall watchdog` — §7.4, `dump_reclaim_state()`, structure tests.

Every message carries `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

## 12. Validation gates

1. Both arches build clean, zero warnings (`#[must_use]` plus the project's zero-warning policy makes a
   dropped receipt a hard gate).
2. QEMU aarch64 x10 consecutive clean boots: zero `UNHANDLED_EC`, `INSTRUCTION_ABORT`, `DATA_ABORT`,
   `EL1_INLINE_ABORT`, `FATAL_POSTMORTEM`, `[BUG] dispatch_thread`, `TTBR_GONE`, panic.
3. Fork/exit/waitpid stress >=1000 iterations, driving `reclaim_terminated_threads` +
   `allocate_kernel_stack` + the 64 KiB scrub against still-on-CPU dying threads. Assert **real child
   execution and status** — never "process was created". Assert `GRAVES_RECLAIMED == processes_exited`,
   `GRAVE_STALL_WARNINGS == 0`, `FAULT_EXIT_INTENT_DROPPED == 0`.
4. CLONE_VM owner-first exit test (extend `userspace/programs/src/clonevm_exec_test.rs`): the owner exits
   while siblings keep touching shared memory. The row may be reaped early, but the root must not be
   reclaimed until the last live sharer is gone. **This is the F18 hole: the test must fail before
   slice 7 and pass after.**
5. Non-main-thread fatal fault inside a thread group: the grave reclaims within one watchdog window and
   blocked-pass counts return to steady state (the R2-1 + R2-7 combination the branch made systemic).
6. Wedged-CPU injection (stop one online CPU's scheduling epoch): no panic, no overflow, unrelated roots
   still reclaim, and the diagnostics name the blocking CPU and root.
7. Double-exit matrix: syscall exit, fault after signal kill, signal after fault kill, repeated wait wake.
   CoW decrements and FD closes happen exactly once; reparenting and the parent wake still happen.
8. Debug assertion that must never fire: immediately before CP5, no online CPU's `saved`/`next`, no local
   `TTBR0_EL1`, no thread `cached_ttbr0`, and no live row names any root in the grave.
9. Parallels streak: 10 consecutive PASS with `inject_retries=0`, up to 15 attempts, a fresh epoch-named
   VM per attempt via `./run.sh --parallels` only; `prlctl stop --kill` after each.
10. 90-minute soak with CPU0 tick-rate parity watched (the frozen CPU0 regression alarm is the tripwire).
11. Diff review: the six frozen regions byte-for-byte unchanged; `git diff -- '*.S'` shows only §6.2.

---

## 13. Open questions (only where A and B disagree and the source cannot settle it)

- **OQ-A — third per-CPU word vs. fixing the assembly.** Both close the same proven gap and the source
  confirms both are feasible: `_pad3` really is free at offset 144 with `PerCpuData` staying 192 bytes
  (F19), and the two violating stores really are at `syscall_entry.S:365`/`:467` (F4). This spec fixes
  the assembly (R1) because it removes the invariant-6 violation at its source rather than compensating
  for it. The owner call is a project-culture question the source cannot answer: does the "don't touch
  `syscall_entry.S`" instinct outweigh deleting the violating instruction? If the assembly is judged
  untouchable, add A's `occupied_root` word instead — but **do not do both**; two records is exactly the
  overloaded-state generator (G1) that produced this bug class.
- **OQ-B — scope of the `children` removal.** Deleting the mirror is what makes CP2 allocation-free
  (F12), but it touches ~22 sites across `wait.rs`, `handlers.rs` (the `complete_wait` twin), `procfs`,
  `clone.rs`, `creation.rs` and `manager.rs`. The narrower alternative — keep the mirror and defer
  reparenting to the worker — still leaves a `Vec::push` inside a PM transaction that runs with IRQs
  masked (F11). Owner call on whether the removal lands inside slice 4 or as its own slice first.
- **OQ-C — `AddressSpaceRef` identity (B) vs. raw `inherited_cr3` + a live-sharer predicate (this spec).**
  §7.2 clause 4 closes the reachable UAF (F18) without any type change. B's identity/generation model
  additionally fixes stale-root *semantics* — exec generations, and the "next exec or exit means CR3
  definitely switched" assumption at `process.rs:172,523` / `manager.rs:3054`. The source shows the UAF
  is real but cannot decide whether those broader semantics are wanted now or later. This spec defers
  them and forbids new code from treating a raw root as a lifetime reference.
- **OQ-D — fault-intent overflow policy.** This spec keeps the 16-slot per-CPU ring plus a drop counter
  (R10). B's alternative quarantines a CPU whose single intent slot is occupied, which cannot lose an
  intent but adds a gate inside `dispatch_thread_locked` — the function whose body contains the frozen
  "no CPU0-specific EL0 dispatch guard" region at `context_switch.rs:3261`. Owner call on whether
  "cannot lose a fatal-path intent" is worth touching that function and carrying the PR-signoff note.
