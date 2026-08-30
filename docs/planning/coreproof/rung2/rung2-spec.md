# Core-proof RUNG 2 — Component C (per-CPU identity + stack custody)

**Read at** `main` @ `1987d45ba7e0856f07be0b5df65bfa16ab8b9a70`. Re-diffed against
`main` @ `579a10ccd73156d378a6f241d4b867222018749f` (the head at spec-write time,
a merge of PR #709, unrelated to `kernel/src/proof/`) — every file this spec
anchors to is byte-identical between the two SHAs, confirmed with
`git diff --stat` over the full anchor list. This checkout is read-only for this
slot; another workflow is landing on it concurrently, so re-verify line numbers
before implementation if `main` has moved further.

Sources: `CORE-PROOF-DESIGN.md` §5 row C, §6 rung 2, §7 (esp. M2); the rung-1
pilot's `brief-framework.md`, `fix3-notes.md`, `passbar.md`, `review.md`; and
`kernel/src/proof/*.rs` plus every production file the pilot's Component-A
driver already reads, all re-verified against the tree above rather than taken
on the design doc's word.

---

## 0. What rung 1 already proved about Component C, stated up front

This is not a green-field component. Three facts, load-bearing for everything
below:

1. **The contract is already instrumented in production, not in the harness.**
   Every postcondition this rung needs is a read of a counter or a pure
   function that shipped before `coreproof` existed. Design §5 row C's
   "Missing" column says "nothing," and that is not aspirational — verified
   line-by-line in §1 below.
2. **The M2 mutation (`coreproof_mut_cpu_identity`, #645) already exists,
   already compiles, and has already been hunted once — and missed.**
   `kernel/src/proof/mutations.rs:102-108` registers it; the real
   re-introduction sits at `kernel/src/arch_impl/aarch64/context_switch.rs:6500-6535`.
   Round 3 of the pilot (`fix3-notes.md:109-133`) ran it under Component A's
   driver with a targeted timer squeeze and got **0/3 boots**, cov=50,910–54,149
   samples of the mutation site per boot — a real, non-vacuous miss, not an
   unexercised one. `passbar.md:31,318-320` confirms this is the reason the
   pilot's own pass bar reads `barMet = FALSE` on requirement (1) alone.
3. **The pilot's own diagnosis of the miss is precise and is this rung's
   starting point, not something to re-derive:** "the detector is right here
   in this file... the gap is production of the state, not detection of it...
   The open lever is a migration-forcing arm, not another predicate"
   (`context_switch.rs:6506-6510`, `mutations.rs:46-56`). Component A's driver
   was never built to produce that state — it drives a manufactured,
   single-lock-acquisition probe of `block_current`/`unblock`
   (`driver_a.rs:1-10`) and only *reads* `CPU_IDENTITY_SPLIT_EVENTS` /
   `PERCPU_STACK_ALIEN_REFUSALS` as a side channel
   (`driver_a.rs:119-124,286-296`). Rung 2's job is to build the driver whose
   *job* is producing that state.

---

## 1. Component C contract as testable postconditions

Non-negotiable (c), design §5 header: "a postcondition is a read of an
existing counter or marker." Every postcondition below cites the marker it
reads and nothing else; no new bookkeeping is proposed.

### Postcondition 1 — "identity read at dispatch equals the CPU executing"

**Marker:** `CPU_IDENTITY_SPLIT_EVENTS` (`kernel/src/arch_impl/aarch64/percpu.rs:185`,
`AtomicU64`, never reset) and its sole emitter `record_cpu_identity_split`
(`percpu.rs:198-217`). Verified live: `CpuId::current_checked(carried: usize)`
(`percpu.rs:255-263`) re-reads the hardware `TPIDR_EL1`-backed identity and
compares it against a value the caller carried in from an earlier point; a
disagreement increments the counter and writes
`[CPU_IDENTITY_SPLIT:carried=…:fresh=…:site=…:…]` with the *caller's* source
location (`#[track_caller]` at `percpu.rs:256`). `cpu_identity_split_events()`
(`percpu.rs:194-196`) is the read accessor.

**Postcondition, precisely:** for every one of the six `current_checked` call
sites in `context_switch.rs` the design's verification ledger names (design
doc line 291's "existing oracles read" column), the carried index and the
fresh hardware read agree — i.e., `CPU_IDENTITY_SPLIT_EVENTS` does not advance
during the measured window. The type itself, `CpuId` (`percpu.rs:233-270`),
is deliberately unconstructible from a bare `usize` — "there is no constructor
from an index, only from a hardware read" (`percpu.rs:226-229`) — so this
postcondition is partly enforced at compile time and partly at the one runtime
seam that still accepts a carried value.

**Corroborating signal, not a redefinition:** `RET_STAGE_REFUSALS`
(`context_switch.rs:2897`) and its emitter `record_ret_stage_refusal`
(`context_switch.rs:2944-2958`, writing `[RET_STAGE_REFUSED:reason=…]`).
`stage_ret_dispatch_context` (`context_switch.rs:2908-2942`) copies a
`CpuContext` into a per-CPU staging slot indexed by `cpu_id` and refuses with
reason `"identity"` when `context.identity_is_intact()`
(`task/thread.rs:263-264`, checked against `CPU_CONTEXT_MAGIC`) is false, or
`"copy"` when the staged `x30` disagrees with the admitted resume PC. This is
the TOCTOU closure PR #645 landed *alongside* the identity-capture fix
(`mutations.rs:105` — "fixed_by: PR #645" for M2), guarding the same class of
stale-identity-reaching-a-per-CPU-slot failure one step later in the same
call chain. The harness reads it as a secondary signal on the same delta
pattern as postcondition 1, never as its own predicate — a `RET_STAGE_REFUSED`
during the window is reported alongside a `CPU_IDENTITY_SPLIT` finding, not in
place of one.

### Postcondition 2 — "a CPU pivots only onto an address its own slot admits"

**Markers:** `PERCPU_STACK_ALIEN_REFUSALS` (`percpu.rs:37`) and its sole
emitter `record_percpu_stack_alien` (`percpu.rs:281-309`, writing
`[PERCPU_STACK_ALIEN:cpu=…:owner=…:sp=…:tid=…:site=…:…]`, where `owner` is
either the decoded owning CPU or the literal string `"unpublished"` —
`percpu.rs:294-298`). Four call sites all funnel through this one emitter and
the one predicate `percpu_stack_top_owned_by` (documented at `percpu.rs:55-58`
as "the one custody predicate this arch shares with the producer side"):
`percpu_stack_install_permitted` (`percpu.rs:68-75`, the setter guard),
`percpu_stack_top_for` (`percpu.rs:100-107`, the idle-pivot producer),
`percpu_pivot_top_for` (`percpu.rs:135-150`, the scheduler-stack pivot —
closes the exact CPU-3-on-CPU-0's-scheduler-stack gap the doc comment names),
and `percpu_stack_resume_permitted` (`percpu.rs:162-173`, the dispatch-resume
check).

**Postcondition, precisely:** across every one of the four sites,
`PERCPU_STACK_ALIEN_REFUSALS` does not advance during the measured window —
i.e., no pivot, install, producer choice or resume ever named a slot outside
the calling CPU's own. All four production call sites are verified to live
inside `context_switch.rs` (`percpu_pivot_top_for` at `context_switch.rs:2227`,
`percpu_stack_top_for` at `context_switch.rs:3315`,
`percpu_stack_resume_permitted` at `context_switch.rs:3573` and `:4759`) — the
file this harness may never seam (§4). The postcondition is therefore read,
never produced, from outside that file.

The `:4759` site is the work-stealing arm: its own comment
(`context_switch.rs:4740-4746`) names the round-3 field specimen directly —
"a kernel thread carrying an EL1 frame SP from CPU 3's slot being restored on
CPU 1... because nothing on the dispatch path adjudicated `context.sp` and the
ready queues are work-stealing" — and its refusal path
(`context_switch.rs:4757-4779`) requeues the thread onto the *owning* CPU
rather than looping locally. This is production evidence that cross-CPU
requeue is a live, ordinary code path, not a hypothetical one — which is what
makes Adversarial mode's `Steal` antagonist op (§2) a faithful stimulus and
not an invented one.

### Postcondition 3 — "the ownership record decodes as claim or unpublished, never arbitrary"

**Marker:** `decode_owner_record` (`kernel/src/arch_impl/aarch64/percpu_custody.rs:97-104`),
a pure function: `word0 ^ magic == word1` and the result names a CPU
`< max_cpus`, or the record reads as unpublished (`None`). Its companions
`slot_of` (`percpu_custody.rs:83-88`, exclusive-upper-bound slot attribution)
and `slot_admits` (`percpu_custody.rs:126-135`, the one function every
custody site above reduces to) are the arithmetic this postcondition is made
of.

**Postcondition, precisely:** for the fixed layout constants
(`BOUNDARY_CANARY_OFFSET`, `OWNER_RECORD_OFFSET`, `OWNER_RECORD_BYTES`,
`OVERRUN_SENTINEL_OFFSET` — `percpu_custody.rs:43-57`, each ordering pinned by
a `const _: () = assert!(...)` at build time, `percpu_custody.rs:64-66`),
`decode_owner_record` returns `Some(owner)` only when both words agree and
`owner` is in range, and `None` for every other bit pattern — half-written,
stack-data-overwritten, or pre-publication zero. This is exhaustively provable
by construction over the small input domain and needs no live boot at all.

**Where this is already proven, completely, at tier 0:** `percpu_custody.rs`'s
own header states the host test `tests/percpu_stack_custody.rs` "includes this
file directly and executes them, so deleting or inverting the custody rule
reddens a host test rather than only a source-shape ratchet"
(`percpu_custody.rs:15-18`). This is `ns`-cost, exact, and — per design §2's
own tier table — already the harness's Tier 0 for this component. **Rung 2
adds nothing here.** It is scoped as already-complete inherited work, not
re-verified in this rung's own driver.

### Postcondition 2, setter-side, already covered by a real (non-live-race) mechanism

`kernel/src/task/percpu_stack_oracle.rs` — feature `percpu_stack_custody_oracle`
(independent of `coreproof`), four legs, all reported once per boot as
`[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg={A,B,C,D}:...:{PASS,FAIL}]`:

* **Leg A** (`percpu_stack_oracle.rs:644-706`) — plants a recognisable image
  in an *offline* CPU's exception-stack slot, installs that address on a
  *different* CPU through the ordinary public setters, and requires the
  install be refused and the image undisturbed. This is postcondition 2's
  negative case, produced deterministically (no race needed — the target CPU
  is offline, so nothing else touches the slot) rather than probabilistically.
* **Leg B** (`:713-736`) — the control arm: a CPU's own top and an ordinary
  heap-backed thread stack must both still be accepted, shipped in the same
  build so leg A's pass cannot be a blanket refusal.
* **Leg C** (`:743-761`) — passive whole-boot census of slot occupancy.
* **Leg D** (`:768-779`) — tid-0 (`swapper/0`) is a live thread id and does
  not resolve through the scheduler's idle lookup as an ordinary thread.

Already wired into `docker/qemu/run-aarch64-percpu-stack-custody-gate.sh` and
into `docker/qemu/run-aarch64-service-sequence-gate.sh` (both `grep -l` hits,
confirmed against the tree). **Rung 2 reads these markers as further
"existing oracle" evidence per non-negotiable (c) and does not modify, extend,
or re-run this gate.** It is a genuinely separate, already-complete mechanism
that predates `coreproof`, proves the setter side of postcondition 2 without
needing a live race, and is out of scope by design, not by oversight.

### What this leaves as rung 2's actual, non-inherited work

Given the above, only **postcondition 1** (and its postcondition-2 corollary
at the work-stealing resume site) is *not* already proven by an existing
mechanism — because it is the one postcondition whose failure mode requires a
genuine, live, cross-CPU race (§0.3), and the pilot's own attempt to produce
that race under Component A's driver measured zero hits in a real, non-vacuous
trial set. Rung 2's driver exists to be the mechanism purpose-built to produce
it. §3 states the mutation bar this is judged against; the rest of this
section is inherited, verified, and closed.

---

## 2. Torture modes for Component C

Reuses the pilot's three-mode `quiesce::Mode` (`kernel/src/proof/quiesce.rs:43-66`)
and the existing `AntagonistOp` census (`kernel/src/proof/rng.rs:153-180`) —
`Unblock`, `Placement`, `KernelSchedule`, `ThreadChurn`, `ReclaimDrain`,
`Steal` — verbatim. No new antagonist operation is proposed; §3's mechanism is
built entirely from the existing six plus one new, narrowly-scoped stimulus
site (§3.2).

| Mode | What it drives for C | Quiescence |
|---|---|---|
| **Pen** (default) | Driver CPU runs the torture loop; every other online CPU is parked on the harness gate (`quiesce.rs:113-171`), still taking interrupts. Produces almost no cross-CPU scheduling churn on its own — parked CPUs do not call `schedule()` — so Pen alone is the WEAKEST mode for Component C, unlike Component A where Pen is the default working mode. Retained as the baseline/control arm per Risk 1 (§9 design doc; a finding that only appears once the pen is dropped may be the pen's own artifact). | Pen releases between iterations at a PRNG-selected cadence (`quiesce.rs:173-195`); peers report `ACTIVE_CPUS`/`CLEAR_OBSERVED` before the driver proceeds. |
| **Adversarial** (**default for C**, per design §2 line 158 — "Default mode for C and H") | Parked CPUs instead run `adversarial_step` (`quiesce.rs:232-288`) from the same seeded stream. For C specifically: `KernelSchedule` (`quiesce.rs:257,299-302`) drives peer CPUs through `scheduler::schedule()` — i.e., through `run_deferred_reclamation()` + `schedule_from_kernel()` — at high frequency (the pilot's own round-3 measurement: cov 50k–54k executions of the mutated site per boot from this op alone, `fix3-notes.md:14`); `Steal` (`quiesce.rs:278-286`) unblocks the driver's designated victim thread from a *foreign* CPU and immediately drives that CPU's own dispatch, exercising the exact work-stealing resume arm named in postcondition 2's evidence (`context_switch.rs:4740-4779`); `ThreadChurn` (`quiesce.rs:258-274`) and `Placement` (`quiesce.rs:247-256`) keep the ready queues non-trivial so a requeued thread has real cross-CPU competition instead of an idle field. | Same rendezvous mechanics as Pen; peers run their antagonist step once per PARKED cycle instead of spinning. |
| **Ambient** | No pen; the harness rides the full, otherwise-ordinary boot. Deliberately the least sensitive — this is where the natural, unforced rate of the same race (if any) would show up during ordinary boot-test-cohort activity, and it is the *mandatory confirmation arm*: `strand_oracle.rs:554-556`'s own self-collateral warning (7 aborts/4 hangs in 49 boots with the loop running vs. 1/50 without it) is why "a finding is not a finding until it survives Ambient" (design §9 risk 1, `quiesce.rs:9-14`). | `Window::Overlap` is forced for Ambient (`quiesce.rs:80`); the loop rides alongside the boot-test cohort rather than waiting for it. |

**What quiescence means for C specifically, stated because it is not obvious:**
unlike Component A (where "quiescence" bounds a manufactured single-lock probe
to microseconds), Component C's postcondition-1 window is a live hardware
race whose production requires *other CPUs actually running the scheduler*.
"Quiescence" for C therefore does not mean "hold the machine still" — Pen mode
literally suppresses the condition under test. It means "the harness controls
*which* CPUs are moving and on what schedule," so a catch's draw vector names
a reproducible antagonist recipe rather than an unattributable ambient
coincidence. This is stated explicitly because getting it backwards (treating
Pen as the strong mode, as Component A correctly does) would make Component
C's own driver arrive at the same negative result the pilot already measured.

---

## 3. The mutation bar

### 3.1 M2 — `coreproof_mut_cpu_identity` (#645)

**Already registered and already compiles.** `mutations.rs:102-108`; real
re-introduction at `context_switch.rs:6500-6535` (moves the `CpuId::current()`
read from after `disable_interrupts()` to before it, restoring the exact
pre-PR-#645 shape — verified against the PR-B root-cause description in the
project memory index: "schedule_from_kernel captured cpu_id ONE LINE BEFORE
disable_interrupts()"). Cargo feature already declared,
`kernel/Cargo.toml:56` (`coreproof_mut_cpu_identity = ["coreproof"]`).

**Expected detection signature:** `CPU_IDENTITY_SPLIT_EVENTS` advances during
the measured window, reported as
`[COREPROOF:VIOLATION:v1:comp=C:...:pred=CPU_IDENTITY_SPLIT:detail=<count>]`
via the same `record::violation` call Component A already uses
(`record.rs:136-151`, once generalized per §4). Secondary/corroborating:
`PERCPU_STACK_ALIEN_REFUSALS` or `RET_STAGE_REFUSALS` advancing in the same
window is reported as additional evidence on the same finding, not as a
separate predicate — per postcondition 1's corroborating-signal note in §1.

**What the pilot's predicate work already settled, folded in as given:**
`fix3-notes.md:109-133` is explicit that this is **not a predicate problem**.
`CPU_IDENTITY_SPLIT` is correct, already wired, already reachable from
outside the prohibited file (it is read via `cpu_identity_split_events()`,
`percpu.rs:194-196`, a plain atomic load — no seam of any kind is needed to
observe it). **No new predicate is proposed or needed for M2.** The entirety
of rung 2's M2 work is stimulus: building the driver whose job is producing
the state, not defining what "caught" means.

**The stimulus plan, concretely, and why it is different from round 3's
attempt:** round 3 armed a squeeze in *peers'* `kernel_schedule()` "immediately
before the scheduler entry" and measured 0/5 (`fix3-notes.md:125-132`),
diagnosing the structural cause precisely: "a preempted peer is requeued onto
its own CPU, and its own CPU has nothing else runnable, so it is
re-dispatched at home before any steal can take it." Two things were missing
from that attempt, both addressed here:

1. **Timing precision.** Round 3's squeeze rode the existing draw's
   log-uniform tick range (`stimulus.rs:111-119`, `[1, 20×TICKS_PER_INTERRUPT]`
   — up to ~0.83 ms per §1's C2 correction) — a squeeze that lands anywhere in
   that range mostly does NOT land inside the ~3-instruction pre-mask window
   at `schedule_from_kernel`'s entry (`context_switch.rs:6511-6516`). A new,
   narrowly-scoped `proof_point!` site is proposed: `ScheduleEntry`, placed at
   the top of `scheduler::schedule()` (`scheduler.rs:4459-4463`), *before* the
   call to `run_deferred_reclamation()`. This is the closest legal point to
   the vulnerable window: `scheduler.rs` is untiered (Component A already
   seams it) and is not in `check-coreproof-seams.sh`'s `PROHIBITED` array
   (`scripts/check-coreproof-seams.sh:59-73`), while `context_switch.rs` —
   where the actual window lives — is (`:66,72-73`, both architectures, file
   granularity, "no line pins"). `SiteClass::Open` (interrupts are enabled on
   entry to `schedule()` in every caller this design has verified — the
   function's own job is to mask them), which admits `TimerSqueeze` at its
   full drawable range including the minimum, `ticks=1`
   (`stimulus.rs:161-166`). This wrapper is not a hypothetical seam location —
   it is already the exact function the existing `KernelSchedule` antagonist
   op calls on every peer step (`quiesce.rs:299-302`'s `kernel_schedule()`
   helper calls `crate::task::scheduler::schedule()` directly, the same
   function at `scheduler.rs:4459-4463`; `driver_a.rs:241` and
   `strand_oracle.rs:591` reach it too), so wiring the seam here means the
   pilot's own already-measured 50k-54k-per-boot call volume through this path
   (`fix3-notes.md:14`) becomes the seam's own trial count for free, with no
   new call site needed to reach it. Armed with a low-`ticks` draw, the timer
   fires within a handful of cycles of the seam — inside `run_deferred_reclamation()`
   or the still-unmasked entry of `schedule_from_kernel()` — which is a much
   tighter aim than an ambient or peer-side squeeze, though **not a
   guarantee**; the residual gap between "fires very soon after the seam" and
   "fires inside the exact 3-instruction window" is real and is exactly what
   `iters`/`replay_hit_rate` must measure, never assert.
2. **Home-CPU occupancy.** The requeue-races-home-redispatch problem needs the
   victim's own CPU to be doing something else when the preempted thread is
   requeued, so a peer has time to steal it first. `ThreadChurn`
   (`quiesce.rs:258-274`) already keeps other runnable work on each parked
   CPU; Adversarial mode's existing `Steal` op
   (`quiesce.rs:278-286`) already drives the foreign-CPU dispatch attempt.
   Rung 2's driver sequences these against the `ScheduleEntry` seam rather
   than introducing a new mechanism: the driver selects one peer as "victim"
   (running `KernelSchedule`, hence hitting the seam) and a second peer as
   "stealer" (running `Steal` against the victim's tid), so the same draw
   vector that arms the squeeze also has a nonzero-probability antagonist on
   deck to contest the redispatch.

**The ≤5-minute re-find bar, inherited from pilot §7.3 item 1 unchanged:**
once implemented, M2 must be caught — a `[COREPROOF:VIOLATION:...:pred=CPU_IDENTITY_SPLIT:...]`
line — within 5 minutes wall clock **including that mutation's build** (cold
build 3-5 min per design §8, so this bar is tight; a warm rebuild against an
already-built tree is the practical target). This is measured across a small
seed set (the pilot used 3-5 seeds per leg; rung 2 should not need more, per
the gate-sizing discipline — more *boots* is the wrong lever, design §8).

**Honest fallback, pre-registered before any result exists (matching the
pilot's own discipline, `mutations.rs:20-26`):** if M2 is still not caught
within budget after the `ScheduleEntry` + victim/stealer pairing described
above, the reading is **not** "the bug is unfindable" — it is that this
component's stimulus is still under-aimed, a scoped follow-up (the icount
single-CPU escalation lane, design §4.3 and §10-O2, deliberately deferred
from the pilot but available as a targeted diagnostic rather than the
engine — re-running the exact M2 seed at `-smp 1 -icount rr=record` would
either reproduce the split as a single-CPU replay artifact of the timer
squeeze itself, which is diagnostic information, or fail to, which narrows
where the residual timing gap is). This fallback is **not** claimed as part
of rung 2's committed scope (§5) — it is the documented next lever if the
primary plan measures short, so a miss is reported with a concrete next step
rather than reported as a dead end.

### 3.2 Second C-domain mutation — the faithful #609 completion

**Why this one, and why not #608 or #576/#626:** design §7.2's own rule —
"a mutation is a known-fixed defect by definition" — excludes #608 (open) and
#576/#626 (open; the TTBR0-read-out fork, project-memory-index-confirmed
unresolved). The existing `coreproof_mut_masked_lock` (M6, `mutations.rs:130-136`)
is already registered against #609 and reads `PERCPU_STACK_ALIEN` — squarely
Component C's own oracle — but `fix3-notes.md:135-165` and the mutation site's
own comment (`scheduler.rs:4401-4414`) both document, with a measurement (15
mutated boots, zero census movement; 3 fresh boots at real coverage, zero
violations — `fix3-notes.md:158-164`), that the *registered* mutation only
restores **half** of #609's real defect: it removes the `without_interrupts`
bracket around `drop(reclaimed_threads)` but leaves `ARM64_STACK_BITMAP`
typed as `IrqSafeMutex` (`kernel/src/memory/kernel_stack.rs:706-707`, PR #632),
so the lock cannot be held preemptibly regardless of how the drop is entered,
and the orphaned-lock field failure the mutation is supposed to reproduce
cannot form. `passbar.md`'s recommendation is explicit: "#609 needs the bare-
`spin::Mutex` half of the original defect faithfully reintroduced under its
own feature... a deliberate scoping decision" (`passbar.md:318-322`).

**M7 (new register entry, sibling to the existing M6, does not modify it):**

| Field | Value |
|---|---|
| Feature | `coreproof_mut_masked_lock_bare` (new; `["coreproof"]`, following the existing convention at `Cargo.toml:55-60`) |
| Issue | #609 |
| Fixed by | PR #632 (the bitmap-lock typing) + the masking in the commit `scheduler.rs:4401` already attributes to PR #645 |
| Sites (two, must move together) | (a) `kernel_stack.rs:706-707` — `ARM64_STACK_BITMAP`'s declaration swaps from `IrqSafeMutex::new(...)` to a bare `spin::Mutex::new(...)` under the new feature; (b) `scheduler.rs:4415-4420` — the *existing* `coreproof_mut_masked_lock` cfg arm (unmasked drop) is reused/duplicated under the new feature so both halves are present together. Both `.lock()` call sites (`kernel_stack.rs:766,857`) are unaffected — `spin::Mutex` and `IrqSafeMutex` both expose a `.lock() -> Guard: DerefMut<Target = T>` API; verifying the exact type-compatibility is implementation work, not a design blocker. |
| Predicate | Unlike every other mutation in the register, **no `[COREPROOF:VIOLATION:...]` line is the expected outcome.** #609 "presented as a wedged boot with no marker at all" (`fix3-notes.md`'s own recommendation, `passbar.md:320-322`, echoing `mutations.rs:69` on the plain-`spin::Mutex` half). The expected detection is design §2's **second** gate-failing condition: a missing or malformed `RUN` line (`docker/qemu/run-coreproof-gate.sh:11-16`, condition 2) because the boot hangs before the driver ever reaches its `phase=close` record. |

**Detection signature, restated because it is genuinely different from every
other mutation in the register:** success for M7 is the gate's own
missing-RUN detector firing — a boot timeout, not a predicate. This must be
stated explicitly in the driver/gate documentation so a future reader does
not treat "M7 produced no VIOLATION line" as a false negative; it is the
*expected* positive outcome for this specific mutation, and the gate wrapper
(`run-coreproof-gate.sh`) needs no change to catch it — condition 2 already
exists and already fails the gate on exactly this shape.

**The ≤5-minute bar for M7:** interpreted as time-to-hang-detection, not
time-to-violation-line: the gate's own boot timeout (already present in every
gate script in this tree) must fire within its existing bound, which is
already well under 5 minutes per boot. No new timing infrastructure is
required.

### 3.3 What is deliberately NOT a mutation here

**#635's live face** (the whole-context/live-x30 corruption the project
memory index tracks as still open, ~1.5%/boot, unattributed producer distinct
from #645's) is explicitly **not fixed** and is **not planted**. Per design
§7.2's own rule and the campaign-wide non-negotiable against faking a pass:
rung 2 may *hunt* it (Ambient mode, riding a real boot, is exactly the
mechanism that would surface it if it recurs during a coreproof run) and any
occurrence during this rung's gates is reported as a live find against an
open issue — never claimed as a validation, never counted toward the pass
bar. This mirrors exactly how design §7.2 treats #608 on the x86 lane.

---

## 4. Ratchet requirements carried from the pilot

**Both existing ratchets are census-shaped against a generic prefix and
require zero changes for rung 2 to stay covered — verified, not assumed:**

* **Source scan**, `scripts/check-coreproof-seams.sh`. `PROHIBITED` is a file
  list (`:59-73`) that already includes both architectures' `context_switch.rs`
  and `context.rs` at file granularity; `SEAM_PATTERNS` (`:78-83`) matches any
  spelling of `proof_point!`, `proof_cover!`, `crate::proof::`, `kernel::proof::`.
  A new `driver_c.rs`, a new `mutations.rs` entry, and the one new
  `ScheduleEntry` seam (§3.1, sited in the untiered `scheduler.rs`) all pass
  this scan by construction — none touches a prohibited file. The script's own
  anti-vacuity mode (`--prove`, `:129-153`) is unaffected and needs no rung-2
  edit. **If a future implementation instead wants a seam inside
  `context_switch.rs`, this script has to be edited to remove that file from
  `PROHIBITED` in the same diff — which the script's own header (`:20-22`)
  explicitly reserves for "a later rung that... argues for it in its own PR."
  This rung does not do that; §5 makes it an explicit non-goal.**
* **Binary scan**, `scripts/check-coreproof-production-clean.sh`, exercised by
  `tests/coreproof_production_clean.rs`. `SYMBOL_NEEDLE='coreproof'` and
  `MARKER_NEEDLE='[COREPROOF:'` (`check-coreproof-production-clean.sh:87-88`)
  are generic prefixes, not per-component literals — every new symbol
  (`driver_c::*`, `mutations::REGISTER`'s new entry, `MutSite::MaskedLockBare`)
  and every new marker line (`comp=C` once §4's record-module generalization
  lands) is covered automatically. LEG 3 (`.text` byte-identity with seams
  blanked, `coreproof-blank-seams.py`) likewise needs no rung-2-specific
  change — it operates on whatever `proof_point!`/`proof_cover!` invocations
  exist in the tree at build time. **No new work required here either.**
* **Structural census ratchets** — `tests/coreproof_mutation_register_structure.rs`
  (keeps `Cargo.toml`'s `coreproof_mut_*` features, `mutations::REGISTER`, and
  the real `#[cfg]` sites equal in both directions; its own header states
  "Adding a seventh mutation requires no edit to this file," `:18-19`) and
  `tests/coreproof_coverage_structure.rs` (the `MutSite` census) are likewise
  purely additive-safe: M7 is a seventh register entry and a seventh
  `MutSite` variant, both handled by the existing census logic with no edit
  to either test file.

**One real, required framework change, not additive-safe as-is:**
`tests/coreproof_sites_structure.rs` and `kernel/src/proof/sites.rs`'s
`SiteId`/`ALL`/`DECLARED`/`VISITED` are a single global set
(`sites.rs:28-61,145-154`), and the gate's vacuity guard
(`sites_visited < sites_declared`, design §2 line 185, `run-coreproof-gate.sh:17-22`)
compares against the *whole* declared set on every boot. If Component C's
`ScheduleEntry` site (and Component A's existing twelve) were simply merged
into one shared array, a Component-A-only boot would declare a site it can
never visit and permanently redden its own non-vacuity leg — and vice versa
for a Component-C-only boot. **This must become component-scoped before M2's
driver can ship without breaking the pilot's own pass bar retroactively.**
The cleanest shape consistent with the existing "mutually-exclusive
compile-time driver selection" pattern (§5, `option_env!`-gated `MODE`/
`WINDOW`/`SEED` are all compile-time already, `rng.rs:92-95`, `quiesce.rs:52-57,77-83`):
a new `coreproof_component_c = ["coreproof"]` feature selects, at compile
time, which `SiteId` enum + `ALL` + `DECLARED` + `VISITED` the crate builds
with, and `record.rs`'s hardcoded `comp=A` literal (`record.rs:116,140`,
verified — both `emit_run` and `violation` embed the literal string `A`
directly in the format string) becomes a parameter threaded from
`driver_a::COMPONENT_A` / a new `driver_c::COMPONENT_C`. Both changes are
small and mechanical; neither is optional, because without them rung 2 cannot
land without silently breaking the pilot's own non-vacuity guarantee.

---

## 5. Non-goals, explicit

* **No seam anywhere in `context_switch.rs`, either architecture, under any
  spelling.** Mechanically enforced today (§4); this rung neither needs nor
  proposes editing `PROHIBITED` to lift it. The `ScheduleEntry` site (§3.1)
  is deliberately upstream of it.
* **No seam in the ERET epilogue.** Not reachable from this rung's scope at
  all — Component C never touches the epilogue; this is inherited from
  design §9's permanent exclusion and restated only for completeness.
* **No Tier-1 file.** Nothing in Component C's contract or this rung's
  proposed seam touches `kernel/src/interrupts/{handler.rs,time.rs,entry.asm,timer.rs,timer_entry.asm}`
  or their aarch64 moral twins. Rung 8 (Component G) is where a Tier-1 ask is
  ever made, per design §9, and it needs operator approval that this rung
  does not request.
* **No x86 driver for Component C.** Verified by absence: neither
  `kernel/src/arch_impl/x86_64/percpu.rs` nor `kernel/src/per_cpu.rs` defines
  any analogue of `CPU_IDENTITY_SPLIT_EVENTS`, `PERCPU_STACK_ALIEN_REFUSALS`,
  or the `percpu_custody` three-function contract (grep-confirmed, zero
  hits). Component C's entire contract is AArch64-native — the x86 per-CPU
  model (GS-segment-relative, no per-CPU slot-ownership record) does not
  have the analogous defect class. `lib.rs:93-100`'s own comment states the
  x86 driver question is "#608's own hunt... not in this pilot"; nothing in
  design §5's row C or §6's rung-2 row asks for one either. Out of scope here
  by the same reasoning, not by oversight.
* **No modification to the pre-existing `percpu_stack_custody_oracle` (legs
  A-D) or its dedicated gate scripts.** §1 treats it as complete, existing
  evidence for postcondition 2's setter side; rung 2 reads it, never edits it.
* **No modification to `coreproof_mut_masked_lock` (M6)'s existing, honestly-
  disclosed partial re-introduction.** M7 (§3.2) is a new sibling entry, not
  a rewrite of M6 — M6's own miss and its documented reason stay exactly as
  round 3 recorded them.
* **The icount/record-replay lane (design §4.3, §10-O2) stays deferred.** §3.1
  names it only as a *diagnostic* fallback if M2's primary stimulus plan
  measures short within budget — it is not part of this rung's committed
  build, matching the design's own "defer to rung 3, first flaky replay that
  needs it" recommendation.
* **Anything the design gates on operator approval** (Tier-1, per the above)
  is out of scope for the same reason it is out of scope for the whole
  program below rung 8.

---

## 6. What "done" looks like for rung 2

1. `driver_c.rs` (new), analogous in shape to `driver_a.rs`: spawned from
   `proof::start()` when `coreproof_component_c` is the active build feature
   (mutually exclusive with Component A's driver per build, matching the
   existing compile-time-selected `MODE`/`WINDOW`/`SEED` pattern).
2. `sites.rs` and `record.rs` generalized to be component-scoped (§4) — this
   is required, not optional, before M2's driver can ship.
3. `ScheduleEntry` seam added at `scheduler.rs:4459` (top of `schedule()`,
   before `run_deferred_reclamation()`), `SiteClass::Open`.
4. M7 registered in `mutations.rs`, gated `coreproof_mut_masked_lock_bare`,
   at the two sites named in §3.2.
5. Pass bar, mirroring pilot §7.3's shape for a two-mutation validation set:
   both M2 and M7 re-found within their respective ≤5-minute bars (§3.1,
   §3.2); anti-vacuity leg (unmutated tree, same seed set, both profiles,
   zero violations, zero unexplained hangs); `sites_visited == sites_declared`
   on every boot; replay hit rate measured (not asserted) for M2's catching
   seed; both existing ratchets (§4) still pass unmodified; the existing
   five-gate matrix and the pre-existing `percpu_stack_custody_oracle` gate
   both still pass at this rung's SHA, unmodified.
6. If M2 is not caught within budget: reported honestly as a stimulus-scoping
   miss with the icount fallback named as the next lever (§3.1), never
   silently weakened into a pass.

---

## Anchor index (file:line, all re-verified at `579a10ccd731`)

`kernel/src/proof/mod.rs:143-215` (arm/disarm/seam/fire/start) ·
`kernel/src/proof/quiesce.rs:43-66,113-361` (Mode, Controller, worker,
adversarial_step) · `kernel/src/proof/rng.rs:153-238` (AntagonistOp, draw) ·
`kernel/src/proof/stimulus.rs:44-173` (Action, apply, materialize) ·
`kernel/src/proof/sites.rs:28-154` (SiteId, ALL, DECLARED, mark_visited) ·
`kernel/src/proof/coverage.rs:15-58` (MutSite, HARNESS_SIDE note) ·
`kernel/src/proof/record.rs:105-151` (emit_run, violation, hardcoded comp=A) ·
`kernel/src/proof/mutations.rs:1-171` (register, M2 at 102-108, M6 at
130-136) · `kernel/src/proof/driver_a.rs:1-56,69,119-124,279-323` ·
`kernel/src/lib.rs:82-146` (proof_point!/proof_cover! two-polarity macros) ·
`kernel/src/arch_impl/aarch64/percpu.rs:37,48-50,68-75,100-107,135-150,
162-173,185,194-217,233-270,281-309` ·
`kernel/src/arch_impl/aarch64/percpu_custody.rs:15-18,43-66,83-135` ·
`kernel/src/arch_impl/aarch64/context_switch.rs:2227,2891-2958,3315,3555-3580,
4669,4740-4779,6500-6535` ·
`kernel/src/task/scheduler.rs:4382-4421,4459-4463` ·
`kernel/src/task/thread.rs:252,263-264` (CpuContext magic) ·
`kernel/src/task/percpu_stack_oracle.rs:1-25,644-779,790` ·
`kernel/src/memory/kernel_stack.rs:706-707,766,857` ·
`kernel/Cargo.toml:29-60` · `scripts/check-coreproof-seams.sh:1-157` ·
`scripts/check-coreproof-production-clean.sh:1-90` ·
`tests/coreproof_production_clean.rs:1-101` ·
`tests/coreproof_mutation_register_structure.rs:1-60` ·
`docker/qemu/run-coreproof-gate.sh:1-100,125,133,376` ·
`docker/qemu/run-aarch64-percpu-stack-custody-gate.sh` (existence confirmed,
not modified).
