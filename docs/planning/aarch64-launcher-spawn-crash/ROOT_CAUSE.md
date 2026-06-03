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
