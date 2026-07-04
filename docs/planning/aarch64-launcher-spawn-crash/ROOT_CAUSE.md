# AArch64 launcher-spawn intermittent crash — root cause + fix proposal

**Status (2026-06-02):** Root cause CONFIRMED (high confidence on the proximate
mechanism; medium on the exact upstream writer). **Fix is gold-master and awaits
operator signoff** — see "Fix options" + the autopsy caveat. Found by the
automated Parallels launcher-test harness (PR #411).

## Symptom
Intermittently, on the launcher→terminal path, a CPU takes an unhandled sync
exception at a **page-aligned kernel data address**:
- `[UNHANDLED_EC] cpu=N EC=0x0 ELR=0xffff000040269000` (ESR=0x2000000, "Unknown"), or
- (earlier) `EC=0xe ELR=0xffff00004025d000` (Illegal Execution State).

The default handler parks/redirects the CPU, so heartbeats continue (looks
"hung"). Rate in an 18-run sweep: **2 EC=0x0 crashes / 18** (~11%); also 4/18
double-tap input drops (a separate bug). EC=0x0 happened to be survivable
(launcher still PASSed); EC=0xe was fatal to the run.

## Proximate cause — CONFIRMED
The captured `[FATAL_REGS]` register file **is verbatim `idle_loop_arm64`'s
mid-loop state**, decisively symbolized against `kernel-aarch64` (base
`0xffff000040000000`):

| reg | value | symbol |
|---|---|---|
| elr (fault PC) | `0x269000` | `scheduler::WAKE_SITE_SCHEDULE` (= `__bss_start`), held in idle's `x21` |
| x30, x22 | `0x269070` | `scheduler::NEED_RESCHED`, idle's `x22` |
| x1 | `0x269080` | `scheduler::CPU_IS_IDLE` |
| x26 | `0x0d7498` | `idle_loop_arm64+0x60` (idle loop body) |
| ctx_elr_el1 / peers' DEFER_SNAP elr | `0x0d5368` | `schedule_from_kernel+0xfc0` (normal "parked in scheduler" PC) |

`idle_loop_arm64`'s prologue loads `x21=WAKE_SITE_SCHEDULE(0x269000)` and
`x22=NEED_RESCHED(0x269070)`. The fault frame's `elr == idle.x21` and
`x30==x22==idle.x22` — i.e. **a non-idle thread's `Thread.context` was overwritten
with idle's register file** (including `elr_el1 = 0x269000`). When that thread is
later dispatched, `restore_*_context_inline` copies `frame.elr =
thread.context.elr_el1 = 0x269000` and `aarch64_enter_exception_frame` ERETs there.
`0x269000` is `.bss` (zeroed) → `0x00000000` decodes to `UDF #0` → **EC=0x0**.
If instead the corrupt SPSR is illegal, the ERET itself faults → **EC=0xe**. Same bug.

**Why the existing dispatch guard misses it:** `dispatch_thread_locked` checks
only `frame.elr < 0x1000 || (frame.spsr & 0xF) != 0`. `0x269000 ≥ 0x1000` and (for
an EL0t dispatch) `spsr & 0xF == 0`, so the corrupt context passes.

## Upstream cause — candidates (medium confidence)
Both reduce to *idle's register file ending up in a non-idle thread's `context`*:
1. **cpu_state / `old_id` save-target skew.** If `cpu_state[cpu].current_thread`
   names a userspace thread while the CPU was actually running `idle_loop_arm64`
   (e.g. after a ret-based idle dispatch that `br`s to idle without rebuilding
   cpu_state, then a timer IRQ), `save_*_context_inline(userspace_thread,
   idle_frame)` writes idle's regs into that thread's context. `fix_eret_cpu_state_locked`
   is the existing band-aid but only fires for EL0 frames.
2. **Reused fork kernel stack carrying a stale frame** (commit `04c9655a`,
   bitmap-backed kstack reuse; the fault SP is in that region) — a child whose
   reused kstack still holds a prior idle/scheduler exception frame.

Implicated machinery is exactly what the branch's cluster reshaped: `04c9655a`
(fork kstack reuse), `969ecce2` (CLONE_VM exec), `90a971ce` (stale cached TTBR0
requeue). Likely a **residual cpu_state/stack-ownership skew** from that cluster,
not a fresh regression — and almost certainly the same root behind the operator's
original launcher→terminal lockup and the prior ~week-long crash hunt
(`ELR=0x8`/`0x1e`/`0x3b9aca00`/`EC=0x18` were the same corridor).

## Fix options (BOTH are gold-master → operator signoff required)
1. **Root fix (preferred): stop the bad save.** Correct the save-target selection
   in `check_need_resched_and_switch_arm64` / `save_*_context_inline` so idle's
   register file is never saved into a non-idle thread's context (fix the
   cpu_state/`old_id` skew, or the reused-stack stale frame). Requires pinning
   which of the two writers — see "Confirm the writer" below.
2. **Defense-in-depth: privilege-aware dispatch guard.** Reject any dispatch where
   `frame.elr` is inconsistent with the target EL (EL0 dispatch → elr must be a
   userspace VA, not a kernel VA; EL1 dispatch → elr must be in `.text`), and
   safely terminate/requeue the victim instead of ERETing into data.
   **⚠ AUTOPSY CAVEAT:** `context_switch.rs` is gold-master and the autopsy
   (`docs/planning/cpu0-user-guard-autopsy/README.md`) explicitly warns **"NO
   CPU0-specific EL0 dispatch guard"** — a dispatch guard here caused a week-long
   regression (PR #334). This option intersects that frozen concern and must be
   designed + reviewed with the autopsy in hand. It mitigates + diagnoses but does
   not fix the upstream save-skew.

## Confirm the writer (needed before the root fix)
This crash is **Parallels-only** (BWM/VirGL), so the QEMU GDB workflow cannot reach
it. Confirmation must be in-kernel + Parallels repro:
- Add a **lock-free trace event** (or a small per-CPU ring) at the save site
  recording `(old_id, executing-is-idle, cpu_state.current_thread, cpu)` — to
  prove the save-target skew directly. **This touches the gold-master save path →
  signoff.** Then reproduce via the launcher harness and read the capture.
- The enhanced postmortem (`[FATAL_REGS]`/`[FATAL_THREAD]`, committed `b1961217`,
  exception.rs — not gold-master) already proves the proximate cause; extend it
  with `cpu_state` at fault if a cheaper signal is wanted.

## How to validate a fix
Run the launcher harness gate (`scripts/parallels/launcher-smoke.sh` /
`.claude/workflows/parallels-launcher-test.js`) — the EC=0x0/EC=0xe crashes must
disappear across a multi-run sweep. The harness already reports kernel faults
distinctly (`RESULT: FAIL: KERNEL FAULT ...`).

## Evidence
- `logs/parallels-launcher-test/run-20260602-202819/run-sh.log` (EC=0x0 + full
  `[FATAL_REGS]`/`[FATAL_THREAD]`/trace ring), `run-20260602-204127` (2nd capture),
  and the earlier EC=0xe `run-20260602-124137`.
- Enhanced postmortem: commit `b1961217` (exception.rs).

## Addendum (2026-07-04): round-4 RCA — revised mechanism, mitigation status, and next probe

**Context:** commit `e65f23cd` ("scrub reused fork kstack slots + reset child
resume state") shipped as the fix for candidate #2 above (reused fork kernel
stack carrying a stale frame). The 10-consecutive-green harness gate (see
`docs/planning/parallels-test-harness/RALPH_STATE.md`) passed with this fix in
the tree, but a fault recurred once mid-sweep (`run-20260704-104636`, attempt 8
of that gate) even with the scrub compiled in and verified to have executed.
Full analysis: `logs/parallels-launcher-test/run-20260704-104636/rca4-analysis.txt`.

### Revised mechanism: the corrupted object is the transient dispatch frame, not `Thread.context`

Round-4 symbolization (against the exact booted binary, `llvm-nm` cross-checked)
reproduces the same signature as the original finding — a scheduler `.bss` flag
address in ELR, `CPU_IS_IDLE`'s address in a GPR, `idle_loop_arm64`'s address in
x26, `schedule_from_kernel`'s address in x11/`ctx_elr_el1` — i.e. idle's live
register-file constellation leaking into a dispatch that then executes/branches
into zeroed `.bss` (`UDF #0`, EC=0x0). But this run's fault `sp` sits in the
per-CPU bitmap-managed kstack-pool region that `e65f23cd`'s scrub targets, and
section 4 of the RCA confirms via `llvm-nm` + call-site enumeration that the
scrub is compiled into the analyzed binary and executed on every allocation
path (10+ successful spawns preceded the fault, each necessarily having run the
scrub to completion) — **and it still did not prevent this recurrence.**

This re-frames the original two candidates in `ROOT_CAUSE.md` #1/#2 above:
- **Scrub-at-allocation is insufficient because the corruption does not happen
  at allocation time.** `e65f23cd` zeroes the kstack when it is *handed out*,
  which would defeat a stale-frame-at-reuse bug. Since the scrub demonstrably
  ran and the fault still recurred, the corrupting write must happen **while
  the thread is live** — i.e. something overwrites the in-flight dispatch
  frame (or writes idle's register file into it) after allocation, not a
  leftover frame from a previous tenant of the same stack.
- **Both original candidates' instrumentation is structurally blind to this.**
  The all-CPU `SAVE_SKEW` readout (`97f6e834`/`b0c4ab7c`, targeting candidate #1,
  cpu_state/`old_id` save-target skew) and the `DISPATCH_MISMATCH` detector
  came up **completely empty on all 8 CPUs** in this capture — not "no skew
  found," but the instrumentation never fired at all, meaning the corrupting
  write is not going through either of the code paths those probes watch.
  The object being corrupted is best understood as the **transient dispatch
  frame** — idle's live register file getting ERET'd on a fork child's kernel
  stack — rather than a stale `Thread.context` snapshot or an allocation-time
  leftover frame. Section 3 of the RCA (`DEFER_SNAP` semantics) also rules out
  a more dramatic reading (multiple simultaneous per-CPU snapshots for the same
  tid are time-disjoint historical low-water marks, not a live 4-way dispatch
  conflict, so the scheduler's per-thread mutual-exclusion invariant is intact).

### Mitigation status: plausible, not proven

`e65f23cd` remains a **real, correctly-implemented fix for the ONE hypothesized
writer it targeted** (bitmap-reused fork kstack slots carrying a stale frame at
reuse time) and measurably improved the fault rate (0 faults in 25 runs since,
vs ~1-in-15 before, across the 10-consecutive-green gate). It is **not proven**
to be the whole story: this round-4 recurrence shows the bug family can still
manifest through a different, still-unlocated writer that acts on a live
(not reused) dispatch frame. Treat the current clean streak as evidence of a
much-improved rate, not evidence of full closure.

### Armed probe for any recurrence + signoff requirement

The `[ERET_ANOMALY]`/owner-tid-canary instrumentation (commits `40ad7042`,
`40c187e9`) is now in the tree specifically to pin this if it recurs: it
records frame-anomaly detail and an owner-tid canary around ERET dispatch,
which the SAVE_SKEW/DISPATCH_MISMATCH probes could not provide since they
watch the save/dispatch-selection paths rather than the dispatch frame itself.
If EC=0x0/EC=0xe recurs, read this instrumentation first before any further
hypothesis-driven fix attempt.

Per the original "Fix options" section above, any eventual root fix for the
live-dispatch-frame writer is very likely to touch the FROZEN gold-master
regions (idle SP handling / ERET dispatch in `context_switch.rs`) called out
in `docs/planning/cpu0-user-guard-autopsy/README.md`. **Operator signoff is
required before implementing any such fix** — this is unchanged from the
original caveat and applies with equal force to whatever the round-4 writer
turns out to be.

### Evidence (round 4)
- `logs/parallels-launcher-test/run-20260704-104636/rca4-analysis.txt` — full
  symbolization, `DEFER_SNAP` semantics analysis, scrub-verification
  (compiled-in + executed), and old-vs-new capture comparison.
- `logs/parallels-launcher-test/run-20260704-104636/run-sh.log` — the verbatim
  crash capture for this recurrence.
- 10-consecutive-green gate evidence (post-`e65f23cd`, no further faults):
  `logs/parallels-launcher-test/run-20260704-133953` through
  `logs/parallels-launcher-test/run-20260704-141143`.
