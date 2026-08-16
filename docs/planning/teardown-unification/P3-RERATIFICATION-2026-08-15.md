# Teardown-Unification Tranche 2 — Re-Ratification Assessment

**Decision artifact for the operator. Ratification is the operator's call; this document recommends and does not presume.** This version folds in a full adversarial critique pass; every correction the critique proved is written in directly (no before/after narrative). Two items remain genuine operator judgment calls and are presented as open questions with both positions in §7.

- **Assessed against:** `main` @ `1db23de0` (read-only), 2026-08-15.
- **Subject:** `docs/planning/teardown-unification/DESIGN.md` + `PLAN.md` (v3.2, ratified for tranche 1 = P0+P1+P2 only). Tranche 2 = **P3 exec detach + P4 kernel-stack ownership parity + P5 init designation / init-group clone refusal**.
- **Prior state:** tranche-2 ratification **refused across 6 adversarial rounds** (~2026-08-10); operator chose **Option A — foundation-hardening first**.

---

## 0. Bottom line up front

**Recommendation: re-ratify tranche 2 WITH A REVISED, FOUR-PR PHASE PLAN — not as-is, and not refuse.**

The foundations did what Option A was supposed to do. Every structural precondition that P3/P4/P5's safety arguments silently assumed — page-table root ownership, frame custody, per-PID teardown observation, a truthful x86 gate, a non-corrupting context-switch dispatch — now exists in-tree and is oracle-proven. **None of the still-open debts (DEBT-1..7) is owned by a tranche-2 phase**, so the design's own binding rule does not block tranche 2.

What blocks re-ratifying *as-is* is not a safety hole: it is that **the tranche-2 documents are factually stale against today's tree** in several independently disqualifying ways — a size-ceiling rule the operator has since abolished, a standard gate citing two removed gates and a removed marker, a file:line anchor set that has wholesale drifted, phase scopes that now overlap merged machinery, and a load-bearing claim about P4's scope that does not hold once the creation paths are read. Ratifying stale text would license implementation against anchors and premises that no longer exist.

---

## 1. Provenance — what survives, and what is lost

### 1.1 LOST (stated plainly)

The **six tranche-2 refusal verdicts are gone.** Confirmed by direct check:

- `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/22d1287c-.../scratchpad/tranche2/` — **does not exist**. That session directory is absent entirely; only six session dirs survive under `/private/tmp/claude-501/-Users-wrb-fun-code-breenix/`, none of them `22d1287c`.
- A filesystem-wide search of `/tmp` and `/private/tmp` for any path matching `*tranche*` returns **nothing**.
- `git log --all --grep=tranche` shows only the v3 design commit (`d5d35b64`) and the tranche-1-complete roadmap commit (`638d1988`). **No tranche-2 verdict was ever committed to the repo.**

**Consequence:** no tranche-2 refusal finding can be quoted verbatim. Everything in §2.2 below is *reconstructed* — inference from surviving artifacts — and should be read as an argument, not a record.

### 1.2 SURVIVES (and is the basis for everything cited)

| Source | What it gives |
|---|---|
| `docs/planning/teardown-unification/DESIGN.md` lines 16–53 and `PLAN.md` lines 16–44 | The **complete DEBT-1..7 register**, verbatim, with owners, closure conditions and source findings. Fully intact. |
| `DESIGN.md` §CHANGELOG (lines 55–190) | The v3 / v3.1 / v3.2 repair history and the *tranche-1-era* pre-check and re-check findings. |
| `PLAN.md` §0 (lines 191–370) | Phasing contract (5 rules), 17-PR ledger, standard gate, dependency graph. |
| `PLAN.md` lines 967–1094 | Full P3, P4, P5 phase specs — scope, files, gate extras, "strictly better", revert story. |
| `~/.claude/projects/.../memory/MEMORY.md` | The single durable sentence about the refusal: *"2026-08-10: tranche-2 ratification refused 6 rounds → Option A foundation-hardening"*, immediately followed by the `#470` branch pause and the `#528` live-bug discovery. |
| `~/.claude/projects/.../memory/r22-minimal-teardown-state.md` line 37 | *"phases beyond tranche 1 … are NOT yet ratified for their own tranches — the design-debt register (DEBT-1..7) still gates any tranche containing a debt owner; no implementation until each later tranche gets its own ratification pass."* |
| `docs/planning/PROJECT_ROADMAP.md` + `git log` since 2026-08-10 | The foundation-hardening record: PRs #531/#534/#539/#542/#547/#549/#551/#557/#558/#565/#566/#570/#574/#577. |
| `docs/planning/470-custody/{DESIGN-470-v2,PR4-RESCOPE,TRAP-LIST-1a,README}.md` | The custody design, recovered and committed durably by PR #571 (`5d94e579`) — explicitly *because* the old session `/tmp` was fragile. **This is the lesson the lost tranche-2 verdicts teach; it was already learned once.** |

---

## 2. Reconstructing the refusal grounds

### 2.1 The register does NOT explain the refusal — say this out loud

DEBT-1..7 were registered by the **tranche-1** v3 pre-check/re-check, and each is bound to an owner phase: P6b, P7, P9, P6a, P10, P12, P8. The design's binding rule reads:

> *"A tranche containing a debt's owner phase CANNOT be ratified until that debt's closure is written into these documents and has passed a pre-check. … A tranche may be submitted while debts owned by later phases remain open, and only while that is true."* (`DESIGN.md` line 39)

Tranche 2 is **P3 + P4 + P5**. It **owns none of DEBT-1..7** — exactly the same structural position tranche 1 was in when it *was* ratified ("Tranche 1 = P0 + P1 + P2. It owns none of the six debts below … which is precisely why it is submitted for ratification now").

So: **whatever refused tranche 2 six times was not the debt register.** An assessment that tallies DEBT-1..7 and concludes "tranche 2 is clear" is answering the wrong question; one that concludes "the debts still block it" is misreading the rule. Both readings are wrong.

### 2.2 Reconstructed grounds (inference, not record)

From what the operator did *next* — Option A, foundation-hardening, targeting `#470` custody and the `#528` corruption — plus the design's own rules and the state of the tree on 2026-08-10, seven grounds reconstruct with high confidence, numbered **G1–G7**.

**G1 — Rule 1 ("strictly better, always") was unarguable on an unsound memory substrate.**
*Evidence:* MEMORY.md puts the refusal in the same breath as the `#470` branch pause ("needs custody design pass, NOT more fix rounds") and `#528` ("IRQ-resume corrupts in-flight page-table construction", live, undiagnosed at the time). P3 and P4 are both *ownership* claims about page-table roots and kernel stacks. On 2026-08-10 there was no frame ledger, no custody record, no root-slot ownership, and `map_page` had **no ownership validation at all** — PR-2 (#547) later proved that base `map_page` was silently mapping frames the caller did not own. You cannot argue "strictly better" about who owns a root when the allocator cannot tell you who owns a frame.

**G2 — the evidence layer could not fail.**
*Evidence:* the x86 gate was hollow until PR #565 landed **2026-08-14** (four days *after* the refusal). `TEST RUNNER: All tests passed` was a hardcoded string fired when the last userspace thread exited, exit codes never examined; all ~97 userspace test programs could FAIL under a green gate — and two live failures did ride green main gates. Every gate extra P3/P4/P5 specify would have been scored by that gate. A tranche whose acceptance evidence is structurally unfalsifiable cannot be ratified by an adversarial reviewer — consistent with **six** refusals rather than convergence.

**G3 — P4's premise was factually wrong about the tree.**
*Evidence:* P4 says *"today the original thread's stack is freed ungated by `remove_process` at `waitpid` reap (`syscall/wait.rs:386` → `manager.rs:1102-1104`)"*. On today's `main`, `remove_process` (`kernel/src/process/manager.rs:1086-1090`) is `self.processes.remove(&pid)` — dropping the whole `Process` row, which owns `main_thread: Option<Thread>` → `Thread` owns `kernel_stack_allocation: Option<KernelStack>` → `impl Drop for KernelStack` returns the slot to the pool. **The hazard P4 names is still structurally live**; it is merely unreached today, and only by accident (§4.2). The anchor drifted (`:1102-1104` → `:1086-1090`), but the anchor drift is not what makes P4's premise wrong — the premise's *conclusion* ("this is fine now, fold it away") is wrong; the mechanism it worried about was never actually closed.

**G4 — P5's held-publication ticket rides a dispatch path that was demonstrably broken.**
*Evidence:* P5's mechanism is *publish the scheduler thread **not-yet-runnable** → validate the ticket → publish to the run queue*, and P3's is *"dispatch refuses `Creating` rows before arming TTBR0."* Both are two-step publish protocols on the context-switch dispatcher. PR #558 (merged 2026-08-13, three days *after* the refusal) found **six early returns in `check_need_resched_and_switch`/`switch_to_thread` that returned after `scheduler::schedule()` had already committed the dispatch** — ready-queue push, `cpu_state.current_thread`, per-CPU pointer, `TSS.RSP0` — **with no rollback**, plus a routing predicate that misrouted kernel-mode-preempted user threads into `terminate(-11)`. A two-phase publish built on a dispatcher that leaks half-committed dispatches is not "strictly better"; it is a new race.

**G5 — no authoritative executing-thread identity.**
*Evidence:* filed as **#560** out of PR #558 and still open. Blocking-syscall prologues identify "the blocking thread" via the scheduler's *recorded* current, not an authoritative identity; `scheduler::current_thread_id()`, `per_cpu::current_thread()` and syscall entry (`handler.rs:585`) all read the same recorded state written by the same dispatcher. P3's clone/exec admission and P5's clone refusal both make decisions *about the calling thread's row* under the PM guard. If "who is calling" is derived rather than authoritative, the admission decision can be stamped on the wrong row.

**G6 — the phasing contract itself was contested.**
*Evidence:* weakest of the seven, flagged as such. **"NEVER establish fix-scope budgets" (operator, 2026-08-11 — one day after the refusal): no line/file ceilings on fixes, ever; safety seams OK, size gates never (killed the ~230-line size law).** `PLAN.md` rule 4 is *literally the ~230-line size law*, stated as "The ceiling is a gate." Whether or not the ceiling was argued in the refusal rounds, it is now a standing operator ruling that PLAN rule 4 violates, and that alone is disqualifying for as-is re-ratification.

**G7 — the Tier-2 file policy at the time of the refusal.**
*Evidence:* P3 names `kernel/src/task/scheduler.rs` (dispatch gate) and P4 names `scheduler.rs`; both are Tier-2 under `CLAUDE.md` ("high scrutiny; explain why GDB is insufficient"). On 2026-08-10 a tranche whose central mechanisms required Tier-2 edits had to argue that hurdle on paper for two of three phases. The operator's **2026-08-12** ruling — Tier-2 files are editable when the approach needs it, don't contort to avoid them — landed *two days after* the refusal and removes it. This ground is now resolved and cuts in the recommendation's favor.

---

## 3. Disposition — each ground, each debt

### 3.1 Reconstructed grounds G1–G7

| Ground | Status | Mechanism / evidence |
|---|---|---|
| **G1** — no ownership substrate under P3/P4's claims | **RESOLVED** | Two-layer custody shipped end to end: allocator frame ledger with unforgeable `FrameLease` + duplicate-alloc refusal (**PR #534**), per-frame custody records via `TableRecorder`/`OwnedTableFrames` with every root drop classified (**PR #539**), aarch64 `retire_bounded` (**PR #542**), leaf custody + fork/CoW/`External` conversion + `frame_decref` fail-closed (**PR #547**), x86 root-slot custody `owned_root_slots` + `PT_ROOT_SLOT_REFUSED` fail-closed + receipt-carried superseded roots (**PR #557**), and the last pre-custody free path deleted — `cleanup_for_exec` is now three lines at `kernel/src/memory/process_memory.rs:2011-2014` on both arches (**PR #574**). `#470` is closed on both arches, leak-oracle-proven. `#528` closed by **PR #531** with a durable zero-NEON kernel-ELF guard (**#544**) — that guard covers the *kernel* half of the #528 family only; **#529** (EL0 FPSIMD trap/preserve) and **#530** (linked-kernel FP/SIMD codegen ratchet) remain open in the same family (see §6.3). |
| **G2** — unfalsifiable gate | **RESOLVED** | **PR #565**: `kernel/src/task/exit_tally.rs` records every real process death exactly once at the `Process::terminate`/`terminate_minimal` choke point (fault-kills included), emits a parseable `TEST_TALLY:` line, and `scripts/x86-gate-verdict.sh` requires tally + marker + exit-code floor to agree, with a two-way empty allowlist. Falsifiability proven three ways (injected `exit(1)` → red, injected segfault → red, vanished process → red). `docker/qemu/run-x86-boot-tests.sh:6` now states in its own header that the `KERNEL_POST_TESTS_COMPLETE` marker "is likewise never used as a gate." |
| **G3** — P4 anchor and premise | **PARTIALLY RESOLVED — hazard structurally live, merely unreached** | `remove_process` (`manager.rs:1086-1090`) still frees whatever the row owns; nothing clears `main_thread`. It is unreached today only because the three `create_main_thread*` constructors permanently `Box::leak` the kernel stack at creation (§4.2) — an unratcheted coincidence, not a closed hazard. Text must be rewritten to state this precisely, not as "resolved." |
| **G4** — broken dispatch under two-phase publish (rollback) | **RESOLVED** | **PR #558**: routing predicate made token-identical to the ring-3 enforcement predicate; all six post-commit aborts now call `abort_dispatch_and_resume` + `set_need_resched`; resume-thread state preserved on rollback (no dropped wakes); the `Terminated` arm completes a real switch to idle. Fault-injection-proven. Ratchets in `tests/context_restore_structure.rs` — **46 `#[test]`s on today's main**, function-scoped, no line pins. **PR #570** added `wake_thread_any_context` + truthful `WakeOutcome` with assertable-zero wake-loss counters, and rewrote the x86 dispatch gate itself (`5eb428d1`: single unconditional PM try-lock before `scheduler::schedule()`, with a refusal arm that neither schedules nor rolls back, ratcheted; `fed61ce9`: a second refusal arm for a thread whose address space is gone). **PR #577** restored the exec-path PM→SCHEDULER lock order (`#527`), ratcheted at `tests/exec_lock_order_structure.rs` (**25 tests**) — but only for the exec path; the creation-path PM→SCHEDULER nesting remains open (§4.2). |
| **G5** — no authoritative thread identity | **UNRESOLVED — the one genuinely still-standing safety ground** | **#560 is open.** PR #558 removed two concrete skew sources but did not make the invariant tree-wide-enforced. A real fix needs a kernel-stack-derived `current` (Linux `thread_info` shape) plus conversion of ~79 `current_thread_mut()` call sites and 42 `still_blocked` polls across six files. See §7 OQ-1. |
| **G6** — phasing contract contested (size ceiling) | **NOT RESOLVED — and now formally in conflict** | Operator ruling of 2026-08-11 abolishes size gates. `PLAN.md` rule 4 must be **deleted**, not softened, and the 17-PR ledger's "Ceiling estimate" column with it. The *named split seams* survive as safety seams (explicitly permitted by the ruling) provided they are given a non-size firing condition (§5.2). |
| **G7** — Tier-2 file policy | **RESOLVED (favorably)** | Operator ruling of 2026-08-12: Tier-2 files editable when the approach needs it. Removes the paper burden P3/P4 carried on 2026-08-10. |

**Tally: five of seven reconstructed grounds closed** (G1, G2, G4, G6-partially via required text deletion, G7); **one open safety ground** (G5/#560); **one premise correction that changes P4's disposition, not the tranche's** (G3).

### 3.2 DEBT-1 through DEBT-7

**Reminder: none of these is owned by a tranche-2 phase.** Their status is *risk context for the tranche after next*, not a tranche-2 gate. Verified against `main @ 1db23de0`.

| Debt | Owner | Status | Detail |
|---|---|---|---|
| **DEBT-1** — `Report` is at-most-once in the R-19 window | P6b | **PARTIALLY RESOLVED (substrate now exists)** | PR #565 built the missing primitive: an **exactly-once-by-construction** death record at the `Process::terminate`/`terminate_minimal` choke point, covering fault-kills, with a fail-closed floor — a working existence proof the class is solvable in this tree. **Remains:** P6b's ledger is a different consumer (SERIAL via `finalize()` → `ktap::emit_summary`), and proving the `claim_exit_slot`/`record_exit` split does not reorder the registry-slot clear against the serial emission is untouched. |
| **DEBT-2** — per-descriptor close token; endpoint-CAS REJECTED | P7 | **UNTOUCHED** | No `CloseTicket`, no per-`(row, fd)` replay token anywhere in `kernel/src`. `close_all_fds` is still a private `Process` method (`process/process.rs:460`, `:518`) called from `process.rs:393`. `sys_dup`/`sys_dup2` live at `syscall/handlers.rs:3206`/`:3159`, so the `dup`-makes-the-CAS-unsafe argument stands unchanged. |
| **DEBT-3** — blocking-primitive inventory not closed at nine | P9 | **UNTOUCHED — and the surface is measurably LARGER than the design recorded** | Both named holes are live: `kernel/src/syscall/futex.rs:115` still publishes `ThreadState::Blocked` directly, and the `scheduler.rs` I/O-blocking path persists. **New, not in the design:** `kthread_park()` (`kernel/src/task/kthread.rs:151`, write at `:183`) publishes `Blocked` with no interlock. `Thread::set_blocked()` (`kernel/src/task/thread.rs:902-903`) is marked `#[allow(dead_code)]` with zero callers (`grep -rn "\.set_blocked(" kernel/src/` returns nothing) — per the repo's zero-tolerance standard this is a **deletion**, not an interlock target; leaving dead code with a bypassing name in an inventory the design is trying to close at nine is itself a violation. `tests/teardown_structure.rs:2029` pins by name family (`BLOCKING_NAME_PREFIXES = ["block_current", "prepare_to_wait"]`) so a tenth family member cannot appear unnoticed — but a direct `thread.state = ThreadState::Blocked` write is invisible to it. **Recommend filing the widened surface as its own issue now, and deleting `Thread::set_blocked`,** before P9's tranche, per the standing "any failure you find is your problem" rule. |
| **DEBT-4** — x86 reap bypasses the tombstone gate | P6a | **UNTOUCHED in behaviour; MATERIALLY EASIER to close** | The site is live and has drifted: `kernel/src/syscall/handlers.rs:3123` (design said `:3101`) still calls `manager.remove_process(child_pid)` directly — and so does its **duplicate** at `kernel/src/syscall/wait.rs:386`. But `remove_process` is now a single four-line choke point that already calls `note_process_row_removed()` → `ROW_REMOVAL_EPOCH` (`task/process_task.rs:355-357`). **The two-event join can be installed inside `remove_process` itself and cover both arches at once**, instead of the design's per-call-site chase. `wait.rs` and `handlers.rs` carry byte-similar copies of the same `complete_wait` reap block — a de-duplication seam worth noting for P6a. |
| **DEBT-5** — `EXIT_BLOCK_REFUSED` never asserted to zero | P10a-d | **UNTOUCHED (correctly — it is a standing guard, not a task)** | Registered because a later "tidy-up" could silently break it. No counter of that name exists yet (P9/P10 unbuilt), so there is nothing to have broken. Carry forward verbatim. |
| **DEBT-6** — P12 group-membership drop scoped to externally-originated signals | P12 | **UNTOUCHED (correctly — same reason)** | A silent-failure guard. `syscall/signal.rs:26` still hardcodes `const INIT_PID: u64 = 1`, read at `:124` and `:402` — P5's literal migration has not happened, so P12's surface is unchanged. |
| **DEBT-7** — defer/reclaim evidence aggregate-only, not per-PID causal | P8 | **RESOLVED** | `fork_exit_defer_reclaim_pairing_test` (`kernel/src/tracing/providers/teardown.rs:1192`) carries a bounded 64-entry `pairing_child_pids` table and asserts, per PID, *exactly one* defer and *exactly one* reclaim — failing separately on **absent** and on **duplicated** (`:1567-1576`). Takes `BootReclaimTestGuard` for single-threaded ownership of the reclaim queues. Extended to the retire cohort (`:1579-1590`) and, by PR #574, to an exec cohort (`:2093-2118`) with global-sum reconciliation. **Caveat:** `#512` is open — that test flakes intermittently on aarch64 with a per-PID reclaim-proof failure. The *mechanism* is resolved; its *reliability* is a tracked open item. |

**Debt scoreboard: 2 resolved (DEBT-7, and DEBT-1's substrate), 1 partially resolved (DEBT-1), 4 untouched (DEBT-2, DEBT-3, DEBT-5, DEBT-6) — with DEBT-3's surface now larger than recorded and carrying a dead-code deletion item, and DEBT-4 untouched but materially cheaper to close.**

---

## 4. What tranche 2 looks like starting from today's machinery

### 4.1 P3 — exec detach + clone/exec admission (`#471` part 1)

**Still real. Core defect is live and untouched.** Across all of `kernel/src` there is exactly one write of `inherited_cr3 = Some(...)` (`syscall/clone.rs:209`) and one of `thread_group_id = Some(...)` (`clone.rs:210`); the only `None` values are struct-literal defaults (`process/process.rs:337-338`). **No exec path clears either field.** The wrong-victim-after-exec defect P3 exists to close is exactly as described.

**What shrank:**
- The design's `P3 → P8` justification — *"a row carrying a stale `inherited_cr3` past an exec would present a root it does not own"* — is no longer an argument that has to be won on paper. Root ownership is now **enforced** (`owned_root_slots`, `PT_ROOT_SLOT_REFUSED`, fail-closed) and superseded roots are **receipt-carried**, drained with PM out of scope. P3's gate extra "fresh root" is now directly observable via the exec-cohort per-PID oracle (`teardown.rs:2093-2118`) rather than needing a new mechanism.
- The live-sibling guard P3 says to keep is intact (`manager.rs:49-64`, `find_live_clone_vm_sibling_holding_cr3`, called at `manager.rs:3063` and `:3368`).

**What must be re-derived, not assumed:** PR #570 rewrote the exact dispatch site P3's third scope item targets — *"dispatch refuses `Creating` rows before arming TTBR0."* `5eb428d1` replaced the prior multi-arm dispatch with a single unconditional PM try-lock before `scheduler::schedule()` and one refusal arm (ratcheted); `fed61ce9` added a second refusal arm for a thread whose address space is gone. `ProcessState::Creating` still exists (`process/process.rs:54`, set at `:318`, cleared to `Ready` by `set_main_thread` at `:352`) and the window P3 targets is real, but P3's gate is now a *third arm on an already-refactored, already-ratcheted site*, and **P3's file list must be corrected: the x86 dispatch gate lives in `kernel/src/interrupts/context_switch.rs`, not `kernel/src/task/scheduler.rs`.** The #527 exec-path lock-order fix (**PR #577**, ratcheted at `tests/exec_lock_order_structure.rs`, 25 tests) gives P3's "at every exec commit point, after all fallible work, before PM release" placement a *ratcheted* definition of where that point is — instead of P3 having to establish it — but that ratchet is specifically about the exec path, and does not by itself validate the dispatch-gate placement, which must be checked against #570's rewritten site.

**What grew:** the exec surface now has two open custody gaps that touch P3's exact commit points — **#573** (failed/never-published exec leaks the half-built address space; PR-4b) and **#572** (`AlreadyTerminated` abandon bypasses custody, leaking table leases and *all* superseded exec roots — visible today at `manager.rs:1131-1136`). P3 says "preserve both fields on **every** exec failure"; the failure path is precisely where #573 lives. **Sequence #573 before or with P3, or P3's failure-path gate extra will be asserting field preservation on a path that leaks the address space underneath it.**

**Verdict: P3 keeps ~its full scope, gains a ready-made oracle, must re-derive its dispatch-gate scope and file list against PR #570, and acquires one hard sequencing edge (`#573 → P3`).**

### 4.2 P4 — kernel-stack ownership parity (AC-8)

**Not dissolved. Its premise was wrong, and reading the creation paths surfaces a live, unfiled leak plus a live lock-order defect that were not previously visible.**

- **The freed-row hazard is structurally live, merely unreached.** `remove_process` (`manager.rs:1086-1090`) is `self.processes.remove(&pid)` — it drops the whole `Process` row. `Process` owns `main_thread: Option<Thread>` (`process/process.rs:198`); `Thread` owns `kernel_stack_allocation: Option<KernelStack>` (`task/thread.rs:428`); `impl Drop for KernelStack` (`memory/kernel_stack.rs:85-99`) returns the slot to the pool. Nothing in `kernel/src` ever clears `main_thread` (`grep -rn "main_thread\s*=\s*None\|main_thread\.take()" kernel/src/` — zero hits). `defer_process_resources` (`task/process_task.rs:475-495`) takes only `page_table` + `pending_old_page_tables`; `exit_process_locked` takes only `process.stack` — the **user** `GuardedStack` (`manager.rs:~1135`, `:~1145`), not the kernel stack. The reap-time free path is live; it is unreached today only because no row happens to hold a `Some(KernelStack)`.
- **The reason no row holds one: all three creation-path constructors permanently leak the kernel stack, unfiled.** `manager.rs:~849`, `:925`, `:1010` — every `create_main_thread*` variant does `Box::leak(Box::new(kernel_stack));` with `kernel_stack_allocation: None` and a `// TODO: proper cleanup` comment. Every process created via `create_process` / `create_process_with_argv` — spawn ×2, direct init, `boot/test_disk.rs` — permanently leaks its kernel stack. Contrast the two paths that do the opposite explicitly: `syscall/clone.rs:250-252` (`scheduler_thread.kernel_stack_allocation = thread.kernel_stack_allocation.take();`, comment *"The scheduler owns the child's kernel stack so reclamation is protected by the two-epoch retirement grace"*) and `arch_impl/aarch64/syscall_entry.rs:961`; fork assigns at `manager.rs:1833`. **P4's residue is not "apply the existing `take()` pattern to three more sites" — there is no allocation object left to transfer, because it was leaked at construction.** Fixing it is a design question: who owns a kernel stack whose thread was published to the scheduler as a `Thread::clone`, which drops the allocation (`thread.rs:514`, *"Can't clone kernel stack allocation"*)? That is strictly larger than a fold-in-as-a-second-commit item. **Nothing in `gh issue list` covers this leak; it must be filed** (per the standing "any failure you find is your problem" rule).
- **P4's scope also includes a live PM→SCHEDULER lock-order inversion that its own text names and that a prior pass missed.** `PLAN.md` P4 scope, verbatim: *"Drop PM before every scheduler registration (removing the existing PM→scheduler nesting in the spawn/test-disk paths)."* It is live: `creation.rs:67` (PM guard) → `:85` (`scheduler::spawn`); `creation.rs:~190` → `:202`; `boot/test_disk.rs:258` → `:263`. `scheduler::spawn` takes `lock_scheduler()` at `task/scheduler.rs:3447` — the identical PM-held→SCHEDULER ordering PR #577 fixed and ratcheted for the exec path only (`tests/exec_lock_order_structure.rs` pins the sole SCHEDULER acquisition living in `scheduler.rs`, running with PM released; `validate_manager_module_has_no_scheduler_lock_acquisition`; marker `[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]`). **P4 is the phase that closes the remainder of #527's class**, at the same three call sites as the leak, with a ready-made ratchet shape to extend.
- The one other live memory hazard in this area, **#546** (owner-side `GuardedStack` reclamation of `External` user-stack frames — the deliberate, bounded, per-exit leak accepted in PR #547 per DESIGN-470-v2 §1.6), is a **separate**, *user*-stack custody issue. It does not substitute for P4's kernel-stack accounting gate (AC-8) and should stay tracked on its own.

**Verdict: P4 stands as its own phase, over its own three call sites (`creation.rs:67/85`, `creation.rs:~190/202`, `boot/test_disk.rs:258/263`), with two concrete, previously-unnamed defects to fix — the `Box::leak` kernel-stack leak and the PM→SCHEDULER nesting — and AC-8's kernel-stack single-owner accounting gate kept as originally specified (1000-iteration stress, allocated == freed), not re-pointed at #546.**

### 4.3 P5 — runtime init designation + init-group clone refusal (`#464` part 1)

**Still real, entirely unbuilt, and now the largest and riskiest slice of tranche 2.** Nothing exists: no `designated_init`, no PID-1 reservation (`manager.rs:118` — `next_pid: AtomicU64::new(1)`), and `syscall/signal.rs:26` still hardcodes `const INIT_PID: u64 = 1`. The `next_pid.fetch_add` surface is **eight** sites, not the design's "all four": `manager.rs:141, 378, 602, 1076, 1419, 1561, 1704, 2169`. The production-literal migration surface is likewise larger than recorded: beyond the three the design names, `task/process_task.rs:647, 720, 723, 726` all carry `ProcessId::new(1)` init literals. Corrected statement of the ground: **≥5 literal sites plus 8 `next_pid` sites, against a design that records 3 and 4 respectively.**

**What changed under it:**
- **Better:** the held-publication ticket's two-phase publish is now safe to build — PR #558 made every post-commit dispatch abort roll back, and #577 fixed the exec-path lock order. On 2026-08-10 this mechanism would have been built on a dispatcher that leaked half-committed dispatches (G4).
- **Better:** the clone refusal (`clone.rs:84` derives the TGID inside the PM guard taken at `:60`; the refusal is 2–3 lines immediately after) is unchanged and still lands exactly where the design says.
- **Worse:** P5's gate extra *"over a full boot, no row other than init itself ever carries init's effective TGID — asserted by walking the process map at quiesce"* now collides with **#575** (init never finishes its service sequence on the QEMU gates: `/bin/bwm` spawn returns EIO, the following `/sbin/telnetd` spawn never returns). **A phase whose acceptance is "walk the process map at quiesce" cannot be accepted while init does not reliably reach quiesce.** This is a hard blocker on P5's evidence, not on its mechanism.
- **Worse:** P5's failure-injection gate extra ("failure injection at **each** fallible stage after provisional PID selection: `designated_init() == None`, no row, and a retry succeeds as PID 1") is exactly the path **#573** says leaks.
- **Unchanged risk:** G5/#560 — P5 decides "is the caller init?" from a row resolved through the recorded current thread.

**Verdict: P5 splits, and its second half waits.** The PID-1 reservation (all eight sites) + literal migration (all ≥5 sites) + held-publication ticket is independently valuable and independently gateable. The `sys_clone` init-group refusal's *full* acceptance (the quiesce walk) should wait on #575.

### 4.4 Machinery that now exists and did not when the plan was written

Worth stating explicitly, because it changes what a phase has to *build* versus what it can *consume*:

- **Per-PID teardown oracles** (defer/reclaim, retire cohort, exec cohort) with absent/duplicated/exactness failures and global-sum reconciliation — DEBT-7's mechanism, reusable by any phase.
- **Receipt-carried retirement** (`RetirementReceipt`, `process/mod.rs:27`; `exit_process_and_retire`, `:252`) with PM dropped before enqueue.
- **A single instrumented row-removal choke point** (`remove_process` + `ROW_REMOVAL_EPOCH`).
- **Four census-anchored structural ratchet suites, no line pins**: `teardown_structure.rs` (**33 tests**), `context_restore_structure.rs` (**46**), `exec_lock_order_structure.rs` (**25**), `dma_and_log_sink_structure.rs` (**4**) — grown well past the 13/11/14/4 an earlier brief cited. Any new tranche-2 invariant gets ratcheted at low marginal cost.
- **A truthful x86 gate** (`scripts/x86-gate-verdict.sh`) and a real net-lock exclusion primitive (`NetLockGuard`, PR #566).

### 4.5 Text that is stale and must be rewritten before any ratification

Independent of the safety argument, these are factual defects in the tranche-2 documents as they stand:

1. **`PLAN.md` rule 4 (hard size ceiling) violates the operator's 2026-08-11 standing ruling.** Delete the rule and the ledger's "Ceiling estimate" column; keep the named split seams as safety seams, given a non-size firing condition (§5.2).
2. **Standard gate item 6 — "Frozen-region hash gate: all six gold-master regions byte-identical vs `main`" — gates on a thing that no longer exists.** The FROZEN gate was removed by **PR #520** on operator directive; `grep -rn "gold-master\|GOLD_MASTER\|FROZEN"` over `kernel/src`, `tests`, `scripts` returns nothing.
3. **Standard gate item 2 requires "the real `KERNEL_POST_TESTS_COMPLETE` marker."** That marker was removed as fakeable in `db7c1f97`, and `docker/qemu/run-x86-boot-tests.sh:6` now documents that it "is likewise never used as a gate." Replace with `scripts/x86-gate-verdict.sh` (tally + marker + exit-code floor) and the aarch64 `[BOOT_TESTS:PASS]` gate.
4. **Standard gate item 2's flake allowance names `timer_quantum_reset_aarch64`**, closed by **#518**. The live flakes are now **#536** (timer_delay starved false-red recurs after #524), **#555** (aarch64 softirq ~1%), **#512** (pairing test), **#576** (~1/80 EL1 INSTRUCTION_ABORT in spawn), **#562** (aarch64 `--features testing` panics 5/5 in a ksoftirqd self-test).
5. **The file:line anchor set has drifted AND the migration surfaces it describes have grown.** Confirmed: DEBT-4's `handlers.rs:3101` → `:3123`; P4's `manager.rs:1102-1104` → `manager.rs:1086-1090` (still the freeing choke point, contra an earlier read of it as "unrelated"); P5's `manager.rs:1178` is still inside the init-reparenting block (`ProcessId::new(1)` literal at `:1165`, read at `:1166`, `:1176`, `:1179` — drift of about one line, not an unrelated function); `any_live_root_matches` is at `process_task.rs:226`, and `process_task.rs:285` is `BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO` (both unrelated to P5, correctly). Beyond drift, P5's literal-migration surface has **grown** past what the design records (§4.3). The design's own practice is to re-verify every anchor against a named `main` SHA at each revision (`985881a6` for v3); that pass has not been run since, and thousands of lines have moved through #534/#539/#542/#547/#549/#551/#557/#558/#565/#566/#570/#574/#577.
6. **Tier-2 file policy changed in tranche 2's favor** and should be recorded: operator, 2026-08-12 — Tier-2 files (`context_switch.rs`, `scheduler.rs`, `interrupts/mod.rs`, …) are editable when the approach needs it; do not contort to avoid them. Tier-1 still requires approval, so gate item 6's Tier-1 byte-identity clause should be retained even as the gold-master clause is deleted.

---

## 5. Recommendation

### 5.1 The call

**Re-ratify tranche 2 with a revised phase plan.** Not as-is (§4.5 is disqualifying on its own), and not a seventh refusal (the substantive grounds G1, G2, G4, G7 are genuinely, provably closed, and refusing on closed grounds would be the review-loop escalation failure this campaign has already recorded as a process lesson).

### 5.2 The revised plan

**Precondition — one document pass, no code (call it P2.5).** Re-verify every file:line anchor in `DESIGN.md`/`PLAN.md` against `main @ <ratification SHA>`; delete rule 4 and the ceiling column, and give the named split seams a non-size firing condition — the natural candidate is rule 5 (a seam fires when a PR would carry two revert stories); rewrite the standard gate to today's real gates; restate DEBT-3's inventory to the widened surface found here (`futex.rs:115`, `kthread_park`, plus the `scheduler.rs` I/O path) and file it as an issue, alongside deleting the dead `Thread::set_blocked`; record the Tier-2 policy change; state what the 17-PR count rests on once the ceiling column is gone. This pass is cheap, is pure documentation, and is what makes the ratification meaningful rather than nominal.

**Revised tranche 2 — four PRs, one phase corrected in scope (not dissolved):**

| PR | Content | Notes |
|---|---|---|
| **T2-a** | **#573** — x86 failed/never-published exec leak (PR-4b of #470) | Prerequisite, not part of tranche 2 proper; gate it and evidence it on its own. P3's "preserve both fields on every exec failure" gate asserts on this path; it must not leak underneath. Already scoped in `docs/planning/470-custody/PR4-RESCOPE.md`. |
| **T2-b** | **P3 — exec detach + clone/exec admission**, scope re-derived against PR #570 (dispatch gate is a third arm on `interrupts/context_switch.rs`, not `task/scheduler.rs`) | Own revert story: delete the two field assignments + the admission check, ~15 lines. Consumes the exec-cohort per-PID oracle for the "fresh root" assertion instead of building one. |
| **T2-c** | **P4 — kernel-stack ownership parity**, over its own three call sites: fix the `Box::leak` at `manager.rs:~849/925/1010`, then remove the PM→SCHEDULER nesting at `creation.rs:67→85`, `creation.rs:~190→202`, `boot/test_disk.rs:258→263`, extending #577's exec-path ratchet to these sites | Its own revert story (per-site, each call site independent) — kept as a separate PR from P3 rather than folded in, so rule 5's one-revert-story-per-phase discipline is not overridden. AC-8's kernel-stack accounting gate is kept as specified; #546 (user-stack) stays tracked separately. |
| **T2-d** | **P5a — init identity only**: PID-1 reservation across all **eight** `next_pid` sites, held-publication ticket, migration of `signal.rs:26` `INIT_PID` plus the (re-anchored) **≥5** production literals to a `designated_init()` accessor | Ships no behaviour change on init death, per the design's deliberate identity/policy split. Failure-injection gate extras land here and are now safe because dispatch rollback is fixed (PR #558). |
| **deferred** | **P5b — the `sys_clone` init-group refusal** | Two or three lines, but its acceptance is the quiesce process-map walk, which **#575** blocks. Hold until #575 closes; the design's `P5 → P9` edge means P5b must land before P9 regardless, so this does not extend the critical path unless P9 arrives first. |

**Carry unchanged into the next tranche:** DEBT-1 (with PR #565's exactly-once choke point cited as the existence proof P6b should generalize), DEBT-2, DEBT-3 (restated wider, with the dead-code deletion noted), DEBT-4 (with the note that the join now installs in one place), DEBT-5, DEBT-6. DEBT-7 is closed — strike it, citing `teardown.rs:1567-1590`.

### 5.3 The honest caveats on this recommendation

- **G5/#560 is not resolved and this document is not claiming it is.** P3's and P5a's admission decisions read the calling row through the recorded current thread. See §7 OQ-1 for both positions on whether that is acceptable for tranche 2.
- **The six original refusal verdicts are lost** and G1–G7 above are a reconstruction (§1.1, §2.2). If any round refused on a ground not reconstructed here, this assessment does not answer it. The mitigation is cheap and available: run **one** adversarial pre-check pass against the revised documents before ratifying, rather than trusting this reconstruction alone.
- **Whatever is ratified should be committed to the repo**, not left in a session scratchpad. The `/tmp` loss documented in §1.1 already cost this campaign one full record; PR #571 was raised specifically to stop it happening again to the #470 design. This artifact and the ratification verdict deserve the same treatment.

---

## 6. Risk context — open pre-existing issues

Grouped by how they bear on tranche 2. All verified open via `gh issue list` at time of writing.

### 6.1 Directly gating or sequencing tranche 2

| Issue | Bearing |
|---|---|
| **#573** (+ PR-4b) | x86 failed/never-published exec leaks the whole half-built address space. **Sequence before P3** — P3's failure-path gate asserts on it. |
| **#575** | init never finishes its service sequence on the QEMU gates (bwm spawn EIO, telnetd never returns). **Blocks P5's quiesce-walk gate extra.** Hence the P5a/P5b split. |
| **#572** | `AlreadyTerminated` abandon bypasses custody on both arches, leaking table leases + all superseded exec roots. Live at `manager.rs:1131-1136`. Touches P3's exec-root surface. |
| **#560** | No authoritative executing-thread identity (G5 residual). §7 OQ-1 — accepted-with-eyes-open for T2-b/T2-d in this recommendation, operator may overrule. |
| **#546** | Owner-side `GuardedStack` reclamation of `External` user-stack frames. A real, separate stack-custody item; does not substitute for P4/AC-8's kernel-stack gate. |
| **#468** | "Refcount CLONE_VM shared address-space lifetime" — the design-level owner of the exact lifetime problem P3 point-fixes. P3's surviving live-sibling guard (`manager.rs:49-64`) exists *because* #468 is open. |
| **#474** | "[bug] E5-1 fork/exec child returns to kernel RIP in Ring 3 after execv" — a live exec-path correctness bug on P3's commit surface. |
| **#427**, **#438** | Pre-existing open trackers for the aarch64 `/bin/bwm` spawn EIO that #575 reports. P5b is waiting on a long-standing unresolved investigation, not a fresh issue. |
| *(unfiled)* | The three `Box::leak(Box::new(kernel_stack))` sites at `manager.rs:~849`, `:925`, `:1010` — a per-process permanent kernel-stack leak on the primary x86 and aarch64 creation paths, with an in-tree `// TODO: proper cleanup`. This is P4's real substance (§4.2) and needs a tracking issue filed before T2-c starts. |

### 6.2 Evidence-integrity risk (these make gate results ambiguous)

| Issue | Bearing |
|---|---|
| **#564** | x86 gate wrapper is untracked and never repacks the userspace test disk — **can boot stale binaries.** Highest-leverage item on this list: it can make a tranche-2 gate green against code that was never built. |
| **#540** | No x86 gate mode exercises process exit/teardown with counter visibility (`mode=full` hangs — `completion.rs` missing an x86_64 timeout). Tranche 2 is *entirely* exit/creation-path work. |
| **#512**, **#536**, **#555**, **#576**, **#562** | Known flakes: pairing-test per-PID reclaim proof; timer_delay starved false-red (recurs post-#524); aarch64 softirq ~1%; ~1/80 EL1 INSTRUCTION_ABORT in spawn; aarch64 `--features testing` panics 5/5 in a ksoftirqd self-test. Every one of these must be named in the revised standard gate's flake allowance, replacing the closed #518 entry. |
| **#533** | Staged boot-test registry not dispatched on x86 — related to PR-3's finding that `run_all_tests()` was unreachable on x86. |
| **#508**, **#509** | x86 boot thread starves under timer preemption during a busy-spin disk-completion wait; instruction-fetch page fault on first userspace process under concurrent-boot contention. Same evidence class as #554. |
| **#499** | Repo not cloneable/buildable off this Mac — a build-provenance risk alongside #564. |

### 6.3 Latent correctness, adjacent to the surfaces tranche 2 touches

**#511** (x86 fault path runs Phase-2 teardown in CPU exception context — safe at 1 CPU, defer if SMP) · **#514** (stage-1 reclaim re-block never advances `proof_failures` / never parks — latent leak) · **#492** (fault-exit deferred drain unbounded under IRQ mask) · **#493** (`check_signals_for_eintr` ignores signal disposition) · **#554** (boot thread wedges in RING3_SMOKE disk read holding the PM lock, blocking every first ring-3 entry) · **#563** (userspace store to address 0 does not fault — null page appears accessible) · **#567** (kernel-thread resume corrupts saved context in the pre-userspace boot window) · **#568** (`sys_poll` wedges guest on a connected TCP socket) · **#529** (preserve/trap EL0 FPSIMD across context switches — open in the #528 family; the kernel-side guard, #544, does not cover this) · **#530** (linked-kernel FP/SIMD codegen ratchet — same family) · **#443** (CoW deadlock audit round 2: epoll user copies) · **#421** (cross-CPU wake reschedule IPI).

**#563 deserves a call-out:** a null page that is accessible in user address spaces is a page-table-construction defect on the same surface P3 hands a fresh root to. Cheap to check while P3 is open.

### 6.4 Design-scope items tranche 2 does not close, and does not foreclose

**#448**, **#464** (P5/P12 own it — P5a is part 1), **#471** (P3 is part 1), **#492**, **#493**, **#522**, **#537** (test-suite hardening: out-of-bar evasion shapes from #470 PR-1a), **#550** (teardown_structure follow-ups), **#556** (x86 free-path custody hardening), **#559** (pinned-nightly core NEON warning), **#535** (pre-existing clippy failures in `kernel/build.rs` — note this against the repo's zero-warning standard).

*(**#527** appears in neither list because PR #577 closed its exec-path instance — counted above as resolving part of G4. Its creation-path remainder is folded into T2-c/P4 per §4.2. PR-4b from the design's original list = #573, promoted to §6.1 as a sequencing prerequisite.)*

---

## 7. Open questions for the operator

Two items in this assessment are genuine judgment calls, not fact disputes — both positions are laid out below rather than pre-decided.

**OQ-1 — Is #560 (no authoritative executing-thread identity) acceptable residual risk for T2-b/T2-d, or must it close first?**
- *Position A (proceed):* P3's and P5a's admission decisions are made **inside the PM guard on the calling path**, not remotely, and #560's failure mode is skew between a *dispatcher-recorded* current and the true executing thread specifically on a *blocking* path — which neither P3 nor P5a is. On this reading, tranche 2 can proceed with the risk named and tracked.
- *Position B (block):* #560 is the one genuinely open safety ground left standing after the foundations work (G5, still unresolved after PR #558). Both P3 and P5a make identity-sensitive decisions ("is this the calling thread's row," "is the caller init") on the same recorded-current substrate #560 flags as non-authoritative. A stricter reading holds that any phase making an identity-sensitive admission decision should wait for a kernel-stack-derived authoritative `current` (the fix #560 describes: ~79 `current_thread_mut()` call sites and 42 `still_blocked` polls across six files).
- This recommendation defaults to Position A but flags it explicitly as overridable.

**OQ-2 — Does the P5a/P5b split (§4.3, §5.2) correctly bound the critical path, or should P5b's dependency on #575 be resolved before ratifying tranche 2 at all?**
- *Position A (split and proceed):* P5a (identity/reservation) is independently valuable, independently gateable, and carries no behavior change on init death. P5b (the actual clone refusal) is two or three lines whose *acceptance evidence* — not its mechanism — is blocked by #575 (init never reaching quiesce on the QEMU gates). Since the design's `P5 → P9` edge means P5b must land before P9 regardless, deferring P5b does not extend tranche 2's critical path unless P9 arrives before #575 closes.
- *Position B (resolve #575 first):* Splitting a phase specifically to route around a currently-failing acceptance gate risks ratifying a partial phase whose sibling has no committed timeline; #575 has two long-standing open trackers (#427, #438) with no resolution in sight, so "wait for #575" could mean an indefinite wait, and P5b's design (init-group clone refusal) is small enough that some reviewers would prefer to hold the whole of P5 until it can be accepted in full.
- This recommendation defaults to Position A but flags it explicitly as overridable.

---

## 8. One-line summary for the operator

The foundations closed five of the seven reconstructed refusal grounds outright (G1, G2, G4, G6-in-part via required deletion, G7) and left one genuine safety ground open (**#560**, OQ-1) plus one policy conflict now resolved by deletion (**the ~230-line ceiling you abolished on 2026-08-11 is still written into the plan as a gate**); **no** open debt is owned by a tranche-2 phase; **P4 is not dissolved — it has a live, unfiled kernel-stack leak (`manager.rs:~849/925/1010`) and a live PM→SCHEDULER lock-order inversion at the same three creation-path call sites, and stands as its own PR**; **P3 is ready behind #573, with its dispatch-gate scope re-derived against PR #570**; and **P5 splits, with P5a ready and P5b waiting on #575 (OQ-2)**. Recommend: one documentation-repair pass, one adversarial pre-check against the repaired text, then ratify the four-PR revised tranche (T2-a → T2-b → T2-c → T2-d, P5b deferred).
