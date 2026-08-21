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
| **DATA_ABORT, wild FAR / wild ELR** (e.g. `far=0xfff7000242c7a464` at `schedule_deferred_requeue+0x774`) | 0 | 0 | 1 | 1 | 1 | 2 | **0** | **NOT fixed — filed as #612.** Recurs at the final r2 HEAD (1/400 in the §6 400-boot battery, field-exact FAR=0x292 ELR≈`schedule_deferred_requeue` ESR=0x96000021, garbage callee-saved register file), still present on the resolver-only control (arm B, 1/50). #605/#607 are recorded as open-family context, not a proven mechanism link; do not attribute to closed #596 |
| **#575-shape** `spawn never returned` | 0 | 0 | **0** | 1 | 0 | 1 | **0** | **stimulus-artifact — fixed**; NOT a regression of closed #575 |
| **U1 — early-boot stall, ~200–250 line serial, no `FUTEX_HANDOFF_ORACLE`/`BLOCK_EINTR_ORACLE`, census clean** | **0** | 2 | **0** | 5 | 5 | 9 | **0** | **NOT fixed at the final r2 HEAD.** The stimulus-driven volume this table measured is gone (absent from pure `main`, the resolver-only control, and once the victim is parked), but the underlying class recurs at background rate: 1/200 starved-leg boots in the §6 400-boot battery, a `memory:early`-stuck variant with the same "census reports `stranded=0`" blind spot. Noted against #609 (comment appended, signature widened to "any subsystem kthread") rather than re-filed |
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

### The #609 arm (coordinator ruling R30)

R30 pre-adjudicates #609 by FIELD signature as an *attributed* non-green, at its filed ~3% rate, and
directs that the classifier carry a tightly-keyed arm for it. `is_609_network_early_stall` in
`run-aarch64-service-sequence-gate.sh` is that arm. Five clauses, every one required, every one a
shape rather than a name list: memory:early reached COMPLETE; the network:early kthread emitted
**nothing at all** (zero `[SUBSYSTEM:network:...]` and zero `[TEST:network:...]` lines — a kthread
that was dispatched and then wedged prints its own `:START` first, which is what separates "never got
a first instruction" from "ran and hung"); no `[STAGE:early:COMPLETE`; no abort, panic or lockup of
any kind; and a strand census still sampling into the hundreds with `stranded=0`. The arm is
consulted **last** of the attributing arms, after every abort signature and both strand arms, so a
boot can only reach it by having crashed nowhere and stranded nothing.

The arm was measured against **every preserved serial in this campaign — 476 of them, spanning all
seven partition arms and both acceptance batteries**. It moves exactly three boots, all of them out
of UNATTRIBUTED: `proof-ss25/cortex-a72/serial-18` and `partition-armA/max/serial-8` and `serial-21`
— and armA is the main-scheduler-behaviour arm, which is the same 2/50 the filing already reports for
it. **84 UNATTRIBUTED boots remain unattributed**, including the entire U1 early-boot-stall family,
which the arm refuses on its first clause: those boots die *before* memory:early completes. No boot
that was already attributed to any bucket moved. The arm absorbs nothing.

Because a pre-adjudication by rate is only honest if the rate is enforced, the gate computes a
ceiling — `max(1, ceil(0.06 × total boots))`, i.e. twice the filed rate with a floor of one, giving 3
at the default 50 boots — and **FAILS the run when the #609 count exceeds it**. At p=0.03 a 50-boot
run crosses that line about 6% of the time, so crossing it means a materially higher rate rather than
ordinary variance. The boots stay attributed; the run stops being covered. `#609` is reported in
every per-profile and total census and is listed among the non-GREEN boots exactly as before.

The strict gate is deliberately **not** given this arm. It is the kernel-merge gate and requires
100%; #576 already hard-fails it despite being pre-adjudicated, so the precedent is that
pre-adjudicated signatures are tolerated where buckets exist and adjudicated externally where they do
not. Adding a tolerance there would be a FAIL-condition relaxation that R30 does not ask for.

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

### The wrong-profile kernel trap (found while re-running the battery)

Re-running the battery at HEAD produced **0/6 on the strict gate** and **21 consecutive
"futex handoff oracle marker missing" boots** on the service-sequence gate. Neither was a kernel
regression. Both gates were booting a **production** kernel.

`cargo` keeps one cached artifact per feature set and hardlinks the requested one into the single
path `target/aarch64-breenix-kernel/release/kernel-aarch64`. Switching profiles takes **0.06 s, with
no recompilation and no output worth reading** — the file's size changes and its mtime moves
*backwards*. `tests/kernel_no_neon_guard.rs` builds that kernel with **no features**, by design. So
running the structural suites silently replaces the boot_tests kernel with a production one, and
every gate that runs afterwards boots the wrong binary and fails on markers the kernel was never
asked to emit. The failures are indistinguishable from a real regression: they name a specific
oracle, they are perfectly reproducible, and they are 100% wrong.

The battery in §6 escaped it only by luck of ordering — its `--rebuild` full test happened to sit
between the suites and the service-sequence gate and rebuilt the boot_tests kernel. Move the full
test to the end, as any reasonable person would, and the whole run is garbage.

Fixed at source rather than by reordering a script. `require_boot_tests_kernel` is now a preflight in
`run-aarch64-service-sequence-gate.sh`, `run-aarch64-boot-test-strict.sh` and
`run-aarch64-full-test.sh`, sitting immediately after the existing `#528` no-neon guard and reading
the same way: it greps the kernel **binary** for a census of five boot_tests-only marker literals
(`[SCHED_STRAND_ORACLE:`, `[STRAND_INJECT_ORACLE:`, `[FUTEX_HANDOFF_ORACLE:`, `[CTX596_ORACLE:`,
`[BOOT_TESTS:` — each verified present in a boot_tests build and absent from a production build) and
exits 1 before booting anything if any is missing. Fifty boots of an attributable-looking false red
is worse than no run at all.

The guard corrected its own first draft: `[BLOCK_EINTR_ORACLE:` was in the initial census and the
guard immediately refused a known-good boot_tests kernel over it. It was right to — that marker is
emitted from **userspace** (`userspace/programs/src/block_eintr_oracle.rs`), lives in the ext2 image,
and was never in the kernel binary at all. It is not a kernel-profile discriminator and was removed.

This is the campaign's own binding lesson recurring: test wiring must be proven to EXECUTE in the
gate's actual feature profile. Here the wiring was correct and the *binary* was wrong.

## 6. The acceptance battery at the final HEAD

Re-run in the correct order (production build -> production-profile gate -> structural suites ->
`boot_tests` build -> QEMU gates), on one host in one session, against `e5d47c81`.

| step | result |
|---|---|
| production profile build | zero Breenix warnings |
| production-profile gate | **PASS** |
| eight structural suites | **251 green, 0 failed, zero warnings** (`strand_handoff` 15, `exec_lock_order` 34, `context_restore` 61, `teardown` 53, `block_request_lifetime` 11, `net_lock` 19, `loopback_pump` 57, `kernel_no_neon_guard` 1) |
| `boot_tests` profile build | zero Breenix warnings |
| `check-kernel-no-neon.sh` | **PASS** on both profiles |
| strict gate (kernel-merge gate), 6 boots | **6/6, 100% PASS** |
| service-sequence gate, 25 boots per profile | **PASS** |
| full-system test `--rebuild` | Phase 1 **106/106**, Phases 1b/1c/1d/1e all PASS, **Phase 2 fails on #593** |

The single warning in each build log is the toolchain's future-incompat note about `core` from the
build-std source, not kernel code.

**Service-sequence gate: 49/50 GREEN and the gate PASSES.**

```
575 0   576 1   DATA_ABORT 0   589 0   596 0   609 0   P5B 0   GREEN 49   UNATTRIBUTED 0
```

`max` was 25/25 with every bucket at zero. The one non-GREEN boot is `cortex-a72` boot 15, an
instruction abort whose field set is exactly `FAR=0x0 ELR=0x0 ESR=0x86000005` — #576's filed
signature, matched field-exactly by the round-2 classifier rather than by exception type. **No
`0x80000000` bucket, no #596-class data abort, no #575-shape stall, no #589, and nothing
UNATTRIBUTED.** The round-1 collateral is gone and this run required no #609 tolerance at all.

The strict gate reaching 6/6 is the first clean run of the kernel-merge gate in this campaign; note
that at #609's filed ~2-3% a six-boot run is clean about 85% of the time, so this is consistent with
#609 still being open rather than evidence it is fixed.

The full-system test's Phase 2 failure is **#593**, filed and pre-existing: init's aarch64 boot
script spawns no terminal, so "shell not detected" is unreachable-by-construction headless on this
architecture for any branch, `main` included. Everything before it passed, including Phase 1c —
#610's TOCTOU false red did not fire this run.

x86 was measured on beast at `993687d8`, and no kernel code changed between there and `e5d47c81`
(only aarch64 gate scripts, host-side structural tests and this document): `run-boot-parallel.sh 5`
twice gave 4/5 then 5/5, the one failure being `loopback_wake_test_child:15,loopback_wake_test:1` —
the pre-adjudicated #586 family, and the same signature `main` produced in the §5 differential. Zero
`sys_read` spin-hangs in either arm. Zero build warnings.

### 6a. Round-2 mac gate slot — 400-boot clean+starved battery at `33f68f52`

The mac gate slot's round-2 pass ran the full battery (not just the 25-boot service-sequence check
above) against `fix/589-deferred-requeue-drift` @ `33f68f52` — the doc-commit HEAD, byte-identical
kernel code to `e5d47c81`. This record was originally dropped when only the 25-boot PASS was folded
in; both results below belong beside it.

| step | result |
|---|---|
| `run-aarch64-full-test.sh --boot-tests-only --rebuild` | PASS, 106/106, oracle markers present |
| production-profile gate | PASS, 0 leaked `boot_tests`-only markers |
| service-sequence gate, 25/profile | **PASS — 50/50 GREEN**, every bucket zero |
| clean-gate, 100/profile (idle host) | **FAILED** — `575=0 576=1 DATA_ABORT=0 589=0 596=1 609=1 P5B=0 GREEN=196 UNATTRIBUTED=1` (out of 200; `589=0` throughout) |
| starved-gate, 100/profile (10× `yes` hogs, `nice -n 19`) | **FAILED** — `575=0 576=0 DATA_ABORT=0 589=0 596=0 609=0 P5B=0 GREEN=198 UNATTRIBUTED=2` (out of 200; `589=0` and `596=0` throughout) |
| strict gate, 3×20 | **60/60, 100% PASS** |
| Parallels, 3× fresh VM | **3/3 green**, 0 fault/abort/panic markers, all VMs stopped and verified |

The two FAILED gate runs are not a regression of this branch's own target buckets (`589` is 0/400
across both legs) but they are real, non-pre-adjudicated reds, none of which is waved:

* **Clean, max/boot 37 — bucket `596`** (now `612`): `[DATA_ABORT] FAR=0x292 ELR=0xffff00004040a52c
  ESR=0x96000021 DFSC=0x21 TTBR0=0x40200000 from_el0=0`. Filed as **#612** (§2 row, §7). Preserved:
  `preserved-serials-r2/clean100-max-boot37-596.txt`.
* **Clean, max/boot 17 — bucket `UNATTRIBUTED`**: two disagreeing INSTRUCTION_ABORT records in one
  serial, `far/elr/esr = 0x0 0x0 0x86000005 | 0x10 0x321c0508eb09039f 0x86000005`. Correctly
  UNATTRIBUTED by classifier design (a disagreement is never folded into a tolerated bucket). Filed as
  **#613** together with the starved/boot-64 occurrence below. Preserved:
  `preserved-serials-r2/clean100-max-boot17-UNATTRIBUTED.txt`.
* **Clean, cortex-a72/boot 88 — bucket `576`**: field-exact match to the filed #576 signature.
  Tolerated, no action.
* **Starved, max/boot 64 — bucket `UNATTRIBUTED`**: disagreeing INSTRUCTION_ABORT records,
  `far/elr/esr = 0x0 0x28 0x86000005 | 0xffff000054242320 0xffff000054242320 0x8600000e`. Filed as
  **#613** (same class as clean/boot 17). Preserved:
  `preserved-serials-r2/starved100-max-boot64-UNATTRIBUTED.txt`.
* **Starved, cortex-a72/boot 13 — bucket `UNATTRIBUTED`**: `oracle marker absent`, 45 s timeout. The
  serial shows `SUBSYSTEM:scheduler:early:COMPLETE:4/4` but `SUBSYSTEM:memory:early` never emits its
  own `COMPLETE` line, while `[SCHED_STRAND_ORACLE:...:stranded=0:...]` keeps sampling cleanly for the
  whole timeout — the same "census does not see it" shape #609 already documents for `network:early`,
  on a different subsystem. Noted as a comment against **#609** recommending the signature be widened
  to any subsystem kthread rather than filed under a fourth number. Preserved:
  `preserved-serials-r2/starved100-cortex-boot13-UNATTRIBUTED-timeout.txt`.

`609=1` in the clean leg is within its filed rate and the run-wide ceiling and is not itself a gate
failure. Total across the 400-boot battery: 394/400 GREEN (98.5%), zero `#589` recurrences, zero
classic `#576`-signature deviations beyond the one tolerated hit, one novel `612`-bucket `DATA_ABORT`,
three `UNATTRIBUTED` boots (two disagreeing-abort-record pairs, one starved-leg early-boot stall) —
all now attributed to a filed issue (#612 or #613) or noted against an existing one (#609), none
waved. `609` at `1/400` and the `612`/`UNATTRIBUTED` hits are all below the gate's own tolerance
ceilings and do not change the round's landing verdict; they are recorded here so the durable record
matches what the gate actually produced rather than only its cleanest 25-boot slice.

## 6b. The earlier acceptance battery, and what it uncovered

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
* **#605 / #607** — the already-consumed handoff slot and the outgoing-thread marker cleared without
  requeuing. `#596` itself is CLOSED (by #600) and must not be used as a bucket name for anything
  still open; the #576-signature aborts and the wild-FAR fault below are recorded as sharing this
  open family's region, not as proven instances of a single mechanism.
* **#612** — the wild-context EL1 `DATA_ABORT` near `schedule_deferred_requeue` (small-garbage FAR,
  garbage callee-saved register file, `ESR=0x96000021`). Filed from the §6 400-boot battery at 1/400,
  present on the resolver-only control (arm B) at 1/50. The service-sequence gate's `612` bucket
  (distinct from the `596` bucket, which is now the `CTX596_ORACLE` oracle only) keys to this issue.
* **Fix 1's independent necessity** is not separately measured (arm D isolates it but is confounded by
  the victim drive it was measured against). It is retained on design grounds and pinned structurally.
* **#609** — the `network:early` subsystem kthread never dispatched, ~2-3%, and blind to the strand
  census (`worst_dwell_ms=0`, roughly two threads examined per sample). No longer *unattributed*:
  coordinator ruling R30 pre-adjudicates it by field signature, the service-sequence classifier has a
  tightly-keyed arm for it, and the gate enforces a rate ceiling so the attribution cannot quietly
  grow. It did not occur at all in the final 50-boot run, which is unremarkable at its filed rate.
  The defect itself is untouched here, and the census blind spot it exposes qualifies every
  "stranded=0" claim in this document. A `memory:early`-stuck variant of the same class (1/200 in the
  §6 400-boot starved leg, same census blind spot) is noted as a comment on #609 with a recommendation
  to widen its signature to any subsystem kthread rather than fork a new issue per subsystem.
* **#610** — the `clonevm_exec_test` post-exec rendezvous race, a false red at roughly 1 in 4
  full-test runs; the first item for a follow-up slot, since fixing it rebuilds the ext2 image.
* **#593** — Phase 2 of the full-system test can never pass headless on aarch64. Until it is fixed,
  "the full test passes" is not an available claim on this architecture for any branch.
* **#611** — `cargo test` silently swaps the aarch64 gate kernel to a no-features build. Filed with
  the measured evidence. The three gates that pin `boot_tests` markers now refuse the result rather
  than booting it, which is the safe outcome and turns a fifty-boot false red into an immediate
  error, but the hazard itself is untouched: any other script that boots
  `target/aarch64-breenix-kernel/release/kernel-aarch64` expecting a particular feature profile is
  still exposed, and building before testing still leaves the wrong kernel on disk. The issue
  suggests giving the no-neon guard its own `--target-dir` so the shared artifact path is never
  disturbed — removing the hazard instead of detecting it.
