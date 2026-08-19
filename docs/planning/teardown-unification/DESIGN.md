# Teardown Unification — FINAL DESIGN v3 (panel synthesis, revised against two ratification passes)

**Status:** **Tranche-ratified document. Tranche 1 (P0+P1+P2): RATIFIED and COMPLETE — merged to `main`. Tranche 2 (P3 + P4 + P5a; P5b held on #575): **RE-RATIFIED, effective on pre-check pass (PLAN §2 condition 7)** — the operator's 2026-08-16 decision ratified *proceeding per* `P3-RERATIFICATION-2026-08-15.md` (document repair → one adversarial pre-check → implementation), and the ratification takes effect when the repaired text lands with a passing pre-check; no tranche-2 phase is cleared for build before that. See PLAN §0.0. Later phases: design-debt register applies; sections may change before their tranche ratifies. Tranche-2 anchors re-verified at `main` @ `2c7b8798` — §0.3.1 governs where earlier sections disagree.**
Design-only proposal for operator ratification. No implementation performed, no runtime
evidence claimed. Every build/QEMU/Parallels statement in this document is a *gate to be run*, never
a result already obtained.

**Base:** `main` @ `eebc8868` (anchors re-verified against the live tree during synthesis — see §0.3;
re-verified during the v2 revision at `main` @ `c9efdcc7`; re-verified again for **v3** at
`main` @ `985881a6` — v2 of these documents merged, no kernel change. Every file:line in this
document was read out of the tree at `985881a6`, and five v2 anchors that had drifted
are corrected in §0.3).
**Scope:** #491 (spine), #464, #471. Acknowledges and does not foreclose #448, #492, #493.
**Inputs:** design A (minimal-incremental, Opus), design B (Linux-fidelity, Codex Sol), design C
(invariant-first, Codex Sol), two adversarial judge verdicts (Opus; Codex Sol), the **first Codex Sol
ratification refusal** (ENDORSE: NO — 2 fatal seams, 7 majors, 1 minor, 7 conditions), and the
**second (re-)ratification refusal against v2** (ENDORSE: NO — conditions 1, 2, 3, 7 NOT-CLOSED;
5 new FATAL, 3 MAJOR, 1 MINOR).

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
> **Tranche 1 = P0 + P1 + P2 (ratified, merged) and Tranche 2 = P3 + P4 + P5a (ratification decided
> 2026-08-16, effective on the pre-check pass; P5b held) own none of the debts below** (their owners
> are P6a, P6b, P7, P8, P9, P10 and P12), which is precisely why each could be submitted while the
> register stands open.

| Debt | Owner phase | What must be true before that phase's tranche can ratify | Source finding |
|---|---|---|---|
| **DEBT-1 — `Report` exactly-once is not achieved; it degrades to at-most-once in one window** | **P6b** | Two things, together. **(a) The R-19 window.** When an effect marker reads `started == 1 && finished == 0`, T4 cannot tell whether `record_exit` landed; the current ruling is `→ Completed` + `LEDGER_EFFECT_AMBIGUOUS{report}`, i.e. a *possibly missing* report. P6b must either close the window with a mechanism that makes the record step itself recoverable, **or** carry an explicit operator acceptance that this is the round's single at-most-once obligation, with the counter asserted `0` on every healthy boot and moved only by deliberate injection. A silent third option — restating exactly-once while shipping at-most-once — is what the pre-check rejected twice and is not available. **(b) `on_process_exit` split phasing.** The split into `claim_exit_slot` (pure atomics, PM-callable) + `record_exit` (reaches SERIAL via `finalize()` → `ktap::emit_summary`) must be shown not to reorder the registry-slot clear relative to the serial emission, and must land in the **same PR** as T4 — a recovery rule and the marker it reads must never be separated by a merge boundary | v3 pre-check and re-check, item **B NOT-CLOSED** (both passes, verbatim unchanged): *"R-19 permits missing report when `started==1 && finished==0`"*. DESIGN §1.6 (class-B `Report`), §6 R-19; PLAN P6b |
| **DEBT-2 — `Fds` close replay safety needs a PER-DESCRIPTOR token; the endpoint-CAS `CloseTicket` design is REJECTED** | **P7** | The v3 repair made `endpoint_hangup` idempotent by a **CAS on the shared endpoint**. The re-check's new FATAL is that live close accounting in the pipe/PTY/TCP endpoints is **per-descriptor**: a `dup`'d descriptor is a second, legitimate decrement on the *same* endpoint, and an endpoint-level CAS would suppress it — trading a double-close for a leaked endpoint that never hangs up. **P7 must not implement the CAS as specified.** It must carry a replay token that is unique to the `(row, fd)` pair being closed and that survives the unlocked window, so that a *replayed exit-close of the same descriptor* is suppressed while a *legitimate close of a different descriptor on the same endpoint* still decrements. DESIGN §1.4/§1.6/§2.5 and PLAN P6b/P7 must be rewritten to that mechanism before P7's tranche is submitted, and P7's gate must include a `dup`-then-close-both workload that fails by construction under an endpoint CAS | v3 re-check, **NEW FATAL**: *"the CloseTicket repair assumes endpoint-level CAS idempotence despite current close accounting being per-descriptor; duplicated descriptors make the CAS unsafe and would suppress legitimate closes"*; re-check item **1 NOT-CLOSED**. Also flagged by the repair pass itself as unreviewed (*"nobody has checked the current endpoints can satisfy it"*) |
| **DEBT-3 — the blocking-primitive inventory is NOT closed at nine, and the surface is WIDER than the design recorded** | **P9** | The no-new-block admission interlock is what makes the boundary-reachability classification a one-way door, and it is specified as living inside "the exact nine" primitives. **Four** live publications sit outside that set, re-verified at `main` @ `2c7b8798`: **(1)** `kernel/src/syscall/futex.rs:115` writes `thread.state = ThreadState::Blocked` **directly**, bypassing every `Scheduler::block_current*` entry point (the state is re-read at `:130`); **(2)** `kernel/src/task/scheduler.rs:2607` publishes `ThreadState::BlockedOnIO` directly — the design cited `scheduler.rs:2175-2194`, which is now `unblock`'s wake predicate, not the publication; **(3)** `kernel/src/task/kthread.rs:151` `kthread_park()` writes `Blocked` at `:183` inside `with_scheduler` with no interlock, then drops the tid from the ready queue — **not in the design at all**; **(4)** `Thread::set_blocked()` (`kernel/src/task/thread.rs:902`, `#[allow(dead_code)]`) is a differently-named mutator whose only caller is `Scheduler::block_current` (`scheduler.rs:2099`, itself `#[allow(dead_code)]`) — a dead two-level pair that publishes `Blocked` outside the inventory's naming convention, and per the repo's zero-tolerance standard a **deletion**, not an interlock target. **The existing ratchet cannot see any of these**: `tests/teardown_structure.rs:2029` pins the family by NAME (`BLOCKING_NAME_PREFIXES = ["block_current", "prepare_to_wait"]`, `pub` definitions only), so a tenth `block_current*` cannot appear unnoticed, but a direct `thread.state = ThreadState::Blocked*` write — or a mutator named anything else — is invisible to it. Before P9's tranche: each path is brought under the interlock **or** proven unreachable once a request is latched; the ratchet gains a rule that catches direct state writes and not only names; the dead `set_blocked`/`block_current` pair is deleted; P0's ratchet rule 2 (*"the blocking-primitive set is exactly the nine above"*) is restated to the corrected inventory; and DESIGN §1.5's one-way-door claim and §0.3's "inventory is declared CLOSED at four families" are restated to match | v3 pre-check and re-check, item **D NOT-CLOSED** (both passes). Surface widened and re-anchored by the 2026-08-15 tranche-2 re-ratification (§3.2), tracked as **#580**. DESIGN §0.3 (closure D), §0.3.1, §1.5; PLAN P0 rule 2, P9 |
| **DEBT-4 — the x86 reap path bypasses the tombstone gate** | **P6a** | P6a's whole claim is that a row is removed only by the **two-event join** (`reaped` ∧ `retired`, whichever writer sees the other flag set performs the removal). **`kernel/src/syscall/handlers.rs:3123`** (re-anchored at `2c7b8798`; the design said `:3101`) removes the row directly on the live x86 reap path, and its **byte-similar duplicate** at `kernel/src/syscall/wait.rs:386` does the same — so the join is not the only remover and the retention gate can pass on aarch64 while x86 still frees a row out from under an un-retired receipt. **Materially cheaper to close than when it was registered:** `ProcessManager::remove_process` is now a single four-line choke point (`manager.rs:1086-1090`) that already calls `note_process_row_removed()` → `ROW_REMOVAL_EPOCH` (`task/process_task.rs:355-357`), so **the join can be installed inside `remove_process` itself and cover both arches at once**, instead of the design's per-call-site chase. The two copies of the `complete_wait` reap block are a de-duplication seam worth taking in the same phase. (`task/process_task.rs:1807` is a third caller, but it is the `p1_row_epoch_gate` boot-test harness, not a live reap.) Before P6a's tranche: the removal routes through the join, **or** P6a's retention gate is honestly re-scoped to aarch64 with the x86 divergence named in AC-12's evidence column, a ratchet pinning both call sites by name, and a stated phase that closes it. Scoping the gate narrowly to avoid tripping on it, without naming it, is not available | v3 pre-check and re-check, item **B NOT-CLOSED** (second half, both passes): *"P6a omits the live x86 reap at `kernel/src/syscall/handlers.rs:3101`, which still removes the row directly"*; anchors and closure cost re-derived by the 2026-08-15 re-ratification (§3.2). PLAN P6a |
| **DEBT-5 — `EXIT_BLOCK_REFUSED` post-migration semantics: it is NEVER asserted to zero** | **P10** *(a/b/c/d)* | The admission interlock is **permanent**, not scaffolding. Migration changes only the fate of a victim *already* blocked; it does not change the refusal owed to an already-latched victim trying to **enter** a migrated wait — which must stay refused, or migration manufactures cancellation work and reopens a lost-wakeup window between block and cancel. Therefore: **no gate anywhere may assert `EXIT_BLOCK_REFUSED{family} == 0`**, before or after migration. `EXIT_BLOCK_REFUSED{family}` is asserted **nonzero** in P9's own admission-race test and **re-asserted nonzero** in each of P10a-d; the migration evidence is the *pair* `EXIT_LEGACY_REMOTE_MARK{family} → 0` and `EXIT_WAIT_CANCELLED{family} → nonzero`. This debt is **repaired in the v3 text**; it is registered because it is a standing guard that a later phase can silently break, and because the failure mode (a "tidy-up" that asserts the counter to zero once migration is done) reads like cleanup | v3 pre-check, MAJOR item 5: *"P10's requirement that `EXIT_BLOCK_REFUSED{family}` hits zero post-migration contradicts the permanent admission interlock"*. DESIGN §1.5, §3 AC-11; PLAN P0 counter table, P9, P10a-d |
| **DEBT-6 — P12's group-membership drop is scoped to EXTERNALLY-ORIGINATED signals only** | **P12** | The S1 group-seal check drops a fatal request when the designated init is a member of the **target group**. That drop applies to **`ExitIntent.origin == Signal` only** — sender-agnostic (a self-directed `kill(getpid())`/`raise` is still a signal and is still dropped, matching Linux's `sig_task_ignore`, which consults `SIGNAL_UNKILLABLE` and disposition and never the sender). **`ExitSyscall`** (init's own `exit_group`, or the exit of its last member) **and `FatalFault` BYPASS the membership test entirely**, so init's own exit still seals, latches and reaches the kernel-fatal panic. An unscoped check makes that panic path unreachable and the system hangs with init alive — a silent inversion of the policy. P12's gate must include the negative (a deliberate init `exit_group` still panics; a `FatalFault` injection still panics; an ordinary group kill still works) and record the unscoped-check pre-image. This debt is **repaired in the v3 text** and is registered because the failure is silent | v3 pre-check, MAJOR item 6: *"P12's group-membership signal drop isn't scoped to externally-originated signals; as written it would also suppress init's own `exit_group`, contradicting the required panic path"*. DESIGN §2.2 (End 2); PLAN P12 |

**Not in this register, and why.** Items the pre-checks raised that are *closed in the v3 text* and
carry no further obligation are recorded in the changelogs, not here: the seven-vs-nine caller count
(closed by the three-class taxonomy — DESIGN §1.7), the reversed P6a join gates, the Report-marker
phasing contradiction, and `parked_at` freshness. The design's *accepted* risks live in DESIGN §6 as
residuals R-1…R-22; a residual is a risk taken with eyes open, a **debt is an unfinished argument**,
and the two are deliberately kept in separate lists.

---

## CHANGELOG

Four revisions are recorded here. **The v3.1 tranche-1 repair is the current one**; the v3 tranche
pass, v3 and v2 tables below it are retained so the provenance of every mechanism stays readable.
Every revision is **targeted**: everything a ratification pass did not criticise is preserved verbatim.

### v3.1 tranche-1 repair — the `EXIT_KICK` slot protocol, its negative gates, and one stale API line *(current revision)*

Codex `gpt-5.6-sol` (xhigh, read-only) refused tranche-1 ratification — **`ENDORSE: NO`** — on **one**
self-contained P2 mechanism defect. Its other verdicts stand and are **not** reopened here: (1a) the
caller-count taxonomy and (1c) the parked-receipt specification both read **CLOSED**, later-debt
spillover read **NO-FLAG**, and the six-item DESIGN-DEBT REGISTER is byte-stable. This pass changes
exactly three things.

| # | Finding | What this pass wrote |
|---|---|---|
| **1** | **FATAL — `EXIT_KICK` buckets were not reusable.** Publication was `seq.fetch_add(2, Release)`; `2` is even, so it can never clear bit 0. Once any bucket had been observed once — `0 → publish 2 → observe 3 → publish 5` — every later kick in that bucket read as already observed and was **silently, permanently** lost, with no error signal. P2's single-victim gate against an initially empty table structurally cannot see this | **DESIGN §2.7's publish/observe protocol is rewritten.** `KickSlot` becomes `{ pid, at, state }`, where `state` carries `gen` (bits 63…2), `LOCK` (bit 1) and `OBSERVED` (bit 0). A publisher **reserves** ownership with an `Acquire` CAS that installs a **fresh generation with `OBSERVED` clear**, **fills** `pid`/`at` under `LOCK`, and **commits** with a `Release` store; an observer samples, **validates with a seqlock re-read**, and **CAS-claims that specific generation**. Publication *assigns* a generation rather than adding to a counter, so no low bit can survive it and a bucket is reusable indefinitely |
| **2** | **MAJOR — concurrent publication was not a coherent record.** `pid` and `at` were two relaxed stores with no publisher-side reservation, so colliding publishers could interleave them into a mismatched pair, and the collision counter — which inspected the slot without owning it — was not guaranteed to notice the race | **The same rewrite closes it, with every ordering spelled out.** The reservation CAS makes its winner the *sole* writer of `pid`/`at`; the stores are bracketed by `LOCK`; the observer's `s1 == s2` re-read rejects any sample spanning a re-publication; and the claim CAS names the exact generation. Collisions are counted in **two exhaustive arms** — reservation lost, or unobserved record displaced — each counted from a position of exclusive knowledge, so no colliding publication is silent. **PLAN P2 gains two negative gates** the old gate could not fail: sequential reuse of one bucket by a second congruent victim, and deterministic simultaneous colliding publishers |
| **3** | **MINOR — stale API narration.** §2.1's Increment-1 still spelled the SIGKILL step `with_process_manager(\|pm\| pm.exit_process(pid, -9))`, contradicting §1.7's wrapper-only receipt-custody contract that PLAN P2 actually implements | §2.1 now names **`exit_process_and_retire(pid, -9)`** and says why the direct shape is unavailable at P2 — crate-private locked half, exactly one permitted caller, zero public call sites for `ProcessManager::exit_process`. Narration only; the mechanism never differed |

**Nothing else moved.** No phase added, split, reordered or renumbered; the ledger is still **13
phases / 17 PRs**; no acceptance criterion changed; no dependency edge changed; the DESIGN-DEBT
REGISTER is untouched. Residual **R-21** gains one disclosed limit — a publisher that faults between
reserve and commit strands its bucket `LOCK`-set, which is **loud** because every later publication
into it counts a collision and P2 gates that counter at zero — and its aliasing limit is restated as
exhaustively counted rather than best-effort.

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
| **ii** | **P2's victim-attributed observation was a placeholder** | Closure F gated AC-11 on per-victim-PID pairing, but `EXIT_REQUEST_OBSERVED{pid}` is written by the return-boundary hook, which does not exist until **P8**, and the live SGI receiver (`arch_impl/aarch64/exception.rs:1761-1768`) carries only an interrupt id — no pid, no batch. The docs said "the P2-era proxy described in their own gates"; the gates pointed back at the counter table. Nothing specified the mechanism, so P2's pairing gate was vacuous | **The mechanism is chosen and written** (option: specify, not downgrade). **DESIGN §2.7** gains the `EXIT_KICK` bucket table: a fixed `[KickSlot; 64]` of three atomics, **published** by `send_exit_expedite_sgi(victim_pid, batch)` before the broadcast, **consumed** by one `compare_exchange` at the peer scheduler pass that declines to dispatch the quarantined victim — lock-free, no allocation, no new per-thread field. Counters `EXIT_KICK_PUBLISHED{bucket}` / `EXIT_KICK_OBSERVED{pid}` / `EXIT_KICK_BUCKET_COLLISION` added to P0's table, declared zero until P2 and **deleted in P8**. The proxy's weaker claim is written into the gate rather than overclaimed, and its limits are **residual R-21**. *(**v3.1**: this pass's publish step, `seq.fetch_add(2, Release)`, could not clear the observed bit and made a bucket single-use; the slot protocol is rewritten — see the v3.1 row above)* |
| **iii** | **Parked receipts: "fresh" was under-specified and the age backstop had no unit** | The repair said `ParkRecord` captures a "freshly captured fence", which still permits reusing the `RetirementSnapshot` the same drain cycle took at step 2 — stale by exactly the interval that caused the park. And the age arm was `now - parked_at_tick` "exceeds the park backstop", with the backstop never defined, while gate 3(c) required completion "within the stated backstop" | **`parked_at` captures a FRESH `RetirementSnapshot` taken AT PARK TIME** — never `reclaim.after_epoch`, never the cycle's earlier snapshot — with a **second negative gate** (P1 gate 5(b)) that an earlier-snapshot implementation fails while passing the first. **The age backstop gets a concrete unit: `PARK_AGE_BACKSTOP_EPOCHS = 64` SCHEDULING EPOCHS, summed over `fence_at_park.online_mask` — not wall time.** `parked_at_tick` is deleted; the key is derived from the captured fence, so `ParkRecord` carries no timestamp. P1 gate 3(c) becomes checkable (fires at a sum advance of 64, still parked at 63) |

**Everything else is byte-stable.** No phase was added, split, reordered or renumbered; the ledger is
still **13 phases / 17 PRs**; no dependency edge changed; no acceptance criterion changed except the
evidence columns of AC-10, AC-11 and AC-13, which were made consistent with the three fixes above.

### v3 deltas — the seven required closures from the re-ratification refusal

v2 was re-ratified and refused again (`ENDORSE: NO`): conditions **1, 2, 3 and 7 NOT-CLOSED**
(4, 5, 6 CLOSED and are not reopened here), plus five new FATAL findings, three MAJOR and one MINOR.
v3 closes them as seven lettered items.

| Closure | What the refusal said | v3 mechanism | Where |
|---|---|---|---|
| **A** | *FATAL — returned receipts can bypass retirement* (cond 3 unclosed): P2 makes every `exit_process` call a receipt producer but adapts only two callers; the other seven can discard the receipt after PM drops and destruct a root with no grace/RootProof. `#[must_use]` is advice, not a mechanism | **Receipt custody**: `RetirementReceipt` becomes crate-private with **no public constructor**; `exit_process_locked(pm, …) -> Option<RetirementReceipt>` is `pub(crate)` and callable **only** from the single public wrapper `exit_process_and_retire(pid, code)`, which takes PM, drops the guard, then enqueues. A receipt therefore never exists in any caller's hands. Its `Drop` impl **re-enqueues rather than frees** and bumps `RECEIPT_DROPPED_UNRETIRED` (CI-asserted 0), so even an unreachable drop cannot free a root. **All nine ADAPTED sites enumerated and adapted in P2**, in three classes — seven `exit_process` callers + the new SIGKILL arm (both call the wrapper) + `handle_thread_exit`'s PM-nested enqueue (routes its receipt through `phase1_result`, does **not** call the wrapper) | **§1.7** (new); PLAN P2 call-site table |
| **B** | *FATAL — T4 orphan recovery is at-least-once* + *MAJOR — tombstone retirement has no two-event join* (cond 2 unclosed) | Obligations are split into **class A** (effect is PM-owned: `Sigchld`, `ParentWake`, `Reparent`, and the take-half of `Resources`) where the effect and `→ Completed` happen in **one PM acquisition** so `Claimed` is never observable and T4 is unreachable; and **class B** (effect must leave PM: `Fds`, `Report`) where a **per-obligation effect marker written by the effect itself** lets T4 distinguish performed-but-unrecorded from unperformed, with the winning side stated per obligation. Row removal gains an explicit **two-event join**: the row carries `reaped` and `retired`, and whichever writer observes the other flag already set performs the removal **in that same PM acquisition** — both orders specified and both gated | **§1.6** (rewritten), §1.4; PLAN P6a, P6b |
| **C** | *FATAL — P1's refusal path can livelock the drain*: v2 re-inserts and "rotates" a refused candidate with no cursor or exclusion rule, so one receipt can be selected forever | The drain becomes a **bounded pass**: every entry carries `last_pass`, the scan skips entries already examined in the current pass (**no candidate can be selected twice in one pass**), and an entry that fails its live-row/cached-root proof `K = 3` times is **parked on a side list** with the fence it was parked at, invisible to the scan until a **scheduling-epoch advance** on every CPU in the captured online mask — the only event that can change the blocker's answer | **§1.8** (new); PLAN P1 |
| **D** | *FATAL — P9's reachability predicate is neither exhaustive nor stable*: no admission interlock, and the wait inventory omits the live `BlockedOnSignal` family while P10c claims the allowlist empties | **No-new-block admission interlock**: every blocking primitive (the eight `Scheduler::block_current*` entries and `WaitQueueHead::prepare_to_wait`) gains a pre-block acquire-load of the caller's exit-request word and **refuses to block** (caller maps to `EINTR`/its cancel path) when it is latched — shipped **in P9**, which makes "boundary-reachable" a one-way door. **`BlockedOnSignal` (`pause`/`sigsuspend`) is added to the inventory as its own subphase P10c**, and the deleting subphase is relabelled **P10d**; the missing hard edges are added | **§1.5** (extended), §2.7; PLAN P9, P10a-d, §0 graph |
| **E** | *FATAL — protected init can be killed through a group sibling*: P9 makes fatal signals group-scoped while P12 drops only signals aimed at the init row | Closed **structurally at both ends**: (i) **clone admission refuses** `CLONE_VM` into the designated init's thread group (`EINVAL`, deliberate and documented) so init can never acquire a sibling — ships in **P5** with the designation authority; (ii) **S1's group-seal check tests designated-init membership of the whole target group** and drops the request silently (send returns 0) if init is a member — ships in **P12** as defence in depth. A ratchet rule pins the single production `thread_group_id = Some(…)` write site | §2.2; PLAN P5, P12 |
| **F** | *MAJOR — P0's SGI evidence is not teardown-causal*: `EXIT_SGI_SENT` wired to the generic scheduler send sites, so unrelated reschedules satisfy the gate | The counter is **re-wired to a teardown-only helper** `send_exit_expedite_sgi(victim_pid, batch)` that does not exist until P2 — so P0 **declares it zero until P2** — and it is **per-victim-PID paired** with an observation event recorded when the victim's CPU observes it can no longer dispatch the victim, mirroring the defer/reclaim pairing. *(v3 tranche pass: the observation half at P2 is now a specified mechanism — the lock-free `EXIT_KICK` bucket table of §2.7, published by the helper and CAS-consumed at the peer's next scheduler pass — because `EXIT_REQUEST_OBSERVED` does not exist until P8 and the live SGI receiver carries no pid. P8 replaces the proxy and deletes the table; residual R-21.)* Generic `send_resched_ipi`/`send_resched_ipi_to_cpu` are explicitly **not** wired | **§2.7** (new); PLAN P0, P2, P9 |
| **G** | *MINOR — acceptance-contract drift*: both changelogs substituted the OQ-1 decision for the actual seventh condition | Condition 7 is restated correctly as **"close conditions 1-6 and obtain a NEW ratification pass before implementation begins"** and is tracked as an open gate, not a closed row. The OQ-1 adoption is recorded separately as a **coordinator decision**, not a ratification condition | this table's cond-7 row below, §7 OQ-1, PLAN §2 |

**New residuals disclosed by these closures rather than hidden:** R-18 (init-sibling belt-and-braces),
R-19 (the one irreducible `Report` ambiguity window and why the ruling is "do not re-run"), R-20
(parked receipts are a bounded, counted, visible stall rather than a silent one), R-21 (the P2-era
expedite observation proxy and its demolition date), and R-22 (P3's one terminal init-refusal
guard-drop carve-out, explicitly not precedent).

### v3 repair — seven gaps closed against the v3 pre-check *(this revision; nothing else changed)*

A pre-implementation check of v3 found two FATAL and five MAJOR defects **inside the v3 closures
themselves**. Each is repaired in place, marked "v3 repair" at the point of change, and nothing the
check did not name has been touched.

| # | Sev | Defect in v3 as written | Repair | Where |
|---|---|---|---|---|
| 1 | **FATAL** | **`Fds` custody is impossible as specified.** `take_next_for_exit` was required to *retain* the descriptor in `fd_in_flight` (the recovery marker) **and** return that same value for an unlocked close. A slot cannot do both, and cloning to satisfy both is exactly the second copy the "double close is unrepresentable" argument rests on not existing — reopening double-close / endpoint-refcount corruption | The operation splits in three: `begin_fd_close` **[PM]** takes into the row's slot (sole owner) and mints a **non-owning `CloseTicket`** (a clone of the *endpoint handle*, with no close operation on it); `endpoint_hangup(&ticket)` **[no lock]** does the only lock-needing step and is idempotent by CAS; `finish_fd_close` **[PM]** drops the owning descriptor and clears the slot in one acquisition. The descriptor never leaves the row | §1.4, §1.6 (class-B `Fds`), §2.5, §3 AC-9/AC-12, §4.3; PLAN P6b, P7 |
| 2 | **FATAL** | **Report-marker phasing self-contradicts.** §1.6 said P2 ships `claim_exit_slot`/`record_exit` "from day one" while the PLAN keeps `on_process_exit` intact until P6b — so P2 would ship a class-B obligation *claiming* a recovery marker it does not have | The PLAN's sequencing holds and the reason is written down: P2 ships the seed's **shape** (T1/T2/T3, never a bool) but no marker, because **T4 does not exist until P6b** and a never-recovered obligation needs no marker. Exactly-once at P2 rests on the sole-redeemer invariant alone; the one lost-report window is identical to `main`'s behaviour today and is stated. **P6b introduces T4 and the btrt split in the same PR** | §1.6 (phase consequence); PLAN P2, P6b, P0 rule 5 |
| 3 | MAJOR | **P0's "exact nine `exit_process` callers" ratchet is false on `985881a6`** — there are **seven**; `manager.rs:1152` and `process_task.rs:244` are `enqueue_process_reclaim` sites, so the ratchet as written could not pass | "Nine" is restated as what it always was — the count of sites **P2 adapts** (seven callers + one PM-nested enqueue + the new SIGKILL arm). The P0 rule pins **seven** `exit_process` call sites; the two enqueue sites stay on their own existing ratchet set | §1.7; PLAN P0 rule 1 + gate 6, changelog |
| 4 | MAJOR | **P6a's two join gates are reversed.** Prompt-reap-before-grace makes *retirement* the second writer (`retire_second`); delayed-reap-after-retirement makes the *reap* second (`reap_second`). v3 asserted the opposite pairing | The two workloads are swapped onto the counters the pseudocode actually increments; both still asserted nonzero in one run | §1.6 (two-event join); PLAN P6a gates (f)/(g) |
| 5 | MAJOR | **"`EXIT_BLOCK_REFUSED{family}` reaches zero post-migration" contradicts the permanent admission interlock** — an already-latched victim must still be refused admission to a *migrated* family | The interlock is permanent and its counter is asserted **nonzero before and after** migration. The migration evidence becomes `EXIT_LEGACY_REMOTE_MARK{family}` → 0 and the new `EXIT_WAIT_CANCELLED{family}` → nonzero | §1.5, §3 AC-11; PLAN P0 table, P9, P10a-d |
| 6 | MAJOR | **P12's group-membership drop is unscoped** — as written it would also swallow init's own `exit_group`, making the required panic path unreachable | `ExitIntent` carries an explicit `origin`; the drop applies to **`Signal` only** (sender-agnostic, Linux-faithful). `ExitSyscall` and `FatalFault` bypass the membership test entirely, preserving the kernel-fatal path by construction | §2.2 End 2; PLAN P12 + gates |
| 7 | MAJOR | **`parked_at` was ambiguous and the unpark trigger incomplete.** Reusing the receipt's retirement fence makes the unpark predicate true at the instant of parking (the drain only selects entries whose fence has elapsed); and a `blocked_live_row` blocker can clear under PM with no scheduling-epoch advance anywhere | `ParkRecord` captures a fence built from a **`RetirementSnapshot` taken AT PARK TIME** (`RECLAIM_PARK_IMMEDIATE_UNPARK` asserted 0), and unpark becomes a three-armed disjunction — epoch, `ROW_REMOVAL_EPOCH` bump (one relaxed increment inside the PM acquisition that already removes the row), and an age backstop. *(v3 tranche pass: "fresh" is stated as a fresh **snapshot**, ruling out reuse of the cycle's earlier snapshot as well as of `reclaim.after_epoch`; and the age backstop is given its unit — `PARK_AGE_BACKSTOP_EPOCHS = 64` **scheduling epochs** summed over the captured online mask, never wall time.)* | §1.4, §1.8, §3 AC-13, §4.3, §4.4, §6 R-20; PLAN P0 table, P1 |

### v2 deltas against the first ratification's conditions *(retained)*

v1 was refused ratification (`ENDORSE: NO`) with two FATAL seam findings, seven MAJOR, one MINOR, and
seven explicit conditions. Every change below is traceable to a numbered condition.

> **Reading note.** This table records the v2 revision **as it was made**, and its phase labels are
> v2 labels. Two of them are superseded by v3: the wait families are now **P10a/b/c/d** (closure D
> inserted `BlockedOnSignal` as P10c), so "P10c deletes the arm / #491 completes at P10c" reads
> **P10d** today; and the honest PR count is **18**, not 16 — the figure moved 16 → 17 at the v3
> tranche pass (closure D) and 17 → 18 when the 2026-08-16 repair split P5 into P5a and the held P5b
> (§7 OQ-8; PLAN §0's ledger is authoritative). The cond-7 row below is the one v2 got wrong and v3
> corrects (closure G).

| Cond | Condition (verbatim intent) | v2 closure | Where |
|---|---|---|---|
| **1** | Reorder so every wait-family PR has a real producer: request-only scheduler publication + victim-owned SIGKILL commit suppression move **before** the killable-wait subphases; re-derive the dependency graph honestly incl. the missing P3→P8 edge | Old P10 becomes **P9**; old P9a/b/c become **P10a/b/c**. P9 is independently implementable against P2 via the named `exit_request_is_boundary_reachable()` predicate with a **live, counted legacy remote-mark arm** for not-yet-migrated wait families; P10a/b/c each move one family into the reachable set; **P10c deletes the arm**. `#491 complete` therefore moves from P9 to **P10c** | §1.5, §2.1 Increment 2, §3 AC-10/AC-11, §6 R-2/R-16; PLAN §0 graph, P9, P10a/b/c |
| **2** | Explicit `Pending→Claimed→Completed` state per obligation (or one exclusive redeemer), restoring what design C's single-worker serialization discharged; add the reap/tombstone retention gate **before** any phase ships row-resident resource bits | New **§1.6** defines the four-state obligation machine (`Absent/Pending/Claimed{claimer}/Completed`), PM as the sole serializer, the sole-redeemer invariant, and orphaned-claim recovery. New **retention rule**: `waitpid` no longer removes the row; it tombstones it. Shipped as **P6a (retention gate) before P6b (ledger)** | §1.4, **§1.6** (new), §3 AC-12, §4.3, §6 R-11/R-13/R-17; PLAN P6a/P6b |
| **3** | P1's proof reads never hold the reclaim queue while acquiring scheduler or PM locks; the SIGKILL phase's retirement receipt is returned/enqueued only **after** the PM guard drops — no interim shape violating the no-overlapping-lock rule even temporarily | P1's drain becomes **detach → drop queue lock → prove → free-or-reinsert**; the under-queue-lock predicate stays epoch/shadow-only (lock-free atomics). P2 changes `exit_process` to **return** a `#[must_use] RetirementReceipt`; both existing PM-nested enqueue sites (`manager.rs:1152`, `process_task.rs:244`) are converted in P2 | §4.1, §4.3 (two new rows), §4.4; PLAN P1, P2 |
| **4** | P8 must state explicitly how normal exit exercises the return-boundary hook in its own PR (no dormant hook) | §2.5 (new) gives the control flow: `sys_exit` publishes a self-request and returns; the **hook is the only entry to `do_exit_current`**, so every normal exit exercises it in the introducing PR. Also fills Codex's named P8 gaps: tombstone control flow and one-at-a-time FD acquisition | **§2.5** (new); PLAN P8 |
| **5** | The fatal-signal convergence phase makes the delivery result **intent-only** and deletes the legacy Terminated-channel parent-notification action in the **same** PR | `DeliverResult::Terminated` (and its documented "caller MUST call notify") is **deleted**, replaced by `DeliverResult::FatalIntent{pid,tid,sig,code}` which is documented as performing no notification, no status write and no wake; notification is discharged solely by the P6b ledger | §2.1, §2.6 (new), §3 AC-12; PLAN P11 |
| **6** | P0's counters wire into **named** existing teardown write-sites with at least one causally-paired nonzero defer/reclaim test; state the honest PR count; delete the false P2/P3/P4/P5 file-disjointness claim | PLAN P0 now names every write site with file:line and marks which counters are legitimately zero until a later phase; adds `fork_exit_defer_reclaim_pairing_test` (nonzero, per-pid paired). Honest count: **13 numbered phases / 16 PRs**. The parallel-merge claim is deleted; all phases merge sequentially | §7 OQ-8; PLAN P0, §0 graph, §0 PR ledger |
| **7** | *(corrected in v3 — closure G)* **Close conditions 1-6 and obtain a NEW ratification pass before implementation begins.** v2 mislabelled this row as the OQ-1 decision | **OPEN by construction.** It cannot be closed by the document that seeks the ratification; it is closed only when a fresh pass returns `ENDORSE: YES`. PLAN §2's ratification gate is the binding statement, and no phase — P0 and P1 included — is cleared for build until then. *(v3 tranche pass: the gate is now **tranche-scoped** — condition 7 is discharged per tranche, not once for the whole document. Tranche 1 (P0+P1+P2) is submitted; P0/P1/P2 are cleared for build when **that tranche's** pass returns `ENDORSE: YES`, and every later phase stays uncleared until its own tranche ratifies, with the DESIGN-DEBT REGISTER's rule gating any tranche that contains a debt owner. The condition itself is not weakened — it is applied more times, not fewer.)* | PLAN §2 (ratification gate), **DESIGN-DEBT REGISTER** |

**Coordinator decision recorded alongside the conditions (NOT a ratification condition — closure G).**
**OQ-1 is DECIDED:** protected init goes **Linux-faithful** — user-originated signals to the
designated init with no handler are **silently dropped and the send returns success (0)**, NOT
`EPERM`; only init's own `exit`/`exit_group` or an unhandleable fatal fault is kernel-fatal. The
`EPERM` claim and its false "Linux-fidelity" label are removed everywhere. *(§1.3, §2.2, §3 AC-2,
§7 OQ-1; PLAN P12.)*

**Also folded in (Codex per-phase notes that named gaps, not conditions):** P8's hook-activation /
tombstone / FD-acquisition control flow (§2.5); P7's statement that it does not supply P6's missing
prerequisite (that is P6a's job); P2's UAF reduction restated with the receipt discipline.

**Explicitly unchanged from v1** (not criticised, preserved verbatim): §0 adjudication in full, §1.1,
§1.2, §1.5's rejection of `ExitPending`, §2.3 (#471 detach + seal), §2.4 (fault-victim attribution),
§5 (scope cuts), §8 (lessons register) except for two added rows, and every residual not listed above.

**Explicitly unchanged from v2** (the re-ratification closed conditions 4, 5 and 6 and criticised
nothing in these areas): §2.3, §2.4, §2.5's activation argument, §2.6 in full, §4.2, §5, and AC-1,
AC-3, AC-4, AC-5, AC-6, AC-7, AC-8, AC-9 in §3.

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
> (spine first, every phase strictly better, no dormant code, one revert story per PR, call-site
> ratchet), with C's proof machinery grafted wholesale and C's `ExitPending` state explicitly
> rejected.**
>
> *(2026-08-16: the ruling as first written said "hard line budget". **That clause is deleted, not
> softened** — operator ruling of 2026-08-11: no line or file ceilings on fixes, ever; safety seams are
> fine, size gates never. Its place in the discipline is taken by PLAN rule 5's revert-story firing
> condition, which is what actually decides where a PR splits.)*

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
(`manager.rs:138`, `:388`, `:612`, `:1092`). *(Both counts are superseded by §0.3.1: the init
literals are at different sites and two of the three named here are not init literals, and there are
**eight** `next_pid` allocation sites on today's tree.)* `Scheduler::terminate_process_threads(owner_pid: u64)`
at `scheduler.rs:2599`. `exit_process` at `manager.rs:1120` carries `#[allow(dead_code)]` and already
performs the unconditional aarch64 grace-stamped defer. `Process::take_fd_entries` (`process.rs:335`)
**returns an allocating `alloc::vec::Vec`** — confirming Judge 2's warning that it cannot be reused
unchanged under PM. `init_shell.rs:1028` reads `getpid().map(|p| p.raw()).unwrap_or(0) != 1`.

**v3 corrections to the v2 anchor set** (five v2 citations had drifted; each was re-read at
`985881a6`, and every other v2 anchor was re-confirmed unchanged):

| v2 said | Actually at `985881a6` |
|---|---|
| `waitpid`'s reap at `syscall/wait.rs:385` | **`:386`** (`manager.remove_process(child_pid)`; `complete_wait` begins at `:335`) |
| `ProcessManager::remove_process` at `manager.rs:1101-1104` | **`:1102-1104`** |
| the drain spans `task/process_task.rs:375-392` | **`:375-391`** |
| the reclaim call at `task/process_task.rs:388` | **`:387`** (`Some(reclaim) => reclaim.reclaim()`; `:388` is `None => break`) |
| `take_fd_entries` called at `task/process_task.rs:236` | **`:233`** (`:236` is a comment line) |

None of these changes an argument — they are cited-line hygiene, corrected because a design document
whose anchors do not resolve cannot be checked.

**v3 additions to the anchor set** (read at `985881a6`; each is load-bearing for a lettered closure):

- **Closure A — `exit_process` has SEVEN live callers today, and v2 named none of them.**
  `ProcessManager::exit_process` is declared at `process/manager.rs:1120`. Its callers are: the four
  aarch64 EL0 fault sites `arch_impl/aarch64/exception.rs:778`, `:1146`, `:1233`, `:1336`; the two
  x86_64 fault sites `interrupts.rs:1429` and `:1735`; and `process::exit_current` at
  `process/mod.rs:264` (function at `:258`, which holds `*manager()` live across the call).
  **Every one of the seven calls `exit_process` inside a live PM guard or `with_process_manager`
  closure**, so all seven must be restructured, not merely annotated. Two further sites must be
  adapted in the same PR because they participate in the same receipt hand-off: `handle_thread_exit`'s
  PM-nested enqueue at `task/process_task.rs:244` (the only one v2 named) and the SIGKILL arm P2
  itself introduces at `syscall/signal.rs:162`. **Nine ADAPTED sites in total** — which is a different
  count from the seven `exit_process` callers and is never used as a substitute for it — plus the
  internal enqueue at `manager.rs:1152` which is deleted rather than adapted. *(v3 tranche pass: the
  three classes, and the three exact sets P2 must leave behind — 8 wrapper calls, 1
  `exit_process_locked` caller, 3 PM-free enqueues — are stated once in §1.7 and are the only
  formulation any gate in these documents may cite. `handle_thread_exit` does **not** call the
  wrapper: it routes its receipt out through `phase1_result`, which is why "all nine call the
  wrapper" was never assertable.)*
- **Closure B — `btrt::on_process_exit` cannot commit under PM, verified.** `test_framework/btrt.rs:393`
  clears its registry slot and then calls `pass`/`fail` and, on the last completion, `finalize()`,
  which calls `ktap::emit_summary` and `serial_println!` — **SERIAL under a DAIF-masked PM guard**.
  That is why `Report` is a class-B obligation with an effect marker instead of a fused PM commit,
  and why the slot-clear-then-record order inside `on_process_exit` is itself restructured in P6b.
- **Closure C — the drain's selection rule is `position()` + `swap_remove` under the queue lock**
  (`task/process_task.rs:379-383`), i.e. it always re-scans from index 0. A re-inserted candidate is
  therefore *guaranteed* to be re-selected, which is exactly the livelock the re-ratification named.
- **Closure D — the blocking-primitive inventory as v3 counted it: nine entry points, all in two
  files.** *(⚠ **DEBT-3**: this count is **not final**. Four publications sit outside this set —
  `syscall/futex.rs:115`, `task/scheduler.rs:2607`, `task/kthread.rs:151`/`:183`, and the dead
  `Thread::set_blocked`/`Scheduler::block_current` pair — none of them visible to a name-family
  ratchet (§0.3.1, **#580**); P9 owns the corrected inventory.
  Everything below is the v3 enumeration, retained because it is what the interlock is written
  against, not because it is complete.)* The nine, as enumerated:
  `Scheduler::block_current` (`task/scheduler.rs:1726`), `block_current_for_signal` (`:1897`),
  `block_current_for_signal_with_context` (`:1916`), `block_current_for_child_exit` (`:2065`),
  `block_current_for_timer` (`:2153`), `block_current_for_io` (`:2218`),
  `block_current_for_io_with_timeout` (`:2227`), `block_current_for_compositor` (`:2386`), and
  `WaitQueueHead::prepare_to_wait` (`task/waitqueue.rs:52`, with `finish_wait` at `:87`).
  The **`BlockedOnSignal` family v2 omitted** is live in four functions:
  `syscall/signal.rs:819` (`sys_pause_with_frame`, x86), `:1314` (`sys_sigsuspend_with_frame`, x86),
  `:1815` (`sys_pause_with_frame_aarch64`), `:2097` (`sys_sigsuspend_with_frame_aarch64`), plus the
  legacy `sys_pause` at `:769`; the scheduler's wake side is `unblock_for_signal`
  (`task/scheduler.rs:1970`), and `tty/driver.rs:602-603` documents that the TTY unblock path already
  spans both `Blocked` (stdin read) and `BlockedOnSignal`.
- **Closure E — there is exactly ONE production write of a non-`None` `thread_group_id`:**
  `syscall/clone.rs:210` (`child_process.thread_group_id = Some(parent_tg_id)`), where
  `parent_tg_id` is derived at `:84` (`process.thread_group_id.unwrap_or(pid.as_u64())`) **inside the
  live PM guard taken at `:60`**. That single write site is what makes the clone-admission half of
  closure E a two-line structural refusal rather than a policy sprinkled across the tree. The
  designated-init accessor it consults replaces `syscall/signal.rs:26`'s `const INIT_PID: u64 = 1`
  (read at `:124` and `:397`).
- **Closure F — the two SGI send sites are generic by construction.** `send_resched_ipi`
  (`task/scheduler.rs:1843`, send at `:1857`) wakes *idle* CPUs and `send_resched_ipi_to_cpu`
  (`:1868`, send at `:1886`) targets the CPU that received a newly runnable task; neither knows a
  victim. Wiring `EXIT_SGI_SENT` there counts every ordinary wakeup, which is why v3 introduces a
  teardown-only send helper and declares the counter zero until P2.

**v2 additions to the anchor set** (verified during the v2 revision, needed by conditions 3 and 6;
re-verified at `985881a6` with the four corrections noted above):

- `PENDING_PROCESS_RECLAIMS` is a `spin::Mutex<Vec<PendingProcessReclaim>>` at `process_task.rs:97`.
  Its drain, `reclaim_deferred_process_resources` (`process_task.rs:375-391`), evaluates
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
- `waitpid`'s reap physically removes the row: `syscall/wait.rs:386` calls
  `manager.remove_process(child_pid)` → `manager.rs:1102-1104` *(now `:1086-1090`; §0.3.1)*. This is
  the row-lifetime seam condition 2 closes with the tombstone gate.
- `Process::terminate` (`process.rs:284`) calls `close_all_fds()` (`process.rs:294`, impl at `:347`);
  `terminate_minimal` (`process.rs:320`) early-returns on a repeat pass;
  `take_fd_entries` (`process.rs:335`) is the allocating extractor used at `process_task.rs:233`
  *(v3: corrected from `:236`, which is a comment line; this is the fifth v2 anchor drift)*.
- `retirement_grace_target`/`retirement_grace_elapsed` are `scheduler.rs:550`/`:563`;
  `is_kernel_stack_slot_live` is `memory/kernel_stack.rs:283`.

### 0.3.1 Tranche-2 re-anchor at `main` @ `2c7b8798` *(2026-08-16 documentation-repair pass)*

§0.3 above is a snapshot taken at `eebc8868`/`985881a6` and is retained as the record of what those
trees held. **Every anchor a tranche-2 phase depends on was re-read at `2c7b8798`.** Where §0.3 or a
phase section disagrees with this table, **this table governs**; sixteen foundation PRs have moved
through these files since the last anchor pass.

| Claim | Design said | At `2c7b8798` |
|---|---|---|
| x86 reap removes the row directly (DEBT-4) | `syscall/handlers.rs:3101` | **`:3123`**, plus a byte-similar duplicate at `syscall/wait.rs:386` |
| `ProcessManager::remove_process` | `manager.rs:1101-1104` → `:1102-1104` | **`:1086-1090`** — a four-line choke point that already bumps `ROW_REMOVAL_EPOCH` via `note_process_row_removed()` (`task/process_task.rs:355-357`) |
| Production `ProcessId::new(1)` literals (P5) | three: `manager.rs:1178`, `process_task.rs:226`, `:285` | **three, two of them different**: `manager.rs:1165` (read at `:1166`, `:1176`, `:1179`), `process_task.rs:647`, `:720` (read at `:723`, `:726`). `process_task.rs:226` is `live_row_names_root`/`any_live_root_matches` and `:285` is the `BOOT_RECLAIM_FORCED_BLOCKER` static (`BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO` is at `:287`) — **neither is an init literal**; both are struck, not re-anchored |
| `INIT_PID` const and its reads | `signal.rs:26`, read at `:124`, `:397` | `signal.rs:26`, read at `:124` and **`:402`** |
| `next_pid` allocation sites (P5) | "all four" | **eight**: `manager.rs:141`, `:378`, `:602`, `:1076`, `:1419`, `:1561`, `:1704`, `:2169` (base at `:118`) |
| Test-only init literals (ratchet allowlist) | three in `test_userspace.rs` | unchanged: `test_userspace.rs:84`, `:203`, `:292` |
| exec-detach write sites (P3) | one each | unchanged: `inherited_cr3 = Some(...)` at `syscall/clone.rs:209`, `thread_group_id = Some(...)` at `:210`; the only `None`s are the struct-literal defaults `process/process.rs:337-338`. **No exec path clears either field** |
| Live-sibling guard (P3 keeps it) | `manager.rs:49-64` | defined at **`manager.rs:46`**, called at `:3063` and `:3368` |
| Dispatch gate (P3) | `task/scheduler.rs` | **`interrupts/context_switch.rs`** — PM try-lock at `:218`, `USERSPACE_DISPATCH_NO_CR3_REFUSED` at `:27` incremented at `:676`/`:1193` (PR #570's rewritten site, ratcheted) |
| `ProcessState::Creating` window (P3) | — | `process/process.rs:54`, set at `:318`, cleared to `Ready` by `set_main_thread` — declared at **`:350`**, clearing write at **`:352`** |
| PM→SCHEDULER nesting on creation paths (**P4**, second commit) | "the spawn/test-disk paths" | **`creation.rs:67→85`**, **`creation.rs:185→202`**, **`boot/test_disk.rs:258→263`**; `scheduler::spawn` at `task/scheduler.rs:3444` takes `lock_scheduler()` at `:3447`. **Corrected in the P4 build pass: the class has SIX members, not three** — `test_exec.rs` publishes under a live PM guard at three more sites and `arch_impl/aarch64/syscall_entry.rs::sys_spawn_aarch64` at one more. All six are fixed by P4, and the detection lives at the publication seam (`spawn`/`spawn_front`/`spawn_as_current`) emitting the distinct marker `[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]` — the exec path's `[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]` is unreachable from a creation site and asserting it at zero was vacuous (coordinator ruling R24) |
| Kernel-stack leak surface (**P4**/AC-8, first commit) | "apply the existing `take()` to three more sites" | **impossible as written, and the surface is FIVE sites, not three.** `grep -n 'Box::leak' kernel/src/process/manager.rs` returns `851`, `925`, `1010` (the three `create_main_thread*` constructors — **`851` is `#[cfg(target_arch = "x86_64")]` and `925`/`1010` are `#[cfg(target_arch = "aarch64")]`; none is compiled on both arches, so the split across the five sites is 3 x86-only / 2 aarch64-only and BOTH profiles are mandatory, coordinator ruling R21**) **plus `1979`** (`complete_fork`, fn `:1920`, `#[cfg(target_arch = "x86_64")]`) **and `2323`** (`fork_process_with_context`, fn `:2148`, `#[cfg(target_arch = "x86_64")]`, `kernel_stack_allocation: None` at `:2339`) — every one leaks the `KernelStack` at construction with an in-tree `// TODO: proper cleanup`. Tracked as **#579**, which **P4 closes** |
| Fork transfer — **CORRECTED** | "fork already transfers correctly" | **false on x86_64.** The only transferring fork site is `manager.rs:1833`, inside `complete_fork_aarch64` (fn `:1779`, `#[cfg(target_arch = "aarch64")]`). Both x86 fork paths leak: `fork_process_with_page_table` (`:1365`) and `fork_process_with_parent_context` (`:1507`) reach `complete_fork` at `:1490`/`:1634`; `fork_process` (`:1357`) reaches `fork_process_with_context`. The genuine correct-transfer contrast sites are the CLONE_VM clones `syscall/clone.rs:250-252` and `arch_impl/aarch64/syscall_entry.rs:961`, plus `manager.rs:1833` **on aarch64 only** |
| Freed-row stack hazard (**P4**'s premise) | "freed ungated at `waitpid` reap" | **structurally live, merely unreached**: `remove_process` drops the row → `Process::main_thread` (`process/process.rs:199`) → `Thread::kernel_stack_allocation` (`task/thread.rs:428`) → `impl Drop for KernelStack` (`memory/kernel_stack.rs:85-99`); nothing ever clears `main_thread`. Unreached only because of the five-site leak — which is why P4's leak fix and AC-8's accounting gate ship together in the same PR |
| `complete_fork` suppression (adjacent to P4) | — | `manager.rs:1919` carries `#[allow(dead_code)]` while the function has **two live callers** (`:1490`, `:1634`) — a stale suppression against the repo's zero-tolerance standard; removed by P4 |
| Blocking publications outside the nine (DEBT-3) | `futex.rs:115`, `scheduler.rs:2175-2194` | `futex.rs:115`; **`scheduler.rs:2607`** (`BlockedOnIO`; `:2175-2194` is now `unblock`'s wake predicate); **`kthread.rs:151`/`:183`**; **`Thread::set_blocked` `thread.rs:902`** with its sole dead caller `Scheduler::block_current` `scheduler.rs:2099`. Tracked as **#580** |
| `AlreadyTerminated` abandon (adjacent to P3) | — | `manager.rs:1131-1136` — bypasses custody, leaking table leases and superseded exec roots (**#572**) |
| Structural ratchet suites | 13 / 11 / — / 4 tests | `tests/teardown_structure.rs` **33**, `tests/context_restore_structure.rs` **46**, `tests/exec_lock_order_structure.rs` **25**, `tests/dma_and_log_sink_structure.rs` **4** |

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
             first_status: Option<i32>, batch: GroupBatchId,
             // v3 (closure B): class-B effect evidence, written by the effect itself.
             report_marker: ReportEffectMarker,   // two lock-free bits, see §1.6
             fd_in_flight: Option<InFlightFd>,             // single-slot custody, not a flag
             reparent_cursor: Option<ProcessId> }          // resumable, idempotent batches

// v3 (closure B): the effect marker for the ONE obligation whose effect cannot commit under PM.
ReportEffectMarker { started: AtomicBool, finished: AtomicBool, token: Option<u16> /*btrt test id*/ }

// v3 REPAIR (closure B): the row slot is the SOLE OWNER of a descriptor in flight. The unlocked
// step is handed a non-owning CloseTicket, never the descriptor itself — a slot cannot both retain
// ownership and hand the same value out, and a clone would be the second copy the no-double-close
// argument depends on not existing. See §1.6 and §2.5.
InFlightFd  { fd: u32, desc: FileDescriptor, hangup_done: bool /* diagnostic only */ }
CloseTicket { fd: u32, endpoint: EndpointRef }   // endpoint-handle clone; has NO close operation

// v2 (condition 2): row lifetime must outlive every obligation.
// v3 (closure B): removal is a TWO-EVENT JOIN, so the row carries both events, not one state.
RowState { Live, Zombie, Tombstone }          // derived; see the join rule in §1.6
reaped:  Option<(ProcessId /*reaped_by*/, i32 /*status*/)>,   // written by the parent's reap
retired: bool,                                                // written by the retire gate
teardown_next: Option<ProcessId>              // intrusive link; no collection grows

// Scheduler/boundary mirrors. Release/acquire. Carry NO ownership, authorize NO free.
GroupExitWord { generation, cause, active }
ThreadExitRequest { generation, reason, state }

// Proof values (from C).
RetirementFence { epochs: [u64; MAX_CPUS], online_mask }
RetirementSnapshot                       // proves Acquire loads + fence ran before liveness reads
RootProof { blocked_epoch, blocked_hw, blocked_shadow, blocked_cached, blocked_live_row }

// v3 (closure A): custody, not advice. No public constructor; crate-private type.
pub(crate) struct RetirementReceipt { /* fixed size; see §1.7 for the Drop contract */ }

// v3 (closure C): drain progress state. Fixed-size fields on the existing queue entry.
PendingProcessReclaim { .., last_pass: u32, proof_failures: u8, parked: Option<ParkRecord> }

// v3 REPAIR (closure C) + v3 TRANCHE PASS: a park record captures a FRESH `RetirementSnapshot`
// TAKEN AT PARK TIME — never the receipt's own `after_epoch` retirement fence (which the drain has
// already proven elapsed before it could select the entry at all), and never a re-use of the
// snapshot step 2 of the same cycle already took. The age key is a SCHEDULING-EPOCH COUNT, not a
// wall-clock or tick duration. See 1.8.
ParkRecord { fence_at_park: RetirementFence,  // derived from a RetirementSnapshot taken AT PARK TIME;
                                              // NOT reclaim.after_epoch, NOT step 2's snapshot
             row_epoch_at_park: u64,          // snapshot of the global ROW_REMOVAL_EPOCH
             age_epoch_sum_at_park: u64 }     // sum of fence_at_park.epochs over its online_mask —
                                              // the age-backstop key, denominated in SCHEDULING
                                              // EPOCHS (backstop: PARK_AGE_BACKSTOP_EPOCHS = 64)
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
> migrated (P10a/b/c/d), so at P9 **every** family is unmigrated — and "latch and hope" would be a kill-latency
> regression against the P2 behaviour already merged. It is therefore replaced by an explicit,
> named, two-armed predicate that ships with both arms live:
>
> ```rust
> /// True iff this victim will demonstrably reach the return-boundary hook (§2.5)
> /// without external help: it is runnable/running (EL0 or a preemptible kernel path),
> /// or it is blocked in a wait family whose victim-owned cancellation has been
> /// audited and tested (P10a/b/c/d).
> fn exit_request_is_boundary_reachable(t: &SchedThread) -> bool
> ```
>
> - **true** → publish `ThreadExitRequest`, **never** mark `Terminated` remotely; the victim commits
>   its own exit at the hook.
> - **false** → publish the request *and* fall back to the legacy remote mark + `exit_process` route,
>   which is exactly the already-merged, already-gated P2 behaviour — no new mechanism, no new
>   hazard, no latency regression. Counted per family as `EXIT_LEGACY_REMOTE_MARK{family}`.
>
> Each of P10a/b/c/d moves one family from the false set to the true set; the false set is a ratcheted
> allowlist that **P10d drives to empty and then deletes along with the fallback arm**. This is the
> same shrink-to-empty discipline the `\.terminate\(` allowlist already uses, and it is what makes
> every wait-family PR a *consumer of a producer that already exists* rather than the reverse.
> Residual R-2 is restated accordingly, and the new interim is disclosed as R-16.

> **v3 (closure D) — the predicate is made STABLE by an admission interlock, and the inventory is
> made COMPLETE.** The re-ratification's fourth FATAL was that `exit_request_is_boundary_reachable()`
> is a *classification at an instant*: a victim classified `true` because it is running can, one
> instruction later, enter an unmigrated wait and become unreachable — the request is published, the
> remote mark was suppressed, and nothing ever forces the victim to a boundary. Two structural fixes,
> both landing with the predicate itself in P9:
>
> 1. **No-new-block admission interlock.** Every blocking primitive gains a pre-block acquire-load of
>    the calling thread's own exit-request word and **refuses to block when it is latched**, returning
>    a refusal the caller maps to `EINTR` (or its family's existing cancel path). The nine entry
>    points are named and complete at `985881a6`: `Scheduler::block_current` (`scheduler.rs:1726`),
>    `block_current_for_signal` (`:1897`), `block_current_for_signal_with_context` (`:1916`),
>    `block_current_for_child_exit` (`:2065`), `block_current_for_timer` (`:2153`),
>    `block_current_for_io` (`:2218`), `block_current_for_io_with_timeout` (`:2227`),
>    `block_current_for_compositor` (`:2386`), and `WaitQueueHead::prepare_to_wait`
>    (`waitqueue.rs:52`). The check lives **inside the primitives**, not at their callers, so no wait
>    site can forget it and no future wait site can be added without it (a P0 ratchet rule asserts the
>    primitive set is exactly these nine). Counted `EXIT_BLOCK_REFUSED{family}`, which P9's own test
>    drives nonzero — and which **no gate may ever assert at zero** (**DEBT-5**: the interlock is
>    permanent; migration changes the fate of an already-blocked victim, not the refusal owed to an
>    already-latched one entering a migrated family).
>
>    ⚠ **DEBT-3 — this inventory is NOT closed at nine, and P9 owns the closure (#580).** Four live
>    paths sit outside the set: `syscall/futex.rs:115` publishes `ThreadState::Blocked` **directly**,
>    bypassing every `block_current*` entry point; `task/scheduler.rs:2607` publishes `BlockedOnIO`
>    the same way; `task/kthread.rs:151` `kthread_park()` writes `Blocked` at `:183` with no
>    interlock; and `Thread::set_blocked()` (`thread.rs:902`) is a bypassing mutator whose only caller
>    is the dead `Scheduler::block_current` (`scheduler.rs:2099`). The ratchet that is supposed to
>    keep this inventory closed pins **names**, so a direct state write is invisible to it. Until each
>    is brought under the interlock or proven unreachable after a latch — and the ratchet is widened
>    to catch state writes — the one-way-door claim below is an argument P9 still owes.
>    See the DESIGN-DEBT REGISTER.
>
>    *Consequence — the classification becomes a one-way door.* `true` means "running or in a migrated
>    family"; from the moment the request is latched, entering an unmigrated family is impossible.
>    A victim can only move from the false set to the true set (by waking), never back. This is what
>    makes the predicate sound rather than merely plausible, and it is why the interlock ships in P9
>    rather than being spread across P10.
>
>    *It is permanent, and it does NOT fall to zero after migration.* **(v3 repair.)** v3 as first
>    written said the interlock is "subsumed by the family's own cancellation, so
>    `EXIT_BLOCK_REFUSED{family}` falls to zero as each family lands". That sentence contradicts
>    itself — a guard cannot be simultaneously non-redundant and never fire — and it misstates what
>    migration changes. Migration changes the fate of a victim **already blocked** when the request
>    lands: remote mark becomes victim-owned cancellation. It changes **nothing** about a victim whose
>    request is already latched and which then *tries to enter* the wait; that thread must still be
>    refused admission in a migrated family exactly as in an unmigrated one, because letting it block
>    would manufacture the very cancellation work the interlock exists to avoid and would reopen a
>    lost-wakeup window between the block and the cancel. The refusal is therefore permanent, and
>    `EXIT_BLOCK_REFUSED{family}` is asserted **nonzero for every family both before and after its
>    migration** — P9's per-family admission-race test is re-run unchanged inside each P10x PR and
>    must still pass. What falls to zero at migration is `EXIT_LEGACY_REMOTE_MARK{family}`; what
>    rises from zero is the new `EXIT_WAIT_CANCELLED{family}` (a victim found already blocked in this
>    family and cancelled by its own continuation). Those two are the migration evidence;
>    `EXIT_BLOCK_REFUSED` is the standing-guard evidence and is never asserted at zero.
>
> 2. **The inventory is completed with `BlockedOnSignal`.** v2's inventory listed futex,
>    `WaitQueueHead`/stdin/TTY, child-wait, timer/nanosleep and completion/I-O, and omitted the live
>    `pause`/`sigsuspend` family — while simultaneously claiming the allowlist reaches empty. It is
>    live in four functions (`syscall/signal.rs:819`, `:1314`, `:1815`, `:2097`, plus the legacy
>    `sys_pause` at `:769`) with its own scheduler wake path (`unblock_for_signal`,
>    `scheduler.rs:1970`). It becomes **its own subphase, P10c**, and the subphase that empties the
>    allowlist and deletes the legacy arm is relabelled **P10d**. The wait inventory is now closed and
>    stated as such: *futex; `WaitQueueHead` + stdin/TTY readers; `BlockedOnSignal` (pause/sigsuspend);
>    child-wait + timer/nanosleep + completion/I-O.* Any family discovered later is a new subphase
>    before P10d, not a silent allowlist entry.

### 1.6 The exactly-once ledger — claim protocol and row retention *(v2, condition 2; class split, effect markers and the two-event join added in v3, closure B)*

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
| T2 | `Pending → Claimed{me, fence}` | any control path that will discharge it **immediately** | **Class B only** (§ below). Claim and the start of work are in the same control path; `fence` is the retirement fence captured at claim time (used only by T4) |
| T3 | `Claimed{me} → Completed` | **only** the claimer | **Class B only.** Performed under a *fresh* PM acquisition after the work completed outside PM. `claimer == current_tid` is asserted; a mismatch bumps `LEDGER_CLAIM_MISMATCH`, which CI asserts is 0 |
| **T2·3** | `Pending → Completed` **with the effect** | **Class A only** *(v3)* | The effect and the state write happen in **one PM acquisition**, so `Claimed` is never observable and no orphan window exists |
| T4 | `Claimed{dead} → Pending` **or** `→ Completed` | **only** the S4 retire/reap gate | **Class B only.** Permitted when the claimer is proven not live by P1's machinery (scheduler thread absent or `Terminated`) **and** the claim-time fence has elapsed. *(v3)* The destination is chosen by the obligation's **effect marker**, not by the ledger — see the ruling table. Bumps `LEDGER_CLAIM_ORPHANED` |

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

#### Exactly-once, really — the class split and the effect markers *(v3, closure B)*

The re-ratification's second FATAL is correct against v2 as written: T4 moved `Claimed{dead}` back to
`Pending` **without any way to know whether the effect had already fired between T2 and T3**, so
reopening could duplicate and not reopening could lose. A recovery rule that cannot observe the
effect is a coin flip. v3 removes the guesswork in two steps.

**Step 1 — most obligations never enter `Claimed` at all.** An obligation is **class A** when its
effect is a write to PM-owned state; for those, the effect and `→ Completed` are performed **in the
same PM acquisition** (transition T2·3). There is then no interval in which the effect has fired and
the ledger does not say so, so T4 is *structurally unreachable* for them:

| Obligation | Class | Effect, and where it commits |
|---|---|---|
| `Sigchld` | **A** | Set the parent row's pending-SIGCHLD bit — PM-owned memory. Written in the same acquisition that marks `Completed` |
| `ParentWake` | **A** | Publish the child's status as *collectable* in the parent's wait state — PM-owned. Same acquisition. **The scheduler kick is NOT part of this obligation**: `unblock_for_child_exit` is idempotent (waking a runnable thread is a no-op), so it is declared a *repeatable* side effect, issued unconditionally after PM drops and counted separately as `PARENT_WAKE_KICKS` (which may exceed the exactly-once count, by design and by declaration). AC-12's `PARENT_WAKE_COMPLETED` counts the PM transition, which is the thing that must be exactly-once |
| `Reparent` | **A** | Re-parent one fixed-size batch of children — PM-owned. Each batch is one acquisition, and the obligation carries a **cursor** (`reparent_cursor`) advanced in that same acquisition, so a claimer that stops mid-way leaves a resumable, idempotent position: re-running a batch skips children already re-parented. `→ Completed` is written in the acquisition that observes the cursor exhausted |
| `Resources` (take half) | **A** | `page_table.take()` into a `RetirementReceipt` — PM-owned. The take and the state write are one acquisition. The *enqueue* half is not an obligation at all: closure A (§1.7) makes take-and-enqueue a single indivisible public operation, so a receipt is never a thing a control path can be holding when it dies |
| `Fds` | **B** | Endpoint close — pipe/PTY/TCP locks, which must not be held under PM |
| `Report` | **B** | `btrt::on_process_exit` — reaches `finalize()` → `ktap::emit_summary` → `serial_println!`, i.e. **SERIAL**, which is forbidden under a DAIF-masked PM guard (`test_framework/btrt.rs:393`). This is verified, not assumed, and it is the whole reason class B exists |

**Step 2 — the two class-B obligations carry an effect marker written by the effect itself, and the
marker outranks the ledger.**

- **`Fds` — custody, and the descriptor never leaves the row.** **(v3 repair — the custody rule as
  first written names a state that cannot exist.)** v3 said `take_next_for_exit` "moves one
  `(fd, FileDescriptor)` out of the table into the row's single-slot `fd_in_flight` **under PM**; the
  close consumes it". The slot cannot simultaneously **retain** the descriptor — which is the entire
  content of the custody marker, and the only thing that lets a dead claimer be recovered — and
  **hand that same value out** to an unlocked close. Cloning it to satisfy both is precisely the
  second copy the "a double close is not representable" claim depends on not existing, and it
  reopens double-close / endpoint-refcount corruption. The contradiction is removed by separating
  *ownership* from *the one step that genuinely cannot run under PM*:

  ```rust
  // row-resident, PM-owned; the SOLE owner of a descriptor between take and close
  InFlightFd  { fd: u32, desc: FileDescriptor, hangup_done: bool /* diagnostic */ }
  // handed to the unlocked step: a cheap clone of the ENDPOINT handle, not the descriptor.
  // There is no close-with-effect operation on this type, so it cannot close anything.
  CloseTicket { fd: u32, endpoint: EndpointRef }
  ```

  - **`begin_fd_close(pid)` [PM]** — if the slot is empty, move one `(fd, desc)` out of the table
    into it; then mint a `CloseTicket` *from* the slot and return it. If the slot is already
    occupied (a claimer died mid-close), it returns a ticket for **that** descriptor and takes
    nothing new. Returns `None` only when the table is empty **and** the slot is empty.
  - **`endpoint_hangup(&ticket)` [no lock]** — the only step that needs the pipe/PTY/TCP lock. It is
    defined as an idempotent state transition guarded by a CAS on the endpoint, so a replay is a
    no-op. That idempotence is a **requirement P7 implements and gates**, not an assumption.
  - **`finish_fd_close(pid)` [PM]** — set `hangup_done`, then drop the owning `desc` out of the slot
    and clear it. This is the single destructive step; it happens under PM, and its refcount
    decrement takes no endpoint lock because the hangup already ran.

  **A double close is not representable** because the owning descriptor is in exactly one place at
  every instant — table, or slot — and is destroyed exactly once, by `finish_fd_close`, in the same
  PM acquisition that clears the slot. The only replayable step is the idempotent hangup, so there
  is no ambiguous window to rule on: `Fds` still needs no started/finished bits and no "which side
  wins" statement. `hangup_done` is diagnostic only (`FD_HANGUP_REPLAYED`) and is never load-bearing
  for correctness.
- **`Report` — two lock-free bits plus a token, and the marker wins.** `on_process_exit` is split so
  the ledger can see inside it:
  - `btrt::claim_exit_slot(pid) -> Option<u16>` — a `compare_exchange` on the registry slot
    (pure atomics, no SERIAL) that yields the test id. Called **inside the T2 acquisition**, and the
    result is stored in the row as `report_marker.token`, so the slot can never be consumed without
    the row recording it.
  - `btrt::record_exit(test_id, code)` — `pass`/`fail`, the completed-count increment, and the
    possible `finalize()`. Called with **no lock held**. It sets `report_marker.started` (CAS 0→1;
    a loser returns without recording) *before* the recording and `report_marker.finished` (release
    store) *after* it.

  **T4's ruling for `Report`, stated so it is not a judgement call at implementation time:**

  | Marker observed | Meaning | T4 destination | Rationale |
  |---|---|---|---|
  | `finished == 1` | The effect completed; only the ledger write was lost | **`Completed`** | Re-running would double-count `tests_completed` and could double-`finalize()` |
  | `started == 0` | The effect never began | **`Pending`**, token preserved | Safe to redo; the token means the next claimer needs no new btrt slot |
  | `started == 1, finished == 0` | **Ambiguous** — the claimer died *inside* `record_exit` | **`Completed`**, and bump `LEDGER_EFFECT_AMBIGUOUS{report}` | **The marker wins over the ledger.** For `Report` a duplicate corrupts the test ledger and can double-finalize the boot, while a missing report is *caught* by the AC-12 equality (`BTRT_EXIT_REPORTED != parented_first_commits` fails the gate). We take the failure mode that is loud over the one that is silently wrong |

  `LEDGER_EFFECT_AMBIGUOUS` is asserted **0** on every healthy boot: reaching it requires a kernel
  control path to die inside a short, non-blocking, lock-free atomic sequence, which is itself a fault
  that fails the run. The orphan-injection test drives it to a known value deliberately so the branch
  is exercised rather than assumed. The residual is R-19.

**The general rule this yields**, and the one a reviewer should check any future obligation against:
*an obligation may only be class B if its effect cannot commit under PM; every class-B obligation must
name a marker that the effect itself writes, and must state which side wins when the marker and the
ledger disagree.* An obligation with neither a PM-committable effect nor a marker does not belong in
the ledger.

**Row retention — the reap/tombstone gate.** Obligations are row-resident, so the row must outlive
every obligation. Today `waitpid` physically removes the row (`syscall/wait.rs:386` →
`remove_process`, `manager.rs:1086-1090`), and the `Resources` obligation **by construction outlives
reap** — grace and RootProof are not complete when the parent collects the status (that is R-13, and
it is a property of the design, not an accident). Shipping row-resident resource bits before fixing
row lifetime is the P6-before-P7 seam the ratification flagged. Therefore:

> **Retention rule.** A row is removed from the process table only when (a) every obligation in its
> ledger is `Completed` or `Absent`, **and** (b) its retirement receipt has been retired (grace
> elapsed + RootProof passed) or was never created. `waitpid` no longer removes the row: it records
> the reap and transitions `Zombie → Tombstone`. A `Tombstone` row is invisible to
> every lookup that means "a live process" — `find_process_by_pid/thread/cr3`, signal delivery, wait
> scanning, procfs enumeration, and PID-reuse allocation — and visible only to the ledger and the
> retire gate. The gate that finally removes it is the same S4 drain that retires resources.

**The two-event join — who actually performs the removal** *(v3, closure B; the MAJOR the
re-ratification raised).* v2 stated the *condition* for removal but named no *trigger*, and
`RowState` carried only the reap. If retirement completed first, nothing revisited the row when the
reap arrived; if the reap completed first, nothing revisited it when retirement landed. Either order
could leave a permanent tombstone, and an implementer patching around it would reach for premature
removal. v3 makes removal a symmetric join over two independently-written flags:

```
row.reaped : Option<(reaped_by, status)>     // written by the parent's reap  (P6a)
row.retired: bool                            // written by the S4 retire gate (P1/P6a)

reap(pid)      [PM]:  row.reaped = Some(..);  if row.retired  { remove_row(pid) }   // reap  is 2nd
retire(pid)    [PM]:  row.retired = true;     if row.reaped.is_some() { remove_row(pid) } // retire is 2nd
```

- **Both writes happen under PM**, so the read-of-the-other-flag and the removal are in the same
  acquisition as the write. PM serializes the two writers, so exactly one of them observes the other
  flag already set, and **removal happens exactly once regardless of order**.
- **Removal also requires the ledger term** (every obligation `Completed`/`Absent`); the ledger is
  PM-owned, so it is read in that same acquisition. If the ledger is not yet satisfied, neither
  writer removes and the row is revisited by the S4 drain — which re-evaluates the join for every
  tombstone it passes, so the join is not a one-shot edge trigger.
- **Rows with no receipt** (`Resources == Absent`, e.g. an x86_64 exit that released synchronously)
  have `retired = true` written at the same moment the ledger determines `Resources` is `Absent`, so
  the ordinary reap removes them immediately — the join degenerates correctly rather than stalling.
- **Rows that will never be reaped** (no parent, or the parent exits first) are covered by the
  reparent cursor handing them to the designated init before the parent's own row tombstones; a build
  with no designated init sets `reaped` at commit via an explicit `auto_reap`, which is a named
  branch, not an implicit fallthrough.
- **Both orders are gated, not just described — and the two workloads map the way the pseudocode
  says, not the way v3 first wrote them. (v3 repair.)** The counter incremented inside `reap()` is
  `{reap_second}`: *the reap was the second event, retirement had already landed.* The one
  incremented inside `retire()` is `{retire_second}`. v3 attached each to the opposite workload.
  Correctly: the **prompt-reap** fork/exit/reap workload — the parent collects the status long before
  grace elapses — makes the **retire gate** the second writer and drives `{retire_second}`; a
  **delayed-reap** injection — the child exits, the test sleeps past two grace epochs so retirement
  completes first, and only then does the parent `waitpid` — makes the **reap** the second writer and
  drives `{reap_second}`. P6b's gate asserts **both are nonzero in one run**. A join arm that no test
  can reach is dormant code wearing a different hat (phasing rule 2).

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

> **v3 repair — what P2's `Report` seed does and does not ship.** v3 as first written said the seed
> ships "with its effect marker and the split `claim_exit_slot`/`record_exit` API from day one",
> while the PLAN keeps `btrt::on_process_exit` intact (and ratcheted to its single call site) until
> P6b performs the split. Both cannot be true, and P2 would otherwise be shipping a class-B
> obligation with no recovery marker while claiming to have one. **The PLAN's sequencing is the one
> that holds**, and the reason is stated here rather than papered over:
>
> - **P2 ships the seed's *shape*, not its marker.** The obligation uses the four-state machine and
>   transitions T1/T2/T3 from day one — it is never a bool that a later phase upgrades. It does not
>   carry `report_marker`, and `on_process_exit` is called unchanged.
> - **At P2 there is nothing to recover with, and nothing that recovers.** T4 is performed *only* by
>   the S4 retire/reap gate, which does not exist until P6b (P6a's removal join runs with a
>   vacuously-true ledger term and performs no recovery). An obligation that is never recovered needs
>   no marker: the marker exists solely to tell T4 which destination to choose.
> - **Exactly-once still holds at P2**, by the sole-redeemer invariant alone — the commit path and
>   `handle_thread_exit` race under PM and exactly one of them claims.
> - **The cost is stated, not hidden.** Between P2 and P6b, a claimer that dies between T2 and T3
>   loses that one `btrt` report. That is **precisely what `main` does today** for a remotely-marked
>   victim (the sole `on_process_exit` call site is inside `handle_thread_exit`, which such a victim
>   never runs), so it is not a regression and rule 1 holds at every commit.
> - **P6b introduces T4 and the `claim_exit_slot`/`record_exit` split in the SAME PR.** A recovery
>   rule and the marker it reads must never be separated by a merge boundary; that pairing is what
>   closure B is actually about. P2's gate asserts `LEDGER_CLAIM_ORPHANED == 0` for exactly this
>   reason — no recovery path exists yet, so any nonzero value means one was added unnoticed.

---

### 1.7 Receipt custody — a receipt is never in a caller's hands *(v3, closure A)*

v2 changed `exit_process` to return a `#[must_use] Option<RetirementReceipt>` and adapted the two
PM-nested `enqueue_process_reclaim` sites. The re-ratification's first FATAL is that this **creates**
a loss channel rather than closing one: `#[must_use]` is satisfied by `let _ = …`, and the **seven
live `exit_process` callers** (enumerated in §0.3, all verified at `985881a6` — neither of the two
sites v2 adapted is one of them) would now be handed a receipt they were never written to carry. A dropped receipt destructs a root **with no grace and no RootProof** — strictly worse than
`main`. `#[must_use]` is a lint. This design does not accept a lint as a safety mechanism.

**The API is restructured so no caller can hold a raw receipt.**

```rust
// crate-private. No public constructor anywhere; the type is not nameable outside the crate.
pub(crate) struct RetirementReceipt { /* fixed size, preallocated fields */ }

// crate-private, PM-guard-taking. Exactly ONE caller is permitted (ratcheted).
pub(crate) fn exit_process_locked(pm: &mut ProcessManager, pid: ProcessId, code: i32)
    -> Option<RetirementReceipt>;

// The ONLY public entry point. Takes PM, drops the guard, then enqueues.
pub fn exit_process_and_retire(pid: ProcessId, exit_code: i32) -> ExitOutcome {
    let receipt = with_process_manager(|pm| exit_process_locked(pm, pid, exit_code));
    // PM guard is provably dropped here: with_process_manager owns the guard's scope.
    if let Some(r) = receipt { enqueue_process_reclaim(r); }        // leaf queue lock only
    ...
}
```

Four properties, each structural rather than conventional:

1. **No caller ever sees a receipt.** `RetirementReceipt` is `pub(crate)` with no public constructor,
   `exit_process_locked` is `pub(crate)`, and the P0 ratchet asserts `exit_process_locked` has
   **exactly one** call site — the wrapper. There is no signature in the public surface that can hand
   a receipt to code that might drop it.

   > **The adapted-site set, in three disjoint classes — one live-verified statement the whole round
   > uses** *(v3 tranche pass; this replaces every earlier "all nine call sites call the wrapper"
   > phrasing, which could not be true of class 3 and therefore could not be gated as an exact set).*
   > Verified by `git grep` at `main` @ `985881a6`. **"Nine" is the count of sites P2 ADAPTS. It has
   > never been the count of `exit_process` callers, which is SEVEN.** The two counts describe
   > different things and are never interchangeable in these documents:
   >
   > | Class | Count | Sites (live at `985881a6`) | What P2 does to it |
   > |---|---|---|---|
   > | **1 — `exit_process` callers** | **7** | `arch_impl/aarch64/exception.rs:778`, `:1146`, `:1233`, `:1336`; `interrupts.rs:1429`, `:1735`; `process/mod.rs:264` | each is restructured to call `exit_process_and_retire(pid, code)` **with no PM guard live** — the call is lifted out of the closure/binding, not renamed |
   > | **2 — the new SIGKILL arm** | **1** | `syscall/signal.rs:162` (today `process.terminate(-9)` under PM) | becomes a call to the **same** wrapper after `drop(guard)` |
   > | **3 — the PM-nested enqueue** | **1** | `task/process_task.rs:244` (`handle_thread_exit` phase 1) | **does NOT call the wrapper.** `handle_thread_exit` already owns its PM guard and its two-phase shape; the receipt rides out of phase 1 through the existing `phase1_result` value and is enqueued in phase 2, where PM is already dropped |
   >
   > **Deleted, not adapted:** `process/manager.rs:1152`, the enqueue inside `exit_process`'s own
   > body — it moves into the receipt returned by `exit_process_locked`. It is not a tenth adapted
   > site and is never counted as one.
   >
   > **The exact sets P2 must leave behind** (this is what the gate asserts, and it is stated as
   > three separate exact sets because one set cannot express three different shapes):
   >
   > - `exit_process_and_retire` — **exactly 8** call sites: classes 1 and 2. Class 3 is *not* among
   >   them, and a gate that expected nine could not pass.
   > - `exit_process_locked` — **exactly 1** caller: the wrapper.
   > - `enqueue_process_reclaim` — **exactly 3** call sites, all provably PM-free: the wrapper's
   >   post-guard enqueue, `handle_thread_exit`'s phase-2 enqueue, and `RetirementReceipt::drop`'s
   >   re-enqueue. `manager.rs:1152` is gone. *(P0's baseline pins the pre-P2 set at **2**; this is
   >   the post-P2 set, and the transition is part of P2's diff.)*
   > - `ProcessManager::exit_process` — **0** call sites in the public surface; the name survives only
   >   as the crate-private `exit_process_locked`.

2. **Dropping a receipt cannot free anything.** `impl Drop for RetirementReceipt` does **not** run
   destructors: it moves its contents into the reclaim queue and bumps `RECEIPT_DROPPED_UNRETIRED`
   (CI-asserted 0). So even an unreachable path that somehow drops one degrades to "retired late",
   never to "freed early". This is the belt to the private-constructor braces, and it is the reason
   the mechanism does not depend on nobody ever making a mistake.
3. **The Drop path cannot nest the queue lock under PM.** A receipt exists in exactly two places: the
   window inside `exit_process_and_retire` between the guard dropping and the enqueue, and P1's local
   detach slot (§1.8). Both are provably PM-free, and `RECLAIM_ENQUEUE_UNDER_PM` catches a violation
   at runtime if a future edit creates a third place.
4. **The take-and-enqueue pair is indivisible from the outside.** Because there is no public API that
   performs only the take, "the root left the row but never entered the pipeline" is not a state a
   caller can construct. That is what lets §1.6 treat the take-half of `Resources` as class A.

**All nine adapted sites move in P2**, in the PR that introduces the wrapper — they are enumerated
per class, with their required restructuring, in PLAN P2's call-site table. The seven class-1 sites
(the four aarch64 fault sites, the two x86_64 fault sites, and `process::exit_current`) currently call
`exit_process` **inside a live PM guard or `with_process_manager` closure**, so each needs the call
lifted out of the closure, not merely renamed. That is the honest size of this closure, and it is why
P2 declares its split seam.

### 1.8 Drain progress — the refusal path cannot livelock *(v3, closure C)*

v2's P1 cycle detaches a candidate under the queue lock, proves it with the lock dropped, and on
refusal "re-inserts and rotates". The re-ratification correctly observed that **there is no rotation**:
the live drain selects with `position()` from index 0 and `swap_remove`s
(`task/process_task.rs:379-383`), and it loops until no candidate is found. A receipt whose cheap
(epoch + shadow) predicate passes but whose live-row or cached-root blocker persists is therefore
re-selected immediately, forever — a single-entry livelock in the *first* behaviour-preserving phase.

Two rules fix it, and neither is a cap on work.

**Rule 1 — a bounded pass: no candidate may be selected twice in one pass.** The drain takes a pass
id (a monotonic `u32` bumped once per `reclaim_deferred_process_resources` invocation). Each queue
entry carries `last_pass`. The under-lock scan skips entries whose `last_pass == current_pass`, and
stamps the entry it selects. A pass therefore examines each live entry at most once and terminates in
at most *queue length* iterations, whatever the proofs say.

**Rule 2 — bounded retry, then park with a FRESH fence and a three-armed unpark.** Each entry
carries `proof_failures`. On the `K`-th consecutive refusal (`K = 3`) attributable to a **liveness**
blocker (`blocked_cached` or `blocked_live_row` — the two whose answer can change without time
passing), the entry is moved to a side list `PARKED_PROCESS_RECLAIMS` and is **not scanned at all**
until unparked. Refusals attributable to grace/hardware/shadow blockers do **not** count toward `K`,
because those clear with time alone and re-checking them is the whole point of the drain.

**(v3 repair — two defects in the park rule as first written.)**

1. **`parked_at` is a FRESHLY CAPTURED fence, never the receipt's retirement fence.** v3 said the
   entry is parked "with `parked_at` = the fence captured at parking", and PLAN P1's cycle said "park
   with `parked_at` = the captured fence" — both readable as reusing `reclaim.after_epoch`. That
   would be inert by construction: step 1 only selects an entry **whose fence has already elapsed**,
   so an unpark predicate keyed on it is *already true at the instant of parking*, the entry unparks
   on the very next sweep, and the livelock returns with two extra counter bumps per cycle. The park
   therefore takes a **FRESH `RetirementSnapshot` at the parking instant** and stores the fence
   derived from it (`ParkRecord.fence_at_park`, §1.4) — new acquire-fenced read of every online CPU's
   scheduling epoch, new online mask. *(v3 tranche pass — stated as a snapshot, not merely a "fresh
   fence", because the two obvious wrong implementations are both "a fence": reusing
   `reclaim.after_epoch`, and reusing the `RetirementSnapshot` step 2 of this same cycle already took
   before the proof ran. Step 2's snapshot predates the three refusals that led to the park; a fence
   built from it is stale by exactly the interval that matters. The park takes its own.)* A parked
   entry that unparks in the same drain invocation that parked it is a bug, counted
   `RECLAIM_PARK_IMMEDIATE_UNPARK` and asserted **0**.
2. **The epoch is NOT "the only event that can change a liveness blocker's answer" — that claim is
   false for `blocked_live_row`, and a park keyed on it alone can strand.** A live row is PM-owned:
   it disappears when the reap/retire join removes it (§1.6), which can happen on another CPU with
   **no scheduling-epoch advance anywhere** — e.g. a parent calls `waitpid` while every CPU in the
   captured mask sits in WFI. The unpark predicate is therefore a **disjunction of three arms**, any
   one of which returns the entry to the live queue for a full re-proof:

   | Arm | Fires when | Clears which blocker |
   |---|---|---|
   | **epoch** | a `RetirementSnapshot` shows every CPU in `fence_at_park`'s captured online mask has advanced its scheduling epoch | `blocked_cached` (a stale cached root can only be dropped by that CPU rescheduling) |
   | **row** | the global `ROW_REMOVAL_EPOCH` differs from `row_epoch_at_park` | `blocked_live_row` — bumped by one relaxed increment inside `remove_row(pid)` and the creating-row completion path, under the PM acquisition that already holds them. The bump takes no lock, walks no list and knows nothing about the parked list, so no ordering is inverted; the sweep reads it outside PM |
   | **age** | the epochs of the CPUs in `fence_at_park.online_mask`, **summed**, have advanced by at least `PARK_AGE_BACKSTOP_EPOCHS = 64` since `age_epoch_sum_at_park` | *nothing specific* — a pure safety net so that even if both keyed arms are somehow missed, the entry re-enters the live queue and is re-proved |

   **The age backstop is denominated in SCHEDULING EPOCHS, not wall time** *(v3 tranche pass — the
   pre-check's residual MINOR was that the arm's own gate required completion "within the stated
   backstop" while no unit was ever stated)*. The unit is deliberate: this design has no wall clock
   it is willing to depend on inside the drain, the epoch words are already captured in
   `fence_at_park`, and an epoch count is exactly the quantity the other two arms are expressed in.
   Concretely, the sweep re-reads the same epoch words the park captured, sums them over the *same*
   captured mask, and fires when `sum_now - age_epoch_sum_at_park >= 64`. Three properties follow and
   each matters: **(i) it needs no extra state** — the key is derived from `fence_at_park`, so the
   park record does not grow a timestamp; **(ii) it is strictly weaker than the `epoch` arm** — that
   arm needs *every* captured CPU to advance, this one accepts 64 advances anywhere in the mask, so a
   single scheduling CPU is enough and a mostly-idle machine cannot starve it; **(iii) it cannot be
   starved at all on this kernel**, because the 1 kHz tick drives a scheduler pass on every online
   CPU including the idle loop, so the sum is monotone and unbounded above. *(Informative only, and
   deliberately not the definition: at 4 online CPUs that is on the order of a few tens of
   milliseconds. No gate is written against the millisecond figure.)*

   The unpark sweep runs at the **head of every drain invocation**, fork's pre-allocation drain
   included, so a row removal is followed by a sweep at the next drain rather than needing one of its
   own. The age arm is explicitly **not a cap**: it only ever *adds* work — it makes a parked entry
   eligible again and never stops examining, skips or drops anything — which is what makes the stall
   in R-20 provably bounded rather than merely visible.

**Why this is not a drain cap** (the r23 round reverted three bounded drains, and phasing rule 1
forbids reintroducing that class): a cap stops examining entries that are *ready*; this stops
**re-examining entries already examined in this pass** and defers entries whose blocker provably
cannot have changed. Fork's pre-allocation drain keeps its full semantics — one full pass over every
live entry plus an unpark sweep first — and no cap parameter is introduced anywhere. The v2 statement
"no cap is added, no drain is moved, no drain is shared" is unchanged and remains true.

**Observability.** `RECLAIM_PASS_SKIPPED`, `RECLAIM_PARKED`, `RECLAIM_UNPARKED{epoch|row|age}`
*(v3 repair: split per unpark arm, so an arm that never fires is visible rather than hidden inside a
total)*, `RECLAIM_PARK_IMMEDIATE_UNPARK` (asserted **0** — the fresh-fence proof), and the gauge
`RECLAIM_PARK_RESIDENT` (with a reader, asserted to return to 0 at quiesce). A parked receipt is a
*visible, bounded, counted* stall rather than a spin that starves the drain — residual R-20. P1's
gate drives every one of these nonzero by forcing refusals at each blocker class **and by exercising
each unpark arm separately**, so no park/unpark arm lands dormant in the PR that introduces them.

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
`exit_process_and_retire(pid, -9)` — the **only** public entry point, per §1.7's wrapper-only receipt
custody. *(v3.1 repair: this paragraph used to spell the step
`with_process_manager(|pm| pm.exit_process(pid, -9))`, a shape P2 does not leave available —
`exit_process_locked` is crate-private with exactly one permitted caller, and `ProcessManager::exit_process`
has zero public call sites after P2. The mechanism was never in doubt; the narration was stale.)* Its
crate-private locked half already performs the unconditional
grace-stamped `defer_process_resources` on aarch64 **before** `terminate()` runs, so
`cleanup_cow_frames()` walks a `None` page table and the CoW decref moves behind grace + RootProof;
the receipt then rides out of the PM guard and is enqueued by the wrapper with no other lock live.
Plus the durable `report`/`sigchld` obligation seed (below).

> **v2 (condition 3) — the receipt leaves PM before it is enqueued.** v1 relied on `exit_process`'s
> existing body, which calls `enqueue_process_reclaim` at `manager.rs:1152` **inside the PM guard**,
> nesting the reclaim-queue lock under PM+DAIF-masked. That is the A-style interim shape the adopted
> no-overlapping-lock rule (§4.1) forbids and the B end-state must rip out — an interim violation is
> still a violation. Phase 2 therefore changes the signature:
>
> ```rust
> pub(crate) fn exit_process_locked(pm: &mut ProcessManager, pid: ProcessId, exit_code: i32)
>     -> Option<RetirementReceipt>;                       // crate-private; ONE permitted caller
> pub fn exit_process_and_retire(pid: ProcessId, exit_code: i32) -> ExitOutcome;  // the only public API
> ```
>
> The locked half *stamps and takes* the root into the receipt under PM (as today) but **returns** it;
> the wrapper's `with_process_manager(...)` yields the receipt, the PM guard drops, and only then does
> the wrapper `enqueue_process_reclaim(receipt)` with no other lock live. The move is allocation-free:
> `PendingProcessReclaim` is a fixed-size value and `core::mem::take(&mut process.pending_old_page_tables)`
> leaves an empty `Vec` behind without allocating. The **other** PM-nested enqueue,
> `handle_thread_exit` phase 1 at `process_task.rs:244`, is converted in the same PR by riding the
> receipt out through the existing `phase1_result` value and enqueuing it in phase 2, where PM is
> already dropped. After Phase 2, `RECLAIM_ENQUEUE_UNDER_PM == 0` is assertable and ratcheted.
>
> **v3 (closure A) — and no caller is handed the receipt at all.** v2 stopped at "return it and trust
> `#[must_use]`", which the re-ratification correctly called a receipt-loss channel: seven further
> live call sites would have been handed a value they were never written to carry. The public surface
> is therefore the wrapper alone, the receipt type is crate-private with no public constructor, its
> `Drop` re-enqueues instead of freeing, and **all nine sites P2 must adapt are enumerated and
> adapted in that PR** — in three classes: the **seven** live `exit_process` callers and the new
> SIGKILL arm (**eight** wrapper calls), plus `handle_thread_exit`'s PM-nested enqueue, which keeps
> its own two-phase shape and routes the receipt out through `phase1_result` instead of calling the
> wrapper. *(v3 repair: nine is the adapted-site count, not the `exit_process` caller count, which is
> seven at `985881a6`. v3 tranche pass: the three classes and the three exact post-P2 sets are stated
> once, in §1.7; no gate in these documents asserts "all nine call the wrapper", because class 3 does
> not and cannot.)* See §1.7 and PLAN P2's call-site table.

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
3. **P10a/b/c/d** migrate the wait families one per PR, each consuming the request/wake mechanism P9
   already publishes and each proving deregistration-before-commit for its family: 10a futex,
   10b `WaitQueueHead` + stdin/TTY readers, **10c `BlockedOnSignal` (`pause`/`sigsuspend`) — the
   family v2 omitted (v3, closure D)**, and 10d child-wait + timer/nanosleep + completion/I-O.
   **P10d** empties the legacy allowlist and deletes the remote-marking body; only then is #491
   complete.
4. **P11** deletes the last direct `Process::terminate` callers (§2.6), at which point
   `Process::terminate` itself is deleted.

`send_signal_to_all_processes` / `send_signal_to_caller_process_group` reach SIGKILL only through
`send_signal_to_process`, so they are fixed by the same change.

### 2.2 #464 — init identity, then (separately) init death policy

Four prior attempts died by bundling "who is init" with "what happens when init dies", and three of
them died specifically on `interactive = ["testing"]` inverting a `#[cfg]` gate. This design ships
them as **two separate PRs** and uses **no `#[cfg]` gate anywhere** — designation is runtime *data*,
so a build that never designates an init can never trip the policy.

**Identity (Phase 5a).** `ProcessManager` gains exactly one authority: `designated_init: Option<ProcessId>`.

- **PID 1 is reserved** for the explicit init constructor; ordinary/test allocation starts at 2 —
  across **all eight** `next_pid.fetch_add` sites (`manager.rs:141`, `:378`, `:602`, `:1076`, `:1419`,
  `:1561`, `:1704`, `:2169`; base at `:118`), not the four this document originally counted. Init is
  built off-table with provisional PID 1. (C's graft, endorsed by Judge 1.)
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
- The production init literals become the accessor: `manager.rs:1165` (read at `:1166`, `:1176`,
  `:1179`), `process_task.rs:647`, `process_task.rs:720` (read at `:723`, `:726`), and
  `signal.rs:26`'s `INIT_PID` constant (read at `:124` and `:402`) — see §0.3.1, which also strikes
  the two sites this document previously named that are not init literals at all. The three
  `test_userspace.rs` literals (`:84`, `:203`, `:292`) are creation-time setup, not teardown, and are
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

> **v3 (closure E) — the group-sibling bypass is closed at BOTH ends.** The re-ratification found a
> FATAL hole in exactly the seam between P9 and P12: P9 makes fatal signals **thread-group scoped**,
> while v2's P12 dropped only a signal aimed at the designated-init **row**. A user who obtains a
> `CLONE_VM` sibling sharing init's effective TGID could target the sibling; S1 would seal and kill
> the whole group — init included — without the drop check ever being consulted. The hole is a
> composition defect, so it is closed compositionally, at both ends, and neither end is load-bearing
> alone:
>
> **End 1 — admission (ships in P5, with the designation authority).** `sys_clone` **refuses to
> publish into the designated init's thread group**: after deriving the parent's effective TGID at
> `syscall/clone.rs:84` — already inside the live PM guard taken at `:60` — it compares that TGID
> against the designated init's effective TGID and returns **`EINVAL`** if they match. **The
> designated init cannot acquire `CLONE_VM` siblings, deliberately and by documented design.** This
> is a real, stated ABI restriction, not an accident: Breenix's init is a single-threaded supervisor,
> nothing in-tree clones from it, and a multi-threaded init would need a separate, explicit design
> pass for group-scoped death anyway. There is exactly **one** production write of a non-`None`
> `thread_group_id` (`syscall/clone.rs:210`), so this refusal is the complete admission surface, and
> a P0 ratchet rule pins that write site by name so a second one cannot appear unnoticed.
>
> **End 2 — the seal itself (ships in P12, defence in depth).** S1's group-seal check does not test
> "is the target row the designated init"; it tests **"is the designated init a member of the target
> effective thread group"**. If it is, the entire request is dropped silently, nothing is sealed,
> nothing is marked, and the send returns 0, counted `INIT_FATAL_SIGNAL_DROPPED{group}`. The check is
> a membership test over the group the transaction is about to seal, evaluated in the **same PM
> acquisition** as the seal, so it cannot be raced by a clone (a clone either published before the
> transaction and is a member the check sees, or acquires PM afterwards and hits a sealed group).
>
> **v3 repair — the drop is scoped by REQUEST ORIGIN, or it would swallow init's own `exit_group`.**
> As first written, end 2 dropped *any* request whose target group contains the designated init.
> That contradicts the policy's own third rule ("kernel-fatal, and only these: init's **own**
> `exit`/`exit_group`, or an unhandleable fatal fault"): init's `exit_group` **is** a group-scoped
> request naming a group that contains init, so the unscoped check would silently discard it and the
> required panic path would become unreachable. Every `ExitIntent` therefore carries an explicit
> origin, and the drop applies to exactly one of them:
>
> | `ExitIntent.origin` | Produced by | Init-group drop applies? |
> |---|---|---|
> | `Signal` | a default-fatal signal reaching disposition with no handler installed — `sys_kill`, `sys_tgkill`, `sys_killpg`, **including a self-directed `kill(getpid(), …)`/`raise`** | **YES** — dropped, nothing sealed, send returns 0, `INIT_FATAL_SIGNAL_DROPPED{group}`++ |
> | `ExitSyscall` | `sys_exit_group`, or `sys_exit` of the group's last member | **NO** — the seal proceeds, the request commits, `INIT_DEATH_LATCH` is set, S5 panics |
> | `FatalFault` | an unhandleable synchronous fatal fault taken by a member (P11's converged path) | **NO** — same; this is the second kernel-fatal producer |
>
> The `Signal` drop is deliberately **sender-agnostic**, which is both Linux-faithful and stricter
> than "externally-originated": Linux's `sig_task_ignore` consults `SIGNAL_UNKILLABLE` and the
> disposition, never the sender, so `kill(1, SIGKILL)` issued *by init itself* is also dropped — only
> `force`d kernel-only signals bypass, which is exactly the `FatalFault` row. `ExitSyscall` and
> `FatalFault` bypass the membership test entirely, so the kernel-fatal path is preserved by
> construction rather than by the check happening not to fire. Note also that P5's clone-admission
> refusal makes init's group a singleton, so an `ExitSyscall`-origin request in init's group can only
> have come from init — there is no sibling that could issue `exit_group` on init's behalf.
>
> Why both: end 1 makes the dangerous state unconstructible; end 2 makes the policy correct **even
> if** the state is somehow constructed — by a future kernel-side init helper thread, a test harness,
> or a `thread_group_id` write added later. Belt-and-braces is recorded honestly as R-18, including
> the maintenance obligation that if init ever legitimately gains siblings, end 1 is what must be
> revisited and end 2 is what keeps the system safe meanwhile.

The fatal action never happens under a guard. S1 sets `INIT_DEATH_LATCH` (one relaxed store) **only
for a committed, certainly-attributed victim** — an attribution miss or a TID/CR3 divergence can
never latch (AC-3, both directions). S5 reads the latch in ordinary kernel context with PM and
scheduler guards out of scope and DAIF restored, records a pre-panic lock/IRQ snapshot (Judge 1's
graft — this is what makes the panic *reportable*, since the panic handler takes SERIAL/framebuffer
locks), then panics. Note honestly: aarch64's panic handler parks the panicking CPU only; peers are
not actively stopped (Residual R-12, OQ-1b).

### 2.3 #471 — group seal + exec detach

**Exec detach (Phase 3).** At each exec commit point — after every fallible step has succeeded and
the new page table is installed — clear both fields. **Sequencing: #573 lands before or with this**,
because the failure path on which both fields must survive byte-identically is the path that leaks the
half-built address space on x86 today; **#572** (`AlreadyTerminated` abandon bypassing custody,
`manager.rs:1131-1136`) sits on the same exec-root surface:

```rust
process.page_table = Some(new_page_table);
process.inherited_cr3 = None;
process.thread_group_id = None;   // effective TGID falls back to pid: a fresh singleton
```

On **any** exec failure both fields are preserved unchanged. The existing live-sibling guard
(`find_live_clone_vm_sibling_holding_cr3`, defined at `manager.rs:46`, called at `:3063` and `:3368`)
stays — it exists because **#468** is open, and this phase does not close #468. Both directions analysed:
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
dispatch independently refusing `Creating`/`ExitRequested` rows before arming CR3/TTBR0.

> **The dispatch refusal lives in `interrupts/context_switch.rs`, not `task/scheduler.rs`.** PR #570
> rewrote that site into a single unconditional PM try-lock before `scheduler::schedule()`
> (`context_switch.rs:218`) with a refusal arm that neither schedules nor rolls back, plus a second
> arm for a thread whose address space is gone — both ratcheted in `tests/context_restore_structure.rs`.
> P3's `Creating` refusal is a **third arm on that already-ratcheted site**, and is derived against
> #570's shape rather than the pre-#570 dispatch this section was originally written against.

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

**FD acquisition control flow (P7's API, used here).** **(v3 repair — restated so the descriptor is
never in two places.)** Exactly one descriptor is in flight at a time, the owning value never leaves
the row, and the obligation states bracket the loop:

```
T2: Fds -> Claimed{me}                            [inside PM #1]
loop {
    // PM: if the slot is empty, move ONE (fd, desc) from the table into the ROW's slot;
    //     then mint a NON-OWNING CloseTicket from the slot and return it.
    //     If the slot is already occupied (a dead claimer), re-ticket THAT descriptor.
    //     None <=> table empty AND slot empty.
    let Some(ticket) = with_process_manager(|pm| pm.begin_fd_close(pid)) else { break };

    endpoint_hangup(&ticket);            // NO LOCK HELD: pipe/PTY/TCP lock taken here only.
                                         // Idempotent by CAS on the endpoint -> replay-safe.

    // PM: mark hangup_done, DROP the owning desc out of the slot, clear the slot.
    //     This is the one destructive step and it is PM-atomic with the custody release.
    with_process_manager(|pm| pm.finish_fd_close(pid));
}
T3: Fds -> Completed                              [fresh PM acquisition; table AND slot proven empty]
```

The owning `FileDescriptor` is moved out of the table into the row's slot and is destroyed there —
it is **never returned to the caller**, so no `Vec` is built and the allocating
`Process::take_fd_entries()` (`process.rs:335`) is retired rather than reused. If the victim is
preempted mid-loop the ledger still reads `Claimed{me}`, and T4 (orphan recovery) can only fire if
`me` is proven dead — which cannot happen while `me` is the running victim.

**v3 repair (closure B) — the slot is the SOLE OWNER, and the unlocked step gets a ticket, not the
descriptor.** v3 as first written had `take_next_for_exit` return the `(fd, FileDescriptor)` to the
caller for an unlocked `close(e)` *and* leave it visible in `fd_in_flight` for recovery. Those are
mutually exclusive: retaining the value and returning the same value cannot both happen, and cloning
it to make them both happen is exactly the second copy that "a double close is not representable"
depends on not existing — reopening double-close and endpoint-refcount corruption, which is the
failure class this whole obligation exists to prevent. The repaired shape splits the operation in
three (§1.6): `begin_fd_close` **[PM]** takes/retains and mints a `CloseTicket` — a clone of the
*endpoint handle* with **no close operation on it**, so it is structurally incapable of closing
anything; `endpoint_hangup(&ticket)` **[no lock]** performs the only step that needs an endpoint
lock and is idempotent by construction (a CAS-guarded state transition — a **requirement P7
implements and gates**, not an assumption); `finish_fd_close` **[PM]** performs the single
destructive step, dropping the owning descriptor and clearing the slot in one acquisition. A claimer
that dies anywhere leaves the descriptor in the slot with `hangup_done == false`; the next claimer
re-tickets it and replays only the idempotent half. **A double close is not representable**, because
the owning descriptor exists in exactly one place at every instant — table, or slot — and is
destroyed exactly once. This is why `Fds` needs no started/finished bits and no "which side wins"
ruling: there is no ambiguous window to rule on. `hangup_done` is diagnostic
(`FD_HANGUP_REPLAYED`), never load-bearing.

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

### 2.7 Expedite evidence is teardown-attributed *(v3, closure F)*

AC-11's central claim is that a victim's time-to-death is bounded by an **SGI round trip** rather than
by its next natural tick. v2 tried to evidence that with `EXIT_SGI_SENT` wired to the scheduler's two
existing `SGI_RESCHEDULE` send sites. Read live at `985881a6`, those sites are
`Scheduler::send_resched_ipi` (`scheduler.rs:1843`, send at `:1857`), which wakes **idle** CPUs, and
`send_resched_ipi_to_cpu` (`:1868`, send at `:1886`), which targets the CPU that received a **newly
runnable task**. Neither knows anything about a victim, so on a busy boot the counter is nonzero
before any kill happens and a "> 0" gate is satisfied by ordinary scheduling. That is not evidence;
it is a coincidence with a counter attached. The MAJOR is sustained and the wiring is replaced.

- **A teardown-only send helper.** P2 introduces
  `Scheduler::send_exit_expedite_sgi(victim_pid: u64, batch: GroupBatchId)`, which broadcasts
  `SGI_RESCHEDULE` to every other online CPU and is the **only** site that records `EXIT_SGI_SENT`.
  The two generic helpers are explicitly **not** wired, and a P0 ratchet rule asserts that
  `EXIT_SGI_SENT` appears at exactly one source location. Because the helper does not exist before
  P2, **P0 declares `EXIT_SGI_SENT` legitimately zero until P2** — the same honesty the v2 P0 table
  already applies to `TEARDOWN_ENTRY{group}`.
- **Per-PID pairing, mirroring the defer/reclaim gate.** Each send emits a `trace_event!` carrying the
  victim pid; the victim's own CPU emits an observation event carrying the same pid the first time it
  observes that this victim can no longer be dispatched. The gate asserts, **for the specific pid the
  test created**, that a `SENT{pid}` event exists, that an `OBSERVED{pid}` event exists, and that the
  observation timestamp follows the send. Two unrelated streams with equal totals cannot satisfy a
  per-pid pairing, which is exactly the property the P0 defer/reclaim test was given.
- **The latency claim is measured, not asserted.** The interval `SENT{pid} → OBSERVED{pid}` is
  compared against the victim's tick period; the gate requires it to be strictly shorter. A gate that
  merely counts SGIs would pass even if the victim died at its next tick, which is the thing AC-11
  exists to rule out.

#### The P2-era observation side is a specified mechanism, not a placeholder *(v3 tranche pass; slot protocol repaired in v3.1)*

The pre-check sustained a further gap that v3 left as the words "the P2-era proxy": **`EXIT_REQUEST_OBSERVED{pid}`
is written by the return-boundary hook, which does not exist until P8**, and the live SGI receiver at
`arch_impl/aarch64/exception.rs:1761-1768` does exactly two things — record
`SCHED_RESCHED_IPI_RECV` with the **interrupt id** as its payload, and `set_need_resched(true)`. It
carries no pid and no batch. A P2 gate keyed on `EXIT_REQUEST_OBSERVED` would be keyed on a counter
that structurally cannot move, which is a vacuous gate wearing a pairing's clothes. **P2 therefore
ships its own attributed observation mechanism, and it is specified here rather than deferred.**

**`EXIT_KICK` buckets — a fixed table, three atomics wide, no lock and no allocation.** A
`[KickSlot; 64]` array lives in the P0 teardown provider alongside the counters;
`KickSlot { pid: AtomicU64, at: AtomicU64, state: AtomicU64 }`. *(**v3.1 repair.** The v3 tranche
pass specified this slot as `{ pid, at, seq }` published with `seq.fetch_add(2, Release)`, which was
broken by construction: `2` is even, so a publish could never clear bit 0, and once a bucket had been
observed once it could never be observed again — `0 → publish 2 → observe 3 → publish 5`, still
reading "observed". Publication now **assigns a fresh generation** instead of adding to a counter,
and the observed flag lives inside that generation.)*

**The `state` word — one atomic that is simultaneously the ownership reservation, the generation, and
the generation's observed flag.**

| Bits | Field | Meaning |
|---|---|---|
| 63…2 | `gen` (62 bits) | generation of the record currently in `pid`/`at`. `gen == 0` is the never-published sentinel; **every** publication installs `gen + 1` |
| 1 | `LOCK` | a publisher owns the slot: `pid`/`at` are in flux and no observer may sample them, and no rival publisher may write them |
| 0 | `OBSERVED` | **this generation** has been claimed by an observer. It is a property of the generation, never of the bucket, and it is re-created *clear* by every publication |

Spellings used below: `gen_of(s) = s >> 2`, `LOCK = 0b10`, `OBSERVED = 0b1`. 62 generation bits at one
kill per bucket cannot wrap in any reachable run; even at wrap, a stale claim would have to name the
full 62-bit generation, so there is no ABA window worth arguing about.

- **Publish — reserve, fill, commit (sender side, in the teardown-only helper).** Inside
  `send_exit_expedite_sgi(victim_pid, batch)`, **before** the broadcast and with no lock held,
  `bucket = victim_pid % 64`:

  1. **Reserve — one CAS, and it is the ownership handshake.** `cur = state.load(Relaxed)`. If
     `cur & LOCK != 0` a rival publisher owns the slot: **do not spin** — bump
     `EXIT_KICK_BUCKET_COLLISION`, publish nothing, and go straight to the broadcast (publication is
     *evidence*; the SGI is the *mechanism*, and evidence never delays a teardown). Otherwise attempt
     `state.compare_exchange_weak(cur, ((gen_of(cur) + 1) << 2) | LOCK, Acquire, Relaxed)`, retrying
     from a fresh load on failure, **bounded at `KICK_RESERVE_ATTEMPTS = 4`**; on exhaustion the
     publisher takes the same count-and-skip arm, so this path contains no unbounded loop. `Acquire`
     on success stops the following `pid`/`at` stores being hoisted above the reservation *and*
     synchronizes-with the previous publisher's `Release` commit, so the reserver inspects a settled
     slot. **This CAS is the whole answer to the torn-pair finding:** the reservation winner is the
     only CPU that may write `pid`/`at`, so two publishers can never interleave stores into one slot.
     Note what the installed value already is — a **new generation, `OBSERVED` clear** — so the
     observed flag is destroyed by the act of reserving, not by a separate clearing step that a later
     edit could drop.
  2. **Displacement accounting — exact, because the slot is now owned.** Still holding `LOCK`, read
     the displaced record: if `gen_of(cur) != 0 && (cur & OBSERVED) == 0 && pid.load(Relaxed) != victim_pid`,
     the previous kick was overwritten before anyone observed it — bump
     `EXIT_KICK_BUCKET_COLLISION`. This read is race-free by construction: no other writer may touch
     the slot while `LOCK` is set.
  3. **Fill.** `pid.store(victim_pid, Relaxed)`, then `at.store(now, Relaxed)` — the same monotonic
     read the tracing framework already uses for event timestamps. `Relaxed` is sufficient because the
     commit's `Release` is what publishes both, and no observer may read them while `LOCK` is set.
  4. **Commit — a plain store, and legally so.** `state.store((gen_of(cur) + 1) << 2, Release)`: same
     generation, `LOCK` cleared, `OBSERVED` **clear by construction**. `Release` orders the two
     preceding stores before any observer's `Acquire` load of this exact value. The unlock is a store
     rather than a CAS because the publisher holds exclusive ownership — observers refuse a locked
     state and rival publishers refuse to reserve one, so nothing else can have modified `state` in
     between. `EXIT_KICK_PUBLISHED{bucket}` increments **after** the commit, never before, so the
     counter counts committed records rather than attempts.

  This is the only site that touches the table's publish side, ratcheted the same way `EXIT_SGI_SENT`
  is.

- **Observe — sample, validate, claim (victim side, at the peer's next scheduler pass).** The event
  AC-11 actually needs at P2 is *"the victim can no longer be dispatched to EL0"* — and that decision
  is already made, on the peer, at the scheduler pass where `terminate_process_threads`' quarantine
  makes the scheduler **decline to dispatch** that thread. At exactly that point, on a path that
  already holds the scheduler's own state and takes no additional lock, for
  `bucket = thread_pid % 64`:

  1. `s1 = state.load(Acquire)`. **Reject and return** — the victim is quarantined, so the decline,
     and with it this sample, recurs at the next pass — if `gen_of(s1) == 0` (never published), if
     `s1 & LOCK != 0` (a publication is in flight), or if `s1 & OBSERVED != 0` (this generation is
     already claimed).
  2. `pid_seen = pid.load(Relaxed)`; `at_seen = at.load(Relaxed)`. The `Acquire` in step 1 orders both
     loads after the publisher's `Release` commit of generation `gen_of(s1)`.
  3. **Seqlock validation.** `s2 = state.load(Acquire)`; **if `s2 != s1`, discard the sample and
     return.** A publication began — or began and completed — while sampling, so the pair may mix two
     generations and must never be recorded.
  4. Require `pid_seen == thread_pid`, then **claim that specific generation**:
     `state.compare_exchange(s1, s1 | OBSERVED, AcqRel, Relaxed)`. Exactly one CPU can win. The winner
     records `EXIT_KICK_OBSERVED{pid}` with the interval `now - at_seen`. A loser returns silently:
     either a peer claimed the same generation — one observation per kick is exactly what the gate
     wants — or a newer generation is already installed and the next pass will see it.

  One CAS on the success path, no lock, no allocation, no serial, and **no new per-thread field** —
  the victim's identity is the pid the scheduler already has in hand.

- **Why a torn `pid`/`at` pair is impossible — four independent barriers.** *(This is the MAJOR
  finding, closed structurally rather than by counting.)* **(1) Publisher mutual exclusion:** only the
  reservation-CAS winner writes `pid`/`at`; a rival that finds `LOCK` set writes nothing at all, so
  interleaved stores from two publishers cannot occur. **(2) Bracketing:** the two stores happen
  strictly between a `LOCK`-setting `Acquire` CAS and a `LOCK`-clearing `Release` store, and no
  observer samples a locked slot. **(3) Seqlock validation:** the `s1 == s2` re-read rejects any pair
  that spanned a re-publication — every publication changes `state`, because the generation
  increments, so a spanning publication is always visible to the validator. **(4) The claim names the
  generation:** the final `compare_exchange` is against the exact `s1`, so even a validated sample
  cannot be recorded if the generation moved before the claim landed. Barriers 3 and 4 are
  deliberately redundant with 1 and 2 — this record is evidence underwriting a safety argument, and
  redundancy that costs one extra acquire load is the right price.

- **Why a bucket is reusable forever — the FATAL finding, closed by construction.** Each publication
  *installs* `(gen + 1) << 2`, a **fresh generation with `OBSERVED` clear**, as one atomic write; it
  never adds to the existing word, so no low bit can survive a publication. `OBSERVED` decorates a
  generation, and an observer claims **that generation's exact state value**, so observing generation
  N neither satisfies nor blocks the observation of generation N+1. The v3 failure mode — bit 0 sticky
  forever after the first observation, silently and permanently killing the evidence for 1/64 of the
  pid space with no error signal — is unrepresentable here:
  `0 → publish (1<<2) → observe (1<<2)|1 → publish (2<<2) → observe (2<<2)|1 → …`, indefinitely.
  Sequential reuse is **exercised by its own negative gate** in PLAN P2, not merely argued here — the
  v3 defect survived precisely because a single-victim gate against an empty table cannot see it.

- **Bucket collisions are counted in two exhaustive arms, so no collision is silent.** A colliding
  publication either **(a) loses the reservation** — it finds `LOCK` set, or exhausts
  `KICK_RESERVE_ATTEMPTS` — and the loser itself bumps `EXIT_KICK_BUCKET_COLLISION` and publishes
  nothing; or **(b) wins the reservation and displaces an unobserved record of a different pid** — and
  the winner, holding exclusive ownership, bumps `EXIT_KICK_BUCKET_COLLISION` after reading a stable
  displaced record. Those two arms partition the space of colliding publications, so the counter is
  not the best-effort heuristic v3 shipped ("an unobserved slot holding a different pid", checked
  without ownership and therefore able to miss a live race). P2's main workload runs a **single named
  victim**, so neither arm can fire there; the counter exists so that a collision during a soak is
  *visible* rather than silently weakening a later run's evidence — and the v3.1 negative gate drives
  **both** arms deterministically through a test hook rather than hoping a race occurs.

- **The one accepted strand, disclosed rather than discovered.** A publisher that faults between its
  reservation and its commit leaves that bucket `LOCK`-set for the rest of the boot; every later
  publication into it then takes arm (a) and counts. The loss is therefore **loud** — P2's gate
  asserts `EXIT_KICK_BUCKET_COLLISION == 0`, so a strand fails the gate instead of quietly degrading
  the evidence — and the window is two relaxed stores with no call, no loop and no allocation, on a
  path whose only fault is already fatal. Residual **R-21**.
- **What the proxy proves, stated exactly, and what it does not.** It proves that **this** send was
  followed by **this** victim's quarantine being observed on a peer CPU, within a **measured**
  interval shorter than a tick. It does **not** prove the victim observed a *latched exit request* —
  that word does not exist until P8. P2's AC-11 claim is therefore precisely *"the expedite reached
  the peer and the peer stopped dispatching this victim, faster than a tick"*, and it is written that
  way in P2's gate. The stronger request-observation claim arrives with the boundary hook in **P8**,
  where `EXIT_REQUEST_OBSERVED{pid}` replaces the proxy and **the bucket table is deleted in the same
  PR** — the proxy is scaffolding with a demolition date, not a permanent second mechanism.
  Residual **R-21**.

The same helper and the same pairing are reused by P9's group cutover, where the payload additionally
carries the batch id so a group kill's expedites are attributable to one batch rather than summed;
between P2 and P8 the observation half of that pairing is the kick bucket, and from P8 it is
`EXIT_REQUEST_OBSERVED{pid}`.

---

## 3. Numbered traceability — all 13 acceptance criteria

*(Rows whose mechanism changed in v2 or v3 are marked; unmarked rows are v1 verbatim. Phase numbers
reflect the corrected order: old P10 → **P9**, old P9a/b/c → **P10a/b/c**, old P6 → **P6a + P6b**, and
in v3 the wait families become **P10a/b/c/d** with `BlockedOnSignal` inserted as P10c and the
legacy-arm deletion moving to **P10d** — closure D.)*

| # | Criterion | Mechanism | Phase | Evidence that must be produced |
|---|---|---|---|---|
| **1** | Init designation only after creation fully succeeds; no phantom PIDs | Held-publication ticket: row inserted **and** non-runnable scheduler thread created before `designated_init` is committed under PM; PID 1 reserved off-table (all eight `next_pid` sites — §0.3.1) so a failed attempt leaves no row, no designation, and PID 1 retryable. Nothing in `create_*` touches designation. | **5a** | Failure injection after PID selection at each fallible stage (page table, ELF, stack, publication): `designated_init() == None`, no row; retry succeeds as PID 1 |
| **2** *(v2 — cond. 7; v3 — closure E)* | No panic/fatal action while PM held with DAIF masked; **and the protection cannot be bypassed through a group sibling** | S1 does **one relaxed store** to `INIT_DEATH_LATCH`. All fatal escalation is a receipt redeemed at S5 with PM and scheduler guards out of scope and DAIF restored; a pre-panic lock/IRQ snapshot is recorded first. No `#[cfg]`, so no build carries a differently-scoped variant. **The latch's producer set shrinks under the adopted Linux-faithful policy: external fatal signals to init are dropped at send and can never latch, so only init's own exit commit and an unhandleable fault remain.** **v3: the drop check is a *group-membership* test evaluated in the seal's own PM acquisition, and `sys_clone` refuses to publish into init's thread group at all (§2.2) — so a `CLONE_VM` sibling cannot be used to seal and kill init's group.** | 5 (clone admission), 12 (drop + latch) | `INIT_PANIC_WITH_LOCK == 0`; injected init death records PM owner `None`, scheduler owner `None`, IRQ state normal immediately before panic; the panic's serial output is **complete** (proving no lock was held). **`kill(1, SIGKILL)` from userspace returns 0, `INIT_FATAL_SIGNAL_DROPPED` increments, init survives, `INIT_DEATH_LATCH == 0`; no test asserts `EPERM`.** **v3: `clone()` from init returns `EINVAL` and no row with init's TGID is ever created (asserted over the whole boot); a group-scoped fatal request naming a group containing the designated init is dropped with `INIT_FATAL_SIGNAL_DROPPED{group}` incremented, nothing sealed, send returns 0; the ratchet asserts `thread_group_id = Some(…)` has exactly one production write site** |
| | *(Phase-cell reconciliation, 2026-08-17 — coordinator ruling R12, updated for P5b shipment.)* AC-2's phase cell read `5 (clone admission), 12 (drop + latch)`, which predates the P5a/P5b split **and** predates what actually landed. Reconciled against the tree: the **clone-admission publication transaction** (`ProcessManager::admit_clone_into` + the `clone_admission_oracle`, pinned on both gates) landed in **P3/PR #587**; the **`sys_clone` init-group refusal** — the half this row's "clone admission" text describes — shipped in **P5b** after #575 closed by PR #594, with its quiesce walk live in the service-sequence gate, and is imported nowhere by P5a; the drop + latch remains **P12**. Read as: **P3 (landed) + P5b (shipped) + P12**. AC-7's cell already reads `3 (admission), 9 (cutover)` and needed no change. Nothing from P5b appears in P5a. | | | |
| **3** | Victim attribution certain before fatal escalation; a heuristic CR3 miss must not panic | §2.4: TID-first, stack-slot cross-check, CR3 as root-consistency only, divergence counted and never fatal, `AttributionUncertain` → safe redirect + deferred intent. The latch keys on a **committed** victim, never on a resolution attempt. | 11 (attribution), 12 (escalation) | `clonevm_fault_test`: a CLONE_VM child faults → the **child** row dies, the parent survives, no refault loop. *This test fails on `main` today* — it is a live bug fixed, not a tautology. TID/CR3 mismatch injection bumps `EXIT_ATTRIBUTION_UNCERTAIN`, latches nothing, kills nobody |
| **4** | One source of truth for init identity; no hardcoded PID 1 beside runtime designation | `ProcessManager::designated_init` is the sole authority; the production literals (`manager.rs:1165`, `process_task.rs:647`, `:720` — re-anchored, §0.3.1) and `signal.rs:26`'s `INIT_PID` become the accessor. `ProcessId::INIT` survives only as the ABI *validation* constant, never a lookup. | **5a** | P0 ratchet fails on any new `ProcessId::new(1)` in production teardown/wait/signal/reparent code; the three `test_userspace.rs` sites are allowlisted **by name** |
| **5** | Kernel and userspace init guards must agree (`init_shell` keys on `getpid()==1`) | Structural, not conventional: PID 1 is **reserved** for the explicit init constructor and production designation is **validated == PID 1 and refuses otherwise**. `init_shell.rs:1028` is not changed. **Corrected 2026-08-17 (coordinator ruling R7):** this row previously claimed *"so no second contract exists to drift"* — false. `userspace/programs/src/bsh.rs` carried a second, unnamed contract keyed on `getpid() == 2 \|\| 3`, which the reservation renumbers straight through. P5a replaces it with an argument-derived contract (init passes `--init-shell`; no PID value participates), so the tree now has exactly one PID-valued userspace init guard — `init_shell.rs:1028` — and it is pinned. A non-PID-1 init would require an explicit userspace ABI change and is not silently supported. | **5a** | Cross-tree source assertion + boot test: designated pid == 1 == the pid init observes via `getpid()`; a build with no real init leaves designation unset and does **not** treat whichever process got the low PID as init; ratchet asserts zero PID-valued init-shell detection in `bsh.rs` and exactly one `--init-shell` conferral from `init.rs` |
| **6** | exec detaches `thread_group_id` **and** `inherited_cr3` | Both assignments at every exec commit point, after all fallible work, before PM release; both preserved on every failure; existing live-sibling guard retained. Both arches in one commit. **(v2: also a hard prerequisite of P8's last-reference decision — §2.3.)** **Sequenced behind #573**, whose leak sits on the failure path this AC asserts over. | 3 | `clonevm_exec_test` extended: successful exec → both `None`, fresh root, effective TGID == pid, and a kill aimed at the **old** group cannot reach it; failed exec → both preserved byte-identical. "Fresh root" is read from the exec-cohort per-PID oracle (`tracing/providers/teardown.rs:1192`, x86 `PT_EXEC_COHORT` line), not argued |
| **7** | Group membership examined atomically; no snapshot stale across a PM drop | **No snapshot exists.** The group-exit PM transaction *is* the seal: mark every live effective-TGID member with one batch id inside one guard; `sys_clone` publication validates group lifecycle under the same lock; scheduler threads carry group id + generation and re-check before first dispatch; threads publish non-runnable until the row is published. | 3 (admission), **9** (cutover) | Deterministic clone-vs-seal barrier test: the child is either included in the batch or `sys_clone` returns `EAGAIN` — never a runnable unrequested member. Ratchet rejects any group PID `Vec` snapshot in teardown code |
| **8** | Sibling kernel stacks freed ONLY behind two-epoch grace via scheduler ownership | **Closed by P4, in this plan** (re-ratification artifact §4.2/§5.2/§8 — P4 is not dissolved). The transfer this AC specifies cannot be applied *mechanically*, because at **five** sites the `KernelStack` is `Box::leak`ed at construction with `kernel_stack_allocation: None`: the three `create_main_thread*` constructors (`manager.rs:851`, `:925`, `:1010`) **and both x86_64 fork paths** — `complete_fork` (`:1979`, fn `:1920`) and `fork_process_with_context` (`:2323`, fn `:2148`, `None` at `:2339`). **"Fork already transfers correctly" is FALSE on x86_64**: the only transferring fork site is `manager.rs:1833` in `complete_fork_aarch64` (fn `:1779`, aarch64-only); the correct-transfer contrast sites are the CLONE_VM clones `syscall/clone.rs:250-252` and `arch_impl/aarch64/syscall_entry.rs:961`. So P4 answers an ownership question — the scheduler copy is the single owner and `Thread::clone` must stop being the reason a stack is leaked (`task/thread.rs:514`) — rather than sweeping a `take()`. The freed-row hazard is **structurally live and merely unreached**: `remove_process` drops the row and its `main_thread`, whose `KernelStack` `Drop` returns the slot, and nothing ever clears `main_thread`; unreached only because the leak means no row holds one, which is why the fix and this gate ship in the same PR. **#579** is the tracking issue and P4 closes it. **#546** is the *user*-stack sibling and does not substitute. | **4** | Ownership assertion after every creation path **and both fork paths** (exactly one owner, and it is the scheduler copy); 1000-iteration fork/clone/spawn exit stress with stack-pool accounting (allocated == freed, driven nonzero); an allocator assertion that never selects a live slot; and a census ratchet asserting zero `Box::leak(Box::new(kernel_stack))` occurrences in `process/manager.rs`. **Both profiles are run explicitly — three of the five sites are `#[cfg(target_arch = "x86_64")]` and two are `#[cfg(target_arch = "aarch64")]`, so neither profile alone sees the whole surface** (corrected in the P4 build pass; the earlier "two of the five sites" reading rested on a wrong `cfg` column, coordinator ruling R21). The census is taken across `kernel/src` as `(file, item, count)` triples, which is what makes "a sixth cannot appear" true kernel-wide; the live-slot assertion is a counted release-mode check on both arches, not a `debug_assert!` that every `--release` gate build compiles out (R25); and on x86 the stress also asserts frame steady-state, since a kernel stack costs 128 frames whose release `Drop` did not perform (R23) |
| **9** | No N-member FD/resource teardown loop in one PM-locked, IRQ-masked section | Each victim closes **only its own** descriptors, **one at a time**, and the owning descriptor never leaves the row: `begin_fd_close()` under PM takes one into the row's slot and returns a **non-owning `CloseTicket`** → drop PM → `endpoint_hangup(&ticket)` (the only step needing an endpoint lock; idempotent by CAS) → `finish_fd_close()` under PM drops the owning descriptor and clears the slot (explicit control flow: §2.5). **(v3 repair: the earlier `take_next_for_exit() -> (fd, FileDescriptor)` shape could not both retain custody and hand the value out.)** The existing allocating `take_fd_entries() -> Vec` (`process.rs:335`) is retired, not reused. Group work under PM is bitmap/flag stores only. No sweep loops over members' FD tables. | 7 | `FD_CLOSES_UNDER_PM == 0`; 256-FD × large-group test measures bounded PM hold; ratchet forbids close/reclaim calls inside any request/commit transaction body; **`fd_in_flight` holds at most one descriptor at any sampled instant and is `None` at quiesce for every row; a unit test proves `CloseTicket` exposes no close operation and that a replayed `endpoint_hangup` leaves endpoint refcounts unchanged (`FD_HANGUP_REPLAYED` nonzero, close count unchanged)** |
| **10** *(v2 — cond. 1, 3; v3 — closure A)* | No eager `cleanup_cow_frames` while the victim may run elsewhere; all kill paths grace-defer; **and no path can drop a receipt instead of retiring it** | Phase 2 routes SIGKILL through the single public `exit_process_and_retire` wrapper, whose aarch64 defer takes the page table into a grace-stamped receipt **inside a crate-private locked half and enqueues it after the PM guard drops** (§1.7, §2.1), so the CoW walk becomes a `None`-walk and no queue lock nests under PM. **v3: `RetirementReceipt` is crate-private with no public constructor, `exit_process_locked` has exactly one permitted caller, and the receipt's `Drop` re-enqueues rather than freeing — so a lost receipt cannot destruct a root even on an unreachable path. All nine adapted sites — the seven live `exit_process` callers plus `handle_thread_exit`'s enqueue and the new SIGKILL arm — are converted in P2 (v3 repair: seven callers, not nine; v3 tranche pass: the post-P2 exact sets are **8** `exit_process_and_retire` call sites, **1** `exit_process_locked` caller and **3** PM-free `enqueue_process_reclaim` sites — `handle_thread_exit` is an adapted site that does NOT call the wrapper, §1.7).** Phases 9–11 remove every direct `terminate` caller; resources stay in the row until the victim's own commit, and release requires grace + RootProof. `Process::terminate` is deleted with its last caller. | 2, 7, **9**, 11 | Ratchet allowlist of `\.terminate\(` shrinks phase-by-phase to **empty**, asserted as an exact set; **`RECLAIM_ENQUEUE_UNDER_PM == 0` from P2 onward**; peer-CPU SIGKILL stress asserts zero reclaim before the fence elapses and a complete RootProof; `TEARDOWN_MASKED_FRAMES_WALKED == 0` for kill paths. **v3: ratchet asserts one call site for `exit_process_locked` and zero public constructors for `RetirementReceipt`; `RECEIPT_DROPPED_UNRETIRED == 0`; a fault injected at each of the nine adapted sites still shows the pid's root entering the reclaim queue exactly once** |
| **11** *(v2 — cond. 1; v3 — closures D, F)* | Killed threads quiesced in the scheduler AND expedited with the existing `SGI_RESCHEDULE` | Phase 2: quarantine via the proven `terminate_process_threads` + **broadcast** `SGI_RESCHEDULE` to other online CPUs through the **teardown-only `send_exit_expedite_sgi` helper** (§2.7; no residency predicate to go stale, and no generic send site wired). **Phase 9 (was 10): the scheduler *requests* instead of remote-marking for every boundary-reachable victim, returns `ExitKickPlan`, and takes the counted legacy arm only for not-yet-migrated wait families. v3: P9 also ships the no-new-block admission interlock in all nine blocking primitives, which makes the reachability classification a one-way door. Phases 10a/b/c/d migrate the four families (10c is the `BlockedOnSignal` family v2 omitted); P10d empties the legacy allowlist and deletes the remote-marking body — that is where AC-11 is fully discharged.** The SGI handler keeps doing only `need_resched`; the victim can never be re-dispatched to EL0 once the generation is observed. **No `ExitPending`.** | 2, **9, 10a/b/c/d** | Victim spinning at EL0 pinned to a peer: **per-pid paired `EXIT_SGI_SENT{pid}` and an observation event for the test's own victim, with the send→observe interval measured strictly shorter than the victim's tick period — the observation half is `EXIT_KICK_OBSERVED{pid}` (the specified P2-era kick-bucket proxy, §2.7) from P2, and `EXIT_REQUEST_OBSERVED{pid}` from P8, when the boundary hook exists and the bucket table is deleted** (a bare "SGI count > 0" gate is explicitly rejected — the generic send sites are not wired and the counter is declared zero until P2); per-wait-family blocked-victim kill tests; **`EXIT_LEGACY_REMOTE_MARK{family}` reported per family and asserted to reach 0 with the allowlist empty at P10d**; **`EXIT_BLOCK_REFUSED{family}` nonzero in P9's own test and re-asserted nonzero in each P10x — the admission interlock is permanent and is never asserted at zero (v3 repair); what falls to 0 per family is `EXIT_LEGACY_REMOTE_MARK{family}`, and what rises from 0 is `EXIT_WAIT_CANCELLED{family}`**; unmigrated families explicitly reported, never silently advertised as killable |
| **12** *(v2 — cond. 2, 5; v3 — closure B)* | Exactly-once SIGCHLD/wake/report with **first-recorded** status, idempotent under repeat passes | **Four-state obligations (`Absent/Pending/Claimed{claimer}/Completed`, §1.6), all transitions under the PM lock, which is the sole serializer — this restores what design C's single-worker serialization discharged after the worker was dropped.** Created at the **first** request; a repeat request returns the stored batch/status and creates no second obligation and no second status. **v3 makes the exactly-once claim real rather than asserted: class-A obligations (`Sigchld`, `ParentWake`, `Reparent`, the take-half of `Resources`) commit their effect in the SAME PM acquisition as `→ Completed`, so `Claimed` is never observable and T4 is unreachable for them; the two class-B obligations carry markers written by the effect itself: `Fds` by single-slot row-resident custody (*v3 repair* — the row's slot is the **sole owner** and the unlocked step receives a non-owning `CloseTicket`, so a double close is unrepresentable), and `Report` by started/finished bits plus the btrt token, with T4's destination chosen by the marker and the marker outranking the ledger. The scheduler kick is declared an idempotent repeatable side effect, not an obligation.** **Row lifetime is extended by the tombstone gate (P6a), and removal is an explicit two-event join over `reaped`/`retired` — whichever writer observes the other flag already set removes the row in that same acquisition, both orders specified and gated.** **The legacy `DeliverResult::Terminated` notification action is deleted in P11, so no second notifier exists.** Still not suppression: the obligation is shared, so a later pass redeems it rather than skipping it. | 2 (seed), **6a (retention + join), 6b (ledger)**, 11 (legacy notifier deleted) | Matrix — exit→fault, SIGKILL→fault, fault→SIGKILL, repeat request/wait: exactly one SIGCHLD, one parent wake, one `btrt` report, and `waitpid` returns the **first** status; equalities `SIGCHLD_FIRST_SET == PARENT_WAKE_COMPLETED == BTRT_EXIT_REPORTED == parented_first_commits` at the nonzero value the 64-child workload produces; **`LEDGER_CLAIM_MISMATCH == 0`, `LEDGER_CLAIM_ORPHANED == 0` on a healthy boot, `LEDGER_EFFECT_AMBIGUOUS == 0` on a healthy boot, `TOMBSTONE_RESIDENT == 0` at quiesce**; **`TOMBSTONE_JOIN{reap_second}` and `TOMBSTONE_JOIN{retire_second}` both nonzero in one run**; forced-orphan injection at each of the three marker states (`finished`, `not-started`, `ambiguous`) proves T4 takes the ruled destination — the notification is delivered exactly once, never twice |
| **13** *(v2 — cond. 3)* | New reclaim/drain respects lock ordering and is bounded on idle paths without throttling fork's drain | **No cap is added, no drain is moved, no drain is shared.** Fork's pre-allocation drain stays full/unbounded. The only addition to a return tail is an acquire-load + branch, placed **after** the `PREEMPT_ACTIVE`/nested-return gate. Every scheduler critical section is entered with **no PM guard live**, structurally: request/commit APIs take `pid`/`tid`, never `&mut Process`. **v2 adds two absolute rules: (i) P1's proof reads never hold `PENDING_PROCESS_RECLAIMS` while acquiring SCHEDULER or PM — the drain detaches a candidate under the queue lock, drops it, proves, then frees or re-inserts (§4.3); (ii) every retirement receipt is enqueued only after the PM guard drops, including in P2's interim shape.** **v3 adds the progress rule the refusal path was missing (§1.8): a bounded pass (`last_pass` stamping — no candidate selected twice in one pass) plus bounded retry (`K = 3` liveness-blocker refusals park the entry on a side list under a fence built from a **`RetirementSnapshot` taken at park time** — never the receipt's already-elapsed retirement fence and never the cycle's earlier snapshot — with a three-armed unpark: all-CPU scheduling-epoch advance, a `ROW_REMOVAL_EPOCH` bump, or an age backstop of **64 scheduling epochs summed over the captured online mask** (no wall clock); v3 repair + v3 tranche pass, §1.8). This is an exclusion rule, NOT a drain cap: fork's pre-allocation drain keeps full semantics and no cap parameter exists.** No `FRAME_METADATA` and no `log::*` is added under any mask. Full analysis: §4. | all | Lock-depth/owner counters (with `try_manager()` instrumented — the r20 blind spot is not repeated); `RECLAIM_CONTEXT_VIOLATIONS == 0`; **`RECLAIM_ENQUEUE_UNDER_PM == 0` and `PROOF_UNDER_QUEUE_LOCK == 0`**; **`RECLAIM_PASS_SKIPPED`/`RECLAIM_PARKED` driven nonzero by forced refusals at each blocker class; `RECLAIM_UNPARKED{epoch}`, `{row}` and `{age}` each driven nonzero by their own injection (the `{row}` arm proved with every other CPU held off the scheduler, which an epoch-only rule cannot pass); `RECLAIM_PARK_IMMEDIATE_UNPARK == 0` (the fresh-fence proof); `RECLAIM_PARK_RESIDENT == 0` at quiesce having been observably nonzero mid-run; and a bounded-iteration assertion that one drain pass performs at most *queue length* selections**; ratchet rejects scheduler-under-PM, queue-lock-then-PM/SCHEDULER, and any reclaim/close/walk in idle or exception tails; fork-pressure test proves a *full* eligible drain, not a capped one. **Declared partial:** this design does not fix #448's or #492's pre-existing boundedness; it must not make them worse, and §5 states the argument |

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

1. **P1's proof under the queue lock.** `reclaim_deferred_process_resources` (`process_task.rs:375-391`)
   evaluates its readiness predicate while holding `PENDING_PROCESS_RECLAIMS`. Today both terms are
   lock-free atomic reads, so `main` is legal; P1's `RootProof` adds scheduler-cached-root and
   live-row blockers, which would take SCHEDULER and PM **under the queue lock**. P1 restructures the
   drain instead (§4.3, "P1 retire cycle"): the under-queue-lock predicate stays **epoch + shadow
   only** (lock-free), the candidate is detached, the queue lock is dropped, and only then is the full
   proof run.
2. **P2's enqueue under PM.** The locked half returns a `RetirementReceipt` and the **wrapper**
   enqueues it after the PM guard drops; the `handle_thread_exit` enqueue is moved into phase 2 of
   that function, where PM is already released. Both conversions are in P2's PR.

**v3 (closure A): the rule is enforced by custody, not by discipline.** "The caller enqueues after
the guard drops" is only a rule if callers exist that could do otherwise. §1.7 removes them: there is
one public wrapper, the receipt type is crate-private with no public constructor, and the receipt's
`Drop` re-enqueues rather than freeing. The two places a receipt can exist — inside the wrapper
between guard-drop and enqueue, and P1's local detach slot — are both provably PM-free, so the `Drop`
path can never nest the queue lock under PM either.

**v3 (closure C): the no-overlap rule must also not cost progress.** §1.8's pass cursor and parked
side list exist because detach-prove-reinsert is only safe if it terminates. The parked list is a
second leaf structure taken **only** under the same discipline as the live queue (never with PM or
SCHEDULER live), and the unpark check reads a `RetirementSnapshot`, which takes no lock at all.

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
| S3 FD close *(v3 repair)* | PM only for `begin_fd_close`; **endpoint lock with PM dropped**; PM only for `finish_fd_close` | strict edge: PM released before any endpoint lock, and re-acquired only after it is released | none | none | one descriptor per take/close pair; the owning descriptor never leaves the row |
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
| **S4 tombstone removal — two-event join** *(v2 cond. 2; v3 closure B)* | **PM only**, in the SAME acquisition that writes `reaped` or `retired` and reads the other | entered with no queue lock and no scheduler lock live; the ledger term is read in that same acquisition | frees the row only | none | one row per pass; both join orders exercised |
| **Class-A obligation commit (T2·3)** *(v3 — closure B)* | **PM only** | the effect and `→ Completed` are one acquisition; takes no second lock | none | none | O(1); `Reparent` is one fixed batch per acquisition with a cursor |
| **Class-B effect marker writes** *(v3 — closure B)* | **no lock** (lock-free atomics in the row) | `started`/`finished` are plain atomics written outside PM; `fd_in_flight` is written under PM by `begin_fd_close` and both **cleared and destroyed** under PM by `finish_fd_close` in one acquisition — the unlocked step holds only a non-owning `CloseTicket` (v3 repair) | none | none | O(1) per descriptor / per report |
| **`btrt::claim_exit_slot`** *(v3 — closure B)* | **PM only** (inside T2) | pure `compare_exchange` on the btrt registry; **no SERIAL** — the SERIAL-reaching `record_exit` half runs with no lock held | none | none | O(registry) scalar scan |
| **P1 park / unpark** *(v3 — closure C; repaired)* | **parked-list lock only** | leaf; never entered with PM, SCHEDULER or the live queue lock held. All three unpark arms are lock-free reads: a `RetirementSnapshot` (epoch), a relaxed load of the global `ROW_REMOVAL_EPOCH` (row), and a second read of the same epoch words the park captured (age — a scheduling-epoch count, no clock). The `ROW_REMOVAL_EPOCH` **bump** is one relaxed increment inside the PM acquisition that already performs `remove_row`, taking no additional lock and touching no list | none | none | O(parked length) scalar scan |
| **Teardown expedite SGI** *(v3 — closure F)* | **no lock** | `send_exit_expedite_sgi` is called after every guard drops, exactly like the v2 broadcast it replaces | none | none | ≤ MAX_CPUS MMIO writes + one trace event each |
| **No-new-block admission check** *(v3 — closure D)* | **whatever the primitive already holds** | one acquire-load of the caller's own per-thread request word, evaluated **before** the primitive takes any wait-queue state; takes no new lock and cannot deadlock because it reads a per-thread atomic | none | none | O(1) |
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
`PREEMPT_ACTIVE` gate, and no drain is added to any tail. *(v3)* **A receipt reaching a destructor** —
`RetirementReceipt`'s `Drop` re-enqueues instead of freeing, and both of the two contexts a receipt can
exist in are PM-free (§1.7). *(v3, repaired)* **A drain that cannot make progress** — the pass cursor bounds one
pass; the park rule bounds retries under a *freshly captured* fence, so a park is never instantly
undone; and the unpark disjunction covers the PM-side event that clears a live-row blocker with no
scheduling anywhere, with an age backstop behind both arms. Detach-prove-reinsert therefore
terminates and a parked entry cannot be stranded (§1.8). *(v3)* **A victim
entering an unmigrated wait after being classified reachable** — every blocking primitive refuses to
block a latched thread (§1.5), so the classification is a one-way door.

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
   (Residual R-6); the phase gates include an explicit retention measurement. *(v3, closure C: the
   pass cursor and parked side list added in §1.8 are an **exclusion** rule, not a cap — they stop the
   drain re-examining an entry it already examined this pass and defer entries whose blocker provably
   cannot have changed. No cap parameter is introduced, no drain is moved, and fork's pre-allocation
   drain keeps full semantics. The three reverted bounded-drain attempts are not reopened.)*
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
in P10a/b/c/d, and that a family with a genuinely stuck wait still blocks *prompt* victim-owned death.
*(v3, closure D: the inventory is now closed at four families — futex; `WaitQueueHead` + stdin/TTY;
`BlockedOnSignal` (`pause`/`sigsuspend`); child-wait + timer/nanosleep + completion/I-O — and the
no-new-block interlock means an unmigrated family can only be **entered before** the request is
latched, never after.)*
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

**R-16. The legacy remote-mark arm is live from P9 until P10d.** *(condition 1; renumbered in v3 by
closure D's added family.)* Between the
request/wake cutover and the last wait-family migration, two teardown shapes coexist for SIGKILL:
victim-owned for boundary-reachable victims, and the merged P2 remote-mark for the rest. This is a
deliberate, counted, ratcheted interim — both arms are exercised in every PR that ships them, the
false set is an explicit allowlist that only shrinks, and P10d deletes the arm — but during that
window two paths must both be correct, and a review of any P10x PR must check both. The alternative
(migrate every family in one PR) is the 15-phase big bang Judge 1 found fatal. *(v3: the window is one
subphase longer than v2 stated, because the `BlockedOnSignal` family was missing from the inventory.
That is a real cost of the correction and it is recorded rather than absorbed.)*

**R-17. Orphaned-claim recovery depends on the same liveness machinery it protects.** *(new,
condition 2.)* T4 (`Claimed{dead} → Pending`) uses P1's proof (`scheduler thread absent or
Terminated` + claim-time fence elapsed) to decide that a claimer is dead. If that proof is wrong in
the *unsafe* direction — declaring a live claimer dead — two paths could redeem one obligation. The
mitigation is that T4 is gated on the *same* two-epoch fence that gates physical free, so a claimer
that could still run cannot be declared dead without also making a use-after-free reachable, which the
existing gates already test for. It is a shared dependency, not an independent proof, and is recorded
as such. *(v3, closure B: the blast radius of a wrong T4 is now much smaller — T4 is unreachable for
the four class-A obligations, `Fds` cannot double-close because custody is single-slot, and only
`Report` has a ruling that could in principle drop a report. So a T4 misfire degrades to "one btrt
report possibly missing, and the AC-12 equality fails loudly", not "an obligation performed twice".)*

**R-18. Init's group protection is belt-and-braces, and the belt is an ABI restriction.** *(new, v3,
closure E.)* End 1 (clone admission refusing to publish into the designated init's thread group,
`EINVAL`) is a **deliberate and documented ABI restriction**: Breenix's designated init cannot have
`CLONE_VM` siblings. Production init now exercises that restriction twice per aarch64 boot — once
early and once at quiesce — and each probe invocation issues both a `CLONE_VM` clone and a
`CLONE_VM | CLONE_FILES` clone. The permanent shipping cost is therefore **four deliberately refused
clone syscalls per aarch64 boot**. These calls are target-gated for aarch64 but deliberately are not
feature-gated: phasing rule 2 requires the production path, not a test-only build, to be the live
caller of the refusal. A later phase may retire the probes only by providing replacement
production-path exercise and equivalent whole-boot evidence for the admission rule, or by replacing
the restriction with a separately reviewed multi-threaded-init design and an equivalent proof that
the init-group kill bypass remains closed. A multi-threaded init would need that design pass, so if
init ever legitimately needs siblings, end 1 is what must be revisited. End 2 (the group-membership
drop in S1's seal) is what keeps the system safe in the meantime, and it is the guard that must never
be removed as "redundant". Both ends are separately tested so neither can silently rot into the
other's shadow.

**R-19. `Report` has one irreducible ambiguity window, and we rule against re-running.** *(⚠ Tracked
as **DEBT-1**, owner **P6b** — this residual is the reason `Report` is at-most-once rather than
exactly-once, and P6b's tranche cannot ratify until the window is either closed or explicitly
accepted by the operator as the round's single at-most-once obligation. It is listed here as an
accepted risk **only** in the sense that v3 named it; the register is where it is owed.)* *(new, v3,
closure B.)* If a claimer dies *inside* `btrt::record_exit` — between the `started` CAS and the
`finished` store — T4 cannot tell whether the recording landed. The ruling is **treat it as done**
(`→ Completed`, bump `LEDGER_EFFECT_AMBIGUOUS{report}`), because a duplicate corrupts the test ledger
and can double-`finalize()` the boot, whereas a missing report is caught by the AC-12 equality. The
window requires a kernel control path to fault inside a short lock-free atomic sequence, which is
itself a run-failing bug; the counter is asserted 0 on every healthy boot and driven to a known value
only by deliberate injection. This is the one place in the design where exactly-once degrades to
at-most-once, it is named, and it is the *only* one.

**R-20. Parked receipts are a bounded, visible stall.** *(new, v3, closure C; scope corrected by the
v3 repair.)* An entry whose liveness blocker never clears sits on the parked side list and is retried
on any of the three unpark arms (all-CPU scheduling-epoch advance, a `ROW_REMOVAL_EPOCH` bump, or the
age backstop of 64 scheduling epochs summed over the captured online mask). Because the age arm
cannot be starved while any captured CPU still schedules — and the 1 kHz tick guarantees one does — a parked entry is **always** eventually
re-proved, so the residual is narrower than v3 first stated: what remains is not "an entry that is
never revisited" but "an entry whose blocker is genuinely permanent (a leaked scheduler-cached root,
say), which is re-proved forever and never retired" — a leak, not a use-after-free, and one that
consumes at most one proof per drain. `RECLAIM_PARK_RESIDENT` is a gauge with a reader asserted to
return to 0 at quiesce in every boot gate, so the leak is loud. The alternative — retrying the same
entry inside a single pass — is the livelock this closure removes.

**R-21. The P2-era expedite observation is a proxy with a demolition date.** *(new, v3 tranche
pass; closure F.)* Between P2 and P8, the observation half of AC-11's send→observe pairing is the
`EXIT_KICK` bucket table (§2.7), not `EXIT_REQUEST_OBSERVED{pid}` — which cannot exist before the
return-boundary hook does. Three limits are accepted and every one of them is counted rather than
argued away *(v3.1: limit (ii) was best-effort in v3 and is now exhaustive; limit (iii) is new, and is
the price of the reservation the v3.1 protocol needs)*.
**(i) It observes a weaker event.** "The peer's scheduler declined to dispatch this victim" is not
"the victim observed a latched exit request"; the latter is not expressible before P8, and P2's gate
is worded to the former rather than overclaiming. **(ii) 64 buckets alias.** Two victims congruent
mod 64 in flight at once cost the earlier one its observation, counted `EXIT_KICK_BUCKET_COLLISION`
and impossible in P2's single-victim workload. *(v3.1: the count is now exhaustive rather than
best-effort — a colliding publisher either loses the reservation CAS or wins it and displaces an
unobserved record, and each arm counts from a position of exclusive knowledge, so no colliding
publication is silent. Sequential reuse of a bucket is **not** a limit: it works, and P2 gates it.)*
**(iii) A stranded reservation.** *(new, v3.1.)* A publisher that faults between reserving a slot and
committing it leaves that bucket `LOCK`-set for the rest of the boot; every later publication into it
counts a collision, so P2's `EXIT_KICK_BUCKET_COLLISION == 0` gate **fails** rather than passing on
degraded evidence. The window is two relaxed stores with no call, no loop and no allocation, on a path
whose only fault is already fatal. The residual is bounded by construction: the table and
both counters are **deleted in P8**, in the same PR that introduces the hook, so the round never
carries two observation mechanisms past the phase that unifies them. A P8 that keeps the bucket table
alive is the failure mode this residual exists to make visible.

**R-22. P3's admission-to-publication guard-custody invariant has one terminal refusal carve-out.**
P3's structural rule ordinarily permits no `manager_guard` drop between clone admission and child
publication. P5b narrowed it at exactly one place: the init-group refusal arm must drop the guard
before returning `EINVAL`, because that terminal path publishes no child and must release PM custody
before it exits. `tests/context_restore_structure.rs` keeps the exception honest by requiring exactly
one `drop(manager_guard)`, exactly one `return`, and the `EINVAL` result inside that arm; its mutation
control replaces the `return` with a non-terminal binding and must make the suite red. The invariant's
purpose therefore survives — the refusal publishes nothing — but the exception is a recorded
residual in a custody-adjacent ratchet, **not precedent**: no later phase may cite it to drop the guard
on another admission-to-publication path.

---

## 7. Open questions — operator decisions only

**OQ-1. Init death policy — DECIDED (coordinator-adopted at v2; recorded here as a coordinator
decision, NOT as ratification condition 7 — see closure G).** **Linux-faithful
protected init:** a user-originated signal to the designated init whose effective disposition is the
default fatal action and for which init has no handler is **silently dropped and the send returns
success (0)** — not `EPERM`. Handled signals are delivered normally. Only init's own `exit`/`exit_group`
or an unhandleable synchronous fatal fault (or a nonviability invariant) is kernel-fatal. v1's `EPERM`
recommendation is withdrawn: the ratification is right that `EPERM` is a deliberate ABI divergence and
was mislabelled as fidelity, and rather than document a divergence we take the faithful behaviour.
**OQ-1b (still open):** accept today's non-stop-the-world panic (recommended; file `smp_send_stop`
separately), or add an SMP stop broadcast in this round?

**OQ-2. PID-1 reservation.** Reserve PID 1 for the explicit init constructor and start ordinary/test
allocation at 2 (all **eight** `next_pid.fetch_add` sites — §0.3.1)? Recommended **yes** — it is what converts AC-5
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
parked and #471 with exec detach (P3) but no seal. *(v3 note: closure E's **end 1** — the clone
admission refusal — ships in **P5** and therefore survives an OQ-5 refusal. End 2 rides with P12 and
would be parked; that is acceptable precisely because end 1 makes the state end 2 guards against
unconstructible, which is the whole reason the closure is at both ends.)* v1's claim that "#464 and #471 ship complete"
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

**OQ-8. Phase count — CORRECTED AGAIN (condition 6; updated by v3 closure D and by the 2026-08-16
documentation-repair pass).** v1 claimed "13 phases / 13 PRs", which the first ratification correctly
called a contradiction with the split phases; v2 said 16 PRs; v3 said 17. The honest figure today is
**13 numbered phases / 18 PRs** (one of them, P5b, held): P6 splits into P6a + P6b (condition 2), P10
into P10a + P10b + P10c + P10d (four wait families, the last of which also deletes the legacy arm),
and P5 into P5a + P5b. **Every other phase is exactly one PR** — including P3, P4, P5a and P9, whose
split seams are **named in their PLAN sections but not fired**: the ratified tranche-2 shape
(`P3-RERATIFICATION-2026-08-15.md` §5.2 `T2-b`/`T2-c`/`T2-d`, §8's four-PR tranche) gives each a
single hand-written revert story, and PLAN rule 5 is applied at implementation time rather than
pre-emptively on paper — a phase whose implementation genuinely yields two independent revert stories
splits then and amends PLAN §0's ledger in the same pass. The sum, written out: ten unlettered phases
(P0, P1, P2, P3, P4, P7, P8, P9, P11, P12) × 1 + P5a/P5b (2) + P6a/P6b (2) + P10a–d (4) = 10 + 2 + 2 +
4 = **18**. **There is no size rule of any kind.** The ~230-line / 5-file
ceiling this OQ used to state was abolished by the operator on 2026-08-11 (*no line or file ceilings
on fixes, ever*) and is **deleted, not softened**; the only thing that splits a PR is PLAN rule 5 —
one revert story per PR. The full PR ledger is enumerated in PLAN §0. That is a lot of review cycles,
and it is the deliberate price of Judge 1's fatal finding against a 15-phase big bang. **Open decision:
accept the count, or batch adjacent phases and accept a larger review surface per PR?** *(The
coordinator's recommendation is unchanged: accept the count. The one batching candidate a reviewer
might reasonably propose is P10c into P10b, since `tty/driver.rs:602-603` shows the TTY unblock path
already spans `Blocked` and `BlockedOnSignal` — but the two families are separately revertable
migrations, each with its own allowlist entry to remove, so batching them would put two revert stories
in one merge commit. That, and not any measure of size, is why they stay apart.)*

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
| **A lint standing in for a safety mechanism** *(new, v3 — closure A)* | **No** | v2 relied on `#[must_use]` to stop a returned receipt being dropped, which `let _ =` defeats and which seven unadapted call sites would have met head-on. Custody replaces advice: crate-private type, no public constructor, one permitted caller of the locked half, a `Drop` that re-enqueues instead of freeing, and all nine adapted sites moved in the introducing PR |
| **A recovery rule that cannot observe the thing it recovers** *(new, v3 — closure B)* | **No** | v2's T4 reopened an orphaned claim without knowing whether the effect had fired. v3 removes the question for four obligations (effect and completion in one PM acquisition), removes it by ownership for `Fds`, and answers it with an effect-written marker for `Report` — with the winning side stated in a table rather than left to the implementer |
| **A refusal path with no progress argument** *(new, v3 — closure C)* | **No** | v2's "re-insert and rotate" had no rotation: the live drain re-scans from index 0. v3 stamps a pass id (no candidate selected twice per pass) and parks entries after `K` liveness refusals until a scheduling-epoch advance. Stated explicitly as an exclusion rule, not a cap, so the three reverted bounded-drain attempts are not reopened |
| **A classification treated as stable without an interlock** *(new, v3 — closure D)* | **No** | "Boundary-reachable" is now a one-way door: every blocking primitive refuses to block a thread whose exit request is latched, so a victim classified reachable cannot later become unreachable. The wait inventory is closed at four families with `BlockedOnSignal` added, and the missing graph edges are present |
| **A protection checked at one end of a two-end mechanism** *(new, v3 — closure E)* | **No** | Group-scoped kill plus row-scoped protection left a sibling bypass. v3 refuses the sibling at clone admission **and** tests designated-init membership of the whole target group at the seal, with each end separately tested so neither rots in the other's shadow |
| **Evidence attached to a counter that unrelated work increments** *(new, v3 — closure F)* | **No** | `EXIT_SGI_SENT` moves off the two generic scheduler send helpers onto a teardown-only helper that does not exist until P2 (declared zero until then), and the gate is per-victim-PID paired with the observation event plus a measured send→observe interval — the defer/reclaim pairing pattern, applied to expedite |
