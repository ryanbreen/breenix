# F32m Tick-Audit Reliability Exit

## Summary

F32m did not meet the merge gate. The diagnosis and implementation work found
and addressed the original missing-line mechanism, but the final implementation
failed the required `wait_stress` gate before the 5x Parallels sweep could be
run. Per the factory contract, no PR was opened and no merge was attempted.

## Root Cause

`[timer] cpu0 ticks=...` was not an end-of-run audit. It was a CPU0-only
periodic timer breadcrumb at exact 5000-tick boundaries, originally emitted
through raw UART writes. This made the evidence line unreliable in two ways:

- CPU0 can remain below the 5000-tick audit threshold while the rest of the
  system is healthy.
- Raw UART writes can be interleaved with normal serial output, losing the exact
  parser-required substring.

The attempted fix changed the line into a pending-until-printed audit using a
locked serial write, with CPU0 as the primary emitter and CPU1 as the fallback
when CPU0 is stuck in the early timer range.

## Changes

- `docs/planning/f32m-tick-audit/diagnosis.md`: records the producer, parser
  contract, failed-run evidence, and corrected root cause.
- `kernel/src/serial_aarch64.rs`: adds `try_write_bytes()` so rare IRQ-context
  audit lines can acquire the normal serial lock opportunistically and then wait
  for FIFO capacity while writing a complete line.
- `kernel/src/arch_impl/aarch64/timer_interrupt.rs`: adds pending tick-audit
  state, decimal formatting, locked emission, interval claiming, and CPU1
  fallback when CPU0 is stuck at `<=10` ticks.

## Sweep Table

| Gate | Command / Artifact | Result | Evidence |
|---|---|---:|---|
| aarch64 clean build | `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | PASS | `build-aarch64-4.log`; `grep -E '^(warning\|error)'` produced no output |
| x86_64 clean build | `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | PASS | `build-x86_64-4.log`; `grep -E '^(warning\|error)'` produced no output |
| wait_stress attempt 3 | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | FAIL | `wait-stress-3.serial.log`: tick lines `5000..190000`, but no `WAIT_STRESS_*`, no frames, blue screenshot |
| wait_stress attempt 4 | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | FAIL | `wait-stress-4.serial.log`: reached init, then tick lines `5000..120000`, but no `WAIT_STRESS_*`, no frames, blue screenshot |
| 5x normal Parallels | Not run | STOP | Blocked by failed wait_stress gate |

## PR URL

Not opened. The branch did not pass Phase 3 validation.

## Stop Reason

STOP: Final validation failed at `wait_stress`; the factory contract requires
exit documentation instead of PR/merge when the gate is not 5/5 clean.
