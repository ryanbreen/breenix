# Teardown Unification — PHASED IMPLEMENTATION PLAN v3 (revised against two ratification passes)

Companion to `teardown-unification-DESIGN-v3.md`. Design-only: nothing below has been implemented and
no gate result is claimed.

**Status:** **Tranche-ratified document. Tranche 1 (P0+P1+P2): COMPLETE — merged to `main`. P2 (SPINE-1, #491's live UAF) shipped via PR #515, merge commit `6003c7a6758a51c4f2092f8a1e3a502432273795`; exit_kick_protocol_gate + fork_exit_defer_reclaim_pairing_test deterministic 100/100, 0 fault markers, beast x86 3/3. Tranche 2 (P3 + P5a; P4 dissolved, P5b held): RE-RATIFIED by the operator on 2026-08-16 — see §0.0. Later phases: design-debt register applies; sections may change before their tranche ratifies.**

**Base:** `main` @ `eebc8868` (docs re-verified at `main` @ `c9efdcc7`; re-verified for v3 at
`main` @ `985881a6`; **tranche-2 sections re-anchored for v3.3 at `main` @ `2c7b8798`**, 2026-08-16 —
every file:line in the tranche-2 phases, the DEBT-3/DEBT-4 register rows and the standard gate was
re-read out of the tree at that SHA; the drifted set is tabulated in DESIGN §0.3.1).
**Issues:** #491 (spine), #464, #471.

---

## DESIGN-DEBT REGISTER

*(New in the v3 tranche pass. This section is normative: it is the complete list of open items that
these documents do **not** close, each bound to the phase that owns its closure.)*

**Why this exists.** Two full ratification passes and two pre-checks refused this design as a whole
document. The whole-document bar is replaced by **tranche ratification**: a tranche of phases is
submitted, pre-checked and ratified on its own, and the items a later phase owns are recorded here as
numbered debts instead of blocking the tranche in front of them. Nothing is being waved through — a
debt is an obligation with a named owner and a written acceptance condition, and the rule below makes
it un-skippable.

> **RULE (binding).** **A tranche containing a debt's owner phase CANNOT be ratified until that
> debt's closure is written into these documents and has passed a pre-check.** Writing the closure is
> not enough; it must survive the same adversarial pre-check that produced the debt. A tranche may be
> submitted while debts owned by *later* phases remain open, and only while that is true.
>
> **Tranche 1 = P0 + P1 + P2 (ratified, merged) and Tranche 2 = P3 + P5a (re-ratified 2026-08-16;
> P5b held, P4 dissolved) each own none of the seven debts below** (their owners are P6a, P6b, P7,
> P8, P9, P10 and P12), which is precisely why each could be submitted while the register stands open.

| Debt | Owner phase | What must be true before that phase's tranche can ratify | Source finding |
|---|---|---|---|
| **DEBT-1 — `Report` exactly-once is not achieved; it degrades to at-most-once in one window** | **P6b** | Two things, together. **(a) The R-19 window.** When an effect marker reads `started == 1 && finished == 0`, T4 cannot tell whether `record_exit` landed; the current ruling is `→ Completed` + `LEDGER_EFFECT_AMBIGUOUS{report}`, i.e. a *possibly missing* report. P6b must either close the window with a mechanism that makes the record step itself recoverable, **or** carry an explicit operator acceptance that this is the round's single at-most-once obligation, with the counter asserted `0` on every healthy boot and moved only by deliberate injection. A silent third option — restating exactly-once while shipping at-most-once — is what the pre-check rejected twice and is not available. **(b) `on_process_exit` split phasing.** The split into `claim_exit_slot` (pure atomics, PM-callable) + `record_exit` (reaches SERIAL via `finalize()` → `ktap::emit_summary`) must be shown not to reorder the registry-slot clear relative to the serial emission, and must land in the **same PR** as T4 — a recovery rule and the marker it reads must never be separated by a merge boundary | v3 pre-check and re-check, item **B NOT-CLOSED** (both passes, verbatim unchanged): *"R-19 permits missing report when `started==1 && finished==0`"*. DESIGN §1.6 (class-B `Report`), §6 R-19; PLAN P6b |
| **DEBT-2 — `Fds` close replay safety needs a PER-DESCRIPTOR token; the endpoint-CAS `CloseTicket` design is REJECTED** | **P7** | The v3 repair made `endpoint_hangup` idempotent by a **CAS on the shared endpoint**. The re-check's new FATAL is that live close accounting in the pipe/PTY/TCP endpoints is **per-descriptor**: a `dup`'d descriptor is a second, legitimate decrement on the *same* endpoint, and an endpoint-level CAS would suppress it — trading a double-close for a leaked endpoint that never hangs up. **P7 must not implement the CAS as specified.** It must carry a replay token that is unique to the `(row, fd)` pair being closed and that survives the unlocked window, so that a *replayed exit-close of the same descriptor* is suppressed while a *legitimate close of a different descriptor on the same endpoint* still decrements. DESIGN §1.4/§1.6/§2.5 and PLAN P6b/P7 must be rewritten to that mechanism before P7's tranche is submitted, and P7's gate must include a `dup`-then-close-both workload that fails by construction under an endpoint CAS | v3 re-check, **NEW FATAL**: *"the CloseTicket repair assumes endpoint-level CAS idempotence despite current close accounting being per-descriptor; duplicated descriptors make the CAS unsafe and would suppress legitimate closes"*; re-check item **1 NOT-CLOSED**. Also flagged by the repair pass itself as unreviewed (*"nobody has checked the current endpoints can satisfy it"*) |
| **DEBT-3 — the blocking-primitive inventory is NOT closed at nine, and the surface is WIDER than the design recorded** | **P9** | The no-new-block admission interlock is what makes the boundary-reachability classification a one-way door, and it is specified as living inside "the exact nine" primitives. **Four** live publications sit outside that set, re-verified at `main` @ `2c7b8798`: **(1)** `kernel/src/syscall/futex.rs:115` writes `thread.state = ThreadState::Blocked` **directly**, bypassing every `Scheduler::block_current*` entry point (the state is re-read at `:130`); **(2)** `kernel/src/task/scheduler.rs:2607` publishes `ThreadState::BlockedOnIO` directly — the design cited `scheduler.rs:2175-2194`, which is now `unblock`'s wake predicate, not the publication; **(3)** `kernel/src/task/kthread.rs:151` `kthread_park()` writes `Blocked` at `:183` inside `with_scheduler` with no interlock, then drops the tid from the ready queue — **not in the design at all**; **(4)** `Thread::set_blocked()` (`kernel/src/task/thread.rs:902`, `#[allow(dead_code)]`) is a differently-named mutator whose only caller is `Scheduler::block_current` (`scheduler.rs:2099`, itself `#[allow(dead_code)]`) — a dead two-level pair that publishes `Blocked` outside the inventory's naming convention, and per the repo's zero-tolerance standard a **deletion**, not an interlock target. **The existing ratchet cannot see any of these**: `tests/teardown_structure.rs:2029` pins the family by NAME (`BLOCKING_NAME_PREFIXES = ["block_current", "prepare_to_wait"]`, `pub` definitions only), so a tenth `block_current*` cannot appear unnoticed, but a direct `thread.state = ThreadState::Blocked*` write — or a mutator named anything else — is invisible to it. Before P9's tranche: each path is brought under the interlock **or** proven unreachable once a request is latched; the ratchet gains a rule that catches direct state writes and not only names; the dead `set_blocked`/`block_current` pair is deleted; P0's ratchet rule 2 (*"the blocking-primitive set is exactly the nine above"*) is restated to the corrected inventory; and DESIGN §1.5's one-way-door claim and §0.3's "inventory is declared CLOSED at four families" are restated to match | v3 pre-check and re-check, item **D NOT-CLOSED** (both passes). Surface widened and re-anchored by the 2026-08-15 tranche-2 re-ratification (§3.2), tracked as **#580**. DESIGN §0.3 (closure D), §0.3.1, §1.5; PLAN P0 rule 2, P9 |
| **DEBT-4 — the x86 reap path bypasses the tombstone gate** | **P6a** | P6a's whole claim is that a row is removed only by the **two-event join** (`reaped` ∧ `retired`, whichever writer sees the other flag set performs the removal). **`kernel/src/syscall/handlers.rs:3123`** (re-anchored at `2c7b8798`; the design said `:3101`) removes the row directly on the live x86 reap path, and its **byte-similar duplicate** at `kernel/src/syscall/wait.rs:386` does the same — so the join is not the only remover and the retention gate can pass on aarch64 while x86 still frees a row out from under an un-retired receipt. **Materially cheaper to close than when it was registered:** `ProcessManager::remove_process` is now a single four-line choke point (`manager.rs:1086-1090`) that already calls `note_process_row_removed()` → `ROW_REMOVAL_EPOCH` (`task/process_task.rs:355-357`), so **the join can be installed inside `remove_process` itself and cover both arches at once**, instead of the design's per-call-site chase. The two copies of the `complete_wait` reap block are a de-duplication seam worth taking in the same phase. (`task/process_task.rs:1807` is a third caller, but it is the `p1_row_epoch_gate` boot-test harness, not a live reap.) Before P6a's tranche: the removal routes through the join, **or** P6a's retention gate is honestly re-scoped to aarch64 with the x86 divergence named in AC-12's evidence column, a ratchet pinning both call sites by name, and a stated phase that closes it. Scoping the gate narrowly to avoid tripping on it, without naming it, is not available | v3 pre-check and re-check, item **B NOT-CLOSED** (second half, both passes): *"P6a omits the live x86 reap at `kernel/src/syscall/handlers.rs:3101`, which still removes the row directly"*; anchors and closure cost re-derived by the 2026-08-15 re-ratification (§3.2). PLAN P6a |
| **DEBT-5 — `EXIT_BLOCK_REFUSED` post-migration semantics: it is NEVER asserted to zero** | **P10** *(a/b/c/d)* | The admission interlock is **permanent**, not scaffolding. Migration changes only the fate of a victim *already* blocked; it does not change the refusal owed to an already-latched victim trying to **enter** a migrated wait — which must stay refused, or migration manufactures cancellation work and reopens a lost-wakeup window between block and cancel. Therefore: **no gate anywhere may assert `EXIT_BLOCK_REFUSED{family} == 0`**, before or after migration. `EXIT_BLOCK_REFUSED{family}` is asserted **nonzero** in P9's own admission-race test and **re-asserted nonzero** in each of P10a-d; the migration evidence is the *pair* `EXIT_LEGACY_REMOTE_MARK{family} → 0` and `EXIT_WAIT_CANCELLED{family} → nonzero`. This debt is **repaired in the v3 text**; it is registered because it is a standing guard that a later phase can silently break, and because the failure mode (a "tidy-up" that asserts the counter to zero once migration is done) reads like cleanup | v3 pre-check, MAJOR item 5: *"P10's requirement that `EXIT_BLOCK_REFUSED{family}` hits zero post-migration contradicts the permanent admission interlock"*. DESIGN §1.5, §3 AC-11; PLAN P0 counter table, P9, P10a-d |
| **DEBT-6 — P12's group-membership drop is scoped to EXTERNALLY-ORIGINATED signals only** | **P12** | The S1 group-seal check drops a fatal request when the designated init is a member of the **target group**. That drop applies to **`ExitIntent.origin == Signal` only** — sender-agnostic (a self-directed `kill(getpid())`/`raise` is still a signal and is still dropped, matching Linux's `sig_task_ignore`, which consults `SIGNAL_UNKILLABLE` and disposition and never the sender). **`ExitSyscall`** (init's own `exit_group`, or the exit of its last member) **and `FatalFault` BYPASS the membership test entirely**, so init's own exit still seals, latches and reaches the kernel-fatal panic. An unscoped check makes that panic path unreachable and the system hangs with init alive — a silent inversion of the policy. P12's gate must include the negative (a deliberate init `exit_group` still panics; a `FatalFault` injection still panics; an ordinary group kill still works) and record the unscoped-check pre-image. This debt is **repaired in the v3 text** and is registered because the failure is silent | v3 pre-check, MAJOR item 6: *"P12's group-membership signal drop isn't scoped to externally-originated signals; as written it would also suppress init's own `exit_group`, contradicting the required panic path"*. DESIGN §2.2 (End 2); PLAN P12 |
| **DEBT-7 — P0 defer/reclaim evidence is aggregate-only, not per-PID causal pairing** | **P8** | P0's `TraceCounter` substrate stores only per-CPU scalar totals, so its nonzero workload delta can prove aggregate balancing but cannot prove that process X's defer is followed by process X's reclaim. Sampling live `TRACE_BUFFERS` is not an acceptable substitute: `iter_events()` requires tracing to be disabled, and disabling every provider during a boot test drops unrelated live-boot evidence. Before P8's tranche can ratify, add a race-free, bounded correlation mechanism keyed by PID and restore the stronger gate: every one of the test's 64 child PIDs has exactly one defer followed by exactly one reclaim. The mechanism must not sample live trace rings, disable tracing or providers system-wide, or add unbounded test storage; P8 owns this because it introduces the round's real per-PID boundary-observation infrastructure | P0 observability STRIP+SIMPLIFY review round 2, S1. `tracing/providers/teardown.rs`; `tracing/buffer.rs::iter_events`; PLAN P0 gate extra 1 and P8 |

**Not in this register, and why.** Items the pre-checks raised that are *closed in the v3 text* and
carry no further obligation are recorded in the changelogs, not here: the seven-vs-nine caller count
(closed by the three-class taxonomy — DESIGN §1.7), the reversed P6a join gates, the Report-marker
phasing contradiction, and `parked_at` freshness. The design's *accepted* risks live in DESIGN §6 as
residuals R-1…R-21; a residual is a risk taken with eyes open, a **debt is an unfinished argument**,
and the two are deliberately kept in separate lists.

---

## CHANGELOG

### v3.2 gate-6 repair — accounting identity + exact per-generation storm oracle *(current revision)*

Second scoped re-ratification refusal (rat2, Codex `gpt-5.6-sol` xhigh read-only) found P2 gate item 6
NOT ADEQUATE on one self-contained accounting/oracle defect introduced by v3.1; §2.7, §2.1, and gate
item 5 were independently re-confirmed CLOSED/ADEQUATE and are untouched here. This pass changes
exactly gate item 6's text, in two places:

1. **Accounting identity double-counted displacements.** `attempts == EXIT_KICK_PUBLISHED +
   collisions_reservation_lost + collisions_displaced` double-counts every displacement, since a
   displacement is a successful commit and therefore already increments `EXIT_KICK_PUBLISHED` on its
   own. Replaced with `attempts == EXIT_KICK_PUBLISHED + collisions_reservation_lost` plus a separate,
   exact identity `EXIT_KICK_BUCKET_COLLISION == collisions_reservation_lost + collisions_displaced`.
   `collisions_reservation_lost` (rival's reservation CAS found `LOCK` set, wrote nothing) and
   `collisions_displaced` (a commit replaced an unobserved prior generation, which also advances
   `EXIT_KICK_PUBLISHED`) are now defined explicitly as distinct test-local counters.
2. **Storm oracle was publisher-coherent, not generation-coherent.** Part (c)'s `at in range_of(pid)`
   membership check cannot catch a same-publisher cross-generation mismatch (generation *N*'s `pid`
   paired with generation *N±1*'s `at`, both still inside that publisher's range). Replaced with a
   unique per-publication token written alongside `pid`/`at` under the same reservation, and every
   observation is now asserted against the exact `(generation, pid, token)` tuple recorded by its
   publisher, across the full `N >= 10_000`-iteration storm.

Nothing else moved: no phase added, split, reordered or renumbered; still 13 phases / 17 PRs; DESIGN
untouched; gate item 5 untouched.

### v3.1 tranche-1 repair — the `EXIT_KICK` slot protocol, its negative gates, and one stale API line *(prior revision)*

Codex `gpt-5.6-sol` (xhigh, read-only) refused tranche-1 ratification — **`ENDORSE: NO`** — on **one**
self-contained P2 mechanism defect. Its other verdicts stand and are **not** reopened here: the
caller-count taxonomy (P0 rule 1 / P2's call-site table) and the parked-receipt gates (P1) both read
**CLOSED**, later-debt spillover read **NO-FLAG**, and the six-item DESIGN-DEBT REGISTER is
byte-stable. This pass changes exactly three things.

| # | Finding | What this pass wrote |
|---|---|---|
| **1** | **FATAL — `EXIT_KICK` buckets were not reusable.** Publication was `seq.fetch_add(2, Release)`; `2` is even, so it can never clear bit 0. Once a bucket had been observed once — `0 → publish 2 → observe 3 → publish 5` — every later kick in it read as already observed and was **silently, permanently** lost. P2's single-victim gate against an initially empty table structurally cannot see this | **DESIGN §2.7's protocol is rewritten** (`KickSlot { pid, at, state }`; `state` = `gen` bits 63…2 / `LOCK` bit 1 / `OBSERVED` bit 0; reserve with an `Acquire` CAS installing a **fresh generation, `OBSERVED` clear**, fill under `LOCK`, commit with a `Release` store, observe by seqlock-validated sample plus a CAS that claims **that** generation). **P0's counter table and ratchet rule 4 are restated to the new protocol** (publish/commit/claim each pinned to one site; no site may clear `OBSERVED` except by installing a new generation), and **P2 gains gate item 5** — sequential reuse of one bucket by a second congruent victim, with its detection power demonstrated against the v3 publish step |
| **2** | **MAJOR — concurrent publication was not a coherent record.** `pid` and `at` were two relaxed stores with no reservation, so colliding publishers could interleave them, and the collision counter — inspecting the slot without owning it — was not guaranteed to notice | The reservation CAS makes its winner the sole writer of `pid`/`at`; collisions are counted in **two exhaustive arms** (reservation lost / unobserved record displaced), each from a position of exclusive knowledge. **P2 gains gate item 6**: a deterministic reservation-lost case, a deterministic displacement case, and a `N >= 10_000`-iteration two-CPU storm asserting zero torn pairs per observation plus an **exact** attempts-accounting identity |
| **3** | **MINOR — stale API narration** in DESIGN §2.1's Increment-1 (`with_process_manager(\|pm\| pm.exit_process(pid, -9))`), contradicting the wrapper-only custody contract this PLAN's P2 implements | Fixed in **DESIGN §2.1** only — it now names `exit_process_and_retire(pid, -9)`. **No PLAN change**: P2's scope, call-site table and revert note already spelled the wrapper correctly |

**Nothing else moved.** No phase added, split, reordered or renumbered; still **13 phases / 17 PRs**;
no dependency edge, acceptance criterion, line budget or commit-split seam changed; the DESIGN-DEBT
REGISTER is untouched. The two new gate items are test code and do not consume P2's production-line
budget.

### v3 tranche pass — acceptance model changed, three tranche-1 items closed, six debts registered *(prior revision)*

**The acceptance model changed, and that is the headline.** The whole-document ratification bar is
**replaced by TRANCHE ratification**. A tranche of phases is submitted, pre-checked and ratified on
its own; items owned by later phases are recorded as numbered debts in the **DESIGN-DEBT REGISTER**
above rather than blocking the tranche in front of them. **Tranche 1 = P0 + P1 + P2**, and it is
submitted for ratification now. The register's binding rule: *a tranche containing a debt's owner
phase cannot be ratified until that debt's closure is written and pre-checked.* Tranche 1 owns none
of the six registered debts (owners: P6a, P6b, P7, P9, P10, P12).

This pass is **surgical**. It fixes exactly the three tranche-1-relevant items the v3 pre-check and
re-check left open, adds the register and the status header, and changes nothing else.

| # | Tranche-1 item | What was wrong | What this pass wrote |
|---|---|---|---|
| **i** | **The seven-vs-nine caller count contradicted itself across the two documents** | The v3 repair correctly pinned P0's baseline at **seven** `exit_process` callers, but did not propagate: DESIGN §1.7 and PLAN P2's gate still demanded that "all NINE adapted sites call `exit_process_and_retire`", asserted as an exact set. `handle_thread_exit` (`task/process_task.rs:244`) routes its receipt through `phase1_result` and never calls the wrapper, so that gate was **unsatisfiable by construction** — the docs were half-updated | One live-verified taxonomy, stated once in **DESIGN §1.7** and mirrored verbatim in **PLAN P0 rule 1** and **P2's call-site table** (which gains a `Class` column). **Three disjoint classes: 7 `exit_process` callers + 1 new SIGKILL arm + 1 PM-nested enqueue = 9 ADAPTED SITES.** Callers and enqueue sites are never conflated again. The post-P2 gate becomes **three exact sets — 8 / 1 / 3** (`exit_process_and_retire` call sites / `exit_process_locked` callers / PM-free `enqueue_process_reclaim` sites), because one set cannot express three shapes. Verified by `git grep` at `main` @ `985881a6` |
| **ii** | **P2's victim-attributed observation was a placeholder** | Closure F gated AC-11 on per-victim-PID pairing, but `EXIT_REQUEST_OBSERVED{pid}` is written by the return-boundary hook, which does not exist until **P8**, and the live SGI receiver (`arch_impl/aarch64/exception.rs:1761-1768`) carries only an interrupt id — no pid, no batch. The docs said "the P2-era proxy described in their own gates"; the gates pointed back at the counter table. Nothing specified the mechanism, so P2's pairing gate was vacuous | **The mechanism is chosen and written** (option: specify, not downgrade). **DESIGN §2.7** gains the `EXIT_KICK` bucket table: a fixed `[KickSlot; 64]` of three atomics, **published** by `send_exit_expedite_sgi(victim_pid, batch)` before the broadcast, **consumed** by one `compare_exchange` at the peer scheduler pass that declines to dispatch the quarantined victim — lock-free, no allocation, no new per-thread field. Counters `EXIT_KICK_PUBLISHED{bucket}` / `EXIT_KICK_OBSERVED{pid}` / `EXIT_KICK_BUCKET_COLLISION` added to P0's table, declared zero until P2 and **deleted in P8**. The proxy's weaker claim is written into the gate rather than overclaimed, and its limits are **residual R-21**. *(**v3.1**: this pass's publish step, `seq.fetch_add(2, Release)`, could not clear the observed bit and made a bucket single-use; the slot protocol is rewritten and P2 gains two negative gates — see the v3.1 row above)* |
| **iii** | **Parked receipts: "fresh" was under-specified and the age backstop had no unit** | The repair said `ParkRecord` captures a "freshly captured fence", which still permits reusing the `RetirementSnapshot` the same drain cycle took at step 2 — stale by exactly the interval that caused the park. And the age arm was `now - parked_at_tick` "exceeds the park backstop", with the backstop never defined, while gate 3(c) required completion "within the stated backstop" | **`parked_at` captures a FRESH `RetirementSnapshot` taken AT PARK TIME** — never `reclaim.after_epoch`, never the cycle's earlier snapshot — with a **second negative gate** (P1 gate 5(b)) that an earlier-snapshot implementation fails while passing the first. **The age backstop gets a concrete unit: `PARK_AGE_BACKSTOP_EPOCHS = 64` SCHEDULING EPOCHS, summed over `fence_at_park.online_mask` — not wall time.** `parked_at_tick` is deleted; the key is derived from the captured fence, so `ParkRecord` carries no timestamp. P1 gate 3(c) becomes checkable (fires at a sum advance of 64, still parked at 63) |

**Everything else is byte-stable.** No phase was added, split, reordered or renumbered; the ledger is
still **13 phases / 17 PRs**; no dependency edge changed; no acceptance criterion changed except the
evidence columns of AC-10, AC-11 and AC-13, which were made consistent with the three fixes above.

### v3 — the seven required closures from the re-ratification refusal

The re-ratification of v2 refused endorsement again (`ENDORSE: NO`): conditions **1, 2, 3 and 7
NOT-CLOSED**, plus 5 new FATAL, 3 MAJOR and 1 MINOR. This revision is **targeted**: phases and text
neither pass criticised are preserved verbatim.

| Closure | What changed in this PLAN | Where |
|---|---|---|
| **A** — receipts cannot be dropped | **P2** no longer just "returns a `#[must_use]` receipt". `RetirementReceipt` becomes crate-private with no public constructor; `exit_process_locked` is `pub(crate)` with exactly **one** permitted caller; the single public `exit_process_and_retire(pid, code)` takes PM, drops the guard, then enqueues; the receipt's `Drop` re-enqueues instead of freeing. **All nine sites P2 must adapt are enumerated with their required restructuring, in three disjoint classes** *(v3 repair: nine is the count of ADAPTED sites — seven of them are `exit_process` callers; `985881a6` has seven, not nine. v3 tranche pass: class 1 = the seven callers, class 2 = the new SIGKILL arm — these eight call the wrapper — and class 3 = `handle_thread_exit`'s PM-nested enqueue, which routes its receipt through `phase1_result` and does **not** call the wrapper. The post-P2 exact sets are 8 / 1 / 3, not one set of nine)* — the four aarch64 fault sites, the two x86_64 fault sites, `process::exit_current`, `handle_thread_exit`, and the new SIGKILL arm | P2 (call-site table), P0 (ratchet + `RECEIPT_DROPPED_UNRETIRED`) |
| **B** — exactly-once, really | **P6b** splits obligations into class A (effect commits in the SAME PM acquisition as `→ Completed`; `Claimed` never observable; T4 unreachable) and class B (`Fds` by single-slot row custody — *v3 repair:* the row's slot is the **sole owner** and the unlocked step gets a non-owning `CloseTicket`; `Report` by an effect-written marker with a stated winning side). **P6a** gains the **two-event join**: the row carries `reaped` and `retired`, whichever writer sees the other flag set removes the row in that same acquisition, **both orders gated nonzero** | P6a, P6b, P2 (seed) |
| **C** — drain progress | **P1** adds a pass cursor (`last_pass` stamping — no candidate selected twice in one pass) and bounded retry (`K = 3` liveness-blocker refusals park the entry on a side list under a fence built from a **`RetirementSnapshot` taken at park time**, unparked by a three-armed disjunction — epoch / `ROW_REMOVAL_EPOCH` bump / age backstop of **64 scheduling epochs** summed over the captured mask; *v3 repair + v3 tranche pass*). Stated as an exclusion rule, **not** a cap | P1, P0 (park counters) |
| **D** — wait-migration soundness | **P9** ships the **no-new-block admission interlock** inside all nine blocking primitives (latched request ⇒ refuse to block ⇒ `EINTR`), making the reachability classification a one-way door. **`BlockedOnSignal` (`pause`/`sigsuspend`) becomes its own subphase P10c**; the legacy-arm deletion moves to **P10d**. The missing hard edges are added to the graph | P9, P10a-d, §0 graph |
| **E** — init group-kill hole | **P5** refuses `sys_clone` publication into the designated init's thread group (`EINVAL`, deliberate); **P12**'s drop check tests **designated-init membership of the whole target group**, not just the target row | P5, P12, P0 (ratchet on the single `thread_group_id` write site) |
| **F** — SGI evidence attribution | `EXIT_SGI_SENT` moves off the two generic scheduler send sites onto a teardown-only `send_exit_expedite_sgi(victim_pid, batch)` introduced in P2, is **declared zero until P2** in P0, and is gated by **per-victim-PID pairing** with `EXIT_REQUEST_OBSERVED{pid}` plus a measured send→observe interval | P0 table, P2, P9 |
| **G** — changelog honesty | Condition 7 is restated as its real content — *close 1-6 and obtain a NEW ratification pass before implementation* — and tracked as an open gate; the OQ-1 adoption is recorded separately as a coordinator decision | this changelog's cond-7 row, §2 |

**Honest count change:** adding the omitted wait family makes the ledger **13 numbered phases /
17 PRs** (was 16). The dependency graph, PR ledger, stop-point table and OQ-8 are updated together.

### v3 repair — seven gaps closed against the v3 pre-check *(this revision; nothing else changed)*

A pre-implementation check of v3 found two FATAL and five MAJOR defects **inside the v3 closures
themselves**. Each is repaired in place and marked "v3 repair" at the point of change; no phase, gate
or edge the check did not name has been touched, and **the ledger is still 13 phases / 17 PRs** — no
repair adds or splits a PR.

| # | Sev | Defect in v3 as written | Repair in this PLAN | Phases touched |
|---|---|---|---|---|
| 1 | **FATAL** | **`Fds` custody is impossible as specified.** `take_next_for_exit` had to *retain* the descriptor in the row's `fd_in_flight` slot (the recovery marker) **and** return that same value for an unlocked close. A slot cannot do both; cloning to satisfy both is exactly the second copy the "double close is unrepresentable" argument rests on not existing, and reopens double-close / endpoint-refcount corruption | The exit-close API becomes three calls: `begin_fd_close` **[PM]** takes into the row's slot (sole owner) and mints a **non-owning `CloseTicket`** (endpoint-handle clone, no close operation on the type); `endpoint_hangup` **[no lock]** does the only lock-needing step and is idempotent by CAS (a P7 obligation, gated); `finish_fd_close` **[PM]** drops the owning descriptor and clears the slot in one acquisition. The descriptor never leaves the row | **P7** (API), **P6b** (class-B bullet + gate 3) |
| 2 | **FATAL** | **Report-marker phasing self-contradicts.** DESIGN §1.6 said P2 ships `claim_exit_slot`/`record_exit` "from day one"; P0 rule 5 and P6b keep `on_process_exit` intact until P6b. P2 would ship a class-B obligation *claiming* a recovery marker it does not have | This PLAN's sequencing holds and the design was corrected to it. **P2 ships the seed's shape (T1/T2/T3, never a bool) and no marker**, because **T4 does not exist until P6b** — P6a's join runs a vacuously-true ledger term and performs no recovery. Exactly-once at P2 rests on the sole-redeemer invariant; the single lost-report window equals `main`'s behaviour today and is declared. **P6b introduces T4 and the btrt split in the same PR** | **P2** (scope + new gate 4: `LEDGER_CLAIM_ORPHANED == 0`), **P6b**, **P0** rule 5 unchanged |
| 3 | MAJOR | **P0's "exact nine `exit_process` callers" ratchet is false on `985881a6`** — a grep finds **seven**; `manager.rs:1152` and `process_task.rs:244` are `enqueue_process_reclaim` sites (one of them inside `exit_process`'s own body). The baseline ratchet could not pass | P0 rule 1 pins **seven** `exit_process` call sites; the two enqueue sites stay on the existing v2 `enqueue_process_reclaim` ratchet set and `signal.rs:162` on the `\.terminate\(` allowlist. "Nine" is restated as the count of sites **P2 adapts** (7 callers + 1 enqueue + the new SIGKILL arm), which is unchanged. Gate 6's broken variant becomes an **eighth** caller, plus a **third** enqueue site | **P0** (rule 1, gate 6), changelog closure-A row |
| 4 | MAJOR | **P6a's two join gates are reversed.** The counter incremented in `reap()` is `{reap_second}` (retirement landed first); the one in `retire()` is `{retire_second}`. Prompt-reap-before-grace therefore drives `{retire_second}` and delayed-reap-after-retirement drives `{reap_second}` — the opposite of what the gates asserted, so neither could pass | Gates (f) and (g) swapped onto the counters the pseudocode actually increments, with the "who is second" reasoning written into each. Both still asserted nonzero in one run | **P6a** gates (f)/(g) |
| 5 | MAJOR | **"`EXIT_BLOCK_REFUSED{family}` reaches zero post-migration" contradicts the permanent admission interlock** (and contradicts the same sentence's "not redundant after migration"): an already-latched victim must still be refused admission to a **migrated** family | The interlock is permanent; its counter is asserted **nonzero before and after** migration, and P9's admission-race test is re-run unchanged in each P10x. The migration evidence becomes the pair `EXIT_LEGACY_REMOTE_MARK{family}` → 0 and the **new** `EXIT_WAIT_CANCELLED{family}` → nonzero | **P0** counter table (+1 counter), **P9**, **P10a-d** |
| 6 | MAJOR | **P12's group-membership drop is unscoped** — as written it drops *any* request whose target group contains init, including init's **own `exit_group`**, making the kernel-fatal panic path unreachable and inverting the policy | `ExitIntent` carries an explicit `origin`; the drop applies to **`Signal` only** (sender-agnostic — Linux-faithful, so init's self-`kill` is dropped too). `ExitSyscall` and `FatalFault` bypass the membership test entirely. Ratchet pins that `origin` is set at every construction site and tested explicitly | **P12** (scope bullet + new gate 4) |
| 7 | MAJOR | **`parked_at` freshness unspecified and the unpark trigger incomplete.** Reusing the receipt's retirement fence makes the unpark predicate true at the instant of parking (step 1 only selects entries whose fence has elapsed) ⇒ immediate unpark ⇒ the livelock returns. And `blocked_live_row` can clear under PM with **no** scheduling-epoch advance anywhere, so an epoch-only unpark can strand an entry | `ParkRecord` captures a fence built from a **`RetirementSnapshot` taken AT PARK TIME** (`RECLAIM_PARK_IMMEDIATE_UNPARK` asserted 0, plus two negative gates). Unpark becomes a **three-armed disjunction** — epoch / `ROW_REMOVAL_EPOCH` bump (one relaxed increment inside the PM acquisition that already removes the row) / age backstop — swept at the head of every drain. The age arm only ever adds work, so it is not a cap. *(v3 tranche pass: "fresh" specified as a fresh **snapshot**, ruling out reuse of the cycle's earlier one; and the age backstop given its unit — `PARK_AGE_BACKSTOP_EPOCHS = 64` scheduling epochs summed over the captured mask, never wall time.)* | **P0** counter table, **P1** (cycle, bullet 2, gates 3 & 5, revert) |

### v2 deltas against the first ratification's conditions *(retained)*

The first Codex Sol ratification refused endorsement (`ENDORSE: NO`) with 2 FATAL seams, 7 MAJOR,
1 MINOR and 7 conditions. Phase labels in this table are **v2 labels**: the wait families are now
P10a/b/c/d, so "P10c empties the allowlist" reads **P10d** today, and "16 PRs" reads **17**.

| Cond | What changed in this PLAN | Where |
|---|---|---|
| **1** | **Phase reorder.** Old P10 (request-only scheduler publication + victim-owned SIGKILL commit suppression + group cutover) becomes **P9**; old P9a/b/c (killable-wait families) become **P10a/b/c**. P9 is independently implementable against P2 via the named `exit_request_is_boundary_reachable()` predicate with a live, counted legacy remote-mark arm; each P10x migrates one family; **P10c empties the allowlist and deletes the arm**, which is where #491 becomes complete. Dependency graph fully re-derived, including the **missing `P3 → P8` edge** | §0 graph, P9, P10a/b/c, §1 |
| **2** | **New P6a** (reap/tombstone retention gate) ships **before** **P6b** (the ledger), because the `Resources` obligation outlives reap while today's `waitpid` physically removes the row. P6b's bits become the four-state `Absent/Pending/Claimed{claimer}/Completed` machine with PM as sole serializer | P6a, P6b |
| **3** | **P1** restructures the reclaim drain to *detach → drop queue lock → prove → free-or-reinsert*; the under-queue-lock predicate stays lock-free. **P2** changes `exit_process` to return a `#[must_use] RetirementReceipt` and converts **both** existing PM-nested `enqueue_process_reclaim` sites | P1, P2 |
| **4** | **P8** now states the hook-activation control flow explicitly (the hook is the *only* entry to `do_exit_current`, so normal exit exercises it in this PR), plus the tombstone and one-at-a-time FD-acquisition control flow Codex's P8 note called for | P8 |
| **5** | **P11** deletes `DeliverResult::Terminated` **and** its caller-side parent-notification action in the same PR, replacing it with intent-only `DeliverResult::FatalIntent` | P11 |
| **6** | **P0** names every existing teardown write-site (file:line) each counter attaches to, marks which counters are legitimately zero until a later phase, and adds a **nonzero aggregate-delta** defer/reclaim test. The stronger per-PID causal claim originally written here is not supported by P0's aggregate counters and is now registered as **DEBT-7**, owned by P8. The **honest PR count is 13 numbered phases / 16 PRs**, enumerated below. The false "P3/P4/P5 are file-disjoint from P2, so they can merge in parallel" claim is **deleted** — the file lists overlap and all phases merge sequentially | P0, §0 PR ledger, §0 graph, DEBT-7 |
| **7** | *(corrected in v3 — closure G)* The real condition 7 is **"close conditions 1-6 and obtain a NEW ratification pass before implementation begins"**. v2 mislabelled this row as the OQ-1 decision. It is **OPEN by construction** and is closed only when a fresh pass returns `ENDORSE: YES`. *(v3 tranche pass: the gate is now **tranche-scoped** — condition 7 is discharged per tranche, not once for the whole document. Tranche 1 (P0+P1+P2) is submitted; P0/P1/P2 are cleared for build when **that tranche's** pass returns `ENDORSE: YES`, and every later phase stays uncleared until its own tranche ratifies, with the DESIGN-DEBT REGISTER's rule gating any tranche that contains a debt owner. The condition itself is not weakened — it is applied more times, not fewer.)* | §2 ratification gate, **DESIGN-DEBT REGISTER** |

**Coordinator decision recorded separately (NOT a ratification condition — closure G).** **P12**
implements the coordinator-adopted Linux-faithful protected-init policy: user-originated default-fatal
signals to the designated init with no handler are **silently dropped and the send returns 0**. All
`EPERM` language and its test assertion are removed. *(P12.)*

**Unchanged from v1:** the five phasing rules (with one refinement noted in rule 2), the standard gate,
P3, P4, P5, P7 (except an honesty note), the merge discipline, and the round-level stop rule.

**Unchanged from v2** (the re-ratification closed conditions 4, 5 and 6 and criticised nothing here):
the phasing contract and standard gate, P3, P4, P7, P8's hook-activation control flow, P11's
intent-only delivery, and P0's counter-to-write-site wiring except the one `EXIT_SGI_SENT` row that
closure F re-wires.

---

## 0.0 Ratification record

| Tranche | Phases | Status |
|---|---|---|
| **Tranche 1** | P0 + P1 + P2 | **Ratified** (v3.1 pass, `ENDORSE: YES`) — **COMPLETE, merged to `main`** |
| **Tranche 2** | **P3** (exec detach + clone/exec admission + creation-path parity) + **P5a** (init identity) | **RE-RATIFIED by the operator, 2026-08-16**, against `docs/planning/teardown-unification/P3-RERATIFICATION-2026-08-15.md` (assessed at `main` @ `1db23de0`; this repair re-anchored at `main` @ `2c7b8798`) |
| — | **P5b** (`sys_clone` init-group refusal) | **HELD** on **#575** — its acceptance evidence is a quiesce walk of the process map, and init does not reliably reach quiesce on the QEMU gates. Mechanism unchanged; only its gate is blocked |
| — | **P4** (kernel-stack ownership parity) | **DISSOLVED as a standalone phase.** Its surviving creation-path work folds into P3; its kernel-stack substance is now **#579** |
| **Tranche 3+** | P6a, P6b, P7, P8, P9, P10a-d, P11, P12 | **Uncleared.** Each arrives with its own tranche pass; the DESIGN-DEBT REGISTER gates any tranche containing a debt owner |

**What the 2026-08-16 ratification supersedes.** Tranche 2 was refused across **six** adversarial
rounds around 2026-08-10, after which the operator chose Option A — foundation-hardening first. Those
six verdicts are **lost**: they lived only in a session scratchpad under `/private/tmp`, that session
directory no longer exists, no `*tranche*` path survives anywhere under `/tmp` or `/private/tmp`, and
no tranche-2 verdict was ever committed to the repo. They cannot be quoted, and this document does not
pretend otherwise. The re-ratification artifact reconstructs seven probable grounds (G1–G7) from
surviving evidence and dispositions them; five are closed outright by the foundation work
(**#531/#534/#539/#542/#547/#549/#551/#557/#558/#565/#566/#570/#574/#577**), one is the size-ceiling
policy conflict closed by deletion here, and one — **#560**, no authoritative executing-thread
identity — is **open and accepted with eyes open** for this tranche (artifact §7 OQ-1, Position A):
P3's and P5a's admission decisions are made inside the PM guard on the calling path, not remotely,
and #560's failure mode is skew on a *blocking* path, which neither is.

**Sequencing prerequisite.** **#573** (a failed/never-published exec leaks the entire half-built
address space on x86) ships **before or with** P3. P3's gate asserts that `inherited_cr3` and
`thread_group_id` are preserved byte-identically on **every** exec failure; that is exactly the path
#573 leaks underneath. It is not part of tranche 2 proper — it gates and evidences on its own, scoped
in `docs/planning/470-custody/PR4-RESCOPE.md`.

**Standing obligation.** The ratified artifact and this record live in the repo, not in a scratchpad.
The `/tmp` loss above already cost this campaign one full record; PR #571 exists because the same
thing nearly happened to the #470 custody design.

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
4. **No size gate.** Operator ruling, 2026-08-11: *no line or file ceilings on fixes, ever — safety
   seams are fine, size gates never.* A PR is as large as its correct fix requires. The named split
   seams below survive as **safety** seams with a non-size firing condition, stated in rule 5.
5. **One revert story per phase**, written in the PR body before merge, and the phase's code must
   actually be revertable alone (verified by `git revert` dry run on the merge commit). **This is
   also the firing condition for every named split seam:** a seam fires when the PR in front of it
   would carry **two** revert stories — two independently revertable mechanisms in one merge commit —
   never because of a line count.

### The honest PR ledger — 13 numbered phases, **17 PRs** *(condition 6; updated by v3 closure D)*

v1 claimed "13 phases / 13 PRs" while splitting P9 into three; the first ratification counted 15 and
called the contradiction a MAJOR. v2 said 16. Adding the wait family v2 omitted (`BlockedOnSignal`)
as its own subphase makes the true figure **17 PRs**. Every one is listed; there are no others. **The
count rests on rule 5 alone** — one revert story per PR — now that rule 4's ceiling is gone: a PR
splits when it would carry two revert stories, so the ledger's shape is a function of revertability,
not of size. Tranche 2's own entries below already show the count moving for that reason and no other.

| # | PR | Phase |
|---|---|---|
| 1 | Teardown observability + call-site ratchet | P0 |
| 2 | Retirement fence + RootProof taxonomy + drain restructure **+ pass cursor / park list** | P1 |
| 3 | SPINE-1: SIGKILL stops eager-freeing **+ receipt custody across all 9 adapted sites** *(7 `exit_process` callers + 1 new SIGKILL arm + 1 PM-nested enqueue)* | P2 |
| 4 | exec detach + clone/exec admission **+ creation-path scheduler-registration parity** *(P4's surviving half, folded in)* | P3 |
| — | *(Kernel-stack ownership parity — **dissolved**; substance is #579, see §0.0 and the Phase 4 section)* | ~~P4~~ |
| 5 | Runtime init designation — **identity only** | **P5a** |
| 5b | Init-group clone refusal | **P5b** — **HELD on #575** |
| 6 | Reap/tombstone retention gate **+ two-event join** | **P6a** |
| 7 | Exactly-once ledger (class A/B obligations + effect markers) | **P6b** |
| 8 | FD closure leaves the PM lock | P7 |
| 9 | Victim-owned `do_exit_current` + boundary hook | P8 |
| 10 | Request-only scheduler termination + group cutover **+ no-new-block interlock** | **P9** (was P10) |
| 11 | Killable wait: futex | **P10a** (was P9a) |
| 12 | Killable wait: `WaitQueueHead` + stdin/TTY readers | **P10b** (was P9b) |
| 13 | Killable wait: **`BlockedOnSignal` — `pause`/`sigsuspend`** *(NEW in v3 — closure D)* | **P10c** |
| 14 | Killable wait: child-wait + timer/nanosleep + completion/I-O; **delete the legacy arm** | **P10d** (was P9c/P10c) |
| 15 | Fatal-signal + fault convergence (intent-only delivery) | P11 |
| 16 | Init death policy **+ group-membership drop check** | P12 |

> **17 PRs is now 16 in the ledger plus P5b, which is held.** P4's dissolution removes one entry and
> P5's split adds one back; #573 is sequenced ahead of P3 but is not a tranche-2 PR (it is PR-4b of
> #470, gated and evidenced on its own). Any further movement in this count is a rule-5 split, and
> the PR that splits says so in its body.

### Standard gate — run on EVERY phase, no exceptions

1. **Zero-warning builds** (all three configs; the grep must produce no output):
   - `cargo build --release --features testing,external_test_bins --bin qemu-uefi` (x86_64)
   - `cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
   - the aarch64 `ec0_fault_inject` config

   > **The aarch64 kernel target is `aarch64-breenix-kernel.json` — soft-float — in every gate
   > command, without exception.** `aarch64-breenix.json` is the *userspace* hard-float target;
   > building the kernel with it re-arms **#528** (compiler-emitted NEON in kernel `.text` before the
   > FPU trap is configured) at roughly 1-in-600 boots and produces false DATA_ABORTs that read as
   > branch defects. Throwaway gate scripts did exactly this once. Every aarch64 gate therefore also
   > runs `scripts/check-kernel-no-neon.sh` against the **kernel ELF it actually booted** — the guard
   > objdumps every `.text*` section and fails on any non-allowlisted FP/SIMD load or store
   > (`scripts/kernel-neon-allowlist.txt` is intentionally empty).
2. **Boot/regression gates — the current matrix.** There is no GitHub Actions CI in this repo
   (`.github/workflows` does not exist); every gate below is run explicitly and its output read.
   - **x86_64 userspace verdict:** `docker/qemu/run-boot-parallel.sh` and
     `docker/qemu/run-x86-boot-tests.sh` both score their serial logs through
     **`scripts/x86-gate-verdict.sh`**, invoked with `EXPECTED_EXITS=<profile floor>`. The verdict
     requires **all** of: `USERSPACE TEST COMPLETE` present; a parseable `TEST_TALLY:` line;
     `exited >=` the profile's `EXPECTED_EXITS` floor; `nonzero == 0`; and the `failed=[…]` set inside
     the two-way allowlist `scripts/x86-gate-allowlist.txt`. The tally is written exactly once per
     real process death at the `Process::terminate`/`terminate_minimal` choke point (fault-kills
     included), so a vanished, crashed or `exit(1)`-ing test program is a red gate by construction
     (PR #565; proven red three ways — injected `exit(1)`, injected segfault, vanished process).
   - **x86_64 custody boot-tests:** `docker/qemu/run-x86-boot-tests.sh` additionally pins the
     frame/page-table custody counter lines (`FRAME_CUSTODY_COUNTERS`, `PT_CUSTODY_COUNTERS`,
     `PT_RETIRE_COHORT`, `PT_EXEC_COHORT`) as literals. **This gate never retries a hung run** — a
     blanket retry would swallow the wake regressions it exists to catch. Its `EXPECTED_EXITS` floor
     is a consciously re-pinned literal in the script; a phase that adds or removes a userspace test
     program re-pins it in the same PR.
   - **aarch64 runtime gates:** `docker/qemu/run-aarch64-full-test.sh --rebuild --boot-tests-only`
     must reach **`[BOOT_TESTS:PASS]`** over the registered suite, plus
     `docker/qemu/run-aarch64-boot-test-strict.sh`. `[BOOT_TESTS:PASS]` is *never* accepted on its
     own from a stage-advance path — `advance_stage_marker_only`
     (`kernel/src/test_framework/executor.rs:126`) runs no tests and still emits it, carrying
     whatever `[TESTS_COMPLETE:c/t]` progress happens to stand (`0/0` when nothing has run). The
     runner script only reports those numbers; reading them is the human's job.
   - **aarch64 soaks:** **100 clean cycles** and **100 starved cycles** (host-contended) of the
     boot-test gate on `aarch64-breenix-kernel.json`, with the no-NEON guard run against the booted
     ELF. Starvation is applied by the runner, not by a committed script; the cycle count and the
     starvation method go in the PR body.
   - **Parallels:** **3× mandatory for any kernel-path merge** (`./run.sh --parallels`, fresh
     epoch-named VM, `prlctl stop --kill` between runs, serial log truncated before each boot).
     QEMU-only evidence has already missed a deterministic boot fault (#525) — Parallels gates the
     kernel path, not the other way round.
   - **QEMU concurrency capped at 4** per standing operator rule (batch 4 and 4, never 8+).

   **Pre-adjudicated flake signatures — currently exactly one.** **#555** (aarch64 softirq boot-test
   flake under host starvation, ~1%) may be retried on that exact signature, up to two times, with
   every occurrence recorded in the PR body. **Nothing else is pre-adjudicated.** The other live
   flakes — **#512** (pairing-test per-PID reclaim proof), **#536** (`timer_delay` starved false-red,
   recurs after #524), **#576** (~1/80 EL1 INSTRUCTION_ABORT during spawn), **#562** (aarch64
   `--features testing` panics 5/5 in a ksoftirqd self-test) — are **hard failures**: each is RCA'd to
   a root cause before the phase proceeds, never re-run until it disappears. The old
   `timer:timer_quantum_reset_aarch64` allowance is gone; it was closed by PR #518.
3. **Phase-specific assertions** below — every one an observed outcome (counter equality, actual
   `waitpid` status, zero fault markers). "The process was created" is never evidence, and **a
   counter equality that holds at zero is never evidence** (condition 6): every equality gate names
   the workload that drives it nonzero, or declares explicitly why that counter is legitimately zero
   until a named later phase.
4. **Parallels launcher streak:** 10 consecutive PASS with `inject_retries=0`, ≤15 attempts, fresh
   epoch-named VM via `./run.sh --parallels`, `prlctl stop --kill` after each.
5. **Soak** on any phase that changes kill timing or retention (2, 6a, 6b, 7, 8, 9, 10d, 11): 30-min
   minimum, plus the retention measurement where noted. *(v3: 10c — the `BlockedOnSignal` family — does
   not change retention, but it does change kill timing for `pause`/`sigsuspend`, so it takes the soak
   too; the list reads 2, 6a, 6b, 7, 8, 9, 10c, 10d, 11.)*
6. **Tier-1 byte-identity:** all five Tier-1 files (`syscall/handler.rs`, `syscall/time.rs`,
   `syscall/entry.asm`, `interrupts/timer.rs`, `interrupts/timer_entry.asm`) byte-identical vs `main`
   unless the operator has approved the change in writing. **Tier-2 files carry no such bar** —
   operator ruling, 2026-08-12: `context_switch.rs`, `scheduler.rs`, `interrupts/mod.rs` and the rest
   of Tier 2 are editable when the approach needs it, and a phase must not contort itself to avoid
   them; the timing-safety constraints (no logging, no page-table walks, no contending locks, no heap,
   <1000 cycles) still bind. *(The gold-master frozen-region hash gate that used to sit here was
   removed from the tree by PR #520 on operator directive; nothing named `gold-master`, `GOLD_MASTER`
   or `FROZEN` exists under `kernel/src`, `tests` or `scripts`, so gating on it was gating on nothing.)*
   **Structural ratchets take its place** as the standing anti-regression bar: `tests/teardown_structure.rs`
   (33 tests), `tests/context_restore_structure.rs` (46), `tests/exec_lock_order_structure.rs` (25),
   `tests/dma_and_log_sink_structure.rs` (4) — all census-anchored (file + enclosing-item path +
   occurrence count), no line pins. Every new tranche-2 invariant is ratcheted in one of them.
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

P4  ── DISSOLVED ──                     (lock-order half folded into P3; kernel-stack half is #579,
                                         outside this plan. The old "all stacks scheduler-owned
                                         before P8/P9" edge is NOT satisfied by any phase here —
                                         AC-8 is discharged on #579, and P8/P9 cite it there)

P5a ──> P5b                             (the refusal consults designated_init(); identity first)

P5b ──> P12                             (identity before policy — never bundled)
    ──> P9    *** NEW (closure E) ***    (P9 makes fatal signals THREAD-GROUP scoped. Today a kill
                                         aimed at a CLONE_VM sibling kills only that row; from P9 it
                                         seals and kills the whole group. If the designated init
                                         could be in that group, P9 would introduce a way to kill
                                         init that main does not have — a regression, which rule 1
                                         forbids. P5b's clone-admission refusal makes an init sibling
                                         unconstructible BEFORE the group scope exists, so P9 is
                                         strictly better than main at every commit. P12's
                                         group-membership drop check is the second end, not the
                                         first. P5b is HELD on #575, so if P9 arrives first it
                                         waits here — the edge is not negotiable)

P6a ──> P6b   *** NEW ***               (row must outlive obligations before Resources becomes
                                         row-resident — DESIGN §1.6)
P6b ──> P8                              (the victim commit claims ledger obligations)
    ──> P11                             (P11 deletes the legacy notifier and relies on the ledger)

P7  ──> P8                              (one-at-a-time FD acquisition is what the commit uses)

P8  ──> P9                              (the hook + do_exit_current are what a published request
                                         resolves to)
    ──> P12                             (init's own exit commit is a latch producer)

P9  ──> P10a, P10b, P10c, P10d   *** REORDERED (v2) ***
                                        (every wait-family PR consumes the request/wake mechanism
                                         AND the no-new-block interlock P9 ships; v1 had this
                                         edge backwards)
    ──> P11                             (the terminate() allowlist cannot reach empty until SIGKILL
                                         has left the exit_process/terminate route)

P10a ──> P10d   *** MISSING IN v2 ***   (P10d deletes exit_request_is_boundary_reachable()'s legacy
P10b ──> P10d   *** MISSING IN v2 ***    arm and asserts the allowlist EMPTY as an exact set; it
P10c ──> P10d   *** NEW (closure D) ***  cannot do that until futex, WaitQueueHead/stdin/TTY and
                                         BlockedOnSignal have each left the allowlist. Deleting the
                                         arm with any family still on it would strand that family
                                         with a published request and no producer — the exact defect
                                         condition 1 exists to prevent)

P10d ──> P11    *** MISSING IN v2 ***   (P11 deletes Process::terminate; the legacy arm is one of its
                                         live callers, so the arm must be gone first. v2's graph let
                                         P11 precede P10c, which would have deleted a function the
                                         fallback still calls)
     ──> (otherwise no successor is blocked; P10d is the completion point for #491 and AC-11)

P11 ──> P12                             (the unhandleable-fault latch producer lives here)
```

> **The three edges the re-ratification required are present, under v3's labels.** The refusal named
> them as `P10a → P10c`, `P10b → P10c`, `P10c → P11`, using v2's labels in which P10c was the
> *deleting* subphase. Closure D inserts `BlockedOnSignal` as P10c and moves the deletion to **P10d**,
> so the required edges are written above as `P10a → P10d`, `P10b → P10d`, `P10d → P11` — the same
> three constraints ("every migration precedes the deletion" and "the deletion precedes
> `Process::terminate`'s removal") — plus the fourth edge the new family creates, `P10c → P10d`, and
> a fifth the re-ratification did not name, `P5 → P9` (closure E). No edge was dropped in the
> relabel: **five arrows are added and none removed.**

**No parallel-merge path is claimed.** *(condition 6 — v1's "P3, P4, P5 can run in parallel with P2
(disjoint files)" was false and is deleted.)* The production file lists overlap materially:
`process/manager.rs` appears in P2, P3, P5a, P6a, P6b, P7, P8, P9; `task/scheduler.rs` in P1, P2, P9,
P10a/b/c; `interrupts/context_switch.rs` in P3; `syscall/clone.rs` in P3 and P5b;
`syscall/signal.rs` in P2, P5a, P9, P12; `task/process_task.rs` in P1, P2, P5a, P6a,
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
| `EXIT_FIRST_REQUESTS` / `EXIT_REPEAT_REQUESTS` | the `already_terminated` reads at `process/manager.rs:1121` and `task/process_task.rs:222` | yes (first); repeat is exercised by the P0 repeat test |
| `TEARDOWN_QUARANTINE` | `task/scheduler.rs:2599 terminate_process_threads` (five call sites: `exception.rs:768,1135,1230,1333`, and `signal.rs` from P2) | yes under the fault tests |
| `EXIT_SGI_SENT` *(re-wired in v3 — closure F)* | **NOT** the generic sends. `task/scheduler.rs:1857` is inside `send_resched_ipi` (`:1843`), which wakes **idle** CPUs, and `:1886` is inside `send_resched_ipi_to_cpu` (`:1868`), which targets the CPU that got a **newly runnable task** — neither knows a victim, so wiring them makes every ordinary wakeup satisfy a "> 0" gate. The counter is written **only** by the teardown-specific `Scheduler::send_exit_expedite_sgi(victim_pid, batch)` that P2 introduces, carrying the victim pid in the `trace_event!` payload | **declared zero until P2** — the write site does not exist before then; excluded from every gate until P2 |
| `EXIT_REQUEST_OBSERVED` *(new, closure F)* | the boundary hook's first observation of a latched request, carrying the observed pid — i.e. the paired consumer side of `EXIT_SGI_SENT{pid}` **from P8 onward** | **declared zero until P8** (the hook does not exist before then). P2 and P9 pair against the **`EXIT_KICK` bucket proxy** specified in DESIGN §2.7 — *not* against this counter, and not against an unnamed "P2-era proxy" *(v3 tranche pass: the proxy is now a specified mechanism; the earlier wording pointed at gates that pointed back at it)* |
| `EXIT_KICK_PUBLISHED{bucket}`, `EXIT_KICK_OBSERVED{pid}`, `EXIT_KICK_BUCKET_COLLISION` *(new, v3 tranche pass — closure F's P2-era observation half)* | the `[KickSlot; 64]` table in this provider (`KickSlot { pid, at, state }`, three atomics; `state` = `gen` in bits 63…2, `LOCK` in bit 1, `OBSERVED` in bit 0 — DESIGN §2.7, rewritten in v3.1). **Publish:** the teardown-only `Scheduler::send_exit_expedite_sgi(victim_pid, batch)` — **reserve** with `compare_exchange_weak(cur, ((gen+1)<<2)\|LOCK, Acquire, Relaxed)` (bounded at 4 attempts; a rival that finds `LOCK` set counts a collision and publishes nothing), **fill** `pid`/`at` Relaxed while `LOCK` is held, **commit** with `state.store((gen+1)<<2, Release)`; the only publish site, ratcheted. **Observe:** the peer scheduler pass that declines to dispatch a quarantined victim — `s1 = state.load(Acquire)` (reject if `gen == 0`, `LOCK` set, or `OBSERVED` set), sample `pid`/`at` Relaxed, re-read `s2 = state.load(Acquire)` and **discard unless `s2 == s1`**, then claim `compare_exchange(s1, s1\|OBSERVED, AcqRel, Relaxed)`; the single winner records `EXIT_KICK_OBSERVED{pid}` with the interval `now - at`. **Every publication installs a fresh generation with `OBSERVED` clear**, so a bucket is reusable indefinitely, and only the reservation winner ever writes `pid`/`at`, so the pair cannot tear. No lock, no allocation, no per-thread field | **declared zero until P2** (the table and the helper do not exist before then); **deleted in P8**, in the same PR that introduces the boundary hook and `EXIT_REQUEST_OBSERVED{pid}` — the round never carries two observation mechanisms past the phase that unifies them. Residual R-21 |
| `TEARDOWN_DEFER` | `task/process_task.rs:129 defer_process_resources` (reached from `:125 defer_live_process_resources` and `process/manager.rs:1151`) | **yes** — every aarch64 exit |
| `TEARDOWN_RECLAIM` | `task/process_task.rs:387` (`reclaim.reclaim()` inside `reclaim_deferred_process_resources`, `:375-391`) *(v3: line corrected from `:388`)* | **yes** |
| `TEARDOWN_MASKED_FRAMES_WALKED` | `task/process_task.rs:100 release_process_resources` → `cleanup_cow_frames()`, incremented only when the PM-owner instrumentation says a PM guard is live (call sites `manager.rs:1156`, `process_task.rs:247`, `:250`) | yes on `main` (that is the defect P2/P7/P11 drive to 0) |
| `FD_CLOSES_UNDER_PM` | `process/process.rs:347 close_all_fds` reached under PM via `Process::terminate` (`:284`, `:294`) from `manager.rs:1161`, `signal.rs:162`, `delivery.rs:224`, `:258`, `interrupts/context_switch.rs:1021`; plus `take_fd_entries` (`process.rs:335`) at `process_task.rs:233` *(v3: corrected from `:236`)* | yes on `main` (driven to 0 by P7) |
| `TEARDOWN_VICTIM_DIVERGENCE` / `TEARDOWN_CR3_MISS` | `process/manager.rs:1313-1335 find_process_by_cr3_mut` vs the TID-derived owner at the four EL0 sites | zero on a clean boot; **driven nonzero by the P11 `clonevm_fault_test`**, declared until then |
| `EXIT_ATTRIBUTION_UNCERTAIN` | the total-resolution-failure branch at the four EL0 sites | declared zero until injected |
| `DEFERRED_FAULT_RING_DROPPED` | `task/process_task.rs:43` — `DeferredFaultExitBuffer::push` returning `false` (caller `defer_fault_sigsegv_exit`, `:352`) | zero normally; **P0 ships a 17-deep injection that drives it nonzero**, proving the counter is real (#492's overflow is invisible today) |
| `RECLAIM_ENQUEUE_UNDER_PM` *(new, cond. 3)* | `task/process_task.rs:140 enqueue_process_reclaim`, incremented when the PM-owner instrumentation says PM is live — true today at **both** call sites (`manager.rs:1152`, `process_task.rs:244`) | **yes on `main`** — this is the pre-existing violation P2 drives to 0 |
| `PROOF_UNDER_QUEUE_LOCK` *(new, cond. 3)* | `task/process_task.rs:375-391` *(v3: span corrected)*, incremented if SCHEDULER or PM is acquired while `PENDING_PROCESS_RECLAIMS` is held | zero on `main` (both predicates are lock-free); P1 must keep it zero |
| `RECEIPT_DROPPED_UNRETIRED` *(new, closure A)* | `RetirementReceipt::drop` — the self-healing destructor that re-enqueues instead of freeing | **declared zero until P2** (the type does not exist before then); asserted 0 from P2 onward |
| `RECLAIM_PASS_SKIPPED`, `RECLAIM_PARKED`, `RECLAIM_UNPARKED{epoch\|row\|age}`, `RECLAIM_PARK_IMMEDIATE_UNPARK`, `RECLAIM_PARK_RESIDENT` *(new, closure C; split + immediate-unpark counter added by the v3 repair)* | the drain's pass-stamp check and the park/unpark transitions in `task/process_task.rs:375-391`; the `{row}` arm additionally keys on the global `ROW_REMOVAL_EPOCH` bumped by one relaxed increment inside `ProcessManager::remove_process` (`manager.rs:1086-1090`, and P6a's `remove_row`); the `{age}` arm keys on `PARK_AGE_BACKSTOP_EPOCHS = 64` **scheduling epochs** summed over `fence_at_park.online_mask` (no wall clock, no timestamp field) | **declared zero until P1** (the fields do not exist before then); P1 drives `RECLAIM_PASS_SKIPPED`/`RECLAIM_PARKED` and **each of the three `RECLAIM_UNPARKED` arms** nonzero, asserts `RECLAIM_PARK_IMMEDIATE_UNPARK == 0`, and asserts the gauge returns to 0 at quiesce having been observably nonzero mid-run |
| `LEDGER_EFFECT_AMBIGUOUS{report}` *(new, closure B)* | T4's ruling branch when `report_marker.started == 1 && finished == 0` | **declared zero until P6b**; asserted 0 on every healthy boot, driven to a known value only by deliberate injection |
| `TOMBSTONE_JOIN{reap_second}`, `TOMBSTONE_JOIN{retire_second}` *(new, closure B)* | the two removal branches of the two-event join in `ProcessManager` | **declared zero until P6a**; P6b asserts **both** nonzero in one run |
| `EXIT_BLOCK_REFUSED{family}` *(new, closure D)* | the pre-block interlock inside the nine blocking primitives: `task/scheduler.rs:1726`, `:1897`, `:1916`, `:2065`, `:2153`, `:2218`, `:2227`, `:2386`, and `task/waitqueue.rs:52` | **declared zero until P9**; P9 drives it nonzero and **each P10x re-asserts its own family's value is still nonzero** — *v3 repair:* the interlock is permanent, so this counter never reaches 0; what falls to 0 per family is `EXIT_LEGACY_REMOTE_MARK{family}` |
| `EXIT_WAIT_CANCELLED{family}` *(new, v3 repair)* | the migrated family's own cancellation path — a victim found **already blocked** in this family at request time and cancelled by its own resumed continuation. This is the counter that actually rises at migration, and it is the partner of `EXIT_LEGACY_REMOTE_MARK{family}` falling | **declared zero until P10a**; each P10x drives its own family's value nonzero |
| `INIT_FATAL_SIGNAL_DROPPED`, `INIT_FATAL_SIGNAL_DROPPED{group}` *(the group variant new, closure E)* | the disposition-time drop in `syscall/signal.rs` and S1's group-membership check | **declared zero until P12** |
| `RECLAIM_CONTEXT_VIOLATIONS`, `TEARDOWN_LOCK_ORDER_SUSPECT` | the lock-depth/owner instrumentation, **including `try_manager()`** (the r20 detector blind spot) | zero; asserted |

Ratchet (`tests/teardown_structure.rs`) asserts the exact current sets of: `\.terminate\(` /
`terminate_minimal\(` call sites; production `ProcessId::new(1)` sites (with the three
`test_userspace.rs` sites allowlisted **by name**); `terminate_process_threads` call sites;
`kernel_stack_allocation` mutation sites; **`enqueue_process_reclaim` call sites** (v2). It passes on
`main` unchanged; later phases shrink the allowlists, and any *new* bypass fails on arrival.

**v3 adds five ratchet rules**, each pinning a set that a lettered closure depends on being exactly
what it is today:

1. *(closure A; **corrected by the v3 repair**)* **`exit_process` call sites — the exact SEVEN.**
   Pinned by name at `arch_impl/aarch64/exception.rs:778`, `:1146`, `:1233`, `:1336`;
   `interrupts.rs:1429`, `:1735`; `process/mod.rs:264`.
   > **Why seven and not nine.** v3 as first written pinned "the exact nine" and listed those seven
   > plus `process/manager.rs:1152` and `task/process_task.rs:244`. Those two are
   > **`enqueue_process_reclaim` call sites, not `exit_process` callers** — `manager.rs:1152` is
   > *inside* `exit_process`'s own body and `process_task.rs:244` is inside `handle_thread_exit`.
   > A ratchet asserting nine `exit_process` call sites therefore **cannot pass on `985881a6`**,
   > where a grep finds seven. The two enqueue sites are already pinned by the v2 rule above
   > (`enqueue_process_reclaim` call sites — exactly two, both PM-nested), which is where they
   > belong; and `syscall/signal.rs:162` is pinned by the `\.terminate\(` allowlist. **"Nine" is,
   > and always was, the count of sites P2 *adapts*** — seven `exit_process` callers + one PM-nested
   > enqueue + the SIGKILL arm P2 itself introduces — and that count is unchanged in P2's table.

   From P2 the rule inverts into **three separate exact sets** *(v3 tranche pass — one set cannot
   express three different shapes, and the single-set version could not pass)*:

   | Post-P2 exact set | Value | Members |
   |---|---|---|
   | `exit_process_and_retire` call sites | **8** | the seven pinned class-1 sites above, each now naming the wrapper, **plus** `syscall/signal.rs`'s new SIGKILL arm |
   | `exit_process_locked` call sites | **1** | the wrapper, and nothing else |
   | `enqueue_process_reclaim` call sites | **3** | the wrapper's post-guard enqueue, `handle_thread_exit` phase 2, and `RetirementReceipt::drop`'s re-enqueue — all three provably PM-free; `manager.rs:1152` is deleted. *(The pre-P2 baseline pinned by the v2 rule is **2**; P2's diff is what moves it.)* |

   Plus: `RetirementReceipt` must have **zero** `pub fn new`/`pub` constructors and must not be
   nameable outside the crate; and `ProcessManager::exit_process` must have **zero** call sites in the
   public surface.

   > **`handle_thread_exit` is an adapted site that does NOT call the wrapper.** It owns its own PM
   > guard and its own two-phase shape; its receipt rides out of phase 1 through the existing
   > `phase1_result` value and is enqueued in phase 2. Any gate phrased as "all nine now call
   > `exit_process_and_retire`" is therefore unsatisfiable by construction, and no gate in these
   > documents is phrased that way. DESIGN §1.7 carries the same three-class table; the two documents
   > state it identically or one of them is wrong.

2. *(closure D)* **The blocking-primitive set — the exact nine** listed in the counter table above.
   A tenth blocking primitive added without the interlock fails CI on arrival, which is what keeps
   the reachability predicate total after P9.

   > ⚠ **DEBT-3 — this rule is written against an inventory that is not closed, and the rule's
   > NAME-based shape is itself part of the gap (#580).** Four publications are outside the nine:
   > `syscall/futex.rs:115` (direct `ThreadState::Blocked`), `task/scheduler.rs:2607` (direct
   > `BlockedOnIO`), `task/kthread.rs:151` `kthread_park()` (writes `Blocked` at `:183`), and the dead
   > `Thread::set_blocked()`/`Scheduler::block_current` pair (`thread.rs:902`, `scheduler.rs:2099`).
   > A name-family census cannot see a direct `thread.state = ThreadState::Blocked*` write at all.
   > **P9 owns the closure**, and P9's tranche cannot ratify until this rule is restated to the
   > corrected inventory. P0 may still ship the rule as-is — it is a ratchet on today's set and
   > shipping it does not make the gap worse — but it must not be read as evidence the set is
   > complete. See the DESIGN-DEBT REGISTER.

3. *(closure E)* **`thread_group_id = Some(` production write sites — exactly one**
   (`syscall/clone.rs:210`). A second one would reopen the init-sibling bypass silently.
4. *(closure F; extended by the v3 tranche pass)* **`EXIT_SGI_SENT` appears at exactly one source
   location**, and neither `send_resched_ipi` nor `send_resched_ipi_to_cpu` may reference it. **The
   `EXIT_KICK` table's publish side is pinned the same way**: `EXIT_KICK_PUBLISHED` and the
   **reserving `compare_exchange` on `state`** that publishes a slot appear at **exactly one** source
   location — inside `send_exit_expedite_sgi` — so the observation gate can never be satisfied by a
   second publisher wired somewhere convenient *(v3.1: the pinned publish step was `seq.fetch_add`,
   which no longer exists)*. The consume side — the **claiming `compare_exchange` on `state`** — is
   likewise pinned to the one scheduler site that declines to dispatch a quarantined victim. Two
   further single-site rules follow from the v3.1 protocol: `state.store(..., Release)` (the commit)
   appears **only** inside `send_exit_expedite_sgi`, and **no site anywhere may clear `OBSERVED`
   except by installing a new generation** — a bare `fetch_and`/`fetch_or` on `state` outside the two
   pinned sites fails the ratchet.
5. *(closure B)* **`btrt::on_process_exit` has exactly one call site** (`task/process_task.rs:267`),
   pinned before P6b splits it into `claim_exit_slot` + `record_exit`; after P6b, `record_exit` must
   have exactly one call site and `claim_exit_slot` must not appear outside a PM-guarded body.

**Files.** `kernel/src/tracing/providers/teardown.rs` (new), tracing registration,
`tests/teardown_structure.rs` (new). **~200 lines, 2 commits.** *(v3: the added counters are
declarations in the same new provider file and the added ratchet rules are assertions in the same new
test file, so the file count does not grow.)*

**Gate extras.** *(v2 — condition 6: no vacuous zero-baseline gate.)*
The two P0 runtime extras below are registered in the parallel boot-test framework and execute under
the aarch64 standard-gate command above: `run-aarch64-full-test.sh --rebuild --boot-tests-only`
enables `boot_tests`, runs the EarlyBoot and PostScheduler stages, and accepts only the final
`[BOOT_TESTS:PASS]` result.

1. **`fork_exit_defer_reclaim_pairing_test` (the aggregate-delta nonzero gate).** Fork and exit **64**
   children in a loop, then quiesce. Assert (a) `TEARDOWN_ENTRY{exit} >= 64`; (b) `TEARDOWN_DEFER >= 64`;
   (c) after the drain, `TEARDOWN_DEFER == TEARDOWN_RECLAIM` **at a value ≥ 64** — the equality is only
   accepted at a nonzero, workload-explained value. This proves aggregate balancing only; the current
   counters have no PID dimension and therefore cannot prove per-PID causality. **DEBT-7** assigns that
   stronger proof to P8, where real per-PID observation infrastructure enters the round.
2. Ring-overflow injection drives `DEFERRED_FAULT_RING_DROPPED` nonzero and back to a quiescent read.
3. Baseline snapshot of `TEARDOWN_MASKED_FRAMES_WALKED`, `FD_CLOSES_UNDER_PM` and
   `RECLAIM_ENQUEUE_UNDER_PM` on `main` — these are **expected nonzero** and are the pre-existing
   defects later phases drive to zero; recording the baseline is what makes those later gates meaningful.
4. `TEARDOWN_LOCK_ORDER_SUSPECT == 0`, `PROOF_UNDER_QUEUE_LOCK == 0`, and every counter has a reader
   (no write-only counters).
5. *(v3 — closure F)* **`EXIT_SGI_SENT` is declaration-only until P2 and is excluded from P0 runtime
   gates.** The structural ratchet forbids it in the generic reschedule helpers and pins its eventual
   producer to the teardown-only helper; P0 deliberately does not use equality-at-zero as evidence.
6. *(v3 — closures A, D, E)* The five new ratchet rules pass on `main` unchanged, and each is proven
   to be a real constraint by a deliberately-broken variant in the test: adding a tenth blocking
   primitive, a second `thread_group_id` write, an **eighth** `exit_process` caller *(v3 repair — the
   pinned set is seven, not nine)*, a **third** `enqueue_process_reclaim` call site, or an
   `EXIT_SGI_SENT` reference inside `send_resched_ipi` must each fail the ratchet.

**Strictly better.** Every later phase's evidence becomes a counter equality instead of a log-reading
exercise, and the bypass surface can only shrink. #492's silent drops become visible, and two
pre-existing lock violations become measurable before anyone tries to fix them. *(v3: the complete
`exit_process` caller set, the complete blocking-primitive set and the single `thread_group_id` write
site are pinned **before** any phase depends on them, so the three structural closures A, D and E
cannot be quietly undermined by an unrelated PR landing between phases.)*

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
> `reclaim_deferred_process_resources` (`task/process_task.rs:375-391`) currently evaluates
> `retirement_grace_elapsed(&reclaim.after_epoch) && !reclaim.root_is_live()` **while holding**
> `PENDING_PROCESS_RECLAIMS`. Both terms are lock-free atomic reads today, so `main` is legal — but
> `RootProof` adds *scheduler cached roots* and *live rows*, which take SCHEDULER and PM. Acquiring
> either under the queue lock is the overlapping-lock violation the design forbids, and "it's only
> until P8" is not an exemption. P1 therefore ships the cycle as:
>
> ```
> -1. UNPARK SWEEP first (parked-list lock alone): return to the live queue every parked
>     entry satisfying ANY of the three arms — epoch / ROW_REMOVAL_EPOCH bump / age
>     backstop.                                                (v3 repair, closure C)
> 0. bump PASS_ID once per drain invocation.                            (v3, closure C)
> 1. QUEUE LOCK ONLY:  scan for a candidate whose last_pass != PASS_ID, whose fence has
>                      elapsed, AND whose lock-free blockers (hardware/shadow) are clear;
>                      stamp last_pass = PASS_ID; swap_remove it into a local fixed slot.
>                      No candidate -> the pass is DONE.               -> DROP QUEUE LOCK
> 2. NO LOCK:          RetirementSnapshot (acquire fence) + hardware/shadow re-read.
> 3. SCHEDULER ONLY:   cached-root blocker.                            -> DROP
> 4. PM ONLY:          live/creating-row blocker.                      -> DROP
> 5. proof passed  -> free with NO lock held.
>    proof refused -> bump that blocker's counter; if the blocker was a LIVENESS one
>                     (cached-root or live-row) increment proof_failures, else leave it;
>                     if proof_failures == K (3): PARKED LIST LOCK ALONE -> take a NEW
>                     RetirementSnapshot AT THIS INSTANT and park with a ParkRecord holding
>                     the fence derived from it (never the receipt's own after_epoch, which
>                     step 1 already proved elapsed; and never step 2's snapshot, which
>                     predates the three refusals), the current ROW_REMOVAL_EPOCH, and
>                     age_epoch_sum_at_park = sum of that fence's epochs over its own
>                     online_mask;                     (v3 repair + v3 tranche pass)
>                     otherwise QUEUE LOCK ALONE ->
>                     re-insert (still stamped, so this pass will not re-select it).
> ```
>
> The under-queue-lock predicate is permanently restricted to lock-free reads; `PROOF_UNDER_QUEUE_LOCK`
> (P0) asserts it. A candidate detached and then refused is re-inserted or parked, never dropped — the
> re-insertion and park/unpark paths are part of this PR's tests, not a later phase's.

> **v3 (closure C) — the refusal path is given a progress argument, because v2's had none.** The
> re-ratification's third FATAL: v2 said a refused candidate is "re-inserted and rotated", but there
> is no rotation in the live code — the scan is `position()` from index 0 over the whole vector
> (`task/process_task.rs:379-383`) and the drain loops until no candidate is found. A receipt whose
> epoch and shadow checks pass but whose live-row blocker persists is therefore re-selected
> immediately and forever: a single-entry livelock in the first behaviour-preserving phase, which
> would have been shipped as "hardening". Two fields on `PendingProcessReclaim` fix it:
>
> - **`last_pass: u32` — a pass cursor.** The drain bumps a monotonic pass id once per invocation and
>   stamps every entry it selects. The under-lock scan skips entries already stamped with the current
>   pass. **No candidate can be selected twice in one pass**, so a pass terminates in at most *queue
>   length* selections regardless of what the proofs say. Counted `RECLAIM_PASS_SKIPPED`.
> - **`proof_failures: u8` + `parked: Option<ParkRecord>` — bounded retry, then park.** After
>   `K = 3` consecutive refusals attributable to a **liveness** blocker (`blocked_cached` or
>   `blocked_live_row` — the only two whose answer can change without time passing), the entry moves
>   to a side list `PARKED_PROCESS_RECLAIMS` and is **not scanned at all** until unparked.
>   Grace/hardware/shadow refusals do **not** increment `proof_failures`: those clear with time, and
>   re-checking them is the drain's whole job. Counted `RECLAIM_PARKED` /
>   `RECLAIM_UNPARKED{epoch|row|age}`, gauge `RECLAIM_PARK_RESIDENT`.
>
> **v3 repair — the park rule as first written had two defects, and both would have been shipped.**
>
> **(i) `parked_at` was never said to be fresh, and reusing the retirement fence makes the park
> inert.** The cycle said "park with `parked_at` = the captured fence" and DESIGN §1.8 said "the
> fence captured at parking" — both readable as reusing `reclaim.after_epoch`. That fence is, by
> construction, **already elapsed**: step 1 will not even select an entry whose fence has not
> elapsed. An unpark predicate keyed on it is true at the instant of parking, so the entry unparks on
> the very next sweep and the livelock returns with two extra counter bumps per cycle. The park
> therefore takes a **NEW `RetirementSnapshot` at the parking instant** and stores the fence derived
> from it — new acquire-fenced epoch read, new online mask — inside `ParkRecord` (DESIGN §1.4).
> *(v3 tranche pass: "fresh" is specified as a fresh **snapshot**, because there are two wrong
> implementations and both are "a fence" — reusing `reclaim.after_epoch`, and reusing the
> `RetirementSnapshot` step 2 of this same cycle already took. Step 2's snapshot predates the three
> refusals that caused the park, so a fence built from it is stale by exactly the interval that
> matters, and gate 5 below would pass against it while the park stayed near-inert.)* Gated by
> `RECLAIM_PARK_IMMEDIATE_UNPARK == 0`: a parked entry that unparks in the same drain invocation that
> parked it is a bug, not a race.
>
> **(ii) An all-CPU epoch advance is NOT "the one event that can change a liveness blocker's
> answer" — that is false for `blocked_live_row`.** A live row is PM-owned and disappears when the
> reap/retire join (P6a) or today's `remove_process` removes it. That can happen on another CPU with
> **no scheduling-epoch advance anywhere** — a parent calling `waitpid` while every CPU in the
> captured mask sits in WFI — so an epoch-only unpark can wait for an event that never arrives.
> Unpark becomes a **disjunction of three arms**, any one of which returns the entry to the live
> queue for a full re-proof:
>
> | Arm | Fires when | Clears |
> |---|---|---|
> | **epoch** | a `RetirementSnapshot` shows every CPU in `fence_at_park`'s captured online mask advanced its scheduling epoch | `blocked_cached` |
> | **row** | the global `ROW_REMOVAL_EPOCH` differs from `row_epoch_at_park`. Bumped by **one relaxed increment** inside the PM acquisition that already performs the row removal — no extra lock, no list walk, no knowledge of the parked list, so no ordering is inverted; the sweep reads it with no lock held | `blocked_live_row` |
> | **age** | the epochs of the CPUs in `fence_at_park.online_mask`, **summed**, have advanced by at least `PARK_AGE_BACKSTOP_EPOCHS = 64` since `age_epoch_sum_at_park` | *nothing specific* — a safety net so a parked entry is **always** eventually re-proved even if both keyed arms are missed |
>
> **The age backstop's unit: 64 SCHEDULING EPOCHS, summed over the captured mask — never wall time**
> *(v3 tranche pass; the pre-check's residual MINOR was that gate 3(c) required completion "within the
> stated backstop" while no backstop was ever stated)*. The sweep re-reads the same epoch words the
> park captured, sums them over the *same* captured mask, and fires at `sum_now - age_epoch_sum_at_park
> >= 64`. It needs no extra state (the key is derived from `fence_at_park`, so `ParkRecord` carries no
> timestamp), it is strictly weaker than the `epoch` arm (which needs *every* captured CPU to advance,
> where this accepts 64 advances anywhere in the mask), and it cannot be starved on this kernel — the
> 1 kHz tick drives a scheduler pass on every online CPU including the idle loop, so the sum is
> monotone and unbounded above. *(Informative, and deliberately not the definition: ~tens of ms at 4
> online CPUs. No gate is written against a millisecond figure.)*
>
> The sweep runs at the **head of every drain invocation** (step -1 above), fork's pre-allocation
> drain included, so a row removal is followed by a sweep at the next drain rather than needing one
> of its own. **The age arm is not a cap:** it only ever *adds* work — it makes a parked entry
> eligible again and never stops examining, skips or drops anything — which is what makes R-20 a
> bounded stall rather than a possibly-permanent one.
>
> **This is an exclusion rule, not a cap — stated explicitly because three bounded drains were
> reverted in r23 and phasing rule 1 forbids reintroducing that class.** A cap stops examining
> entries that are *ready*; this stops **re-examining an entry already examined in this pass** and
> defers entries whose blocker provably cannot have changed. Fork's pre-allocation drain keeps full
> semantics — an unpark sweep followed by one complete pass over every live entry — and **no cap
> parameter is introduced anywhere in this PR**. The parked list is a leaf lock taken alone, never
> with PM, SCHEDULER or the live queue held (DESIGN §4.3).

**Files.** `kernel/src/task/scheduler.rs`, `kernel/src/task/process_task.rs`,
`kernel/src/arch_impl/aarch64/ttbr0.rs`, tracing provider, targeted tests. **3 commits; the named split seam (rule 5) is
{fence + RootProof taxonomy} / {drain restructure + pass cursor + park list} — it fires if the two
would land as two revert stories in one merge commit.**

**Gate extras.** Unit injection with a zero online mask refuses reclaim; wrap-safe epoch comparison
test; the existing epoch-before-stack-liveness ordering becomes a structural test; every refusal in a
normal boot is attributable to exactly one blocker. **v2:** `PROOF_UNDER_QUEUE_LOCK == 0` with the
richer proof live; a forced refusal at each of the four blocker classes proves the detach/re-insert
cycle preserves the entry (queue length returns to its prior value, the same receipt is retried and
eventually retires) — asserted at a **nonzero** refusal count, not at zero.

**v3 gate extras (closure C) — the livelock must be shown to be impossible, not merely unlikely:**
1. **Bounded-pass assertion.** Instrument the drain to record selections per invocation and assert
   `selections <= queue_length_at_pass_start` over a workload that forces refusals. Before the
   cursor this assertion fails by construction (the same entry is selected repeatedly); the PR body
   records that pre-image, so the test is proven to be a real test rather than a tautology.
2. **The exact livelock scenario, replayed.** Inject one receipt whose epoch and shadow blockers
   clear immediately but whose `blocked_live_row` persists. Assert: `RECLAIM_PASS_SKIPPED > 0`, the
   drain **returns** (does not spin), `RECLAIM_PARKED == 1` after the third refusal, and no other
   ready receipt is starved — a second receipt enqueued behind it retires in the same pass.
3. **All three unpark arms are exercised separately** *(v3 repair — one aggregate `RECLAIM_UNPARKED`
   cannot show that an arm is dead).*
   (a) **epoch:** release the injected cached-root blocker, advance the scheduling epoch on every CPU
   in the captured mask, assert `RECLAIM_UNPARKED{epoch} == 1` and the receipt retires.
   (b) **row:** park an entry on `blocked_live_row`, then remove the blocking row via a reap **while
   every other CPU is held off the scheduler (no epoch advance anywhere)**; assert
   `RECLAIM_UNPARKED{row} == 1` and the receipt retires. **Under an epoch-only unpark rule this test
   hangs** — the PR body records that pre-image, so the arm is proven to be load-bearing.
   (c) **age:** park on an injected blocker that neither keyed arm can clear, **and suppress both
   keyed arms by construction**: hold every CPU in the captured mask *except one* off the scheduler,
   so the `epoch` arm (which needs all of them to advance) cannot fire, and remove no row, so the
   `row` arm cannot fire. Let the one running CPU accumulate scheduling epochs. Assert
   `RECLAIM_UNPARKED{age} == 1` **once the captured mask's epoch sum has advanced by
   `PARK_AGE_BACKSTOP_EPOCHS = 64`** — a counted quantity the test reads directly, not a wall-clock
   wait *(v3 tranche pass: the backstop now has a unit, so this assertion is checkable)* — and that
   the entry is re-proved (not dropped, not freed). Assert the *negative* in the same test: at a sum
   advance of 63 the entry is still parked.
   In all three, `RECLAIM_PARK_RESIDENT` returns to **0** at quiesce, having been **observably
   nonzero mid-run** (both halves asserted — a gauge that is always zero proves nothing).
4. **Not-a-cap assertion.** Fork-pressure test proves a *full* eligible drain: with N ready receipts
   and no refusals, one pass retires all N. A cap would fail this; the cursor does not. The **age**
   unpark arm is covered by the same assertion in reverse: it only ever returns entries to the queue,
   so no workload can show it removing an entry from consideration.
5. **The fence is a fresh SNAPSHOT, proven negatively against both wrong implementations** *(v3
   repair; second half added by the v3 tranche pass).* (a) Park a receipt whose retirement fence
   elapsed long ago and assert the very next sweep does **not** unpark it, with no epoch advance, no
   row removal and no age expiry: `RECLAIM_PARK_IMMEDIATE_UNPARK == 0` and `RECLAIM_UNPARKED == 0` at
   that point. Reusing `reclaim.after_epoch` as the park fence fails this by construction. (b) Force
   an epoch advance on every captured CPU **between step 2's snapshot and the third refusal**, then
   park; assert the entry does **not** unpark on the next sweep. An implementation that reuses the
   cycle's earlier `RetirementSnapshot` instead of taking a new one at park time passes (a) and fails
   (b) — which is why (b) exists.

**Strictly better.** Grace can no longer elapse on an unordered or empty observation; reclaim
refusals stop being a single opaque boolean; the local hardware register enters the proof (closing the
local half of the shadow/hardware gap `main` has today); the drain stops being a place where a future
proof could nest locks. *(v3: and the drain acquires a termination argument it did not have before —
today's loop only terminates because its predicate is cheap and monotone; the moment a richer proof
is added, it needs the cursor. Shipping the proof without the cursor is the defect the re-ratification
caught, and it would have been a livelock in the first phase of the round.)*

**Revert.** Restore the bare arrays, the boolean predicate and the in-lock scan; delete the three
`PendingProcessReclaim` fields (`last_pass`, `proof_failures`, `parked`), the `ParkRecord` type with
its park-time snapshot and its `age_epoch_sum_at_park` key, the parked list and the one-line
`ROW_REMOVAL_EPOCH` bump. Counters in P0 go unused but harmless (they are
read by the reader, not dead). The cursor and park list revert **with** the richer proof, which is
correct — neither is needed without the other.

---

## Phase 2 — **SPINE-1: SIGKILL stops eager-freeing** *(#491's live UAF)*

**Scope.** Rewrite the SIGKILL arm at `syscall/signal.rs:162` to: validate under the existing PM
guard, capture `pid`, mutate **nothing**, `drop(guard)`; then
`with_scheduler(|s| s.terminate_process_threads(pid))`; then **broadcast** `SGI_RESCHEDULE` to every
other online CPU via the new teardown-only `send_exit_expedite_sgi(pid, batch)` helper (no
`cpu_state[].current_thread` residency predicate — that read is stale-prone and was a fatal panel
finding; and no reuse of the generic `send_resched_ipi*` helpers, so the expedite evidence is
teardown-attributable — closure F). **That helper also publishes the victim's `EXIT_KICK` bucket
before it broadcasts — reserve, fill, commit on the slot's `state` word — and the peer scheduler pass
that declines to dispatch a quarantined victim consumes it with one seqlock-validated
generation-claiming `compare_exchange`** — the P2-era observation half of closure F's pairing,
specified in DESIGN §2.7 and deleted in P8 (v3 tranche pass). Then `exit_process_and_retire(pid, -9)`, whose
merged aarch64 path already grace-defers the page table **before** `terminate()` runs. Keep the
existing `set_need_resched()` tail. Additionally install the **durable report/SIGCHLD obligation seed**:
one row obligation moved `Absent → Pending` at first commit and redeemed exactly once outside PM by
whichever of {commit path, `handle_thread_exit`} **claims** it first (the four-state machine of
DESIGN §1.6 — the seed uses transitions T1/T2/T3 from day one; P6b generalizes it, it does not
upgrade a bool) — because `btrt::on_process_exit` has exactly one call site at
`task/process_task.rs:267` inside `handle_thread_exit`, which a remotely-marked victim may never run.

> **v3 repair — what the seed does NOT ship, stated so the two documents agree.** DESIGN §1.6 as
> first written said P2's `Report` seed ships "with its effect marker and the split
> `claim_exit_slot`/`record_exit` API from day one"; this PLAN keeps `btrt::on_process_exit` intact
> and ratcheted to its single call site until **P6b** performs the split. Both cannot be true, and
> the version where P2 ships a class-B obligation *claiming* a recovery marker it does not have is
> the dangerous one. **This PLAN's sequencing is what holds**, and the design has been corrected to
> match it:
>
> - P2 ships the seed's **shape** — the four-state machine, transitions T1/T2/T3, never a bool — and
>   calls `btrt::on_process_exit` **unchanged**. No `report_marker`, no btrt split, no new file.
> - **T4 does not exist at P2.** T4 is performed only by the S4 retire/reap gate, which arrives in
>   P6b; P6a's removal join runs with a vacuously-true ledger term and performs no recovery. An
>   obligation nothing ever recovers needs no marker — the marker exists solely to tell T4 which
>   destination to take.
> - **Exactly-once still holds at P2** by the sole-redeemer invariant alone: the commit path and
>   `handle_thread_exit` race under PM and exactly one claims.
> - **The cost is declared, not hidden.** Between P2 and P6b a claimer that dies between T2 and T3
>   loses that one `btrt` report — precisely what `main` does today for a remotely-marked victim, so
>   phasing rule 1 holds at every commit.
> - **P6b introduces T4 and the `claim_exit_slot`/`record_exit` split in the same PR**, because a
>   recovery rule and the marker it reads must never be separated by a merge boundary.

> **v2 (condition 3) — the retirement receipt is enqueued only after the PM guard drops.**
> `exit_process` stops calling `enqueue_process_reclaim` at `manager.rs:1152` inside its own PM
> guard; the enqueue happens after the guard drops. The move allocates nothing (`core::mem::take` on
> the existing `pending_old_page_tables` `Vec` leaves it empty without allocating). The **second**
> PM-nested enqueue, `handle_thread_exit` phase 1 at `task/process_task.rs:244`, is converted in the
> same PR: the receipt rides out through the existing `phase1_result` value and is enqueued in
> phase 2, where PM is already dropped. The `#[allow(dead_code)]` marker on `exit_process`
> (`process/manager.rs:1119-1120`) comes off — P2 is its first live caller.

> **v3 (closure A) — and NO caller is ever handed a receipt, because that is the only thing that
> actually closes the hole.** v2's mechanism was "return `#[must_use] Option<RetirementReceipt>` and
> adapt two callers". The re-ratification's first FATAL is correct: `#[must_use]` is defeated by
> `let _ =`, and **seven further live call sites** would have been handed a value they were never
> written to carry. A dropped receipt destructs a root with **no grace and no RootProof** — strictly
> worse than `main`, in the phase whose entire purpose is closing a UAF. The API is restructured:
>
> ```rust
> pub(crate) struct RetirementReceipt { /* fixed size; NO public constructor */ }
>
> pub(crate) fn exit_process_locked(pm: &mut ProcessManager, pid: ProcessId, exit_code: i32)
>     -> Option<RetirementReceipt>;              // crate-private; ratchet: exactly ONE caller
>
> pub fn exit_process_and_retire(pid: ProcessId, exit_code: i32) -> ExitOutcome {
>     let receipt = with_process_manager(|pm| exit_process_locked(pm, pid, exit_code));
>     // PM guard provably dropped here (with_process_manager owns its scope)
>     if let Some(r) = receipt { enqueue_process_reclaim(r); }
>     ...
> }
>
> impl Drop for RetirementReceipt {           // self-healing: re-enqueue, never free
>     fn drop(&mut self) { RECEIPT_DROPPED_UNRETIRED.inc(); reenqueue(self.take_contents()); }
> }
> ```
>
> **All nine ADAPTED SITES move in THIS PR**, in the three disjoint classes of DESIGN §1.7. Rows 1-7
> are **class 1** (`exit_process` callers — the seven, and only the seven, that a `git grep` finds at
> `985881a6`); row 9 is **class 2** (the new SIGKILL arm); row 8 is **class 3** (the PM-nested enqueue,
> which does **not** call the wrapper). The eight class-1+2 rows call `exit_process_and_retire`; the
> seven class-1 rows currently call `exit_process` **inside a live PM guard or `with_process_manager`
> closure**, so each needs the call lifted out of the closure, not merely renamed:
>
> | # | Class | Call site (verified at `985881a6`) | Shape today | Required adaptation |
> |---|---|---|---|---|
> | 1 | **1** | `arch_impl/aarch64/exception.rs:778` | `pm.exit_process(pid, -11)` inside `with_process_manager(...)`, after the `terminate_process_threads` call at `:768` | close the closure after the victim lookup, call `exit_process_and_retire(pid, -11)` outside it; the mandatory tails (`terminate_current_scheduler_thread`, `set_need_resched`, frame redirect to `idle_loop_arm64`, `set_idle_stack_for_eret`, `switch_to_idle`) stay **unconditional** and are not reordered |
> | 2 | **1** | `arch_impl/aarch64/exception.rs:1146` | same, paired with `:1135` | same |
> | 3 | **1** | `arch_impl/aarch64/exception.rs:1233` | same, paired with `:1230` | same |
> | 4 | **1** | `arch_impl/aarch64/exception.rs:1336` | same, paired with `:1333` | same |
> | 5 | **1** | `interrupts.rs:1429` (x86_64) | `pm.exit_process(pid, -11)` inside `with_process_manager(...)`, with `faulting_thread_id` captured in the same closure | capture the pid/tid in the closure, call the wrapper after it; the `with_thread_mut` tail is unchanged |
> | 6 | **1** | `interrupts.rs:1735` (x86_64) | same | same |
> | 7 | **1** | `process/mod.rs:264` (`exit_current`, fn at `:258`) | `manager.exit_process(pid, code)` with `*manager()` held live across the call by an `if let Some(ref mut manager)` binding | end the binding's scope before the call; `exit_current` becomes a thin wrapper over `exit_process_and_retire` |
> | 8 | **3** | `task/process_task.rs:244` (`handle_thread_exit` phase 1) | `enqueue_process_reclaim(reclaim)` under PM | receipt rides out through `phase1_result`, enqueued in phase 2 (v2's fix, unchanged). **This site does NOT call `exit_process_and_retire`** — it already owns its PM guard and its two-phase shape, and folding it into the wrapper would mean taking PM twice. It is an adapted site, not a wrapper caller, and no exact-set gate counts it as one |
> | 9 | **2** | `syscall/signal.rs:162` (the SIGKILL arm this phase rewrites) | `process.terminate(-9)` under PM | becomes `exit_process_and_retire(pid, -9)` after `drop(guard)` |
>
> Plus the internal `manager.rs:1152` enqueue, which is **deleted** (it moves into the wrapper).
> Four properties make this custody rather than advice: the type is crate-private with no public
> constructor; `exit_process_locked` has exactly one permitted caller (ratcheted); `Drop` re-enqueues
> instead of freeing; and a receipt exists in only two provably PM-free places (inside the wrapper
> between guard-drop and enqueue, and P1's local detach slot), so `Drop` can never nest the queue lock
> under PM. DESIGN §1.7.

**Files.** `kernel/src/syscall/signal.rs`, `kernel/src/process/manager.rs`,
`kernel/src/task/scheduler.rs`, `kernel/src/task/process_task.rs`, plus the six fault sites in
`kernel/src/arch_impl/aarch64/exception.rs`, `kernel/src/interrupts.rs` and
`kernel/src/process/mod.rs`, tests. **3 commits; the named split seam (rule 5) is
{receipt custody + all nine adapted sites} / {SIGKILL arm + expedite helper + obligation seed}, and
this PR is expected to take it — two independent revert stories, not a line count.** *(v3 honesty: closure A
made this phase bigger, and the plan says so up front rather than discovering it at review. Split
before review, per rule 5.)*

**Gate extras.** New `sigkill_teardown_test` (userspace): parent forks a child spinning at EL0;
parent `kill(child, SIGKILL)`. Assert (a) `waitpid` reaps **-9**; (b) SIGCHLD arrived at kill time
(parent's `pause()` returns); (c) `TEARDOWN_QUARANTINE`/`TEARDOWN_DEFER`/`TEARDOWN_RECLAIM` all
increment for that pid; (d) `TEARDOWN_MASKED_FRAMES_WALKED == 0` **for kill paths** (the baseline
recorded in P0 must drop, not merely stay flat); (e) **(v3 — closure F; the observation half specified by the v3
tranche pass)** `EXIT_SGI_SENT{pid}` is nonzero **for the test's own victim pid**, recorded by the new
teardown-only `Scheduler::send_exit_expedite_sgi(victim_pid, batch)` and by no other site; **the
peer's observation is recorded with the same pid as `EXIT_KICK_OBSERVED{pid}`, via the `EXIT_KICK`
bucket table of DESIGN §2.7** — published by the helper before the broadcast, consumed by one
`compare_exchange` at the peer scheduler pass that declines to dispatch the quarantined victim; and
the send→observe interval carried by that CAS is **measured strictly shorter than the victim's tick
period**. A bare "`EXIT_SGI_SENT > 0`" gate is explicitly rejected, because the P0 baseline proves the
generic reschedule IPIs alone would satisfy it. Three sub-assertions keep the proxy honest:
`EXIT_KICK_PUBLISHED == EXIT_KICK_OBSERVED == 1` for this single-victim workload;
`EXIT_KICK_BUCKET_COLLISION == 0`; and `EXIT_REQUEST_OBSERVED == 0`, since the boundary hook does not
exist until P8 and any nonzero value would mean the gate is reading a counter it was told is
unavailable. **What this asserts is stated as what it is:** *the expedite reached the peer and the
peer stopped dispatching this victim, faster than a tick.* It is **not** "the victim observed a
latched exit request" — that claim needs P8's hook, arrives with it, and is where the bucket table is
deleted (residual R-21);
(f) exactly one `btrt` report; (g) zero fault markers over the 10-boot Parallels streak; **(h)
`RECLAIM_ENQUEUE_UNDER_PM == 0` — the P0 baseline was nonzero at both sites, so this is a measured
drop, not a vacuous zero.** Repeat with the child inside a CLONE_VM group — the **sibling must
survive** (this phase does not sweep the group). Repeat as self-kill `kill(getpid(), SIGKILL)`.
**Soak + retention measurement.**

**v3 gate extras (closure A) — receipt custody is proven, not asserted:**
1. **Ratchet — the three exact sets from P0's post-P2 inversion, asserted together** *(v3 tranche
   pass: the earlier phrasing, "the pinned nine-site `exit_process` list from P0 is replaced by 'all
   nine now call `exit_process_and_retire`', asserted as an exact set", was unsatisfiable — P0 pins
   **seven** `exit_process` callers, not nine, and the ninth adapted site, `handle_thread_exit`, does
   not call the wrapper at all)*:
   (i) `exit_process_and_retire` — **exactly 8** call sites: the seven former `exit_process` callers
   named in P0 rule 1, plus the new SIGKILL arm;
   (ii) `exit_process_locked` — **exactly 1** call site, the wrapper;
   (iii) `enqueue_process_reclaim` — **exactly 3** call sites (wrapper post-guard, `handle_thread_exit`
   phase 2, `RetirementReceipt::drop`), up from the P0 baseline of 2 and with `manager.rs:1152` gone;
   plus `RetirementReceipt` has zero public constructors and is not nameable outside the crate, and
   `ProcessManager::exit_process` has zero public call sites.
2. **Per-call-site retirement proof.** A fault injected at each of the nine adapted sites (six EL0/x86
   fault paths, `exit_current`, `handle_thread_exit`, SIGKILL) still shows that pid's root entering
   the reclaim queue **exactly once** — `TEARDOWN_DEFER{pid} == TEARDOWN_RECLAIM{pid} == 1`, per-pid
   paired exactly like P0's fork/exit gate, never as a global total.
3. **`RECEIPT_DROPPED_UNRETIRED == 0`** on every boot, plus a deliberate drop injected in a unit test
   proving the `Drop` impl **re-enqueues** (queue length increases, the counter increments, and no
   frame is freed) rather than destructing.
4. *(v3 repair — the seed's declared limit is asserted, not just written down.)*
   **`LEDGER_CLAIM_ORPHANED == 0`** for the whole run: no recovery path exists before P6b, so any
   nonzero value means a T4 was added unnoticed. Plus the P0 ratchet re-asserted: `on_process_exit`
   still has **exactly one** call site and `claim_exit_slot`/`record_exit` do not yet exist.

**v3.1 gate extras — the two negative tests the single-victim gate structurally cannot fail.** *(The
v3 tranche pass shipped a bucket protocol whose observed bit was sticky forever; its gate passed
anyway, because one named victim against an initially empty table exercises neither reuse nor
collision. These two tests exist so that class of defect fails a gate instead of surviving one. Both
land in the SAME PR as the slot protocol — a mechanism and the test that can falsify it are never
separated by a merge boundary, the same rule P6b applies to T4 and its marker. Both are test code and
neither carries a revert story of its own.)*

5. **Sequential bucket reuse — a SECOND victim in the SAME bucket must still be observed.** After the
   single-victim assertions above complete for victim `V1`, the test forks a second victim `V2` chosen
   so that **`V2 % 64 == V1 % 64`** — the harness forks and discards until the congruence holds, and
   the test **prints and asserts both pids and the congruence itself**, because a reuse test that
   cannot prove it reused the bucket is vacuous — then kills `V2` and asserts:
   (i) `EXIT_KICK_OBSERVED{V2}` fires, per-pid, with its **own** measured send→observe interval
   strictly shorter than the victim's tick period (not merely that a global counter advanced);
   (ii) for the two-victim workload, `EXIT_KICK_PUBLISHED{bucket} == 2` and `EXIT_KICK_OBSERVED == 2`;
   (iii) `EXIT_KICK_BUCKET_COLLISION == 0` — `V1`'s generation was observed *before* `V2` published, so
   the displacement arm must **not** fire, which simultaneously proves that arm does not over-count.
   **Falsification is demonstrated, not assumed:** a unit test instantiates the slot protocol directly
   with the v3 publish step (`seq.fetch_add(2, Release)`) and asserts the second observation is
   **absent** — this gate is shown to FAIL against the exact defect it exists to catch, and to PASS
   against the v3.1 protocol. A negative test whose detection power is never demonstrated is how the
   v3 defect got through.
6. **Simultaneous colliding publishers — one bucket, two CPUs: no torn pair, and both collision arms
   provably fire.** A kernel-internal test in three parts, none of which relies on winning a race by
   luck. The test maintains two test-local counters, distinct from the single aggregate
   `EXIT_KICK_BUCKET_COLLISION`: **`collisions_reservation_lost`** — incremented when a rival's
   reservation CAS finds `LOCK` already set, so the rival writes neither `pid` nor `at` and publishes
   nothing — and **`collisions_displaced`** — incremented when a publisher's commit **replaces** a
   prior generation that was never claimed/observed; a displacement is a *successful* publish (it
   still stores a new `pid`/`at` pair and commits a fresh generation), so it **also** increments
   `EXIT_KICK_PUBLISHED` — which is exactly why `collisions_displaced` must not be added a second
   time into the `attempts` identity below (doing so double-counts the same event once as a publish
   and once as a collision):
   (a) **Reservation-lost arm, deterministic.** A test-only hook holds CPU A between its reserving CAS
   and its commit store while CPU B publishes into the same bucket. B **must** find `LOCK` set, bump
   `EXIT_KICK_BUCKET_COLLISION` and `collisions_reservation_lost` exactly once each, write neither
   `pid` nor `at`, and still broadcast. A is then released; the subsequent observation must carry
   **A's** pid with **A's** `at` — proving the loser never contaminated the record and that a lost
   publication costs evidence, never coherence.
   (b) **Displacement arm, deterministic.** A publishes and commits; **no** observation is taken; B
   then publishes into the same bucket with a different pid. B must bump
   `EXIT_KICK_BUCKET_COLLISION` and `collisions_displaced` exactly once each — and, because B's
   publish committed successfully, `EXIT_KICK_PUBLISHED` also advances for B's commit — and the next
   observation must claim **B's** generation with **B's** `pid`/`at` pair — never A's, and never a
   mix.
   (c) **Free-running storm, for the torn-pair invariant, with an exact per-generation oracle.** Two
   CPUs publish into one bucket for `N >= 10_000` iterations; each publication carries a **unique
   per-publication token** (monotonically issued, never reused) written alongside `pid`/`at` under
   the same reservation, while a third CPU observes. Assertions: **every** recorded observation is
   checked against the **exact** `(generation, pid, token)` tuple recorded by the publisher of that
   generation — not membership in a publisher-wide range, which cannot distinguish generation *N* of
   a publisher from that same publisher's generation *N±1* — so **zero** torn pairs **and zero
   cross-generation mismatches** across the whole run, asserted per observation rather than as a
   sample; the accounting identity `attempts == EXIT_KICK_PUBLISHED + collisions_reservation_lost`
   holds **exactly**, and the separate identity
   `EXIT_KICK_BUCKET_COLLISION == collisions_reservation_lost + collisions_displaced` also holds
   **exactly** (a displacement is counted once, in the collision-arm identity, never a second time in
   the attempts identity — an inequality in either identity would let a silent collision or a
   double-count hide, which is precisely the v3 weakness); the bucket is left with
   `state & LOCK == 0`; and one final publish→observe cycle after the storm still succeeds, proving
   the storm neither stranded nor wedged the bucket.

**Strictly better.** The confirmed-live eager `cleanup_cow_frames`-while-remote-runs UAF class is
gone; quarantine, expedited reschedule, and SIGCHLD arrive for the first time on this path; and both
pre-existing reclaim-queue-under-PM nestings are removed. *(v3: and the round's most dangerous
possible self-inflicted wound — a receipt-loss channel opened by the very refactor meant to close a
UAF — is closed by construction rather than by a lint. Nine adapted sites move together, which is
larger, but a partial migration here is the one shape that would have been worse than `main`.)* *Honest bound:* the exit is still remotely
marked (upgraded in P8/P9/P10a-c), and FD closure still happens inside `exit_process` under PM —
unchanged from today's SIGKILL, which does that **plus** the full CoW walk. Not a regression; not yet
a fix. Fixed in P7.

**Revert.** Restore the four-line `process.terminate(-9)` arm (class 2), the in-PM enqueues at
`manager.rs:1152` and `process_task.rs:244` (class 3 plus the deleted one), and the public
`exit_process` signature with its **seven** original call sites (class 1); drop the obligation, the
expedite helper, the kick-bucket table and the receipt type. The exact pre-image of every one of the
nine adapted sites is preserved in the commit body — that is what makes a nine-site change revertable
alone (rule 5), and it is why the split seam puts the custody refactor in its own commit.

---

## Phase 3 — exec detach + clone/exec admission + creation-path parity *(#471 part 1; absorbs P4's surviving half)*

**Prerequisite: #573 ships before or with this phase.** P3 asserts that both detached fields survive
**every** exec failure; on x86 a failed/never-published exec currently leaks the entire half-built
address space, so without #573 that assertion holds over a path that is leaking underneath it.
**#572** (`AlreadyTerminated` abandon bypasses custody, leaking table leases and all superseded exec
roots — live at `manager.rs:1131-1136`) touches the same exec-root surface and is named here so the
gate reads its counters rather than being surprised by them.

**Scope — part 1, exec detach.** At every exec commit point (after all fallible work, before PM
release), set `process.inherited_cr3 = None` and `process.thread_group_id = None` alongside
`process.page_table = Some(new_page_table)`; preserve both on **every** exec failure. Keep the
existing live-sibling guard (`find_live_clone_vm_sibling_holding_cr3`, defined at `manager.rs:46`,
called at `manager.rs:3063` and `:3368`) — it exists because **#468** is open, and this phase does not
close #468. The defect is untouched on today's tree: across all of `kernel/src` there is exactly
**one** write of `inherited_cr3 = Some(...)` (`syscall/clone.rs:209`) and **one** of
`thread_group_id = Some(...)` (`clone.rs:210`); the only `None`s are the struct-literal defaults at
`process/process.rs:337-338`. No exec path clears either field.

**Scope — part 2, clone/exec admission.** `sys_clone` (`syscall/clone.rs:36`) validates the parent row
is `Live` under the same PM transaction that publishes the child row — the guard is already taken at
`clone.rs:60` and the TGID is already derived at `:84`. User-thread creation publishes the scheduler
thread **non-runnable** until the row is published, and dispatch refuses `ProcessState::Creating` rows
(`process/process.rs:54`, set at `:318`, cleared to `Ready` by `set_main_thread` — declared at `:350`,
clearing write at `:352`) before
arming CR3/TTBR0.

> **The dispatch gate lives in `kernel/src/interrupts/context_switch.rs`, not `task/scheduler.rs`, and
> its scope must be re-derived against PR #570.** #570 rewrote this exact site: a single unconditional
> PM try-lock before `scheduler::schedule()` (`context_switch.rs:218`) with a refusal arm that neither
> schedules nor rolls back, plus a second refusal arm for a thread whose address space is gone
> (`USERSPACE_DISPATCH_NO_CR3_REFUSED`, `context_switch.rs:27`, incremented at `:676` and `:1193`),
> both ratcheted in `tests/context_restore_structure.rs`. P3's `Creating` gate is a **third arm on an
> already-refactored, already-ratcheted site** — it is written against #570's shape, and the ratchet is
> extended, not re-invented.

**Scope — part 3, creation-path scheduler-registration parity** *(folded in from the dissolved P4)*.
Drop the PM guard **before** every scheduler registration on the creation paths, removing the live
PM→SCHEDULER nesting that is the remainder of **#527**'s class:

| PM guard taken | `scheduler::spawn` called under it |
|---|---|
| `kernel/src/process/creation.rs:67` | `:85` |
| `kernel/src/process/creation.rs:185` | `:202` |
| `kernel/src/boot/test_disk.rs:258` | `:263` |

`scheduler::spawn` (`task/scheduler.rs:3444`) takes `lock_scheduler()` at `:3447`, so each of these is
the identical PM-held→SCHEDULER ordering PR #577 fixed and ratcheted **for the exec path only**
(`tests/exec_lock_order_structure.rs`, 25 tests, marker `[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]`). This
phase extends that ratchet's shape to the creation sites. #527 is closed as an issue; this is its
creation-path remainder, and it is folded here rather than left as its own phase because it shares
these call sites with nothing else in the tranche.

**What is NOT in this phase — the kernel-stack half of the old P4.** AC-8's *"transfer
`kernel_stack_allocation` into the scheduler copy"* cannot be applied to the creation paths, because
there is no allocation object left to transfer: all three `create_main_thread*` constructors
permanently `Box::leak(Box::new(kernel_stack))` — `manager.rs:851`, `:925`, `:1010` — and store
`kernel_stack_allocation: None`, with an in-tree `// TODO: proper cleanup`. That is **#579**: a
permanent per-process kernel-stack leak on the primary x86 and aarch64 creation paths, and it is also
what *masks* the freed-row hazard the old P4 was written against. `remove_process`
(`manager.rs:1086-1090`) drops the whole `Process` row → `Process::main_thread`
(`process/process.rs:199`) → `Thread::kernel_stack_allocation` (`task/thread.rs:428`) →
`impl Drop for KernelStack` (`memory/kernel_stack.rs:85-99`) returns the slot to the pool, and nothing
in `kernel/src` ever clears `main_thread`. **The hazard is structurally live and merely unreached** —
unreached only because no row ever holds a `Some(KernelStack)`, which is an unratcheted coincidence,
not a closure. Deciding ownership for a stack whose thread is published to the scheduler as a
`Thread::clone` (which drops the allocation — `thread.rs:514`, *"Can't clone kernel stack
allocation"*) is a design question, tracked on **#579**, and is not a tranche-2 deliverable. **#546**
(owner-side `GuardedStack` reclamation of `External` **user**-stack frames) is a separate item and
does not substitute for either.

**Files.** `kernel/src/process/manager.rs`, `kernel/src/syscall/clone.rs`,
`kernel/src/interrupts/context_switch.rs` (dispatch gate), `kernel/src/process/creation.rs`,
`kernel/src/boot/test_disk.rs`, `userspace/programs/src/clonevm_exec_test.rs`, plus the two ratchet
suites. **Split seam (rule 5, not a size rule): {exec detach + clone/exec admission} /
{creation-path lock-order parity}** — two independent revert stories, so this phase splits into two
PRs the moment both are in one merge commit.

**Gate extras.**
- Extended `clonevm_exec_test`: successful exec → both fields `None`, fresh root, effective
  TGID == pid, and a kill aimed at the **old** group cannot reach it; failed exec → both fields
  byte-identical to pre-exec.
- "Fresh root" is **observed, not argued**: the exec-cohort per-PID oracle already in
  `kernel/src/tracing/providers/teardown.rs` (`fork_exit_defer_reclaim_pairing_test`, `:1192`) and its
  x86 counter line `PT_EXEC_COHORT` carry the assertion. Root ownership itself is enforced in-tree now
  (`owned_root_slots`, `PT_ROOT_SLOT_REFUSED` fail-closed, receipt-carried superseded roots), so P3
  consumes that machinery instead of building an argument on paper.
- Futex behaviour across an exec verified explicitly (the group id falls back to pid — `futex.rs` is
  the main consumer). Deterministic clone-vs-exec race.
- Lock-order parity: the extended `exec_lock_order_structure` census fails on any
  `scheduler::spawn`/`lock_scheduler` reachable with a PM guard live at the three creation sites, and
  the runtime marker `[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]` stays at zero across the full gate.

**Strictly better.** Closes the wrong-victim-after-exec defect that was one of the four blockers which
killed PR #418's group sweep — *before* any group-scoped kill exists to trip over it — and removes the
last live PM→SCHEDULER nesting on the creation paths.

**Dependency note (v2, condition 4).** This phase is a hard prerequisite of **P8**, not only of P9:
P8's last-reference decision and `RootProof` read the row's own root *and* `inherited_cr3`, so a row
carrying a stale `inherited_cr3` past an exec would present a root it does not own (DESIGN §2.3). The
`P3 → P8` edge is in the graph; v1 omitted it. The old `P4 → P8, P9` edge is subsumed: the lock-order
half arrives here, and the kernel-stack half is #579, outside the plan.

**Accepted residual.** **#560** — blocking-syscall prologues identify the executing thread from the
scheduler's *recorded* current rather than an authoritative identity. P3's admission decisions are made
inside the PM guard **on the calling path**, not remotely, and #560's failure mode is skew on a
*blocking* path; the risk is named, tracked, and accepted for this tranche (§0.0).

**Revert.** Part 1: delete the two assignments + the admission check, plus the third dispatch arm.
Part 2: per-site; each of the three creation sites is independently revertable.

---

## Phase 4 — DISSOLVED *(was: kernel-stack ownership parity for all three creation paths, AC-8)*

**This phase no longer exists.** Reading the creation paths at ratification time split its scope in
two, and neither half is a phase:

- **The lock-order half** — drop PM before every scheduler registration at `creation.rs:67→85`,
  `creation.rs:185→202`, `boot/test_disk.rs:258→263` — is **folded into P3, part 3**. It shares those
  call sites with nothing else in the plan and carries its own revert story there.
- **The kernel-stack half** — AC-8's single-owner accounting — is **#579**. The `take()` pattern P4
  proposed to generalize cannot be applied: `manager.rs:851`, `:925` and `:1010` `Box::leak` the
  `KernelStack` at construction, so no allocation object survives to transfer. Fixing it requires an
  ownership design pass, and the leak it creates is a live per-process defect in its own right.

**AC-8 stays open** and is now discharged by #579, not by this plan. Its acceptance shape is unchanged
and carried on the issue: ownership assertion after every creation path (exactly one owner, and it is
the scheduler copy); 1000-iteration fork/clone/spawn exit stress with stack-pool accounting
(allocated == freed); an allocator assertion that never selects a live slot. **#546** is the *user*-stack
sibling and does not substitute for it.

*Honesty note, retained:* the original input package reported the spawn asymmetry as **fact, not a
diagnosed bug**. What re-reading it produced was two previously unnamed live defects — the #579 leak
and the #527-class nesting — which is why the phase dissolved into a filed bug plus a fold-in rather
than staying a uniformity chore.

---

## Phase 5a — Runtime init designation, identity ONLY *(#464 part 1 — no fatal behaviour)*

**Scope.** `ProcessManager::designated_init: Option<ProcessId>` as the single authority. **Reserve
PID 1** for the explicit init constructor; ordinary/test allocation starts at 2. Init is built
off-table with provisional PID 1 through a **held-publication ticket**: fallible image → row inserted
→ scheduler thread created **not-yet-runnable** → ticket returned → PM validates the ticket names a
live PID 1 → `designated_init` set → thread published to the run queue. Production designation is
**validated == PID 1 and refuses otherwise**. **No `#[cfg]` anywhere.** `init_shell.rs:1028` is not
touched.

**The two migration surfaces, re-counted at `2c7b8798` — both larger than the design recorded.**

- **PID allocation: `next_pid` has EIGHT `fetch_add` sites, not four.** `next_pid` is
  `AtomicU64::new(1)` (`manager.rs:118`); the allocation sites are `manager.rs:141`, `:378`, `:602`,
  `:1076`, `:1419`, `:1561`, `:1704`, `:2169`. The reservation must cover all eight — a single missed
  site hands PID 1 to an ordinary process and defeats the reservation silently.
- **Init literals: four production sites plus five dependent reads** (the design recorded three, and
  two of the three it named are not init literals at all).

  | Site | What it is |
  |---|---|
  | `process/manager.rs:1165` | `let init_pid = ProcessId::new(1);` in the exit-path reparenting block; read at `:1166`, `:1176`, `:1179` |
  | `task/process_task.rs:647` | `if pid == ProcessId::new(1)` — the "init has no children to reparent" test |
  | `task/process_task.rs:720` | `let init_pid = ProcessId::new(1);` in the deferred reparent block; read at `:723`, `:726` |
  | `syscall/signal.rs:26` | `const INIT_PID: u64 = 1;` — read at `:124` and `:402` |

  The design's `task/process_task.rs:226` and `:285` are **not** init literals on any current tree:
  `:226` is inside `live_row_names_root`/`any_live_root_matches` and `:285` is the
  `BOOT_RECLAIM_FORCED_BLOCKER` static (`BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO` is two lines further at
  `:287`). They are struck, not re-anchored.

  The three test-only literals stay allowlisted **by name**: `test_userspace.rs:84`, `:203`, `:292`.

**Files.** `kernel/src/process/manager.rs`, `kernel/src/process/mod.rs`,
`kernel/src/main_aarch64.rs`, `kernel/src/syscall/signal.rs`, `kernel/src/task/process_task.rs`.
**Split seam (rule 5): {PID-1 reservation + held-publication ticket} / {literal migration}** — two
revert stories, so the seam fires when both would land in one merge commit.

**Gate extras.** Failure injection at **each** fallible stage after provisional PID selection:
`designated_init() == None`, no row, and a retry succeeds as PID 1. This is now safe to gate: PR #558
made every post-commit dispatch abort roll back (`abort_dispatch_and_resume` + `set_need_resched`,
resume-thread state preserved), so a failure injected mid-publication no longer leaves a
half-committed dispatch behind. Boot test: designated pid == 1 == the pid `init_shell` observes via
`getpid()`. A build with no real init leaves designation unset and does **not** treat whichever
process got a low PID as init. Existing orphan-reparent test still green. P0 ratchet allowlist shrinks
to the three named `test_userspace.rs` sites, and pins all eight `next_pid` allocation sites so a
ninth cannot appear unnoticed.

**Strictly better.** Converts AC-5 from "convention plus a boot log line" into a structural guarantee,
and makes a failed init creation deterministically retryable. Ships **no** behaviour change on init
death — deliberately, because bundling identity with policy is what killed four prior attempts.

**Accepted residual.** **#560**, as for P3: "is the caller init?" resolves the calling row through the
recorded current thread. Decision is made inside the PM guard on the calling path; named and accepted
for this tranche (§0.0).

**Revert.** Restore the literals and the `next_pid` base. The ticket is additive.

---

## Phase 5b — `sys_clone` init-group refusal *(#464 part 2 — **HELD on #575**)*

> **HELD, mechanism ratified, evidence blocked.** The refusal itself is two or three lines and lands
> exactly where the design says. What is blocked is its **acceptance**: the gate extra below asserts
> that *over a full boot, no row other than init itself ever carries init's effective TGID*, by walking
> the process map **at quiesce** — and **#575** means init does not reliably reach quiesce on the QEMU
> gates (`/bin/bwm` spawn returns EIO, the following `/sbin/telnetd` spawn never returns; long-standing
> trackers **#427**, **#438**). A phase whose acceptance is a quiesce walk cannot be accepted while
> quiesce is unreachable. P5b ships when #575 closes. The design's `P5 → P9` edge means P5b must land
> before P9 regardless, so holding it does not extend the critical path unless P9 arrives first — and
> if it does, P9 blocks on P5b, not the other way round.

> **v3 (closure E, end 1) — `sys_clone` refuses to publish into the designated init's thread group.**
> The re-ratification found a FATAL composition hole: P9 makes fatal signals **thread-group scoped**
> while v2's P12 dropped only signals aimed at the designated-init **row**, so a `CLONE_VM` sibling
> sharing init's effective TGID could be targeted and the seal would kill the whole group, init
> included. It is closed at both ends, and **this is the end that must ship first** — see the new
> `P5 → P9` graph edge, which exists because without it P9 would introduce a way to kill init that
> `main` does not have.
>
> `sys_clone` (`syscall/clone.rs:36`) already derives the parent's effective TGID at `:84`
> (`process.thread_group_id.unwrap_or(pid.as_u64())`) **inside the live PM guard taken at `:60`**.
> Immediately after that derivation, if `designated_init()` is `Some(init)` and the derived TGID
> equals init's effective TGID, return **`EINVAL`**. Two or three lines, in the guard that is already
> held, with the authority this phase introduces.
>
> **This is a deliberate, documented ABI restriction:** *the designated init cannot acquire `CLONE_VM`
> siblings.* Nothing in-tree clones from init; a multi-threaded init would need its own design pass
> for group-scoped death regardless. There is exactly **one** production write of a non-`None`
> `thread_group_id` (`syscall/clone.rs:210`), so this refusal is the complete admission surface, and
> P0's ratchet pins that write site by name so a second one cannot appear unnoticed. Recorded as
> residual **R-18** with the maintenance obligation attached.
>
> *Not dormant (rule 2):* the refusal has a live caller and a live test in this PR (`clone()` from
> init returns `EINVAL`; `clone()` from any non-init process is unaffected), and it is observable
> from the moment it lands rather than "activated by P9".

**Files.** `kernel/src/syscall/clone.rs` (the admission refusal), plus the P0 ratchet.

**Gate extras (closure E, end 1).** `clone()` issued from the designated init returns **`EINVAL`**
and creates no row; `clone()` from a non-designated process is unaffected (the existing CLONE_VM
tests stay green unchanged); and **over a full boot, no row other than init itself ever carries
init's effective TGID** — asserted by walking the process map at quiesce, not by a source grep **(this
is the assertion #575 blocks)**. A build with no designated init exercises the `None` arm and refuses
nothing.

**Strictly better.** Makes the init-sibling state unconstructible before any group-scoped kill exists,
which is what keeps P9 strictly better than `main`. The refusal is an admission rule, not a death
policy — identity and policy stay unbundled.

**Revert.** Delete the clone refusal. *(Note: reverting P5b after P9 has merged would reopen the init
group hole, so the revert story is "revert P5b only while P9 is unmerged" — recorded in the PR body
per rule 5.)*

---

## Phase 6a — Reap/tombstone retention gate *(NEW in v2 — condition 2)*

> ⚠ **DEBT-4 — the x86 reap path bypasses this phase's gate, and this phase owns the closure.**
> `kernel/src/syscall/handlers.rs:3123` removes the process row **directly** on the live x86 reap
> path — as does its byte-similar duplicate at `kernel/src/syscall/wait.rs:386` — so the two-event
> join below is not the only remover: the retention gate can pass on aarch64 while x86 still frees a
> row out from under an un-retired receipt. Before this phase's tranche ratifies, those sites either
> route through the join or the gate is honestly re-scoped to aarch64 with the divergence named in
> AC-12's evidence, a ratchet pinning both sites by name, and a stated closing phase. **The join now
> installs in one place:** `remove_process` is a four-line choke point (`manager.rs:1086-1090`) that
> already bumps `ROW_REMOVAL_EPOCH` via `note_process_row_removed()` (`task/process_task.rs:355-357`),
> so both arches are covered by one edit rather than a per-call-site chase. See the DESIGN-DEBT
> REGISTER.

**Scope.** Make a process row outlive its reap so that row-resident obligations cannot be destroyed
before they are discharged. Today `waitpid`'s `complete_wait` (`kernel/src/syscall/wait.rs:335`)
physically removes the row at `wait.rs:386` → `ProcessManager::remove_process`
(`manager.rs:1086-1090`, re-anchored at `2c7b8798`) — which
is exactly the seam the ratification flagged: P6b's `Resources` obligation **by construction outlives
reap** (grace + RootProof are still pending when the parent collects the status — R-13).

- Add `RowState { Live, Zombie, Tombstone }` to the process row, **plus the two join flags**
  `reaped: Option<(ProcessId, i32)>` and `retired: bool` *(v3, closure B)*.
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

> **v3 (closure B) — the removal TRIGGER, which v2 never specified.** v2 stated the *condition* for
> removal (ledger complete **and** receipt retired) but named no trigger, and the row carried only the
> reap. If retirement finished first, nothing revisited the row when the reap arrived; if the reap
> finished first, nothing revisited it when retirement landed. Either order could strand a permanent
> tombstone — the MAJOR the re-ratification raised — and an implementer patching around it would
> reach for premature removal. Removal becomes an explicit **two-event join**:
>
> ```
> reap(pid)   [PM]: row.reaped = Some((by, status));
>                   if row.retired && ledger_settled(pid) { remove_row(pid); JOIN{reap_second}++ }
>
> retire(pid) [PM]: row.retired = true;
>                   if row.reaped.is_some() && ledger_settled(pid) { remove_row(pid); JOIN{retire_second}++ }
> ```
>
> - **Both writes happen under PM**, so the write, the read of the other flag, the ledger check and
>   the removal are all in **one acquisition**. PM serializes the two writers, exactly one observes
>   the other flag already set, and **removal happens exactly once whichever order occurs**.
> - **The join is not a one-shot edge trigger.** If `ledger_settled` is false when the second event
>   lands, neither writer removes; the S4 drain re-evaluates the join for every tombstone it passes,
>   so a late-settling ledger still gets its row removed.
> - **Rows with no receipt** (`Resources` will be `Absent` — e.g. an x86_64 exit that released
>   synchronously) get `retired = true` written at the same moment that is determined, so the ordinary
>   reap removes them immediately. The join degenerates correctly instead of stalling.
> - **Rows that are never reaped** (no parent, or the parent exits first) are covered by the reparent
>   cursor handing them to the designated init before the parent's own row tombstones; a build with no
>   designated init sets `reaped` at commit through an explicit, named `auto_reap` branch.
> - **Both arms are gated nonzero** — see the gate extras. An unreachable join arm is dormant code
>   wearing a different hat (rule 2).

**Files.** `kernel/src/syscall/wait.rs`, `kernel/src/process/manager.rs`,
`kernel/src/process/process.rs`, `kernel/src/task/process_task.rs`, tests. **2 commits; the named split seam (rule 5) is
{`RowState` + liveness-predicate migration of the lookup call sites} / {two-event join + removal
gate}.**

**Gate extras.** (a) `waitpid` still returns the correct status and the parent's `children` list is
still pruned — existing wait/orphan tests green unchanged. (b) `TOMBSTONE_RESIDENT` returns to **0**
at quiesce after a 64-child fork/exit/reap workload, having been **observably nonzero mid-run** (both
halves asserted — a gauge that is always zero proves nothing). (c) A pid whose row is tombstoned is
not returned by any live-process lookup: negative tests for `kill(pid)` → `ESRCH`, `waitpid` repeat →
`ECHILD`, and procfs absence. (d) PID reuse does not hand out a tombstoned pid. (e) P4's stack-pool
accounting re-run (allocated == freed) now that reap no longer drops the row. **Soak + retention
measurement** (this phase changes retention by construction).

**v3 gate extras (closure B) — both join orders must be observed in one run:**
*(v3 repair — the two workloads were attached to the opposite counters. The counter incremented
inside `reap()` is `{reap_second}`: **the reap was the second event and retirement had already
landed.** The one incremented inside `retire()` is `{retire_second}`. A prompt reap therefore drives
`{retire_second}`, and a delayed reap drives `{reap_second}` — the reverse of what v3 asserted, which
no run could have satisfied.)*
(f) `TOMBSTONE_JOIN{retire_second} > 0` — produced naturally by the 64-child fork/exit/reap workload,
where the parent reaps **promptly, before grace has elapsed**, so the reap lands first and the retire
gate is the second writer that performs the removal. Assert the removal happens at retirement, not at
the reap, and that `waitpid` had already returned the correct status.
(g) `TOMBSTONE_JOIN{reap_second} > 0` — produced by a deliberate **delayed-reap** workload: the
child exits, the test sleeps past two grace epochs so **retirement completes first**, and only then
does the parent call `waitpid`, making the reap the second writer. Assert the row is removed at the
reap, exactly once, with the correct status still returned.
(h) A **repeat-retire** and a **repeat-reap** injection each remove the row exactly once
(`TOMBSTONE_REMOVED` increments by exactly 1 per pid over the whole run, per-pid keyed).
(i) A row whose `Resources` is `Absent` (x86_64 synchronous release) is removed at reap with no
retirement event at all — the degenerate arm.

**Strictly better.** Row lifetime stops being shorter than the lifetime of the work the row owns — the
precondition every later phase's row-resident state depends on. It also converts today's "reap frees
the row and whatever is still attached to it" into a proof-gated removal, which is the same discipline
already applied to page tables and stacks.

**Revert.** Restore the `remove_process` call in `complete_wait` and delete `RowState::Tombstone`
together with the two join flags; P6b is not yet merged, so nothing depends on it.

---

## Phase 6b — Exactly-once ledger: four-state obligations + first status *(AC-12)*

> ⚠ **DEBT-1 — `Report` is NOT exactly-once as specified, and this phase owns the closure.** Two
> passes sustained it: when the effect marker reads `started == 1 && finished == 0`, T4 cannot tell
> whether `record_exit` landed, and the ruling `→ Completed` + `LEDGER_EFFECT_AMBIGUOUS{report}`
> means a *possibly missing* report — at-most-once, not exactly-once (DESIGN §6 R-19). Before this
> phase's tranche ratifies: either the window is closed by a mechanism that makes the record step
> itself recoverable, **or** an explicit operator acceptance is carried that this is the round's
> single at-most-once obligation. Restating exactly-once while shipping at-most-once is the thing the
> pre-check rejected twice and is not available. The same debt covers the `on_process_exit` split
> phasing: `claim_exit_slot`/`record_exit` must be shown not to reorder the registry-slot clear
> relative to `finalize()`'s serial emission, and must land in the **same PR** as T4. See the
> DESIGN-DEBT REGISTER.

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

> **v3 (closure B) — the class split, which is what makes "exactly once" true rather than intended.**
> The re-ratification's second FATAL: v2's T4 moved `Claimed{dead} → Pending` **without any way to
> observe whether the effect had already fired between T2 and T3**, so reopening could duplicate and
> not reopening could lose. A recovery rule that cannot see the effect is a coin flip. Two steps:
>
> **Step 1 — four of the six obligations never enter `Claimed` at all.** An obligation is **class A**
> when its effect is a write to PM-owned state; for those, the effect and `→ Completed` are performed
> **in the same PM acquisition** (transition T2·3), so T4 is *structurally unreachable*:
>
> | Obligation | Class | Effect and where it commits |
> |---|---|---|
> | `Sigchld` | **A** | parent's pending-SIGCHLD bit — PM-owned; same acquisition as `Completed` |
> | `ParentWake` | **A** | publish the status as collectable in the parent's wait state — PM-owned; same acquisition. **The scheduler kick is NOT part of the obligation**: `unblock_for_child_exit` is idempotent, so it is declared a *repeatable* side effect issued after PM drops and counted separately as `PARENT_WAKE_KICKS` (which MAY exceed the exactly-once count — declared, not hidden). `PARENT_WAKE_COMPLETED` counts the PM transition |
> | `Reparent` | **A** | one fixed-size batch per acquisition, with `reparent_cursor` advanced in that same acquisition — a stopped claimer leaves a resumable position and re-running a batch skips children already re-parented |
> | `Resources` (take half) | **A** | `page_table.take()` into the receipt — PM-owned; same acquisition. The enqueue half is not an obligation at all, because closure A makes take-and-enqueue one indivisible public operation |
> | `Fds` | **B** | endpoint close — pipe/PTY/TCP locks must not be held under PM |
> | `Report` | **B** | `btrt::on_process_exit` reaches `finalize()` → `ktap::emit_summary` → `serial_println!`, i.e. **SERIAL** (`test_framework/btrt.rs:393`) — verified in the tree, and the whole reason class B exists |
>
> **Step 2 — each class-B obligation carries a marker written by the effect itself, and the marker
> outranks the ledger.**
>
> - **`Fds` — custody, and the descriptor never leaves the row** *(v3 repair)*. v3 as first written
>   had `take_next_for_exit` **retain** the descriptor in the row's `fd_in_flight` slot *and* return
>   that same value for an unlocked close. Those are mutually exclusive; cloning to make both true is
>   exactly the second copy that "a double close is not representable" depends on not existing, and
>   it reopens double-close / endpoint-refcount corruption — the failure class the obligation exists
>   to prevent. The operation is therefore split in three (DESIGN §1.6, §2.5):
>   `begin_fd_close(pid)` **[PM]** moves one `(fd, desc)` into the row's slot (or re-tickets one
>   already there) and returns a **non-owning `CloseTicket`** — a clone of the *endpoint handle*,
>   with no close operation on it, so it cannot close anything; `endpoint_hangup(&ticket)`
>   **[no lock]** performs the only step that needs an endpoint lock and is idempotent by a CAS on
>   the endpoint; `finish_fd_close(pid)` **[PM]** drops the owning descriptor and clears the slot in
>   one acquisition — the single destructive step. A dead claimer leaves the descriptor in the slot
>   and the next claimer replays only the idempotent half. **A double close is not representable**
>   because the owning value is in exactly one place at every instant and is destroyed exactly once;
>   `Fds` still needs no started/finished bits and no ruling, because there is no ambiguous window.
>   `hangup_done` is diagnostic (`FD_HANGUP_REPLAYED`), never load-bearing.
> - **`Report` — two lock-free bits plus the btrt token.** `on_process_exit` is split so the ledger
>   can see inside it: `btrt::claim_exit_slot(pid) -> Option<u16>` (a `compare_exchange` on the
>   registry slot; pure atomics, **no SERIAL**) is called **inside the T2 acquisition** and its result
>   is stored as `report_marker.token`; `btrt::record_exit(test_id, code)` (`pass`/`fail`, the
>   completed increment, possibly `finalize()`) runs with **no lock held**, setting
>   `report_marker.started` (CAS 0→1; a loser returns without recording) before and
>   `report_marker.finished` (release store) after.
>
>   **T4's ruling for `Report` — written down so it is not a judgement call at implementation time:**
>
>   | Marker observed | Meaning | T4 destination | Why |
>   |---|---|---|---|
>   | `finished == 1` | effect completed, only the ledger write was lost | **`Completed`** | re-running double-counts `tests_completed` and can double-`finalize()` |
>   | `started == 0` | effect never began | **`Pending`**, token preserved | safe to redo, and no new btrt slot is needed |
>   | `started == 1, finished == 0` | **ambiguous** — the claimer died inside `record_exit` | **`Completed`** + `LEDGER_EFFECT_AMBIGUOUS{report}` | **the marker wins over the ledger**: a duplicate corrupts the test ledger, while a missing report is caught loudly by the AC-12 equality |
>
> The general rule for any future obligation: *class B requires a marker the effect itself writes and
> an explicit statement of which side wins.* An obligation with neither a PM-committable effect nor a
> marker does not belong in the ledger. DESIGN §1.6.

A repeat request returns the stored batch/status and creates no second obligation and no second status
write. `ExitLedger`'s `Resources` obligation subsumes design A's proposed separate
`ResourceState{Held,HandedOff}` field. Both already-terminated branches (`manager.rs:1137`,
`process_task.rs:234`) re-key onto the ledger state.

**Files.** `kernel/src/process/process.rs`, `kernel/src/process/manager.rs`,
`kernel/src/task/process_task.rs`, `kernel/src/test_framework/btrt.rs` (the
`claim_exit_slot`/`record_exit` split), tracing provider, tests. **3 commits; the named split seam (rule 5) is
{class-A fused transitions + ledger array} / {class-B markers + btrt split + T4 ruling}.**

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
ledger term of the removal gate now live. CoW frame accounting unchanged vs P5a for the same workload.

**v3 gate extras (closure B) — every branch of the exactly-once argument is exercised:**
1. **Class A has no `Claimed` state, observably.** Instrument the ledger reader to sample obligation
   states across the 64-child workload and assert `Sigchld`, `ParentWake`, `Reparent` and
   `Resources` are **never observed in `Claimed`** — the structural claim is measured, not asserted.
2. **T4's three `Report` destinations, one injection each.** (a) kill the claimer after `finished` →
   assert `→ Completed`, exactly one btrt report total, `LEDGER_EFFECT_AMBIGUOUS == 0`; (b) kill it
   before `started` → assert `→ Pending`, the token is preserved, the next claimer records, exactly
   one report total; (c) kill it between the two → assert `→ Completed`,
   `LEDGER_EFFECT_AMBIGUOUS{report} == 1`, and **still exactly one report** (never two). Assertion
   (c) is the one that would have failed under v2's rule, and the PR body records that.
3. **`Fds` custody** *(v3 repair — assertions restated against the three-step API).* Kill a claimer
   between `begin_fd_close` and `endpoint_hangup`, and again between `endpoint_hangup` and
   `finish_fd_close`; in both cases assert the descriptor is still owned by `fd_in_flight`, that the
   recovering claimer re-tickets **that** descriptor rather than taking a new one, that
   `endpoint_hangup` replays as a no-op (`FD_HANGUP_REPLAYED > 0`, endpoint close count unchanged),
   that the descriptor is destroyed **exactly once** by `finish_fd_close`, and that endpoint refcount
   accounting balances. A double close is proven unrepresentable at compile time by a unit test
   asserting `CloseTicket` exposes no close/consume operation, and at run time by an injection that
   calls `finish_fd_close` twice and finds an empty slot on the second call.
4. **`PARENT_WAKE_KICKS >= PARENT_WAKE_COMPLETED`** is asserted as an inequality, with a comment
   naming the kick as deliberately repeatable — so a future reviewer does not "fix" the inequality
   into an equality and reintroduce a lost wake.
5. **Both join arms nonzero** (P6a's `TOMBSTONE_JOIN{reap_second}`/`{retire_second}`) now that the
   ledger term of the removal gate is live rather than vacuous.

**Strictly better.** Closes PR #418's own declared follow-up ("duplicate SIGCHLD / stale exit code on
the already-terminated path") **with a mechanism, not a convention**. Explicitly **not** notification
suppression (disproven in review): the obligation is shared by every producer, so a later pass claims
and redeems it rather than being skipped.

**Revert.** Re-key the two branches onto `is_terminated()` and drop the ledger, the markers and the
btrt split; P6a's tombstone gate survives independently (its ledger term becomes vacuously true
again, and both join arms still fire).

---

## Phase 7 — FD closure leaves the PM lock *(AC-9)*

> ⚠ **DEBT-2 — the endpoint-CAS idempotence below is REJECTED and must not be built as written.**
> The `CloseTicket` repair makes `endpoint_hangup` idempotent by a **CAS on the shared endpoint**.
> Live close accounting in the pipe/PTY/TCP endpoints is **per-descriptor**: a `dup`'d descriptor is a
> second *legitimate* decrement on the same endpoint, and an endpoint-level CAS would suppress it —
> trading a double-close for an endpoint that never hangs up. This phase owns the replacement: a
> replay token unique to the `(row, fd)` pair being closed, surviving the unlocked window, so a
> replayed exit-close of the same descriptor is suppressed while a legitimate close of a *different*
> descriptor on the same endpoint still decrements. The scope and gate below are retained as the
> shape of the phase, **not** as a ratified mechanism, and this phase's tranche cannot ratify until
> they are rewritten and pre-checked. See the DESIGN-DEBT REGISTER.

**Scope.** *(v3 repair — the API is three calls, not one, because a single `take_next_for_exit()`
returning the descriptor cannot also retain custody of it.)* Add the three-step exit-close API:

- **`FdTable::begin_fd_close(pid) -> Option<CloseTicket>` [PM]** — if the row's single `fd_in_flight`
  slot is empty, move exactly **one** `(fd, FileDescriptor)` out of the table into it; if the slot is
  already occupied (a claimer died mid-close), re-ticket **that** descriptor and take nothing new.
  Returns a `CloseTicket { fd, endpoint: EndpointRef }` — a clone of the *endpoint handle*, **not**
  the descriptor, with **no close/consume operation defined on the type**. Returns `None` only when
  the table and the slot are both empty. Allocates nothing.
- **`endpoint_hangup(&CloseTicket)` [no lock held]** — the only step that takes a pipe/PTY/TCP lock.
  **P7 must make this idempotent and gate it**: the hangup is a CAS-guarded state transition on the
  endpoint, so a replay after a dead claimer is a no-op. This is an obligation of this PR, not an
  assumption inherited from elsewhere.
- **`FdTable::finish_fd_close(pid)` [PM]** — set `hangup_done`, then drop the owning descriptor out
  of the slot and clear it, in one acquisition. This is the **single destructive step**, and its
  refcount decrement takes no endpoint lock because the hangup already ran.

Retire the existing allocating `Process::take_fd_entries() -> alloc::vec::Vec` (`process.rs:335`)
rather than reusing it. Apply at both convergence points and at Phase 2's SIGKILL commit. The `Fds`
obligation brackets the loop (`T2` before the first `begin_fd_close`, `T3` when the table **and the
slot** are proven empty under PM) — the explicit control flow is in DESIGN §2.5.

**Files.** `kernel/src/ipc/fd.rs`, `kernel/src/process/process.rs`,
`kernel/src/process/manager.rs`, `kernel/src/task/process_task.rs`, tests.
**~160 lines, 2 commits.**

**Gate extras.** `FD_CLOSES_UNDER_PM == 0` — a **measured drop** from the nonzero P0 baseline, not a
zero that was always zero. 256-FD test with a large process set: measure and assert a bounded PM hold
(one descriptor per acquisition). P0 ratchet forbids any close/reclaim call inside a request/commit
transaction body. Soak. *(v3, closure B — restated by the v3 repair against the three-step API: also assert
`fd_in_flight` holds at most one descriptor at any sampled instant and is `None` at quiesce for every
row; that the owning `FileDescriptor` is **never** observed outside the table or the slot; that a
replayed `endpoint_hangup` leaves the endpoint's close count unchanged while `FD_HANGUP_REPLAYED`
increments; and that `CloseTicket` exposes no close/consume operation — the custody invariant that
makes `Fds` exactly-once by ownership rather than by a flag.)*

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
> flow:** `T2: Fds→Claimed` inside the commit, then P7's three-step loop — `begin_fd_close()`
> **[PM]** per descriptor, `endpoint_hangup(&ticket)` with PM dropped, `finish_fd_close()` **[PM]** —
> then `T3: Fds→Completed` under a fresh acquisition once the table **and the slot** are proven empty
> (DESIGN §2.5). *(v3, closure B, restated by the v3 repair: the descriptor lands in the ROW's
> single-slot `fd_in_flight` and **stays there** — the unlocked step receives a non-owning
> `CloseTicket`, and `finish_fd_close` is the one place the owning value is destroyed, in the same PM
> acquisition that clears the slot. The earlier shape, in which one call both retained custody and
> returned the descriptor for an unlocked close, is not a state that exists. A descriptor is
> therefore in exactly one place at every instant and a double close is unrepresentable, which is
> what lets `Fds` be a class-B obligation with no started/finished bits.)*

**Files.** new `kernel/src/task/teardown.rs`; `kernel/src/task/process_task.rs`,
`kernel/src/arch_impl/aarch64/context_switch.rs` (Tier-2), `kernel/src/arch_impl/aarch64/syscall_entry.rs`,
`kernel/src/process/manager.rs`. **3 commits; the split seam (rule 5) is
{trampoline + commit} / {boundary hook + normal-exit routing}.**

**Gate extras.** **Hook liveness (the anti-dormancy gate): `EXIT_HOOK_ENTRIES > 0` and
`EXIT_HOOK_ENTRIES == EXIT_COMMITS` on a normal boot** — if any exit reached `do_exit_current` without
traversing the hook, or the hook never fired, the phase fails. Repeated/nested exit injection produces
one status, one SIGCHLD, one parent wake, one report; FD closes and reclaim enqueue observed with PM
unlocked (`FD_CLOSES_UNDER_PM == 0`, `RECLAIM_ENQUEUE_UNDER_PM == 0` still). Disassembly review of the
hook proving no logging, allocation, page-table walk, or contended lock on the return tail (Tier-2
requirement). **x86_64 user-return audit (OQ-9): enumerate every user-return path and prove each
reaches the common hook; if one bypasses it, halt and escalate for operator approval rather than
patching Tier-1 syscall entry.** Soak. All five Tier-1 files byte-identical (the hook is *outside*
every Tier-1 file).

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
> suppresses the victim-owned commit's remote counterpart; P10a/b/c/d consume it, one family per PR.

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
P0 ratchet that may only shrink. P10a/b/c shrink it; **P10d empties it and deletes the arm.** Both
arms are exercised by this PR's own tests (rule 2), so nothing dormant lands.

> **v3 (closure D) — the no-new-block admission interlock ships HERE, with the predicate.** The
> re-ratification's fourth FATAL is that the predicate is a *classification at an instant*: a victim
> classified `true` because it is running can, one instruction later, enter an unmigrated wait — the
> request is published, the remote mark was suppressed, and nothing ever forces it to a boundary. It
> is not enough for the predicate to be right when evaluated; the world must not change under it.
>
> **Every blocking primitive gains a pre-block acquire-load of the calling thread's own exit-request
> word and refuses to block when it is latched**, returning a refusal that the caller maps to
> `EINTR` or its family's existing cancel path. The check lives **inside the primitives**, so no wait
> site can forget it and no future wait site can be added without it. The nine entry points are
> complete at `985881a6` and are pinned by a P0 ratchet rule:
>
> | Primitive | file:line |
> |---|---|
> | `Scheduler::block_current` | `task/scheduler.rs:1726` |
> | `Scheduler::block_current_for_signal` | `task/scheduler.rs:1897` |
> | `Scheduler::block_current_for_signal_with_context` | `task/scheduler.rs:1916` |
> | `Scheduler::block_current_for_child_exit` | `task/scheduler.rs:2065` |
> | `Scheduler::block_current_for_timer` | `task/scheduler.rs:2153` |
> | `Scheduler::block_current_for_io` | `task/scheduler.rs:2218` |
> | `Scheduler::block_current_for_io_with_timeout` | `task/scheduler.rs:2227` |
> | `Scheduler::block_current_for_compositor` | `task/scheduler.rs:2386` |
> | `WaitQueueHead::prepare_to_wait` | `task/waitqueue.rs:52` |
>
> **The classification becomes a one-way door.** From the moment a request is latched, entering an
> unmigrated family is impossible; a victim can only move from the false set to the true set (by
> waking), never back. That is what makes the predicate sound rather than merely plausible.
>
> The check costs one acquire-load of a per-thread atomic and takes **no new lock**, so it cannot
> deadlock and cannot invert any ordering (DESIGN §4.3).
>
> **v3 repair — the interlock is PERMANENT, and `EXIT_BLOCK_REFUSED{family}` does not fall to zero.**
> v3 as first written said the interlock "is **not** redundant after migration" and, in the same
> sentence, that its counter "falls to zero as each family lands". Those cannot both hold — a guard
> that never fires *is* redundant — and the second half is wrong about what migration changes.
> Migration changes the fate of a victim **already blocked** when the request lands (remote mark →
> victim-owned cancellation). It changes nothing for a victim whose request is **already latched**
> and which then tries to *enter* that wait: it must still be refused admission, in a migrated family
> exactly as in an unmigrated one, because letting it block would manufacture the very cancellation
> work the interlock exists to avoid and would reopen a lost-wakeup window between the block and the
> cancel. P9's own per-family admission-race test (gate extra 1 below) constructs exactly that
> scenario, and it is **re-run unchanged inside each P10x PR and must still pass**. So:
> `EXIT_BLOCK_REFUSED{family}` is asserted **nonzero before and after** migration and is never
> asserted at zero; the counters that actually move at migration are
> `EXIT_LEGACY_REMOTE_MARK{family}` (nonzero → 0) and the new `EXIT_WAIT_CANCELLED{family}`
> (0 → nonzero).

**Files.** `kernel/src/task/scheduler.rs`, `kernel/src/syscall/signal.rs`,
`kernel/src/task/teardown.rs`, `kernel/src/process/manager.rs`, `kernel/src/syscall/clone.rs`,
`kernel/src/task/waitqueue.rs` (the ninth interlock site). **3 commits; the named split seam (rule 5) is
{request API + kick plan + predicate + interlock} / {group scope + seal}, and this PR is expected to
take it — two revert stories.**

**Gate extras.** Two-CPU aarch64: kill a thread running remotely at EL0 and prove **its own TID**
executes the exit commit, with zero post-request EL0 trace for the victim and `EXIT_VICTIM_OWNED > 0`.
Kill a thread blocked in an (unmigrated) futex wait and prove it still dies with the correct status
via the legacy arm, `EXIT_LEGACY_REMOTE_MARK{futex} > 0` — **the fallback is tested, not assumed**.
Deterministic clone-vs-seal barrier: the child is either in the batch or `sys_clone` returns `EAGAIN`
— never a runnable unrequested member. No resource claim before the batch commits. Soak.

**v3 gate extras (closure D) — the race the predicate must survive, run deliberately:**
1. **The exact instability scenario.** A victim spins at EL0, is classified `true`, and then attempts
   to enter `sys_futex` / `pause` / `nanosleep` / a `WaitQueueHead` read — one test per family.
   Assert in each case that the syscall returns `EINTR` **without** the thread ever reaching
   `Blocked*`, that `EXIT_BLOCK_REFUSED{family} > 0`, and that the victim reaches the boundary hook
   and commits its own exit. **Without the interlock this test hangs** — the PR body records that
   pre-image so the test is proven to be a real test.
2. **All nine primitives are covered**, not just the four with a syscall entry point: a unit-level
   injection calls each primitive with a latched request and asserts refusal, including
   `block_current_for_compositor` and `block_current_for_io_with_timeout`, which no userspace test
   reaches directly.
3. **Ratchet:** the blocking-primitive set is exactly the nine above; adding a tenth without the
   interlock fails CI. ⚠ **DEBT-3 must be closed before this phase's tranche ratifies (#580):**
   `syscall/futex.rs:115`, `task/scheduler.rs:2607`, `task/kthread.rs:151`/`:183` and the dead
   `Thread::set_blocked()` pair are live blocking publications outside the nine, and a victim can
   still block after request publication through any of them. This gate as written would pass while
   the hole stands — and so would the ratchet, which pins names, not state writes.
4. *(closure F; observation half named by the v3 tranche pass)* the group cutover's expedite evidence
   is **batch-attributed**: every `EXIT_SGI_SENT` event for a group kill carries the same batch id,
   the count matches the number of kicked members, and no event from an unrelated reschedule appears —
   the generic send helpers are still unwired and P0's zero-baseline assertion still holds for them.
   P9 runs **before** P8's boundary hook exists in the phase order only if the tranche is built that
   way; as ordered here P8 precedes P9, so the observation half of this gate is
   `EXIT_REQUEST_OBSERVED{pid}` and the `EXIT_KICK` table is already deleted. **If P9 is ever
   re-sequenced ahead of P8, this gate reverts to `EXIT_KICK_OBSERVED{pid}` and must additionally
   assert `EXIT_KICK_BUCKET_COLLISION == 0` for the batch** — a group kill of more than one victim is
   exactly the workload that can alias two pids into one 64-entry bucket, which is why the collision
   counter exists rather than being argued away.

**Strictly better.** Nothing that can reach a return boundary is torn down remotely any more; group
membership is atomic by construction; #471's seal ships. *Honest bound (R-16):* victims blocked in
unmigrated wait families are still torn down remotely — that is P10's job, it is counted per family,
and it is unchanged from what P2 already shipped rather than a new regression. *(v3: and a victim can
no longer slip from the reachable set into an unmigrated family after classification, which was a
correctness hole rather than a bound. Note also the new `P5 → P9` edge: this phase introduces
group-scoped kill, so the init-sibling refusal must already be merged or P9 would hand userspace a
way to kill init that `main` does not have.)*

**Revert.** Restore the remote-marking body and pid-scoped SIGKILL — i.e. fall back to Phase 2's
already-safe behaviour, not to `main`.

---

## Phase 10 (a/b/c/d) — Killable-wait contract, one family per PR *(was Phase 9)*

**Scope — v3 (closure D): the inventory is CLOSED at four families, not three.** 10a futex;
10b `WaitQueueHead` + stdin/TTY readers; **10c `BlockedOnSignal` — `pause`/`sigsuspend` (NEW: v2
omitted this live family while simultaneously claiming the allowlist reaches empty)**; 10d child-wait
+ timer/nanosleep + completion/I-O **and the deletion of the legacy arm**. Each sub-phase: on a fatal request the victim is made **runnable with its saved
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

**10c — the `BlockedOnSignal` family** *(new in v3, closure D)*. Live at `985881a6` in four
functions with two architecture variants each way: `syscall/signal.rs:819`
(`sys_pause_with_frame`, x86), `:1314` (`sys_sigsuspend_with_frame`, x86), `:1815`
(`sys_pause_with_frame_aarch64`), `:2097` (`sys_sigsuspend_with_frame_aarch64`), plus the legacy
`sys_pause` at `:769`; the wake side is `Scheduler::unblock_for_signal` (`task/scheduler.rs:1970`),
and `tty/driver.rs:602-603` documents that the TTY unblock path already spans both `Blocked` (stdin
read) and `BlockedOnSignal`. Each of the four blocking functions runs its own HLT/re-check loop, so
the migration is "the resumed loop gives a latched fatal request priority over the ordinary
signal-arrived branch, deregisters, then branches to the trampoline" — the same shape as the other
families, applied four times. *(Why it is not folded into 10b despite the TTY adjacency: four
syscall entry points across both architectures would put 10b well over the ~150-line/4-file target.
OQ-8 records the batching alternative for the operator.)*

**10d additionally deletes the legacy arm**: with the allowlist empty, `exit_request_is_boundary_reachable`
becomes total, the remote-marking body of `terminate_process_threads` is deleted, and
`EXIT_LEGACY_REMOTE_MARK` must read 0 for a full run. **This is where #491 is complete and AC-11 is
fully discharged** — v1 claimed that at the cutover phase, which was premature; v2 claimed it at
P10c, which was one family short.

**Files (per sub-phase).** the family's own file(s) + `kernel/src/task/scheduler.rs` +
`kernel/src/task/teardown.rs` + its test. **~150 lines each (10c ~170, 10d ~180), 1-2 commits each.**

**Gate extras.** Per family: inject an exit request and prove the family's registry/heap **no longer
contains the TID before the exit commit runs** (deregistration-before-commit is the load-bearing
assertion the ratification asked for); SIGKILL of a victim blocked in that family reaps the right
status; `EXIT_VICTIM_OWNED` increments for that family and `EXIT_LEGACY_REMOTE_MARK{family}` drops to
0 while remaining nonzero for families not yet migrated (both halves asserted); **(v3 repair — the
migration evidence is a *pair* of counters, and `EXIT_BLOCK_REFUSED` is not one of them)**
`EXIT_WAIT_CANCELLED{family}` rises from 0 to nonzero — a victim found **already blocked** in this
family at request time and cancelled by its own resumed continuation, which is the observable
difference between "torn down remotely" and "cancelled cleanly"; and **P9's admission-race test for
this family is re-run unchanged, still asserting `EXIT_BLOCK_REFUSED{family} > 0`**. The interlock is
permanent: an already-latched victim is still refused admission to a *migrated* family, so a gate
demanding this counter reach 0 would be demanding a regression. Families not yet migrated are
**reported by counter** — never silently advertised as killable. **10c only:** all four `BlockedOnSignal` entry points tested on **both** architectures
(two prior rounds shipped accidental x86 divergences), including the documented pause/kill race at
`signal.rs:834` where the signal can arrive before the thread reaches `BlockedOnSignal`.
**10d only:** allowlist
empty asserted as an exact set, `EXIT_LEGACY_REMOTE_MARK == 0` over a full run including the group and
CLONE_VM stress, and the ratchet asserts the remote-marking body is gone by name. Soak on 10d.

**Strictly better.** Each family converts from "torn down remotely by the legacy arm" to "dies
promptly, on its own thread, with its wait registration cleanly removed". Unmigrated families are
unchanged and visible.

**Revert.** Per family, independently: return the family to the allowlist (10a/10b/10c), or restore
the legacy arm plus the family (10d).

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

**Dependency note (v3, closure D).** `Process::terminate` cannot be deleted until **P10d** has landed:
the legacy remote-mark arm is one of its live callers, so deleting the function while the arm exists
would break the fallback that R-16 depends on. v2's graph permitted P11 before the deleting subphase;
the `P10d → P11` edge is now explicit in §0.

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

> ⚠ **DEBT-6 — the group-membership drop is scoped to EXTERNALLY-ORIGINATED SIGNALS ONLY, and
> inverting that scope fails silently.** The drop applies to `ExitIntent.origin == Signal` and only
> that — sender-agnostic, so a self-directed `kill(getpid())`/`raise` from init is still a signal and
> is still dropped (Linux's `sig_task_ignore` consults `SIGNAL_UNKILLABLE` and disposition, never the
> sender). **`ExitSyscall` (init's own `exit_group`, or the exit of its last member) and `FatalFault`
> BYPASS the membership test entirely**, so init's own exit still seals, latches and reaches the
> kernel-fatal panic. An unscoped check makes that panic path unreachable and the system hangs with
> init alive — no assertion fires, nothing logs. This phase's gate must carry all three negatives (a
> deliberate init `exit_group` still panics; a `FatalFault` injection still panics; an ordinary group
> kill still works) and record the unscoped-check pre-image. See the DESIGN-DEBT REGISTER.

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
- **v3 (closure E, end 2): the drop check is a GROUP-MEMBERSHIP test, not a row test.** S1's
  group-seal transaction asks *"is the designated init a member of the target effective thread
  group?"* — not *"is the target row the designated init?"*. If it is, the **entire request is
  dropped**: nothing is sealed, no member is marked, no obligation is created, and the send returns
  0, counted `INIT_FATAL_SIGNAL_DROPPED{group}`. The membership test runs in the **same PM
  acquisition** as the seal it guards, so a racing clone cannot slip past it (the clone either
  published before the transaction and is a member the test sees, or acquires PM afterwards and
  finds a sealed group). This is defence in depth behind P5b's clone-admission refusal, which already
  makes an init sibling unconstructible; **neither end is load-bearing alone and both are separately
  tested**, so neither can rot in the other's shadow (residual R-18).
- **v3 repair: the drop is scoped by REQUEST ORIGIN, or it swallows init's own `exit_group`.** As
  first written the membership test dropped *any* request whose target group contains the designated
  init — which includes init's own `exit_group`, the very thing the bullet above declares
  kernel-fatal. The unscoped check would silently discard it and make the required panic path
  unreachable, inverting the policy it was added to protect. Every `ExitIntent` therefore carries an
  explicit `origin`, set by its producer, and the drop applies to exactly one value:

  | `ExitIntent.origin` | Produced by | Init-group drop? |
  |---|---|---|
  | `Signal` | a default-fatal signal reaching disposition with no handler installed — `sys_kill`, `sys_tgkill`, `sys_killpg`, **including a self-directed `kill(getpid(), …)` / `raise`** | **YES** — dropped, nothing sealed, send returns 0, `INIT_FATAL_SIGNAL_DROPPED{group}`++ |
  | `ExitSyscall` | `sys_exit_group`, or `sys_exit` of the group's last member | **NO** — the seal proceeds, the request commits, `INIT_DEATH_LATCH` is set, S5 panics |
  | `FatalFault` | an unhandleable synchronous fatal fault taken by a member (P11's converged path) | **NO** — same; this is the second kernel-fatal producer |

  The `Signal` drop is deliberately **sender-agnostic**, which is both Linux-faithful and stricter
  than "externally-originated": Linux's `sig_task_ignore` consults `SIGNAL_UNKILLABLE` and the
  disposition, never the sender, so `kill(1, SIGKILL)` issued *by init itself* is dropped too — only
  `force`d kernel-only signals bypass, which is exactly the `FatalFault` row. `ExitSyscall` and
  `FatalFault` **bypass the membership test entirely**, so the kernel-fatal path is preserved by
  construction rather than by the check happening not to fire. Note that P5b's clone-admission refusal
  makes init's group a singleton, so an `ExitSyscall`-origin request inside init's group can only
  have come from init — no sibling exists that could issue `exit_group` on init's behalf.
  A P0-style ratchet rule pins that `origin` is set at every `ExitIntent` construction site and that
  the drop branch tests `origin == Signal` explicitly, never by default.
- S1 sets `INIT_DEATH_LATCH` with one relaxed store, **only** for a committed, certainly-attributed
  victim. Because external kills are dropped at send, the latch's only producers are init's own exit
  commit (P8) and the unhandleable-fault path (P11) — a strictly smaller producer set than v1's.
- S5 reads the latch in ordinary kernel context with all guards out of scope and DAIF restored,
  records a pre-panic lock/IRQ snapshot, then panics. **No `#[cfg]` gate** — designation is runtime
  data, so a build that never designates an init can never trip the policy, and
  `interactive = ["testing"]` cannot invert anything.

**Files.** `kernel/src/syscall/signal.rs`, `kernel/src/signal/delivery.rs`,
`kernel/src/task/teardown.rs`, `kernel/src/process/manager.rs` (the group-membership test), tracing
reader, tests. **~140 lines, 2 commits.**

**Gate extras.** `kill(1, SIGKILL)` from userspace **returns 0**, init survives, its pending signal
set is unchanged, `INIT_FATAL_SIGNAL_DROPPED == 1`, and `INIT_DEATH_LATCH == 0`. **No test asserts
`EPERM`** — the v1 assertion is deleted, not inverted. A signal init *does* handle is delivered
normally (handler runs, counter does not increment). Deliberate init-death test (init calls
`exit_group`) asserts the panic message **and that the panic reports completely** on serial (proving
no lock was held), with the snapshot showing PM owner `None`, scheduler owner `None`, normal IRQ
state. `INIT_PANIC_WITH_LOCK == 0`. A normal boot asserts the latch stays 0 for the whole run
**including the `smoke_hello_time` harness** — the exact build all four prior attempts broke. A
test-build PID-1 process that is *not* designated exits normally.

**v3 gate extras (closure E, end 2) — the group path is tested, not assumed unreachable:**
1. A **group-scoped** fatal request naming the effective TGID that contains the designated init is
   dropped: `INIT_FATAL_SIGNAL_DROPPED{group} == 1`, **nothing is sealed** (the group lifecycle stays
   `Open`, asserted directly), no member is marked, the send returns 0, and init survives.
2. The **sibling** attack is proven unconstructible end-to-end rather than argued: the test attempts
   `clone()` from init (refused with `EINVAL` by P5b), then walks the process map at quiesce and
   asserts **no row other than init carries init's effective TGID**. The two ends are asserted
   separately so a regression in either is attributed correctly.
3. A group kill aimed at an **ordinary** group is unaffected — the whole group dies, proving the
   membership test is scoped to the designated init and is not a blanket group-kill disable.
4. *(v3 repair — the origin carve-out is proven, not assumed.)* **Init's own `exit_group` still
   panics through the group path.** The deliberate init-death test is re-run as an `ExitSyscall`-origin
   group request naming init's own effective TGID: assert `INIT_FATAL_SIGNAL_DROPPED{group}` does
   **not** increment, the group **is** sealed, `INIT_DEATH_LATCH == 1`, and S5's panic reports
   completely with PM owner `None`. Under the unscoped membership test this request is dropped and
   the test hangs with init alive — the PR body records that pre-image, so the carve-out is proven to
   be load-bearing. Paired with a `FatalFault`-origin injection (init takes an unhandleable fault)
   asserting the same latch-and-panic outcome, and with a self-directed `kill(getpid(), SIGKILL)`
   **from init** asserting the opposite: dropped, returns 0, init survives, `INIT_DEATH_LATCH == 0`.

**Strictly better.** Closes #464 with the runtime flag the issue asked for, on the fifth attempt,
without the cfg landmine that killed the first four — and with Linux's actual semantics rather than an
undocumented ABI divergence. *(v3: and the protection is scoped to the group the seal actually
operates on, so it cannot be walked around through a sibling — the composition hole the
re-ratification found between P9's group scope and v2's row-scoped check.)*

**Revert.** Delete the latch, the drop rule, the group-membership test and the escalation call;
identity (P5a) and the clone-admission refusal (P5b) survive independently — which is the point of
putting closure E's first end in P5.

---

## 1. Cumulative outcome at each stop point

| Stop after | #491 | #464 | #471 | Net vs `main` |
|---|---|---|---|---|
| P0 | — | — | — | Bypass surface pinned; #492 overflow visible; two pre-existing lock violations measured |
| P1 | — | — | — | + grace cannot elapse unordered/empty; refusals attributable; no proof under the queue lock |
| P1 *(v3)* | — | — | — | + the drain cannot livelock: bounded pass + park-until-epoch-advance |
| **P2** | **live UAF closed** (remote-mark strength) | — | — | + quarantine, expedite, SIGCHLD on kill; + no reclaim enqueue under PM; **+ receipts cannot be dropped by any of the nine adapted sites (7 callers + SIGKILL arm + `handle_thread_exit`); + expedite evidence is teardown-attributed** |
| P3 | ↑ | — | **detach done** | + wrong-victim-after-exec impossible; **+ no PM→SCHEDULER nesting left on any creation path** |
| ~~P4~~ | — | — | — | *Dissolved: lock-order half is in P3, kernel-stack half is #579. AC-8 is not closed by this plan* |
| P5a | ↑ | **identity done** | ↑ | + AC-1/4/5 structural (identity only; no behaviour change on init death) |
| P5b *(held on #575)* | ↑ | ↑ | ↑ | **+ init can never acquire a CLONE_VM sibling** |
| **P6a** | ↑ | ↑ | ↑ | + a row outlives its reap; removal is proof-gated **and has an explicit two-event trigger in both orders** |
| P6b | ↑ | ↑ | ↑ | + exactly-once notification with a claim protocol (#418's own follow-up); **+ four obligations commit their effect with their completion, and the two that cannot carry effect markers with a stated winner** |
| P7 | ↑ | ↑ | ↑ | + no FD/endpoint lock or alloc under PM on any exit |
| P8 (needs OQ-5) | ↑ | ↑ | ↑ | + normal exit is victim-owned, through the boundary hook |
| **P9** | **nothing boundary-reachable dies remotely** | ↑ | **seal done** | + atomic group membership; legacy arm counted per family; **+ a latched victim can no longer enter a new wait, so the classification is a one-way door** |
| P10a/b/c/d | **complete at 10d** | ↑ | ↑ | + each of the **four** wait families becomes promptly killable; legacy arm deleted at 10d |
| P11 | ↑ | ↑ | ↑ | + one machine for all five paths; one notifier; live livelock fixed |
| P12 | ↑ | **complete** | ↑ | + Linux-faithful init protection without the cfg landmine, **scoped to the group the seal operates on** |

Stopping anywhere is a shippable, defensible state. **P2, P3, P5, P9, P10d, P12 are the six points
where an issue's headline claim actually changes** — those are the PR bodies that must be written most
carefully, and the ones where an overstated commit message would be a blocking finding. *(v2: #491's
"complete" milestone moved from the cutover phase to the family-deleting subphase, because the legacy
remote-mark arm is still live until then. Claiming completion at P9 would be exactly the kind of
overstatement the round-level stop rule exists to catch. **v3: that milestone moves once more, from
P10c to P10d, because the `BlockedOnSignal` family was missing from v2's inventory — the completion
point is wherever the allowlist actually empties, and it moved because the inventory was wrong, not
because the criterion changed.*)**

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

**Ratification gate — this IS condition 7, correctly labelled** *(v3, closure G)*. The seventh
condition of the first refusal was **"close conditions 1-6 and obtain a NEW ratification pass before
implementation begins"**. Both the v2 DESIGN and PLAN changelogs mislabelled that row as the OQ-1
init-policy decision, which the re-ratification flagged as acceptance-contract drift; the OQ-1
adoption is a **coordinator decision** and is now recorded as such, separately, in both changelogs.

The gate itself is unchanged in force and binding: **no phase is cleared for build until a
ratification pass returns `ENDORSE: YES` for the tranche that contains it.** Condition 7 **cannot be
closed by the document that seeks the ratification**; it is listed as OPEN in both changelogs by
construction, and the re-ratification was right that a document claiming to have closed it is
claiming the wrong thing.

**What the v3 tranche pass changed is the gate's SCOPE, not its strength.** Two full ratification
passes and two pre-checks refused these documents as a whole, each time on items owned by phases far
downstream of the ones ready to build; a single whole-document bar makes P0's readiness hostage to
P12's argument. The gate is therefore **discharged per tranche**:

- **Tranche 1 = P0 + P1 + P2** — ratified (`ENDORSE: YES`) and **merged**. P0 and P1 are the two
  behaviour-preserving PRs the first reviewer called "the correct first two"; P2 is the phase that
  closes #491's live UAF.
- **Tranche 2 = P3 (with the creation-path parity folded in) + P5a** — **re-ratified by the operator
  on 2026-08-16** against `P3-RERATIFICATION-2026-08-15.md`, after six refusals in August and the
  Option-A foundation-hardening campaign that closed the substantive grounds. **P5b is held on #575**
  and **P4 is dissolved** (§0.0). Tranche 2 owns none of the seven debts, so the register's binding
  rule does not gate it — the same structural position tranche 1 was in.
- **Every later phase remains uncleared** until its own tranche is submitted, pre-checked and
  ratified. Ratifying tranche 2 grants nothing to P6a and beyond.
- **The DESIGN-DEBT REGISTER gates the later tranches.** A tranche containing a debt's owner phase
  cannot be ratified until that debt's closure is *written into these documents and pre-checked*.
  Tranche 1 owns none of the seven debts, which is the entire reason it can be submitted while they
  stand open. A tranche that contains P6a, P6b, P7, P8, P9, P10 or P12 arrives carrying its debt
  closure or it does not arrive.

The condition is applied **more** times under this model, not fewer, and no phase ever builds on an
unratified argument.
