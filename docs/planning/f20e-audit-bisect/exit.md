# F20e Exit

## Verdict

PARTIAL / BLOCKED.

Phase 1 reproduced the `PER_CPU_IDLE_AUDIT` side effect. Phase 2 isolated the load-bearing diagnostic operation to the audit's repeated atomic RMW on the one-shot flag. Phase 3 did not produce a production change that satisfies the full acceptance gates.

No PR was opened or merged.

## What the original ask was

Restore the F20d idle audit with idle-loop timer reprogramming removed, bisect which audit operation makes CPU 0 timer delivery work, then replace the intrusive audit with a minimal production fix that preserves CPU 0 timer ticks without regressing AHCI/userspace boot.

## What I changed

- `diagnostic/f20e-audit-bisect`
  - Cherry-picked `c676dcff` (`db6ea3ac` on this branch).
  - Removed idle-loop `rearm_timer()` while leaving the audit present (`0fe3ee45`).
  - Added `scripts/f20e/run-sweep.sh` for repeatable Parallels sweeps.
  - Committed one variant per bisect step through `cec57779`.
- `fix/f20e-atomic-rmw`
  - Current production candidate is Linux-parity `dsb sy` immediately before idle `wfi`, with idle-loop timer reprogramming removed (`bff1d92a`).

## Baseline Reproduction

All five baseline runs reproduced the audit side effect: CPU 0 timer ticks advanced, but the intrusive audit still disrupted userspace/AHCI.

| Run | host_cpu_avg | boot_script_completed | timer_tick_count | post_wfi_count |
| --- | ---: | ---: | ---: | ---: |
| 1 | 79.1 | 0 | 29867 | 989089 |
| 2 | 85.4 | 0 | 29898 | 990627 |
| 3 | 85.8 | 0 | 29886 | 989964 |
| 4 | 88.9 | 0 | 29806 | 962581 |
| 5 | 103.1 | 0 | 29693 | 969773 |

## Bisect Table

| Step | Variant | Result | Verdict |
| --- | --- | --- | --- |
| D-equivalent | Relax audit print-lock release store from `Release` to `Relaxed` | `timer_tick_count=29901`, `post_wfi_count=995442`, `boot_script_completed=0` | Not load-bearing |
| B-equivalent | Relax audit print-lock acquire `compare_exchange` from `AcqRel/Acquire` to `Relaxed/Relaxed` | `timer_tick_count=29894`, `post_wfi_count=990366`, `boot_script_completed=0` | Not load-bearing |
| C | Remove raw UART writes, keep register reads, GICR reads, print lock, DAIF mask/restore | `timer_tick_count=29689`, `post_wfi_count=960093`, `boot_script_completed=0` | Not load-bearing |
| A | Remove audit pre-print system-register `mrs` reads, print zero placeholders | `timer_tick_count=29897`, `post_wfi_count=994235`, `boot_script_completed=0` | Not load-bearing |
| Atomic flag | Replace one-shot `compare_exchange` with `load(Relaxed)` + `store(Relaxed)` | `timer_tick_count=0`, `post_wfi_count=0`, `boot_script_completed=0` | Load-bearing area found |
| Precision split | Restore `compare_exchange(false, true, Relaxed/Relaxed)` | `timer_tick_count=29888`, `post_wfi_count=987954`, `boot_script_completed=0` | The RMW instruction, not AcqRel ordering, is load-bearing |

## Load-Bearing Operation

The load-bearing operation is the audit flag's repeated atomic RMW attempt:

```rust
done[cpu_id].compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
```

The precision split shows relaxed ordering is sufficient. Replacing the RMW with separate relaxed load/store collapses CPU 0 timer delivery back to zero.

## Production Attempts

| Candidate | Evidence | Outcome |
| --- | --- | --- |
| One-time RMW from early idle path | Run reached timer activity but failed boot/AHCI (`boot_script_completed=0`) | Rejected |
| Per-idle RMW on all CPUs, enabled at init launch | Boot completed and CPU0 trace reached `tick_count=173`, but `host_cpu_avg=348.6` | Rejected: host CPU gate failed |
| Per-idle RMW on CPU 0 only | Stalled after init start (`boot_script_completed=0`) | Rejected |
| Late one-shot RMW on all CPUs | Stalled during pre-timer init preload in sampled run | Rejected / inconclusive |
| `dsb sy` before idle `wfi` | Boot completed; CPU0 trace reached `tick_count=937`; `host_cpu_avg=218.7` | Best candidate, but still fails host CPU gate |

## Linux Cite

Linux ARM64 idle executes a full-system barrier immediately before WFI: `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:23` defines `cpu_do_idle()`, with `dsb(sy)` at line 29 and `wfi()` at line 30.

Linux's ARM arch timer next-event path writes CVAL then CTRL in `/tmp/linux-v6.8/drivers/clocksource/arm_arch_timer.c:741`, specifically CVAL at line 756 and CTRL at line 757.

## What I did not complete

- No production fix passed the full acceptance criteria.
- No 5/5 final sweep was run after the single-run gate failures.
- No PR was opened or merged.

## Known Risks and Gaps

- The sweep summary script parses F20d end-audit arrays for `timer_tick_count`; production branches do not emit those arrays. For production runs I verified CPU0 timer delivery from trace lines instead.
- The best production candidate (`dsb sy` before WFI) restores CPU0 timer trace and boot completion in a single run but exceeds the host CPU threshold.
- The atomic RMW side effect is real but is not yet a clean production mechanism.

## Verification Commands

```bash
# Diagnostic baseline and bisect artifacts
git log --oneline diagnostic/f20e-audit-bisect -8

# Production build
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64

# Single-run production gate used for the final candidate
bash <(git show diagnostic/f20e-audit-bisect:scripts/f20e/run-sweep.sh) \
  1 .factory-runs/f20e-audit-bisect-20260417/production-dsb-gate

# CPU0 timer evidence from the final candidate
rg "\\[TRACE\\] CPU0 .*TIMER_TICK" \
  .factory-runs/f20e-audit-bisect-20260417/production-dsb-gate/run1/serial.log | tail -1
```

## Constraint Self-Audit

- Prohibited Tier 1 files: not modified.
- Tier 2 file modified: `kernel/src/arch_impl/aarch64/context_switch.rs`, required because the isolated side effect is in the idle path.
- Polling: no timer polling added.
- F1-F19: not reverted.
- QEMU cleanup: run before handoff.

## PR URL

None.
