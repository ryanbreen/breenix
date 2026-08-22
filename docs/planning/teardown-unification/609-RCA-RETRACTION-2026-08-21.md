# #609 — the proven mechanism, the fix, and two retractions (T3-G, rulings R34 / R35 / R37)

Branch `fix/609-early-kthread-dispatch`, off main `7fbee231`. This file is the durable in-repo record
for #609. It has been rewritten twice, and both rewrites are recorded here rather than deleted:

* under **R34** it replaced `609-RCA-FALSIFIED-2026-08-21.md` and retracted R33's "the RCA is
  falsified" verdict;
* under **R37** (this rewrite) it replaces the *surviving hypothesis* that R34 left standing — a
  failed peer-steal leaving a kthread "Ready-queued forever" — because the r2 confirm round refuted
  it with field evidence, and it removes the mask this file used to recommend.

**Status: #609 is root-caused and fixed on this branch.** Two distinct defects were found; only one of
them is the field failure, and both are repaired at source.

---

## 1. What is retracted, and why

### 1.1 R33's "falsified" verdict (retracted under R34)

R33's arm A (`--features force_609`) armed one CPU-0-pinned kthread per boot whose body incremented a
counter and returned. **Nothing joined it.** A lost dispatch is fatal only because something is joined
on it, so `[FORCE609:HITS=0]` was the only outcome that arm could report on a healthy kernel or a
broken one alike. Its 20/20 negative falsifies nothing. R32(a) required arm A to reproduce the
signature at ~100% before the mechanism could be closed; an arm that structurally cannot emit the
signature did not close it, and reading its silence as a falsification was the error.

### 1.2 The "failed peer-steal / Ready-queued-forever" hypothesis (retracted under R37)

R34 replaced the falsification with this chain: `least_loaded_cpu` ties to CPU 0 → CPU 0 cannot
dispatch during the boot-test window → *a peer steal fails or loses a race* → the kthread sits Ready
and queued forever.

The r2 confirm round measured the field failure directly — ~560 boots, 7 natural wedges (~1.3 %), 6
captured live under GDB — and **every** wedge has `qlen=[0,0,0,0]` on every sample. Nothing is
Ready-queued, so there is no steal to fail. The half about CPU 0 being undispatchable in the window is
real (§3), but it is not the mechanism of the field failure (§2). The steal-failure story is withdrawn.

### 1.3 The mask this file used to recommend (withdrawn under R37)

The R34 text said the fix "most plausibly lands as either a bounded wait/steal-retry in
`kthread_join`'s wait path, or a scheduling change that excludes CPU 0 from `least_loaded_cpu`
tie-break eligibility". The first of those is a **mask** by this campaign's own definition: it unsticks
a boot while leaving a preemptible lock spun on from an atomic context. It must not be cited from this
file again. `kthread_join` is untouched by the fix, there is no re-kick and no queue sweep anywhere in
the tree, and the only bounded join that exists is inside the `arm_a_609` test feature — the
instrument, compiled into nothing else.

---

## 2. The proven mechanism of the field failure: an orphaned `ARM64_STACK_BITMAP`

### 2.1 Corpus

| hunt | kernel | boots | wedges | capture |
|---|---|---|---|---|
| `t3g_r2_hunt2` | `crumb-609` | 142 | 1 (boot 86) | GDB + serial |
| `t3g_r2_ownerhunt` | `owner-609` | 84 | 1 (boot 26) | serial |
| `t3g_r2_gdbhunt` | `owner-609` | 132 | 2 (boots 41, 61) | GDB + serial |
| `t3g_r2_gdbhunt3` | `owner-609` | ~200 | 3 (boots 22, 189, 195) | GDB + serial + raw lock-byte read |

≈ 560 boots, 7 wedges ≈ **1.3 %**, inside the filed ~1–3 % band, all on
`-M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 4`, IOPS throttle 2000. Every wedged serial ends on
the filed #609 shape: `[STAGE:early:ADVANCE]` reached, `[STAGE:early:COMPLETE` never, no abort, no
panic, and the strand oracle still sampling and still reporting `stranded=0`.

### 2.2 The invariant, 6/6 GDB captures, no exceptions

* CPU 0 is stopped on the `isb` of a spin-mutex wait loop inside `allocate_kernel_stack`
  ← `kthread_run_with` ← `run_staged_tests`, CPSR `0x60000305` (**I=0, F=0** — interrupts enabled, its
  tick counter still advancing) under `preempt_disable`.
* The `ARM64_STACK_BITMAP` lock byte is **set with no CPU inside the critical section** — read straight
  out of memory as `0x01` at `0xffff00004085b400` on 3/3 captures where it was sampled (boots 22, 189,
  195); on the other three it is proven held by two CPUs polling it with nobody inside.
* `qlen=[0,0,0,0]` on every sample.

Boot 41 names a holder outright: `[KSTACKLOCK:site=2:owner_cpu=3:owner_tick=50:acq=8:rel=7]` —
acquired in `free_kernel_stack` on CPU 3 at tick 50, still unreleased ~12 000 ticks later, with CPU 3
by then asleep in `idle_loop_arm64`.

**The amplifier (3/6, not required).** In boots 41, 61 and 195 a peer *additionally* wedges itself on
the same lock from `idle_loop_arm64` with **all interrupts masked** (CPSR `0x…03c5`), freezing that
CPU's tick counter (247 / 231 / 1733 while peers pass 10 000) and removing it from the scheduler
permanently. Boots 22 and 189 have healthy peers and wedge anyway, so CPU 0's preempt-disabled spin on
an orphaned lock is sufficient on its own.

### 2.3 The chain, each link at file:line (as it stood pre-fix)

1. **A holder can be preempted.** `ARM64_STACK_BITMAP` (`memory/kernel_stack.rs:453`) was a bare
   `spin::Mutex`, taken with no interrupt masking and no preemption guard at both sites —
   `allocate_kernel_stack` (`:549`) and `free_kernel_stack` (`:638`). `free_kernel_stack` is reached
   with interrupts enabled from `KernelStack::drop` via `drop(reclaimed_threads)` in
   `scheduler::reclaim_terminated_threads` (`scheduler.rs:3815`), which sits **outside** that
   function's `without_interrupts` block.
2. **CPU 0 cannot rescue it.** `main_aarch64.rs:825` `preempt_disable()` runs before `init_scheduler()`
   and is balanced only at `main_aarch64.rs:186`, after `run_all_tests()`;
   `check_need_resched_and_switch_arm64` (`context_switch.rs:3687`) returns early whenever
   `preempt_count & PREEMPT_GUARD_MASK != 0`; and `kthread_join` (`task/kthread.rs:240-242`) spins on
   `arch_halt()` without yielding. CPU 0 therefore executes `schedule_deferred_requeue()` zero times
   for the whole window — and is itself spinning on the orphaned lock.
3. **Each idle peer that touches the reaper destroys itself.** `idle_loop_arm64` masks everything
   (`context_switch.rs:5165` `msr daifset, #0xf`) and then, still masked, called
   `idle_enter_scheduler_if_needed()` → `schedule_from_kernel()` →
   `reclaim_terminated_threads()` → `drop(reclaimed_threads)` → `KernelStack::drop` →
   `free_kernel_stack` → unbounded spin on the orphaned lock with D/A/I/F all set.
4. **Nothing to steal.** `qlen=[0,0,0,0]`: the holder is not Ready-queued, so `schedule_deferred_requeue`
   and `reclaim_unschedulable_cpu_queues` have nothing to act on.

Result: the lock is never released, `kthread_run` never returns, the remaining EarlyBoot subsystems
are never spawned — the filed #609 signature.

### 2.4 The link that is NOT closed

**Where the abandoned holder went, and why it is never re-dispatched, is not established.** Boots 22
and 189 have all CPUs ticking and entering the scheduler, `qlen=[0,0,0,0]`, and the lock held for 12 s
— so in those captures the holder is in a state no ready queue names, which leg 3 above does not
explain. Circumstantially this points at the deferred-requeue / wrong-PC-resume family closed as
#596/#600 (on several captures `SCHEDVIEW cur[i]` names a thread while that CPU's PC is in
`idle_loop_arm64`), but that is **not proven and is not asserted**. Transferred to **#607** with the
boot-41 holder capture and the 22/189 healthy-peer captures (comment posted 2026-08-21T21:08Z).

The fix is sound regardless — with interrupts masked and preemption implied, the critical section
cannot be left by any scheduling event, so the orphan cannot form — but if the holder disappears for
some *other* reason, the same underlying defect will strand something else. That is why #607 stays
open and why this file says so.

---

## 3. The second defect: placement onto a CPU that cannot dispatch (arm A)

Independent of §2, and proven by its own oracle:

`Scheduler::add_thread_inner` routes new threads with `least_loaded_cpu()`, and the wakeup path uses
the structurally identical `find_target_cpu_for_wakeup()`. Both are
`(0..online_cpu_count()).filter(cpu_accepts_wakeups).min_by_key(queue len).unwrap_or(current_cpu)`, and
`min_by_key` returns the first minimum, so an all-empty tie resolves to CPU 0. The filter that exists
to prevent that was defeated by its own fast path (`scheduler.rs:1315-1330`, pre-fix):

```rust
if cpu >= online_cpus { return false; }
if cpu == current_cpu { return true; }          // <-- the defect
let last_schedule_ticks = self.cpu_state[cpu].last_schedule_ticks;
crate::time::get_ticks().wrapping_sub(last_schedule_ticks) <= CPU_STALL_TICKS
```

A CPU running with preemption disabled asserted that it accepts wakeups it provably cannot service,
and then won the placement tie. The staleness test on the next line answers correctly for CPU 0 in the
window — it is simply never reached.

Arm A is the oracle for this, and it is a measurement rather than a foregone conclusion because of its
in-boot control leg:

| kernel | leg | boots | LOST | GREEN | `lega_cpu` |
|---|---|---|---|---|---|
| unfixed `9462bfc6` | unstarved | 12 | **12** | 0 | 0 |
| unfixed `9462bfc6` | starved (10 hogs `nice -n 19`) | 16 | **16** | 0 | 0 |
| fixed `4a9a4e51` | unstarved | 12 | 0 | **12** | 1 |
| fixed `4a9a4e51` | starved | 16 | 0 | **16** | 1 |
| fixed + mutation M4 | unstarved | 8 | **8** | 0 | 0 |

28/28 red before, 28/28 green after, 8/8 red again the moment the defect is reintroduced. Leg B
(`[ARMA609:LEGB:...joined=1]`) is green on all 64 boots in every direction, so `joined=0` is a
measurement and not an instrument failure. `[ARMA609:LATE:lega_body_ran=1]` on every armed red boot
proves the thread itself was fine: it runs to completion the moment `launch_init_from_elf` calls
`preempt_enable()`. The loss is placement inside a window.

---

## 4. The fix — three legs, all at source

1. **`ARM64_STACK_BITMAP` becomes a real spinlock** (`memory/kernel_stack.rs`): an `IrqSafeMutex`
   guard *type*, not a discipline a future third call site can forget — mask → acquire, release →
   restore. Both critical sections stay short (the bitmap scan only, with `drop(bitmap)` before the
   live-slot check, the 16 KiB scrub and the mapping; the free side is a three-instruction bit clear).
2. **Reclamation is hoisted out of the idle loop's all-masked window**
   (`arch_impl/aarch64/context_switch.rs`): `run_deferred_reclamation()` is the first statement of the
   idle-loop body, strictly before `msr daifset, #0xf`, exactly once per iteration; the documented
   `daifset → gate → dsb sy → wfi → daifclr` sequence is byte-identical to before. Every external
   `schedule_from_kernel` caller (`waitqueue.rs`, `completion.rs`, `scheduler::schedule`) now calls it
   immediately beforehand, at the same point in the sequence the old callee used, and
   `every_external_schedule_from_kernel_call_reclaims_immediately_beforehand` makes that a census
   equality rather than three literals.
3. **`cpu_accepts_wakeups` asks whether this CPU can actually reach the scheduler**
   (`task/scheduler.rs`): the unconditional `cpu == current_cpu` fast path now also requires
   `arch_can_dispatch_here()` (aarch64: `preempt_count & PREEMPT_GUARD_MASK == 0`, the same predicate
   `check_need_resched_and_switch_arm64` gates on; x86 keeps today's behaviour, ratcheted), and
   otherwise falls through to the same `last_schedule_ticks` staleness test every peer is judged by.
   Blast radius is bounded: with `CPU_STALL_TICKS = 20`, an ordinary preempt-disabled syscall on a
   healthy CPU still wins local placement; only a CPU that is both non-dispatchable *and* >20 ms
   without a scheduler entry is refused.

**Explicitly not the fix, and not in the tree:** no timeout or retry in `kthread_join`, no watchdog
re-kick of a CPU's queue, no "if the queue looks stuck, migrate it" sweep, no lock-free allocate-
elsewhere retry. `retain_cpu_affine_test_thread` and the affinity pin are test-only scaffolding and
were not weakened in either direction.

### Deviations from the confirm design

* **D1 (structural, load-bearing) — arm A pins the thread WHERE PLACEMENT PUT IT, not to CPU 0.** The
  confirm slot's arm forced placement via `kthread_run_on_cpu_for_test(.., 0)`, which bypasses
  `least_loaded_cpu` / `cpu_accepts_wakeups` entirely; it would have stayed red after a correct fix,
  and the only way to green it would have been to weaken the pin — which the design forbids. Rebuilt:
  leg A is published through the unmodified `add_thread` → `least_loaded_cpu` path and pinned only
  *afterwards*, to whichever CPU placement chose (`spawn_pinned_where_placed_for_test`, symmetric,
  never naming CPU 0). The arm removes the peer rescue that hides bad placement and asks exactly one
  question: can the CPU placement selected actually dispatch this thread?
* **D2 — the Ready-nonprogress axis is derived from the owning CPU's silence, not a per-thread
  `ready_since_ticks` stamp.** The design asked for a stamp at every site that pushes a tid into
  `per_cpu_queues[..]`; there are **23** such sites in `scheduler.rs`, several inside the
  context-switch save/steal paths — a hot-path write in a Tier-2 file for a diagnostic axis.
  `worst_queued_nondispatch_ms` is instead computed inside the census as
  `now - cpu_state[owning_cpu].last_schedule_ticks` for every queued `Ready` thread: same class, zero
  new write sites, and a lower bound rather than an estimate. The limitation is in the field's doc
  comment.

### Mutations (each alone, tree restored byte-identically afterwards)

| # | mutation | oracle | result |
|---|---|---|---|
| M1 | `ARM64_STACK_BITMAP` back to a bare `spin::Mutex` | `teardown_structure` | RED — `arm64_stack_bitmap_is_irqsafe_by_type_and_release_order` |
| M2 | one `PENDING_PROCESS_RECLAIMS` back to a bare `spin::Mutex` | `teardown_structure` | RED — `drop_body_static_locks_are_irqsafe_by_derived_census` named the static it *found* from the Drop body |
| M3 | reclamation moved back inside `schedule_from_kernel` | `teardown_structure` | RED — `aarch64_reclamation_stays_outside_the_masked_scheduler_window` + `phase_one_retirement_fence_and_lock_domains_are_structural` |
| M4 | unconditional `cpu == current_cpu` restored | `strand_handoff_structure` + **arm A** | RED — `wakeup_placement_requires_local_dispatchability`, and arm A **8/8 LOST** |

| # | census mutation | oracle line | verdict |
|---|---|---|---|
| CM1 | `queued_on_nondispatching_cpu` never accumulates | `queued_nondispatching=0` | FAIL |
| CM2 | `worst_queued_nondispatch_ms` never accumulates | `queued_nondispatch_ms=0` | FAIL |
| CM3 | no tids written to `nonprogress_out` | `armed_reported=0` | FAIL |
| CM4 | `worst_cpu_scheduler_silence_ms` never accumulates | `cpu_silence_ms=0` | FAIL |
| CM5 | queued scan moved back **after** the reachability `continue` | `queued_nondispatching=0:queued_nondispatch_ms=0:armed_reported=0` | FAIL |
| CM6 | the oracle arms nothing | `baseline_reported=1:armed_reported=0` | FAIL (anti-vacuity) |

M4 and CM5 are the load-bearing ones: M4's oracle is a booted kernel, and CM5 restores exactly the
"queued means clean" blind spot the widening exists to remove.

---

## 5. The census widening — what it actually changed, in one unit

The widening removed the injection mechanism entirely (`CENSUS_WIDEN_INJECT_TID` and every
`!injected &&` are gone; the oracle now arms a **real** kthread on a CPU the scheduler itself reports
stale and lets the unmodified predicates classify it), moved the queued scan **before** the
reachability `continue`, replaced the "is any CPU's idle thread" skip with a *dormancy* test, and
added three named axes (`queued_on_nondispatching_cpu`, `worst_queued_nondispatch_ms`,
`worst_cpu_scheduler_silence_ms` with `worst_silence_cpu`).

**Correction (R37).** The claim that census coverage went "from ~2/sample to ~500/boot", posted on
#609 on 2026-08-22T01:35 and repeated in the r2 implementation notes, is **withdrawn: it compares two
different units** (threads per sample against threads per boot) and overstates the gain by orders of
magnitude. Re-counted from the serials, in one unit:

| serial | `samples` | `checked` | checked/sample |
|---|---|---|---|
| r1's preserved #609 wedge (`preserved-serials/clean100-cortex-a72-609-serial-8.txt`, pre-widening) | 803 | 2427 | **3.02** |
| post-widening healthy boot (`/tmp/breenix_aarch64_full_test/serial.txt`) | 201 | 754 | **3.75** |

`checked` counts Running/Ready threads that survive the idle-dormancy filter (`scheduler.rs`,
`checked += 1` immediately after that filter), so the per-sample figure also moves with how many
threads exist at sample time and which boot phase the samples fall in — it is not a clean coverage
multiplier either. **The durable claim is structural, not numeric:** queued threads are now examined
instead of being skipped as reachable (CM5 turns the oracle red when that is undone), a registered
idle thread is only skipped when its CPU is genuinely dormant, and three axes exist that did not
before. The widening still cannot see a *completed* #609 wedge — the CPU genuinely is parked when the
loss completes — and that limitation is disclosed here, not papered over.

---

## 6. Acceptance

### 6.1 The instrument

**`clean609 = 0/300`** — `--expect clean` on the arm-A runner: 100 GREEN on `max`, 100 GREEN on
`cortex-a72`, and 100 GREEN on the R37 starved `cortex-a72` re-run; armed 100/100 each, `lega_cpu=1`
on every boot. Verdict `clean` requires
`GREEN=boots && armed=boots && LOST=0 && CONTROL_FAIL=0 && CRASH:UNATTRIBUTED=0 && ORACLE_FAIL=0 &&
OTHER=0`, and a class census that does not sum to the boot count is a FATAL exit.

Field-leg closure is statistical and adequately powered: the wedge hunt measured ~1.3 % over ~560
boots on the same setup; 500+ post-fix boots across the clean, starved, service-sequence and strict
gates produced zero. P(0 wedges | p = 0.013) ≈ 0.002.

### 6.2 Battery at the final tree

| gate | result |
|---|---|
| structural suites, re-run at the R37 tree (R37): teardown 57, context_restore 61, loopback_pump 57, exec_lock_order 34, strand_handoff 29, net_lock 19, block_request_lifetime 12, serial_line_atomicity 9, exit_tally 6, dma_and_log_sink 4, signal_eintr_predicate 2, kernel_no_neon_guard 1 | **291 passed, 0 failed** |
| aarch64 production / `boot_tests` / `arm_a_609` builds, each forced (`aarch64-breenix-kernel.json` only) | zero repository warnings; `check-kernel-no-neon.sh` PASS on each |
| x86 `testing,external_test_bins` and `boot_tests,testing,external_test_bins`, each forced | zero warnings — the second **only after R37/B3**: `kthread_has_exited_for_test` had been widened from `#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]` to `#[cfg(feature = "boot_tests")]` while every call site stayed aarch64-gated, so the x86 custody gate's own build carried a dead-code warning. The arch term is restored, with the reason in the doc comment. |
| `run-aarch64-boot-test-strict.sh 6` (r2) | 6/6 |
| **`run-aarch64-boot-test-strict.sh 20` (R37 re-leg)** | **20/20 SUCCESS** |
| `run-aarch64-boot-test-strict.sh 20` (r2 run 2) | 19/20 — one `FUTEX_HANDOFF` red, now **#627** |
| `run-aarch64-prod-profile-boot-test.sh` (r2) | PASS — futex seam absent, 0 crash markers |
| service-sequence gate `--boots 25 --profile both` (r2) | `609=0/50`, `BOOT_TEST_FAIL=0/50`, every other bucket 0, GREEN 49/50; the one red is **#626**, attributed by field signature and not tolerated |
| **service-sequence gate `--boots 25 --profile both` (R37 re-leg)** | see §6.4 |
| arm A `--expect clean --boots 100 --profile cortex-a72 --starved` (r2) | 99 GREEN / 1 `ORACLE_FAIL` — the kernel-stack ownership red, now **#628** |
| **arm A `--expect clean --boots 100 --profile cortex-a72 --starved` (R37 re-run)** | **100/100 GREEN, verdict PASS**, `slot_balance=0` on all 100 |
| x86 frame-custody gate (beast, r2) | 8/8 PASS across 2x + 3-batch + strict 3x, with `[CENSUS_WIDEN_ORACLE:x86:arm=none:reason=uniprocessor_no_dispatching_peer:...:SKIP]` pinned literally |
| x86 `run-boot-parallel.sh 5` floor gate (beast, r2 + R37 A/B) | see §6.3 |

### 6.3 The x86 floor-gate leg, adjudicated against a control (R37/B4)

The r2 slot's floor-gate leg came back 2/5 with three boots carrying
`[TEST:userspace:loopback_recv_wake:FAIL:reader_exit_15]` +
`TEST_TALLY: … failed=[loopback_wake_test_child:15,loopback_wake_test:1]`, and declined to attribute
it. R37 ran the control the r1/r2 rounds never took — alternating 5-boot rounds between the r1-era
commit `8bcc1999` and branch HEAD `d28407ae` on the same beast container, rebuilding the boot image at
every switch. Results are in §6.5; the shape occurs **at the control**, at a rate that is not lower
than HEAD's, so it is not this branch's. It is field-exact to the x86 occurrence already recorded on
**#586** (2/5 on this same runner at `c79124b8`, before this branch existed): same marker string, same
`TEST_TALLY` failure pair, same `exit(15)` watchdog in
`userspace/programs/src/loopback_wake_test.rs:139-140` firing when `eof_wait_ms > EOF_WAKE_BOUND_MS`.
It is **not** #567 (that issue is ring-0 context corruption on kthread resume, a different signature
entirely) and not #545 (closed). Attribution: **#586, at a comparable rate, pre-existing.**

### 6.4 R37 re-leg results

* strict `20`: **20/20 SUCCESS** (171 s).
* arm-A starved re-run, 100 boots: **100/100 GREEN**, `ORACLE_FAIL=0`, `LOST=0`, armed 100/100,
  `slot_balance=0` on every boot.
* service-sequence gate 25/profile: **PASSED — 50/50 GREEN, every bucket 0** (`609=0`,
  `BOOT_TEST_FAIL=0`, `575=0`, `576=0`, `596=0`, `612=0`, `P5B=0`, `CLONE_EXEC=0`, `STRAND=0`,
  `DATA_ABORT=0`, `UNATTRIBUTED=0`).

### 6.5 The x86 A/B, boot by boot

Alternating 5-boot rounds on the same host, image rebuilt at every switch (control first each round,
so host-load drift is shared):

| round | control `8bcc1999` | HEAD `d28407ae` |
|---|---|---|
| 1 | 5/5 | 5/5 |
| 2 | 4/5 — `loopback_wake_test_child:15` (#586) | 5/5 |
| 3 | 4/5 — `loopback_wake_test_child:15` (#586) | 5/5 |
| 4 | 5/5 | 5/5 |
| 5 | 3/5 — 2 × "USERSPACE TEST COMPLETE was absent; boot did not finish" (**#630**) | 4/5 — `loopback_wake_test_child:15` (#586) |
| 6 | 5/5 | 5/5 |
| **A/B total** | **26/30** — 2 × #586, 2 × #630 | **29/30** — 1 × #586 |

Then six further **control-only** rounds (30 boots) with per-round serial preservation: 29/30, one more
#586 boot, serial preserved in-repo as
`609-serials/x86-586-loopback-reader-exit15-control-8bcc1999.txt`.

With the earlier single runs folded in — control 5/5 (r1) and 3/5 (R37's first control round: one #586
boot and one `clock_gettime_test:1` boot, **#631**), HEAD 2/5 (r2, 3 × #586) — the pooled counts are:

| commit | boots | #586 (`reader_exit_15`) | #630 (boot did not finish) | #631 (`clock_gettime_test:1`) |
|---|---|---|---|---|
| control `8bcc1999` | 70 | 4 (5.7 %) | 2 | 1 |
| HEAD `d28407ae` | 35 | 4 (11.4 %) | 0 | 0 |

Fisher exact on the #586 counts (4/70 vs 4/35): p ≈ 0.44 — not distinguishable — and the only two
shapes that appear on one side appear on the **control** side. Nothing here is attributable to this
branch. Adjudication posted on #586 (2026-08-22).

---

## 7. By-catch filed this campaign round

Every red this round is named by its own field signature; none rides an existing bucket.

* **#622** — `[DATA_ABORT] FAR=0x200 ELR=0xffff0000404b02e4 ESR=0x96000005 DFSC=0x5 from_el0=0`. Not
  the filed #612 signature (`FAR=0x292 ESR=0x96000021`); it landed in the #612 bucket only because
  that classifier arm was a catch-all, which is now removed (below).
* **#623** — `[INSTRUCTION_ABORT] FAR=0x18000 ELR=0x18000 ESR=0x86000005 IFSC=0x5 from_el0=0`. Not
  #576's null-page shape.
* **#624** — starved-profile boot times out with `STRAND_INJECT_ORACLE` / `CENSUS_WIDEN_ORACLE` never
  printed.
* **#625** — `[PC_ALIGN] ELR=0x4b5 FAR=0x5 from_el0=1` then `KERNEL PANIC … LayoutError` in
  `linked_list_allocator`. Serial preserved in `609-serials/`.
* **#626** — `[INSTRUCTION_ABORT] FAR=0x0 ELR=0x0 ESR=0x8600000d IFSC=0xd` — a *permission* fault at
  VA 0, distinct from #576's translation fault (`IFSC=0x5`). 1/50 on `max` at HEAD, 0/25 on the
  pre-fix control (underpowered); the same field set was already classified UNATTRIBUTED in
  `589-ROUND2-PARTITION-2026-08-20.md` before this branch existed. Not attributed to this round, not
  excused by it, not tolerated. Serial preserved in `609-serials/`.
* **#627** (R37) — `FUTEX_HANDOFF` stage 3 reports `stage3_elapsed_ok=0:stage3_elapsed_ms=49`, reddening
  the strict gate. Root-caused in the issue to the oracle's elapsed anchor (`record_arm`, stamped
  after the deadline sample the wait path actually compares against), and the identical shape exists
  on a pre-branch serial. Serial preserved in `609-serials/`.
* **#628** (R37) — `kernel_stack_ownership_oracle` `slot_alloc_delta=1000 slot_free_delta=1001
  slot_balance=-1` on 1/452 post-fix boots. Verdict: **window artifact, not an over-free** — the
  oracle's equality is measured over *global* pool counters, so a pooled stack allocated before the
  window and dropped inside it reads as an imbalance; `two_owner=0`, `drop_refused_live=0` and
  `live_refusals_production=0` exclude a double custody, and the identical arithmetic was already
  root-caused to a concurrent real kthread at `c9c4322b`. The R37 re-run of the identical starved leg
  is 100/100 clean. Serial preserved in `609-serials/`.
* **#629** (R37) — x86 `Scheduler::online_cpu_count()` returns `MAX_CPUS` regardless of `-smp`, so a
  uniprocessor boot carries seven phantom, permanently dispatch-stale CPUs. This is the production
  placement defect that hung the x86 census arm; the branch works around it (aarch64-only real-thread
  arm, explicit `arm=none:…:SKIP` marker on x86) and does not fix it.
* **#630** (R37) — x86 `run-boot-parallel.sh` boots that never reach `USERSPACE TEST COMPLETE`,
  2/30 at the r1-era control commit and 0/30 at HEAD.
* **#631** (R37) — x86 `clock_gettime_test` exits 1, 1/35 at the r1-era control commit.

### The #612 catch-all is retired

`classify_serial`'s DATA_ABORT arm used to match **any** `[DATA_ABORT] … from_el0=0` and bucket it as
`612`. It now takes the union of the `[DATA_ABORT]` header and `[FATAL_REGS]` records, requires a
single-element set, buckets `612` only on `FAR=0x292 ESR=0x96000021`, and sends everything else —
#622's shape included — to `UNATTRIBUTED`, a hard FAIL. `[PC_ALIGN]` and `KERNEL PANIC` got the same
distinct-set treatment (#625, #626 named, still hard failures).

---

## 8. Gate posture

The #609 run-wide rate ceiling stays deleted, and `run-aarch64-service-sequence-gate.sh` now records
the true reason: not "the mechanism was falsified" and not "the class never occurred on main" — both
retracted — but that the defect is root-caused and fixed, so there is no rate left to tolerate. Every
occurrence fails the profile it happened in via `count_609`, and the serial is preserved.

The detector is keyed on the **stage boundary** (`[STAGE:early:ADVANCE]` present,
`[STAGE:early:COMPLETE` absent, no crash, no strand) rather than on which subsystem finished first, so
a wedge that lands before any `memory:early` line — as the `t3g_r2_hunt2` boot-86 capture did — is
attributed rather than dropped into UNATTRIBUTED. Bucket `609` is therefore a *shape*: a future
different early-stage stall will be reported under #609's name until it is examined. Nothing hides —
the bucket is a hard FAIL and `CLASS_REASON` carries the census line.

Both the strict gate's `score_serial` and the service-sequence gate's `classify_serial` now consult
`[BOOT_TESTS:FAIL` / `[TESTS_COMPLETE:…:FAILED:n]`, with a named `BOOT_TEST_FAIL` bucket and a
discovered-gate ratchet. That hole was real: a strict run scored **6/6 SUCCESS** while two of those six
serials carried `[BOOT_TESTS:FAIL:1]`.

---

## 9. Landmines for whoever works here next

* **A concurrent `cargo build` swaps the kernel out from under a running gate.** `cargo` hardlinks the
  requested feature-set artifact into the single output path
  `target/aarch64-breenix-kernel/release/kernel-aarch64` in a fraction of a second; the runner's
  `require_boot_tests_kernel` guard runs once, at start, and cannot see it. This produced 14 false
  `CONTROL_FAIL` boots in r2 (run killed, boots discarded, disclosed as I1). **Never build while a
  gate is booting** — and note that any `cargo test` rebuilds the kernel *without* `boot_tests`.
* **`BOOT_CRUMB` is not a position marker.** It is one global relaxed store; on boots where GDB proves
  CPU 0 is inside `allocate_kernel_stack`, the serial still reads the crumb from before the call.
  Localise from GDB, not from the crumb.
* **The `KSTACK_*` owner statics have two blind windows** (between the `stxrb` that takes the lock byte
  and the counter bump, and between the counter bump and the release store), which is why the owner is
  identified on only 1 of 7 wedges. Read the lock byte directly — that is the only unambiguous "is it
  held" test.
* **The Drop-lock census is one level deep and would not have caught this bug.**
  `drop_body_static_locks_are_irqsafe_by_derived_census` follows a direct `STATIC.lock()` in a `Drop`
  body; `KernelStack::drop` reaches `ARM64_STACK_BITMAP.lock()` *through* `aarch64::free_kernel_stack`.
  It is non-vacuous (M2 found `PENDING_PROCESS_RECLAIMS` from a real Drop body) and its limitation is
  in its own doc comment, but **class prevention rests on the reclamation-hoist ratchet plus the
  module-anchored bitmap type test, not on this census.**
* **The new census axes are diagnostic only.** No gate keys on `nonprogress`,
  `queued_on_nondispatching_cpu` or `worst_cpu_scheduler_silence_ms`, and the census oracle's own probe
  pins `queued_on_nondispatching_cpu=1` / `worst_queued_nondispatch_ms ≈ 1.2 s` into the boot-wide max
  on every boot, so a later genuine occurrence is not separable in that marker.
* From the R33 record, unchanged: `Scheduler::new`'s `EMPTY_STATE` uses `idle_thread: 0` as "not yet
  registered" and `0` is a real thread id, so `is_idle_thread_inner(0)` is true for every CPU that has
  not registered its idle thread — the sentinel has to become `u64::MAX` as part of any double-role
  fix; after the boot tests, `launch_init_from_elf` makes init CPU 0's current thread, so a bootstrap
  thread promoted to an ordinary `Running` kernel thread becomes a strand candidate on every boot
  unless the fix retires it explicitly; and `smp.rs` already initialises secondary idle threads'
  `context.elr_el1`, so the `[boot] Reset N idle thread contexts` guard looks dead for them — prove
  that with a non-mutating audit marker rather than assuming it.
* **A latent affinity bug found while reading a gate red:** `retain_cpu_affine_test_thread` scanned
  `BOOT_TEST_CPU_AFFINITY` with `position(|slot| slot.load() == thread_id)`; the table is zeros when
  nothing is pinned, so `thread_id == 0` matched slot 0 and reported "pinned to CPU 0" for the
  bootstrap thread on every steal — the #609 loss shape, manufactured by test scaffolding. Now refused
  explicitly, with a ratchet.
