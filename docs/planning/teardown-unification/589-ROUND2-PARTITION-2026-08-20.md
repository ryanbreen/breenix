# #589 round 2 — collateral partition, RCA and verdicts

Branch `fix/589-deferred-requeue-drift`. Round 1 (`93fd0b4c`) eliminated the #589 bucket (0/50 and
0/200 against 25/50 on `main`) but shipped a collateral bucket the review blocked on (B1): an abort
rate of 12–15% carrying a `FAR=ELR=0x80000000` signature that is **not** #576's filed signature,
plus revived `#596`-shape data aborts, `#575`-shape spawn stalls, and unattributed early-boot hangs.
This document records the controlled partition that attributed every one of those signatures, the
resulting fixes, and what is left open.

## 1. Method

Six builds, each run through `docker/qemu/run-aarch64-service-sequence-gate.sh --boots 25 --profile
both` (50 boots per arm: 25 × `max`, 25 × `cortex-a72`). All arms ran on the same host in the same
session; arms A/B/C ran as three concurrent QEMUs so they saw identical host load, and the later arms
ran under the same 3-way concurrency. Every arm's kernel is a `boot_tests` build of
`aarch64-breenix-kernel.json` (soft-float; `check-kernel-no-neon.sh` PASS on all six). Only the
`KERNEL=` line of the gate script differed between arms — verified by `diff` against the repo copy.

| arm | build |
|---|---|
| **M** | pure `main` `e377e7a8` |
| **A** | branch, with the whole `pending_next` resolver removed (publication, both resolve calls, the `requeue_thread_after_save` widening) and the injection disarmed — i.e. `main`'s scheduler behaviour plus the census detector |
| **B** | branch, injection stimulus disarmed (victim kthread not spawned, arm never set); resolver + census retained |
| **C** | branch as shipped in round 1, `93fd0b4c` |
| **D** | round-2 HEAD after the first fix (injection armed only when the trampoline's outgoing thread is that CPU's own idle thread) |
| **E** | HEAD with the victim kthread running but the injection never armed |
| **F** | HEAD with the victim parked on a 50 ms timer instead of driving an inline schedule per iteration |

Arms A, B, E and F are uncommitted experiment builds; the repository tree was verified byte-clean
against its commit after each one. Arm F's source change is what commit `ec89198f` ships (plus the
scoring-window repair that arm F itself exposed).

All serials are preserved. Buckets below are the **round-2 signature-keyed classifier** (every arm's
serials were re-scored with it), which attributes an instruction abort only when its FAR/ELR/ESR
fields match a filed signature and calls everything else UNATTRIBUTED.

The classifier's first cut read the two abort record sources in preference order — the
`[INSTRUCTION_ABORT] FAR=... ELR=... ESR=...` header first, the `[FATAL_REGS] label=INSTRUCTION_ABORT
... far=... elr=...` dump only as a fallback — and that reintroduced the very hole review B1 blocks
on, in miniature. Arm A's `max/serial-10` carries **both** records for one fault on one CPU and they
**disagree**: header `FAR=0x0 ELR=0x0 ESR=0x86000005`, byte-identical to #576's filed signature, and
FATAL_REGS `far=0x0 elr=0x4ba`. A header-first reader files that boot under `576`, a bucket this gate
tolerates. Two records of a single fault that disagree describe a CPU state that changed between
them, which is not the filed single-shot signature. The shipped classifier therefore takes the
**union of both sources** and attributes only when the resulting set has exactly one element that
matches a filed signature; a disagreement, a serial carrying two different signatures, and an
unreadable record are all UNATTRIBUTED. Across the 27 preserved serials that carry a fatal abort
record, 14 agree across both sources, 10 parse from one source only, and 3 disagree. Re-scoring every
arm with the corrected classifier moved exactly one boot — arm A's `max/serial-10`, `576` →
UNATTRIBUTED — i.e. strictly out of a tolerated bucket into the FAIL set; the per-signature table
below is unchanged, because it was derived by reading the FATAL_REGS record directly.

Per-arm bucket census under the shipped classifier (50 boots each; `armM`'s 25 non-#589 boots score
UNATTRIBUTED only because pure `main` emits none of the branch's strand markers, which the GREEN arm
requires — a cross-scoring artifact, not a `main` red):

| arm | buckets |
|---|---|
| **M** | `589=25 UNATTRIBUTED=25` |
| **A** | `589=46 UNATTRIBUTED=3 GREEN=1` |
| **B** | `596=1 GREEN=49` |
| **C** | `596=7 UNATTRIBUTED=11 576=2 575=1 GREEN=29` |
| **D** | `596=5 UNATTRIBUTED=11 576=1 GREEN=33` |
| **E** | `596=5 UNATTRIBUTED=16 576=1 575=1 DATA_ABORT=1 GREEN=26` |
| **F** | `589=6† 576=1 GREEN=43` |

## 2. The partition table — one line per signature

Every arm is a complete 50-boot run. Counts are out of 50.

| signature | M pure main | A main sched + census | B resolver + census | C round-1 branch | D idle-gated arm | E victim only | F timer victim | verdict |
|---|---|---|---|---|---|---|---|---|
| **#589** — `live sibling refused exec` / census `stranded>0` | 25 | 46 | **0** | **0** | **0** | **0** | 6† | resolver fixes it; unchanged from round 1, now with an in-session pure-`main` control |
| **INSTRUCTION_ABORT `FAR=ELR=0x80000000 ESR=0x86000005`** | 0 | 0 | **0** | 4 | 5 | 6 | **0** | **stimulus-artifact — fixed** (the victim kthread's ~1 kHz inline-schedule drive) |
| **INSTRUCTION_ABORT `FAR=ELR=0x0 ESR=0x86000005`** (filed #576) | 0 | 0 | 0 | 2 | 1 | 1 | 1 | pre-existing #576 at roughly its filed rate; amplified by the same drive |
| **INSTRUCTION_ABORT, other field sets** (`ESR=0x82000005`, `0x8600000d`, `0x8600000e` with a kernel-stack PC, `far=0x0 elr=0x4ba`) | 0 | 1 | **0** | 2 | 2 | 2 | **0** | **stimulus-artifact — fixed**; now classified UNATTRIBUTED instead of being absorbed into #576 |
| **DATA_ABORT `FAR=0x1f0–0x290` at `check_need_resched_and_switch_arm64+0x4d04/+0x4d50`** | 0 | 0 | **0** | 6 | 4 | 3 | **0** | **stimulus-artifact — fixed**; the site is a `threads` walk (`ldr xN,[x,#0x198]`) reading a garbage element pointer |
| **DATA_ABORT, wild FAR / wild ELR** (e.g. `far=0xfff7000242c7a464` at `schedule_deferred_requeue+0x774`) | 0 | 0 | 1 | 1 | 1 | 2 | **0** | **pre-existing background** wild-context fault (#596/#605 family); present on the resolver-only control |
| **#575-shape** `spawn never returned` | 0 | 0 | **0** | 1 | 0 | 1 | **0** | **stimulus-artifact — fixed**; NOT a regression of closed #575 |
| **U1 — early-boot stall, ~200–250 line serial, no `FUTEX_HANDOFF_ORACLE`/`BLOCK_EINTR_ORACLE`, census clean** | **0** | 2 | **0** | 5 | 5 | 9 | **0** | **stimulus/detector-artifact — fixed**; absent from pure `main`, absent from the resolver-only control, absent once the victim is parked |
| **U2 — `IDLE_CTX_SAVE_K`/`IDLE_CTX_RESTORE` ping-pong hang, ~36k lines** | 0 | 0 | **0** | 4 | 0 | 0 | **0** | **stimulus-artifact — fixed**; the #606-class lost wake, driven by the stimulus |
| **x86 `clonevm_exec_test` second-stage `sys_read` spin** | see §5 | — | — | — | — | — | — | **pre-existing log line; the hang did not reproduce — filed as #608** |

† Arm F's six `#589` reds are NOT strands: all six read
`legA_exercised=1:legA_recovered=1:legB_exercised=1:legB_recovered=0:stranded=1`, all six were emitted
at uptime ≈ 6.1 s (exactly `INJECT_DEADLINE`), and all six carry `resolved_exercised=2` proving both
rollback legs ran. That is the scoring-window truncation described in §4.3, fixed after arm F ran.

## 3. RCA — what actually produced the collateral

**Arm B is the control that settles it.** Arm B carries the *entire* round-1 fix — the `pending_next`
publication, both `resolve_pending_next_locked` call sites, the widened `requeue_thread_after_save`,
and the always-on census — and differs from the shipped branch only in that the injection stimulus is
not started. Arm B is 49/50 GREEN with zero aborts of any injection signature. The resolver is not
the source, and there is no resolver double-dispatch: the hypothesised commit/resolve race would have
had to show up in arm B.

**Arm E localises it inside the stimulus.** Arm E keeps the `strand_victim` kthread but never arms the
injection, so `inject_if_armed` always returns `None` and the forced null-`scheduler_ptr` event never
happens. Arm E still reproduces the whole collateral profile (7 aborts + 4 hangs in 49 boots). So the
damage is not the forced null fallback and not the abandoned publication — it is the victim thread
itself.

`strand_victim` drove

```rust
super::scheduler::yield_current();
crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
VICTIM_PROGRESS.fetch_add(1, Ordering::AcqRel);
crate::arch_halt_with_interrupts();
```

in a loop for six seconds — roughly one inline kernel-side schedule per timer tick, ~1 kHz, against a
path that carries three open defects (#596 inline-save resume point, #605 already-consumed handoff
slot, #607 the null branch skipping the outgoing `elr_el1 = x30` repair and the outgoing requeue).
The failing serials show the mechanism directly: nested `[INLINE_SAVE_OVERWRITE]` records on the
victim's *neighbour* (init, tid 1200) with `saved_slot20 != slot20` and `elr != x30`, a
`[CTX596_ELR_DIVERGENCE]` on the same tid, and then a ret-dispatch to a garbage PC. Every red in arms
C/D/E lands at t≈4 s — inside the stimulus's six-second window — and the 0x80000000 crash register
set is byte-identical across independent boots and both CPU profiles, i.e. a deterministic path, not
noise. The stimulus was manufacturing ten times the natural rate of an already-open defect class and
the gate was scoring the result as if it were the kernel's own health.

**Verdict per signature is in the table. Nothing was bucketed away**: the two signatures that survive
(`FAR=ELR=0x0 ESR=0x86000005` at #576's filed rate, and the ~1/50 wild-FAR `schedule_deferred_requeue`
fault) are present on the resolver-only control and on the main-scheduler control, and the classifier
change below makes any *other* instruction abort a hard gate failure rather than a tolerated bucket.

## 4. The fixes

1. **Arm the stimulus only against an idle outgoing thread** (`inline_schedule_trampoline`). The
   forced null branch cannot repair a non-idle outgoing thread — that is #607 — so arming it over a
   live thread corrupts the thread the oracle is not testing. When the outgoing thread is this CPU's
   own idle thread, the same branch re-establishes it through `reset_idle_continuation_locked`.
   Retained on design grounds; arm D shows it is not sufficient on its own, and its independent
   necessity is not separately measured.
2. **Park the victim on a 50 ms timer** (`sleep_sample_period`, the helper the detector already uses)
   instead of driving `yield_current()` + `schedule_from_kernel()` per iteration. The stimulus's own
   drive rate is not part of what the oracle tests; the victim only has to be dispatchable and to make
   observable forward progress. Dispatch rate drops ~50×; arm F's collateral drops to the background
   rate.
3. **Stop the marker truncating an in-flight scoring window.** Arm F exposed a second, independent
   defect in the oracle: the report deadline (`INJECT_DEADLINE`, 6 s) fired while leg B's 2 s scoring
   window was still open, emitting `legB_recovered=0:stranded=1` in boots whose own
   `resolved_exercised=2` proves both rollbacks ran. That is a FALSE RED, and a false red is as
   dishonest as a false green. The scoring window is shortened to 1 s (a *stricter* recovery
   criterion) and the report is held, up to an absolute cap of `INJECT_DEADLINE + 2 ×
   INJECT_SCORE_WAIT`, while a leg is mid-scoring.
4. **Keep the `pending_next` ownership record on a refused rollback** (review F5): the refusal checks
   now run against a peeked tid and the slot is taken only on the path that enqueues.
5. **Emit the first census at 500 ms instead of 3 s** (review F4), so a boot that dies in the first
   seconds still carries a census and "stranded=0 in every gate boot" is a claim about every boot.

### Gate and classifier changes (review B1/B2)

* `run-aarch64-service-sequence-gate.sh`: instruction aborts are attributed **by field signature**,
  over the **whole set** of abort records the serial carries. `instruction_abort_signatures` unions
  both record sources (the `[INSTRUCTION_ABORT] FAR=...` header and the `[FATAL_REGS]
  label=INSTRUCTION_ABORT ... far=... elr=...` dump), normalises each to `far elr esr`, and `sort -u`s
  them; the classifier attributes the tolerated `576` bucket only when that set has exactly one
  element and it is `0x0 0x0 0x86000005`. An empty set, a set with more than one element (records
  that disagree, or two different signatures in one boot), and a single non-matching element are all
  UNATTRIBUTED, which this gate FAILS on. Previously every `[INSTRUCTION_ABORT]` was bucketed as 576,
  a bucket the gate tolerates, so a new signature was invisible by construction — and a first-record
  preference reopened that hole for any boot whose two records disagree (§1). This is a strict
  tightening throughout: boots can only move from a tolerated bucket into the FAIL set.
* `run-aarch64-boot-test-strict.sh`: `score_serial` now rejects `stranded=[1-9]` on either strand
  marker, scanned over the whole serial, *before* the presence checks. The census is cumulative and
  emitted on a fixed cadence, so every boot surviving the first emission always contains a clean line
  and the old presence-only pin could not fail on a late strand. The build hint also gained
  `--features boot_tests`, without which the gate's own pins fail spuriously (review F10).
* `run-aarch64-full-test.sh`: the strand check moved to a post-run whole-file scan after QEMU exits.
  The Phase 1a3 poll loop breaks the instant both green patterns match, so a strand appearing later
  was never scored.

No FAIL condition was relaxed, no threshold lowered, no literal loosened.

## 5. x86 differential (review B3)

`main` was rebuilt from a **fresh clone** (`git clone --no-local` of the branch checkout, `main`
`e377e7a8`) in the beast `breenix-x86` VM. Three fresh-clone landmines had to be repaired before it
would build and boot, all local-only and all pre-existing: the gitignored `Cargo.lock` (a fresh
resolve picks `x86_64 0.15.5`, which breaks the pinned nightly), the `rust-fork` symlink (points at a
Mac path; must be pointed at the VM's real fork clone or `kernel/build.rs:170` panics), and the
missing `target/ovmf` firmware. These are why the previous attempt's differential was abandoned.

`docker/qemu/run-boot-parallel.sh 5`, twice each, fully isolated:

| tree | batches | gate result | boots with the `sys_read` spin-hang |
|---|---|---|---|
| `main` `e377e7a8` | A, B | 4/5 then 5/5 (the one failure is `loopback_wake_test_child:15,loopback_wake_test:1` — the pre-adjudicated #586 family) | 0/10 |
| branch | A, B | 5/5 and 5/5 | 0/10 |

**The round-1 signature was mis-derived.** `sys_read: fd=<10+ digits>, buf_ptr=0x1, count=0` occurs
**68–126 times in every boot on `main`**, in boots that pass the gate with
`USERSPACE TEST COMPLETE`. The line is normal; the defect round 1 saw was a *hang* whose
distinguishing feature is tens of thousands of repeats with no completion. That hang did not
reproduce in 10 branch boots or 10 `main` boots. On this differential the branch is not worse than
`main` on x86 — it is one gate failure better — and the branch's only x86-visible change is the
census sampler. Nothing here is branch-attributable.

## 6. The acceptance battery at HEAD, and what it uncovered

Run against `18dcb2ef` on one host in one session. Production and `boot_tests` profiles both rebuild
with zero Breenix warnings (the single warning in each log is the toolchain's future-incompat note
about `core` from the build-std source, not kernel code); `check-kernel-no-neon.sh` PASS on both;
nine structural suites green (`strand_handoff` 13, `exec_lock_order` 34, `context_restore` 61,
`teardown` 53, `block_request_lifetime` 11, `net_lock` 19, `loopback_pump` 57, `kernel_no_neon_guard`
1); the production-profile gate PASSES with both new markers pinned absent; beast runs
`run-boot-parallel.sh 5` twice at this HEAD, 5/5 and 5/5.

**Service-sequence gate, 25 boots per profile: 48/50 GREEN, `589=0`, `596=0`, `DATA_ABORT=0`,
`575=0`, no `0x80000000` bucket, one tolerated `576` at its filed field-exact signature — and one
UNATTRIBUTED, which fails the gate.** The round-1 collateral is gone; the gate is red on something
else.

That something else is a real early-boot stall, now filed as **#609**: the boot-test executor reaches
`[SUBSYSTEM:memory:early:COMPLETE:24/24]` and the `network:early` subsystem kthread never emits its
first marker, while the kernel stays alive and the strand census keeps sampling `stranded=0` for the
full timeout. `run_staged_tests` spawns every subsystem kthread before joining any, so the missing
thread was created and never dispatched — a lost dispatch that **this branch's own census does not
see**: `worst_dwell_ms=0` with roughly two threads examined per sample. That blind spot is recorded
in #609 and it qualifies every "stranded=0" claim in this document.

Attribution of #609 is **open**. A same-session interleaved control against `main` `e377e7a8` —
alternating boots, byte-identical QEMU arguments — gave 0/20 per arm unstarved and 0/25 per arm under
ten `nice -n 19` hogs. At the observed 2-3% rate that control has no power; it is not evidence that
`main` is clean. Pooled with the partition arms (main 0/95, branch 2/171) Fisher gives p ≈ 0.5. It is
deliberately not bucketed into #576, #575, #586 or #589.

The strict gate — the kernel-merge gate — was **structurally unable to pass**, on this branch and on
`main` alike, and this battery is what found it. Its poll loop broke as soon as a userspace liveness
pattern and `[EXEC_SMOKE:TARGET_OK]` were present (about 0.5 s and 4.4 s of uptime), killed QEMU, and
only then scored the serial for the block EINTR oracle, the futex handoff oracle and both strand
markers — all emitted later. Measured 0/20. The repair makes the loop poll `score_serial` itself, so
the stop condition and the scoring criteria cannot drift apart; `score_serial` is a strict superset
of the two conditions it replaces. The gate goes to 5/6, with the remaining failure being #609. Until
#609 is fixed the strict gate cannot reach its required 100%.

The full-system test is not green on aarch64 for a reason that predates this work: **#593**, filed,
says init's aarch64 boot script spawns no terminal, so Phase 2 can never pass headless. Three runs at
this HEAD reach Phase 2 and time out there. `main`'s kernel, run through the same script from its own
worktree, dies earlier — `Phase 1c: clonevm_exec_test never completed (30s timeout)`, i.e. #589
itself. One of four branch runs failed at Phase 1c with `ERROR sibling wake of parent failed`; that
is **#610**, a TOCTOU in the test program (the parent publishes readiness before it blocks, so the
sibling's `FUTEX_WAKE` legitimately finds no waiter and its `!= 1` assertion fires) — a false red, not
a kernel wake loss, and not fixed here because changing userspace test source rebuilds the ext2 image
and would invalidate the battery it was measured beside.

## 7. What remains open

* **#607** — the null-`scheduler_ptr` branch does not repair the outgoing thread's resume point and
  does not requeue it. Untouched here deliberately; round 1 measured that a naive outgoing rollback
  trades a hang for a crash. Fix 1 above stops the *test stimulus* from exercising it on live threads;
  it does not fix the production path.
* **#605 / #596** — the already-consumed handoff slot and the inline-save resume-point divergence.
  The ~1/50 wild-FAR fault at `schedule_deferred_requeue+0x774` and the #576-signature aborts belong
  to this family and are present on `main`-behaviour controls.
* **Fix 1's independent necessity** is not separately measured (arm D isolates it but is confounded by
  the victim drive it was measured against). It is retained on design grounds and pinned structurally.
* **#609** — the `network:early` subsystem kthread never dispatched, ~2-3%, unattributed, and blind
  to the strand census. This is what keeps both the service-sequence gate and the strict gate red.
* **#610** — the `clonevm_exec_test` post-exec rendezvous race, a false red at roughly 1 in 4
  full-test runs; the first item for a follow-up slot, since fixing it rebuilds the ext2 image.
* **#593** — Phase 2 of the full-system test can never pass headless on aarch64. Until it is fixed,
  "the full test passes" is not an available claim on this architecture for any branch.
