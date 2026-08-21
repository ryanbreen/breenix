# Tranche 3 — ratification decision artifact

> **DECISION PENDING — no phase is cleared for build until the operator ratifies an option.**
> This document reconciles a first-pass assessment with an independent adversarial re-derivation.
> Every anchor below was re-verified against the working tree during reconciliation (2026-08-20);
> where the two passes disagreed, the disagreement is resolved explicitly and the correct claim is
> marked **[RESOLVED]**. Nothing in this document authorizes a build. Per PLAN §2 condition 7, a
> tranche containing P6a, P6b, P7, P8, P9, P10, P11 or P12 arrives with its debt closure pre-checked
> and ratified, or it does not arrive.

**Assessed at** `main` @ `c0820bad` (merge of PR #614), 2026-08-20.
**Assessed against** `docs/planning/teardown-unification/PLAN.md` (last touched `a6e2e3eb`, the P4-landing
doc commit) and its DESIGN-DEBT REGISTER, both of which were last *anchor-verified* at `main` @
`2c7b8798`.

**How far the tree has moved past the last doc verification.** Nine merges landed after `2c7b8798`:
#581, #582, #585, #587 (P3), #590 (P5a), #594, #600, #595 (P5b), #601 (P4), #604, #614. The PLAN text
was refreshed only through #601; **#604 and #614 are not reflected anywhere in it**, and neither is the
gate matrix those two PRs rebuilt.

**Headline, reconciled.** The ratified phase *shapes* all survive — every named mechanism still exists
and is still in the same file. Three register claims are factually wrong (Findings A/B/C below), one of
them load-bearing for P9. A second, independent adversarial pass reproduced roughly sixty of the first
pass's line citations, found three of them wrong (corrected in place, §1), and — more importantly —
found that **the recommended option's central selling point does not hold arithmetically**, that a
closed issue is silently absorbing live gate reds, and that P6a's blast radius is 6× larger than first
scoped. Both passes' findings are folded below; nothing from either pass is presented without having
been re-checked a third time during reconciliation.

---

## 0. What changed between the draft and this artifact (reconciliation log)

| # | Where | First pass said | Adversarial pass said | Reconciled against tree | Resolution |
|---|---|---|---|---|---|
| R1 | Option A benefit | Fixing T3-S (#609, #607, #580) makes the kernel-merge gate return a **clean verdict**, "roughly a 2.4× improvement" | T3-S does not scope **#576**; the strict gate's own governing doc says #576 alone already hard-fails it. Reachable improvement is **1.84×**, not 2.4×, and the gate is still red ~22% of the time after T3-S | Confirmed: `589-ROUND2-PARTITION-2026-08-20.md:206-209` — *"The strict gate is deliberately not given this arm… #576 already hard-fails it despite being pre-adjudicated."* T3-S's own scope list (§2 below) names #609/#607/#580/#605-stretch only. | **Adversary correct.** Option A's yield is restated as 1.84×, "clean verdict" language removed, and the residual #576 exposure is now stated explicitly as a live choice the operator must make (fold #576 into T3-S, or accept an adjudicated #576 arm on the strict gate, or accept ~1-in-4.5 false reds after T3-S). |
| R2 | Pre-adjudicated flake list / gate description | Lists **#589** as an open, boot-intercepting issue that the SS gate correctly declines to gate on | **#589 is CLOSED** (2026-08-21T02:38:44Z). A `589` bucket keyed to a closed issue is still live in `run-aarch64-service-sequence-gate.sh` (three classify arms, excluded from FAIL), silently absorbing reds — the exact defect class `3f9eb7b3` was written to fix for #596 | `gh issue view 589` confirms CLOSED. Bucket code confirmed present at the cited lines. | **Adversary correct.** #589 is CLOSED everywhere it appears in this artifact. The `589` classifier bucket's retirement/re-keying is added to T3-S scope as a required item (§2, item 0), not optional cleanup — a non-gating bucket keyed to a closed issue is a known-blind gate and must not carry into a build tranche. |
| R3 | P6a blast radius | Four `remove_process` callers: `wait.rs:386`, `handlers.rs:3128`, `manager.rs:184` (new), `process_task.rs:2134` (harness) | **24** non-definition call sites total; 20 of them are inside `kernel/src/tracing/providers/teardown.rs`, all `boot_tests`-gated in-kernel oracles, every one a candidate permanent tombstone or broken oracle under the two-event join | `grep -rn "remove_process(" kernel/` returns 25 lines; 1 is the definition (`manager.rs:1342`), 24 are call sites — 20 in `teardown.rs` (verified line list), 4 production/harness (`wait.rs:386`, `handlers.rs:3128`, `manager.rs:184`, `process_task.rs:2134`). | **Adversary correct, exactly.** P6a's scope table is corrected to 24 callers. "P6a is the smallest remaining phase" is withdrawn as an unqualified claim — it is smallest by *ratified mechanism*, not by call-site blast radius. |
| R4 | P6a liveness-lookup census | "plus nine raw `self.processes.get(&…)` sites in `manager.rs`" | Understated 3.5×: 9 immutable `get(&` sites plus **23** `get_mut(&` sites, all equally live-process lookups that gate extra (c) must cover | `grep -c "self\.processes\.get("` → 9; `grep -c "self\.processes\.get_mut("` → 23. Confirmed independently. | **Adversary correct.** Census restated as 32 raw lookup sites (9 immutable + 23 mutable), all in scope for P6a gate extra (c). |
| R5 | Three anchor citations | `remove_process` is "eleven lines at `manager.rs:1342-1352`"; `EXPECTED_USER_STACK_RESIDUAL` at `teardown.rs:3656`; SS-gate preflight invocation inside `:128-165` | `remove_process` is **nine** lines, `:1342-1350` (`:1351` blank, `:1352` is the next doc comment); the constant is defined at **`teardown.rs:3363`**; the preflight function is defined `:144-167` and **invoked at `:169`**, outside the cited range | All three re-derived directly: `sed -n '1342,1353p' manager.rs` shows the closing brace at line 9 (i.e. `:1350`); `grep -n "EXPECTED_USER_STACK_RESIDUAL"` → `:3363`; `grep -n "require_boot_tests_kernel"` in the SS-gate script → def `:144`, invoke `:169`. | **Adversary correct on all three.** Anchors corrected in place throughout this document. Note the irony preserved from the adversary's own framing: the draft's *correction* of the register's stale "four-line" claim was itself stale by the time it was written — anchors move fast enough in this tree that every citation needs re-derivation at ratification time, not assumption from the previous pass. |
| R6 | Issue-interaction sections (P6a, P9, §12) | Named #583, #588, #493, #603 as the relevant open issues on proposed-phase surfaces | Six more open issues sit on the exact surfaces discussed and are named nowhere: **#560** (P9's direct sibling to the headline finding), **#540** (P6a's x86 leg has no gate mode to execute in), **#572** (P6a/P8 exit-path custody leak), **#492**/**#511** (P11 fault convergence), **#592** (red on main, unrelated toolchain drift, §12 presents the build gate as clean), **#598** (live flake inside the "sound" prod-profile gate item), **#564**/**#567** (x86 gate presented as authoritative with neither named), **#546** (P6a's #583 residual-frame sibling) | `gh issue view` confirms all OPEN: #560, #540, #572, #546. #592/#598 bodies confirm both are live, reproducible defects (#592: `kernel_build_test` fails on main today due to toolchain override; #598: BLOCK_EINTR_ORACLE stage-2 timing race, reproduced 1/25). | **Adversary correct; folded in.** Every named issue added to its respective phase section and to §4 item 12 below. None of these block T3-S or the DEBT-4 doc pass; all are readiness-relevant and must accompany whichever option the operator ratifies, so the tranche does not walk into surfaces it doesn't know it owns. |
| R7 | Option cost framing | Options A and B costed as though A eliminates gate noise and B merely "costs more" | A and B run the identical SS gate, identical Parallels 3×, identical #576 exposure; the real delta is strict-gate red rate, 58% (B, #609 unfixed) vs ~22% (A, #609/#607 fixed but #576 still open) — a 2.6× gap, not the implied "clean vs dirty." Both options also share an unnamed P6a x86-evidence hole (#540). | Re-derived from the same probability arithmetic used in R1: `0.97²⁰≈0.54` for #609 unfixed (B), vs `(79/80)²⁰≈0.78` for #609/#607 fixed and only #576 remaining (A). 1-0.54=0.46 (B fails ~46-58% depending on rounding/#607 contribution), 1-0.78=0.22 (A fails ~22%). | **Adversary correct.** §5 options re-costed with both figures stated side by side and the shared #540/x86 hole disclosed on both, not just B. |
| R8 | T3-S presented as | An ordered scope list, implicitly one PR-shaped unit | T3-S's own rule-5 test (two-revert-story ⇒ separate PRs, correctly applied by the draft to reject Shape 3) applies equally to T3-S's four items — they are 3-4 independent revert stories, hence 3-4 strict-gate batteries, and the #576 tax compounds per battery (p≈0.47 that a 3-PR T3-S sequence gets through clean end-to-end even after #609 is fixed) | `PLAN.md:264-269` verified verbatim: *"a seam fires when the PR in front of it would carry two revert stories."* #609 fix, #607 fix, #580 ratchet-visibility fix + dead-pair deletion, and (now) the #589-bucket retirement are four independently revertable mechanisms. | **Adversary correct.** §2 restates T3-S as an N-PR sequence (now 4 items, see R2's addition) with per-PR gate-battery cost, not a single ordered list. |

**Net effect of reconciliation on the two first-pass headline findings (A, B, C) and the recommendation:**
Findings A and C are unaffected by the adversarial pass and stand as originally derived (independently
re-confirmed below). Finding B's replacement figures were themselves stale and are corrected. The
recommendation (Option A, bounded scheduler tranche then P6a) survives as the better-supported option,
but its arithmetic, its scope (now including the #589-bucket retirement), and its honest ceiling
(1.84×, not "clean") are all revised.

---

## 1. Headline findings (re-verified)

| | Finding | Owner | Status |
|---|---|---|---|
| **A** | **A tenth blocking primitive already exists and the ratchet cannot see it.** `WaitQueueHead::prepare_to_wait_checked` (`kernel/src/task/waitqueue.rs:96`) is `pub(crate)`, publishes `BlockedOnIO`, and is the **live futex wait path** (`kernel/src/syscall/futex.rs:164-165`, state re-read `:242`). It is not in `BLOCKING_PRIMITIVES` (`tests/teardown_structure.rs:2533-2543`, nine entries) and is structurally invisible to `validate_blocking_primitives` (`tests/teardown_structure.rs:2743-2759`): `preceded_by_keyword` (`:1155-1168`) requires the immediately-preceding code byte to be an identifier byte equal to `"pub"`; for `pub(crate) fn` that byte is `)`, so the site is dropped. P9's gate extra 3 — *"the blocking-primitive set is exactly the nine above"* — is **already false on main**, and the ratchet's own comment (`tests/teardown_structure.rs:2670-2675`, *"pinned by name prefix so that a tenth primitive is caught however it is named"*) is a false claim about the tree today. | DEBT-3 / **#580** / P9 | **CONFIRMED — adversarial pass independently reproduced every line cite and could not break it.** The single most valuable finding in either pass. |
| **B** | **DEBT-4's cost analysis is stale, and the corrected figures were themselves stale.** `remove_process` is not "a four-line choke point at `manager.rs:1086-1090`" (register) nor "eleven lines at `manager.rs:1342-1352`" (first pass) — it is **nine lines at `manager.rs:1342-1350`**, and now also clears `designated_init`. There are **24 callers**, not four: `wait.rs:386`, `handlers.rs:3128`, `manager.rs:184` (P5a creation-failure retire of a provisional PID-1 reservation — not a reap, must be exempted from the two-event join), `process_task.rs:2134` (the `p1_row_epoch_gate` harness), and **20 sites inside `kernel/src/tracing/providers/teardown.rs`** (`1581, 1724, 1983, 2001, 2403, 2571, 2756, 2779, 3424, 3863, 4203, 4247, 4287, 4302, 4355, 4490, 4582, 4840, 4842, 5035`), all `boot_tests`-gated in-kernel oracles. Every non-reap remover is a candidate permanent tombstone or broken oracle under the ratified two-event join (`PLAN.md:1754-1760`). Liveness-lookup census is likewise corrected: **32** raw `self.processes.get*(` sites (9 immutable + 23 `get_mut`), not nine. | DEBT-4 / P6a | **CORRECTED (R3/R4/R5).** Figures in this document are the third-derivation, tree-verified numbers: 9 lines, 24 callers, 32 liveness lookups. |
| **C** | **The structural-suite inventory is stale.** PLAN §"Standard gate" item 6 pins ten suites / 256 tests re-derived at `a7be1604`. There are **eleven** today — `tests/strand_handoff_structure.rs` was added by #614 (`e5d47c81`). | Standard gate | **CONFIRMED**, unaffected by the adversarial pass. |

---

## 2. The remaining un-built phases

The ledger (`PLAN.md:315-334`) has 18 rows. Rows 1-7 are merged (P0, P1, P2 #515, P3 #587, P4 #601,
P5a #590, P5b #595). **Rows 8-18 remain** — P6a, P6b, P7, P8, P9, P10a, P10b, P10c, P10d, P11, P12.

Ratification status for all of them, `PLAN.md:204`:

> | **Tranche 3+** | P6a, P6b, P7, P8, P9, P10a-d, P11, P12 | **Uncleared.** Each arrives with its own
> tranche pass; the DESIGN-DEBT REGISTER gates any tranche containing a debt owner |

and the binding rule, `PLAN.md:2556-2560`:

> **The DESIGN-DEBT REGISTER gates the later tranches.** A tranche containing a debt's owner phase
> cannot be ratified until that debt's closure is *written into these documents and pre-checked*. …
> A tranche that contains P6a, P6b, P7, P8, P9, P10 or P12 arrives carrying its debt closure or it does
> not arrive.

**P11 is the only remaining phase that owns no debt** — and it is last in the dependency order.

### P6a — Reap/tombstone retention gate (ledger row 8)

**Ratified scope** (`PLAN.md:1727-1745`): add `RowState { Live, Zombie, Tombstone }` to the process
row plus the two join flags `reaped: Option<(ProcessId, i32)>` and `retired: bool`, so a row cannot be
destroyed before its obligations are discharged.

**Dependency edges.** In: `P1 → P6a` (satisfied, P1 merged tranche 1). Out: `P6a → P6b`.

**Anchors, corrected.**

| Ratified anchor | PLAN cite | Today @ `c0820bad` | Verdict |
|---|---|---|---|
| `complete_wait` | `wait.rs:335` | `kernel/src/syscall/wait.rs:335` | exact |
| the row removal it performs | `wait.rs:386` | `manager.remove_process(child_pid)` | exact |
| x86 duplicate (DEBT-4) | `handlers.rs:3123` | `handlers.rs:3128`, inside `complete_wait` at `:3064` | drifted +5, survives |
| `ProcessManager::remove_process` | `manager.rs:1086-1090` ("four-line") | **`manager.rs:1342-1350` — nine lines**, also clears `designated_init` via `record_init_designation_retired()` | **moved and grew; both prior "four-line" and "eleven-line" figures are stale — nine is correct as of this pass** |
| `note_process_row_removed()` → `ROW_REMOVAL_EPOCH` | `process_task.rs:355-357` | `process_task.rs:355-356`; static at `:269` | exact |
| harness caller | `process_task.rs:1807` | `process_task.rs:2134` | moved, still the harness |
| liveness-query call sites | unanchored | `manager.rs:1354` (`get_process`), `:1522`/`:1530` (`find_process_by_thread`/`_mut`), `:1542`/`:1556` (`find_process_by_cr3`/`_mut`), plus **32** raw `self.processes.get(&…)`/`get_mut(&…)` sites in `manager.rs` (9 + 23) | **census corrected 3.5× wider than first pass** |
| — (not in register) | — | **`manager.rs:184`** — `self.remove_process(provisional_pid)` on the P5a creation-failure path (comment `:174-176`: "only `remove_process` clears `designated_init`") | non-reap fourth production caller |
| — (not in register, not in first pass) | — | **20 sites in `kernel/src/tracing/providers/teardown.rs`** (`1581, 1724, 1983, 2001, 2403, 2571, 2756, 2779, 3424, 3863, 4203, 4247, 4287, 4302, 4355, 4490, 4582, 4840, 4842, 5035`), all `boot_tests`-gated in-kernel oracles | **NEW — 20 additional non-reap callers, each a candidate permanent tombstone or broken oracle under the two-event join** |

**Ratchet already covers the single-choke-point premise.** `ROW_REMOVAL_EPOCH_BUMPS`
(`tests/teardown_structure.rs:2667-2669`) pins `remove_process` occurring exactly once as an
`impl` fn; `manager.rs:174-176` documents the no-raw-`processes.remove` rule in source. DEBT-4's
prescribed closure — install the join inside `remove_process` so both arches are covered by one edit
— is proven, not assumed.

**What got harder, corrected.** 24 non-reap-context callers exist, not 4. `manager.rs:184` strands as
a permanent tombstone under the ratified join unless P6a's scope text gives it degenerate treatment
(`PLAN.md:1768-1770`) — and now so do the 20 `teardown.rs` oracle-harness callers, each of which either
becomes a permanent tombstone or a silently-broken in-kernel oracle depending on how it's exempted.
**P6a's real size is set by this 24-caller / 32-lookup census, not by the 4-caller one either prior
pass used.** "P6a is the smallest remaining phase" is withdrawn as unqualified — it may still be
smallest by ratified *mechanism* (one field, one join, one choke point), but its blast-radius audit is
the largest of any remaining phase measured so far.

**Issue interaction.** #583 (`GuardedStack::drop` does not reclaim user-stack frames) is the source of
every pinned residual (`teardown.rs:2662-2671`, asserted `:3305`/`:3315`, emitted at `:3324-3334`;
sibling pin `EXPECTED_USER_STACK_RESIDUAL` at `teardown.rs:3363`, asserted `:3741`). P6a's gate extra
(e) re-runs P4's stack-pool accounting "now that reap no longer drops the row" and every one of those
literals is a candidate to move. **#546** (owner-side `GuardedStack` reclamation of External user-stack
frames, #470 residual leaf leak) is #583's unnamed sibling on the same frames and will move under the
same re-pin — named nowhere in the first pass. #588 (counted frame residue on creation failure) lands
on the same `manager.rs:184` path. **#540** (x86 test harness has no gate mode exercising process
exit/teardown with counter visibility — `mode=full` hangs, `completion.rs` missing an x86_64 timeout)
is the reason P6a's x86 retention claim may have **no gate mode to execute in at all** — confirmed OPEN,
named nowhere in either prior pass, and material to whichever option the operator picks (§5). **#572**
(AlreadyTerminated abandon paths bypass custody, leaking table leases + superseded exec roots) also
sits on P6a/P8's exit path and is confirmed OPEN. None of #583/#546/#588/#572 block P6a outright; #540
is a genuine evidence gap, not re-pin work, and must be named in P6a's scope text before submission.

### P6b — Exactly-once ledger (ledger row 9)

**Ratified scope** (`PLAN.md:1834-1836`): row-resident `ExitLedger`, six obligations
(`Sigchld, ParentWake, Report, Reparent, Fds, Resources`), `Absent | Pending | Claimed{claimer, at} |
Completed` in a fixed-size array.

**Dependency edges.** In: `P2 → P6b`, `P6a → P6b`. Out: `P6b → P8`, `P6b → P11`. **Blocked until P6a
lands.**

**Anchors — all confirmed, drifted, survive:** `on_process_exit` (`btrt.rs:393`), `finalize()`
(`:307`), `ktap::emit_summary` (`:328`), call site (`:417`); PM-side already-terminated branch
(`manager.rs:1372-1392`, inside `exit_process_locked` at `:1367`); thread-side already-terminated
branch (`process_task.rs:648-669`, inside `handle_thread_exit` at `:638`);
`BTRT_PROCESS_EXIT_REPORTS` ratchet (`tests/teardown_structure.rs:2663-2665`).

**Debt.** **DEBT-1** — `Report` is at-most-once, not exactly-once, in the `started == 1 && finished ==
0` window (`PLAN.md:41`, `:1822-1832`). Two exits: close the window with a recoverable record step, or
carry an explicit operator acceptance that this is the round's single at-most-once obligation with the
counter asserted zero on every healthy boot. **This is an operator decision nobody has made**, and it
is independent of which tranche option is chosen (§5).

### P7 — FD closure leaves the PM lock (ledger row 10)

**Ratified scope** (`PLAN.md:1975-1990`): three-step exit-close API — `begin_fd_close(pid)` [PM] →
`endpoint_hangup(&CloseTicket)` [no lock held] → `finish_fd_close(pid)` [PM].

**Dependency edges.** In: `P2 → P7`, P6a sequenced before it. Out: `P7 → P8`.

**Anchors.** `Process::take_fd_entries()` moved from `process.rs:335` (PLAN cite) to
`process.rs:481`; live caller `process_task.rs:662`, design comment `:568`. Survives.

**Debt.** **DEBT-2** is a **hard REJECT of the ratified mechanism** (`PLAN.md:1964-1973`): endpoint-CAS
idempotence is rejected outright — live close accounting is per-descriptor, and a `dup`'d descriptor is
a second *legitimate* decrement on the same endpoint that an endpoint-level CAS would suppress. P7
needs a **design pass, not a doc repair** — a per-`(row, fd)` replay token, plus a
`dup`-then-close-both workload that fails by construction under an endpoint CAS. The single largest
piece of un-designed work remaining in the ledger.

### P8 — Victim-owned `do_exit_current` + boundary hook (ledger row 11)

**Ratified scope** (`PLAN.md:2024-2030`): claim (atomic) → local TTBR0 leave → short PM commit → drop
PM → pivot to neutral stack → mark only self `Terminated` → schedule away.

**Dependency edges.** Five in-edges (P1, P3, P4, P6b, P7); **three unbuilt (P6b, P7)**.

**Anchors.** `handle_thread_exit` at `process_task.rs:638`; `PREEMPT_ACTIVE` nested-return gate at
`context_switch.rs:3623`; `kernel/src/task/teardown.rs` correctly does not exist yet — P8 creates it.

**Debt + block.** **DEBT-7** — P0's defer/reclaim evidence is aggregate-only; per-PID causal pairing
must be built here, sampling live `TRACE_BUFFERS` or disabling providers is forbidden. **Blocked on
OQ-5** in principle, but Tier-2 approval is generally granted (operator, 2026-08-12, `PLAN.md:460-462`,
`[[tier2-edit-when-necessary]]`), so OQ-5 is arguably discharged. P8 also needs the **x86_64
user-return audit (OQ-9)**, unstarted, would run on beast.

### P9 — Request-only scheduler termination + group-scope cutover (ledger row 12)

**Ratified scope** (`PLAN.md:2099-2104`): `terminate_process_threads` redefined to request-and-wake;
never sets a remote thread `Terminated` for a boundary-reachable victim; `sys_clone` fails `EAGAIN`
into a non-`Open` group.

**Dependency edges.** Six in-edges; `P5b → P9` satisfied (#595). **P8 is the only unsatisfied in-edge.**

**Anchors — all nine drifted, all nine names survive** (`scheduler.rs:2291, 2513, 2532, 2681, 2769,
2834, 2843, 3005`; `waitqueue.rs:63`).

**DEBT-3 (#580) re-derived — worse in the way that matters, per Finding A above.**

| Register item | Today | Verdict |
|---|---|---|
| (1) `futex.rs:115` direct `Blocked` write | **GONE** — futex now blocks through `waitqueue.prepare_to_wait_checked(ThreadState::BlockedOnIO, …)` at `futex.rs:164-165`, re-read `:242` | closed by tree movement — replaced by a worse hole (item 5) |
| (2) `scheduler.rs:2607` direct `BlockedOnIO` publish | `scheduler.rs:2799`, now inside private `publish_current_io_wait_state_inner` (`:2791`) | survives; now a named funnel, cheaper to interlock |
| (3) `kthread.rs:151` `kthread_park()` | `:151`/`:183` exact, `thread.state = crate::task::thread::ThreadState::Blocked;` then `remove_from_ready_queue` | exact, untouched |
| (4) dead `Thread::set_blocked()`/`Scheduler::block_current` pair, register prescribes deletion | `thread.rs:938-940` (`#[allow(dead_code)]` `:937`), sole caller `scheduler.rs:2291` (`#[allow(dead_code)]` `:2290`) | survives intact; prescribed deletion has not happened |
| (5) NOT IN REGISTER | `WaitQueueHead::prepare_to_wait_checked` (`waitqueue.rs:96`) — tenth entry point, `pub(crate)`, live futex wait path | NEW, invisible to the ratchet — Finding A |

**Widened census, not new debt.** `grep -rnE "\.state = (crate::)?(task::)?(thread::)?ThreadState::Blocked"`
returns twelve sites; six inside pinned primitives, five in-kernel self-test fixtures, one `kthread_park`.
The naive grep the register itself used would miss `kthread.rs:183` (fully-qualified path). Any ratchet
rule for DEBT-3 must match on the *state value*, not on a source spelling.

**DEBT-5** (owned by P10, repaired in text) constrains P9's gate: `EXIT_BLOCK_REFUSED{family}` must be
asserted **nonzero** in P9's admission-race test, never zero, before or after migration.

**Issue interaction, added.** **#560** (blocking-syscall prologues identify the executing thread via
recorded scheduler current, not an authoritative identity — OQ1 residual) is confirmed OPEN and is the
direct sibling of Finding A / #580 — the same identity-authority gap on the same wait surface. P9
cannot be honestly scoped without naming it.

### P10a / P10b / P10c / P10d — Killable-wait contract (ledger rows 13-16)

**Ratified scope — v3 closure D**, four families: 10a futex; 10b `WaitQueueHead` + stdin/TTY; 10c
`BlockedOnSignal` (`pause`/`sigsuspend`); 10d child-wait + timer/nanosleep + completion/I-O + legacy-arm
deletion. **No `ExitPending` state.**

**Dependency edges.** All four gated on P9, which is gated on P8.

**Anchors — P10c drifted uniformly +6 lines, all seven intact:** `sys_pause_with_frame` (`signal.rs:825`),
`sys_sigsuspend_with_frame` (`:1320`), `sys_pause_with_frame_aarch64` (`:1821`),
`sys_sigsuspend_with_frame_aarch64` (`:2103`), legacy `sys_pause` (`:775`), `unblock_for_signal`
(`scheduler.rs:2586`), TTY unblock span (`tty/driver.rs:612-613`).

**Issue interaction.** #493 (`check_signals_for_eintr` ignores signal disposition) sits under P10's
EINTR contract, already ratcheted by `tests/signal_eintr_predicate_structure.rs`. #603 (no fault-fixup
table — a faulting user access is fatal; futex reads it under a lock) is a live hazard on P10a's exact
surface. Neither blocks; both should be named in P10's scope before submission.

### P11 — Fatal-signal and fault convergence (ledger row 17)

**Ratified scope** (`PLAN.md:2310-2312`): `deliver_default_action` returns a fatal intent instead of
mutating under the PM borrow; delete both `terminate(...)`+`with_thread_mut(...)` blocks.

**Dependency edges.** Four in-edges (P1, P6b, P9, P10d). **Owns no debt** — the only remaining phase
that doesn't — but sits behind four unbuilt phases.

**Anchors, with the confirmed scope gap:** first/second delivery blocks (`delivery.rs:239/247/258`,
`:274/279/290`); `DeliverResult::Terminated` consumers (`:97`, `:179`); four EL0 fault sites, now
routed through P2's custody-preserving `exit_process_and_retire` (`exception.rs:885, 1256, 1359, 1483`)
— part of P11's convergence is pre-done; x86 fault convergence (`context_switch.rs:1210`);
`with_thread_mut` under the live PM guard (`scheduler.rs:3903`).

`Process::terminate` callers: `delivery.rs:239`, `:274`, `context_switch.rs:1210`, **and
`manager.rs:1408`** (inside `exit_process_locked`) — a **fourth live caller P11's file list omits**.
Either P11's scope grows to cover it or the "`Process::terminate` is deleted" claim and the empty-
allowlist ratchet gate cannot both hold.

**Issue interaction, added.** **#492** (fault-exit deferred drain unbounded under IRQ mask) and **#511**
(x86 fault-path runs Phase-2 teardown in CPU exception context) both sit under P11's fault convergence
and are confirmed OPEN; named nowhere in either prior pass.

### P12 — Init death policy (ledger row 18)

**Ratified scope — v2 condition 7**: Linux-faithful protected init, no `EPERM`; user-originated signals
with no installed handler are silently dropped and the syscall returns 0; only init's own
`exit`/`exit_group`, an unhandleable synchronous fatal fault, or a nonviability invariant are
kernel-fatal.

**Dependency edges.** P5b satisfied (#595); **P8 and P11 not.**

**Anchors.** Mostly forward-looking. Identity substrate landed in P5a: `designated_init` read/cleared
at `manager.rs:1344-1347`, sole-authority comment `:174-176`.

**Debt.** **DEBT-6** — repaired in v3 text, registered because the failure mode is silent: the
group-membership drop must scope to `ExitIntent.origin == Signal`; `ExitSyscall`/`FatalFault` must
bypass the membership test entirely or init's own `exit_group` is swallowed and the kernel-fatal panic
becomes unreachable.

---

## 3. Proposed tranche-3 composition and order

### The three candidate shapes

**Shape 1 — next PLAN phases in order (P6a, then P6b).** *For:* ratified course, P6a's in-edges
satisfied. *Against:* now-corrected 24-caller/32-lookup blast radius, and it walks into a gate matrix
that cannot currently return a clean verdict (§5 arithmetic).

**Shape 2 — a scheduler-family debt tranche (#605, #606, #607, #609).** *For:* shared territory, the
#589 campaign left every instrument warm. *Against:* does not advance the ledger; risk of an
open-ended second foundation detour after the 2026-08-10→08-16 one.

**Shape 3 — a mixed tranche (P6a + a scheduler fix in one branch).** *Rejected, decisively:* PLAN rule
5 (`PLAN.md:264-269`) — a seam fires when the PR in front of it carries two independently revertable
mechanisms. Not available as one PR.

### T3-S, restated as an N-PR sequence with per-PR gate cost (R8)

`PLAN.md:264-269`'s own rule-5 test — applied by both passes to reject Shape 3 — applies equally
inside T3-S. Its items are **at least four independent revert stories**, hence four strict-gate
batteries, each exposed to the #576 tax (§5): a four-battery sequence clears end-to-end on first
attempt with p ≈ 0.78⁴ ≈ **0.37** even after #609/#607 are fixed. T3-S must be planned and reported as
four PRs with four gate batteries, not as one ordered scope list.

1. **Retire or re-key the `589` classifier bucket** (added by reconciliation, R2) — three classify
   arms in `run-aarch64-service-sequence-gate.sh` are keyed to an issue that closed 2026-08-21T02:38Z
   and are still excluded from the FAIL condition, silently absorbing whatever now lands in that
   signature. This is the same defect class `3f9eb7b3` fixed for #596. Must not carry into a build
   tranche as a known-blind gate.
2. **#609** — `network:early` subsystem kthread never dispatched (~3%, blind to the strand census).
   The item that unblocks the kernel-merge gate's dominant red source. Fix must widen the strand
   census's blind spot (a `memory:early`-stuck variant is filed at 1/200) to any subsystem kthread.
3. **#607** — the outgoing-thread `previous_thread` marker cleared without requeuing (1/50 SS-gate
   boots). A naive fix was reverted at `3c72913a` for trading a #589-shaped hang for a #576-shaped
   crash; needs the careful fix.
4. **#580/DEBT-3 re-derivation + the `pub(crate)` ratchet-visibility fix** (`preceded_by_keyword` must
   accept `pub(crate)`, or stop filtering on visibility) + deletion of the dead `set_blocked`/
   `block_current` pair (`thread.rs:938`, `scheduler.rs:2291`) — small, but behind a real live hole
   (Finding A), and leaving it means P9's ratchet gate is provably false before P9 is written.

**Explicit stretch, not committed:** #605 (`INLINE_SCHED_NULL_FALLBACK`, benign today, Tier-2 scope,
#576-shaped blast radius) — take only if 1-4 land clean.

**Explicitly out of T3-S:** #606 (no isolated mechanism yet), #612 (1/400, no reproducer), #613
(classifier issue only).

**Stop rule.** `PLAN.md:2520-2523` — two consecutive rounds of blocking findings, or one net-negative
round, is a hard stop and escalation, not a third round.

**Concurrency that costs nothing.** DEBT-4's doc closure for P6a (repair the "four-line"/"eleven-line"
claim to nine lines at `manager.rs:1342-1350`; add the 24-caller census including the 20 `teardown.rs`
sites and the `manager.rs:184` non-reap case; record the `ROW_REMOVAL_EPOCH_BUMPS` ratchet; name #540's
x86 evidence gap; re-derive the eleven-suite inventory) touches only `PLAN.md`/`DESIGN.md` and should
run **alongside** T3-S, not after it.

---

## 4. Per-phase readiness verdicts

| Phase | Verdict | What stands between it and a build |
|---|---|---|
| **P6a** | **needs-doc-repair** (now: not small — 24-caller/32-lookup census) | DEBT-4 closure text at the corrected figures, the `manager.rs:184` + 20-`teardown.rs`-site cases, the eleven-suite re-derivation, and **#540 named as an unresolved x86 gate-mode gap, not re-pin work**. All in-edges satisfied. |
| **P6b** | **blocked-on-decision** | DEBT-1 needs an **operator ruling** (close the window vs. accept at-most-once). Also blocked on P6a landing. |
| **P7** | **needs-design** (largest un-designed item) | DEBT-2 rejects the ratified mechanism outright; needs a per-`(row, fd)` replay token designed and pre-checked, plus a `dup`-then-close-both gate workload. |
| **P8** | **blocked-on-phase** | Three of five in-edges unbuilt (P6b, P7). DEBT-7 needs a race-free per-PID correlation mechanism. OQ-9 unstarted. OQ-5 arguably discharged. |
| **P9** | **blocked-on-phase + needs-doc-repair** | P8 unbuilt. DEBT-3/#580 must be re-derived to the corrected inventory (tenth primitive, closed futex item, surviving dead pair); ratchet must see state writes and non-`pub` definitions; **name #560**. |
| **P10a** | **blocked-on-phase** | P9. Watch #603 and #493 on this exact surface. |
| **P10b** | **blocked-on-phase** | P9. |
| **P10c** | **blocked-on-phase** | P9. Healthiest anchors in the remaining set (uniform +6-line drift, all seven intact). |
| **P10d** | **blocked-on-phase** | P9 + P10a/b/c. #491's completion point. |
| **P11** | **blocked-on-phase** (owns no debt) | P6b, P9, P10d. Scope gap: fourth `Process::terminate` caller at `manager.rs:1408` unnamed. **Name #492, #511.** |
| **P12** | **blocked-on-phase** | P8, P11. P5b satisfied. DEBT-6 repaired in text; needs its three negative gates built. |

---

## 5. The gate matrix as it stands today (post-#614), and the arithmetic that decides the option

This section supersedes `PLAN.md:355-482` where the two disagree; the scripts are the truth.

1. **Zero-warning builds.** Aarch64 kernel target is `aarch64-breenix-kernel.json` (soft-float)
   without exception — the hard-float `aarch64-breenix.json` is userspace-only and re-arms #528.
2. **#528 NEON guard** now runs in all four aarch64 gates; #614 closed the gap where the kernel-merge
   gate was the only one without it.
3. **Kernel-swap preflight (#611).** `require_boot_tests_kernel()` (defined
   `run-aarch64-boot-test-strict.sh:68`, invoked `:93`; SS-gate equivalent defined `:144`, invoked
   `:169`) greps the booted ELF for five `boot_tests`-only markers and refuses a wrong-profile kernel.
4. **Strand pins in the strict gate** (`:30-32`): `FUTEX_HANDOFF_ORACLE_PATTERN`,
   `SCHED_STRAND_ORACLE_PATTERN`, `STRAND_INJECT_ORACLE_PATTERN`.
5. **Strict gate scoring is its own stop condition** (#599's repair, #614) — `#599 is still filed OPEN`
   and should be closed against `18dcb2ef`/`96d429a3`.
6. **aarch64 init service-sequence gate**: default `BOOTS=25` × `PROFILE=both` = 50 boots. Ten
   signature buckets, including **`589`, keyed to a now-CLOSED issue and still excluded from
   FAIL** (R2 — must be retired/re-keyed as part of T3-S, not left as-is). Run-wide #609 rate ceiling
   `max(1, ceil(0.06×boots))` = 3 at default 50; exceeding it FAILS the run.
7. **Production-profile gate.** Negative control for #584/B1.
8. **x86_64.** Runs on beast only; ten custody counter literals pinned in `run-x86-boot-tests.sh`.
   **#564** (x86 gate wrapper untracked, never repacks the userspace disk — can boot stale binaries)
   and **#567** (x86 kernel-thread resume corrupts saved context pre-userspace) are confirmed OPEN and
   named nowhere in either prior pass; this gate is not authoritative until they are disclosed
   alongside it.
9. **Parallels.** 3× mandatory, fresh epoch-named VM, stop every VM afterward.
10. **Structural ratchets: eleven suites, not ten** (`strand_handoff_structure.rs` new, `e5d47c81`).
    PLAN's "256 tests" predates #604/#614 and needs re-derivation.
11. **Flake law (R17).** Pre-adjudicated: **#555** (~1%, ≤2 retries), **#536** (`timer_delay`),
    **#576** (EL1 INSTRUCTION_ABORT, `FAR=ELR=0x0 ESR=0x86000005`), **#586** (starved wake-loss),
    **#609** (by coordinator ruling R30, field-signature-bounded). **#589 is CLOSED and must be
    removed from any "open/tolerated" framing wherever it still appears in scripts or docs (R2).**
    #612/#613 are filed and bucketed but not tolerated.
12. **What the matrix cannot do today, corrected and widened.** The strict gate cannot reach 100%
    while **#609 and #576 are both open** — T3-S fixes #609 but not #576 (R1). The SS gate's GREEN
    rate is not a gate while #576 intercepts boots and the stale `589` bucket masks its own signature
    drift (R2). The aarch64 full-system test's Phase 2 can never pass headless while #593 stands.
    **#592 is red on main today** (toolchain-pin override in `kernel_build_test`) and is not
    represented anywhere in the matrix's own self-description. **#598** is a live, reproduced (1/25)
    flake inside the prod-profile gate item this document otherwise presents as sound.

### The arithmetic (reconciled)

`docker/qemu/run-aarch64-boot-test-strict.sh` requires 100% (`ITERATIONS=20` default, `:19`, `:376`,
`exit 1` on any failure `:405-421`), and deliberately carries **no #609 tolerance**
(`589-ROUND2-PARTITION-2026-08-20.md:206-209`): *"The strict gate is deliberately not given this arm.
It is the kernel-merge gate and requires 100%; #576 already hard-fails it despite being
pre-adjudicated."*

- **Today (neither #609 nor #607 fixed):** clean-run probability ≈ `0.97²⁰ × (79/80)²⁰ ≈ 0.54 × 0.78 ≈
  0.42` — roughly three in five merge attempts draw a red the phase didn't cause.
- **After T3-S (#609, #607 fixed; #576 still open, not in T3-S scope):** clean-run probability ≈
  `(79/80)²⁰ ≈ 0.78` — roughly **one in 4.5** merge attempts still draws a #576 red. This is the
  reachable ceiling; it is **not** a "clean verdict." Improvement over today is **0.78/0.42 ≈ 1.84×**,
  not the 2.4× first computed against a 100%-clean baseline that #576 alone rules out.
- **T3-S as a 4-PR sequence** (R8): clearing all four batteries clean end-to-end on first attempt, even
  after #609/#607 land, is `0.78⁴ ≈ 0.37` while #576 is still open across the sequence.

That cost is paid by every remaining phase and worst by **P6a**, which changes retention by
construction (`PLAN.md:1789`) and is the phase whose own genuine reds are hardest to distinguish from
ambient ones — now against a 24-caller, not 4-caller, surface.

---

## 6. DECISION — the operator's options

**No option below is authorized. This section exists to be ratified, not executed.**

### Option A — bounded scheduler-family tranche T3-S (now 4 PRs), then P6a

- **Scope.** (0) retire/re-key the closed-#589 classifier bucket → (1) #609 → (2) #607 → (3)
  #580/DEBT-3 re-derivation + `pub(crate)` ratchet fix + dead-pair deletion. #605 declared stretch.
  Concurrently, doc-only: P6a's DEBT-4 closure at the corrected 24-caller/32-lookup figures, the
  `manager.rs:184` + 20-`teardown.rs`-site cases, the eleven-suite re-derivation, and #540 named as an
  x86 evidence gap. Hard stop per `PLAN.md:2520-2523`.
- **Risk.** A second foundation detour; #607's fix has a measured record of trading a hang for a
  crash; #609 has no reproducer beyond a ~3% field rate and needs a 100+-boot soak to prove fixed;
  four independent-revert-story PRs means four gate batteries, not one.
- **Expected yield, corrected.** The kernel-merge gate's red rate falls from ~58% to ~22% — a **1.84×**
  improvement, **not a clean verdict**: #576 remains an open, unscoped exposure through the whole
  tranche unless the operator separately rules on it (fold it into T3-S, or accept an adjudicated #576
  arm on the strict gate). P9's ratchet stops being provably false. P6a starts with its (now
  accurately sized) debt closed and its gate readable, modulo #540's x86 gap.

### Option B — ledger-in-order: DEBT-4 doc closure + pre-check, then P6a immediately

- **Scope.** Repair DEBT-4 text and stale inventories at the corrected figures, run the one
  adversarial pre-check the ratification model requires, build P6a. Scheduler family stays filed and
  tolerated (including the stale #589 bucket, unless retirement is pulled forward regardless of
  option — recommended either way, see R2).
- **Risk.** Kernel-merge-gate clean-run probability stays at ~42% (today's #609-unfixed baseline);
  P6a is the retention-changing phase where distinguishing its own reds from ambient #609/#576 ones is
  hardest, against a 24-caller surface, not the 4-caller one first assumed; #583/#546's pinned residual
  literals move under P6a and must be re-derived per-delta against a noisy baseline; #540's x86
  evidence gap is unaddressed either way.
- **Expected yield.** Ledger row 8 lands and P6b unblocks — real forward progress on the ratified
  course, bought at a materially higher per-attempt gate cost than Option A (~42% vs ~78-times-N clean
  rate) and against a corrected, larger blast radius.

### Option C — P11-first reordering (rejected, recorded so it is not re-proposed)

P11 owns no debt, superficially attractive, but not available: `PLAN.md:558` (`P10d → P11`) exists
because P11 deletes `Process::terminate` while P9's legacy remote-mark arm still calls it; P11 also
depends on P6b's ledger (`:535`). Reordering is permitted only where the graph has no edge (`:584`);
here it has four.

### What needs the operator regardless of which option is chosen

1. **DEBT-1's ruling (P6b, pre-existing).** Close the `Report` `started==1 && finished==0` window with
   a recoverable record step, or ship it as the round's single explicitly-accepted at-most-once
   obligation. P6b cannot be scoped until this is decided.
2. **#576's disposition inside whichever option is chosen (new, from R1).** Fold #576 into T3-S scope,
   accept an adjudicated #576 arm on the strict gate, or explicitly accept the ~22% residual red rate
   as the operating ceiling for this tranche. Silence on this point is what produced the "clean
   verdict" overstatement corrected in §0/§5 — it needs an explicit answer, not a default.
3. **The closed-#589 classifier bucket (new, from R2).** Retire or re-key it before or alongside
   whichever option is chosen; it is a known-blind gate today independent of which build proceeds.

---

**Reconciliation provenance.** First-pass draft: `TRANCHE3-DRAFT.md` (scratchpad). Adversarial pass:
`t3-adversary.md` (scratchpad), which independently re-derived ~60 line citations from the draft
(finding 3 wrong: R5) and raised five further findings (R1-R4, R6-R8) not present in the draft. Every
citation in R1-R8 was re-verified a third time against `main`@`c0820bad` during the writing of this
document (§0 table, "Reconciled against tree" column). No claim in this document rests on a single
unverified pass.
