# F26 Phase 2 - Lockup Diagnosis

Date: 2026-04-18

## Verdict

The post-boot hard lockup was not reproduced in the Phase 1 Parallels run, so there is no observed stuck process or kernel state to root-cause from this data set.

## Evidence

The kernel timer remained active:

```text
[timer] cpu0 ticks=145000
[timer] cpu0 ticks=150000
[timer] cpu0 ticks=155000
[timer] cpu0 ticks=160000
```

The bwm compositor continued presenting frames:

```text
[virgl-composite] Frame #10000
[virgl-composite] Frame #10500
[virgl-composite] Frame #11000
```

No fatal markers were present:

```text
SOFT_LOCKUP|SOFT LOCKUP|TIMEOUT|UNHANDLED|DATA_ABORT|FATAL|panic|PANIC
```

Strict display captures at 60, 90, and 110 seconds all passed, and image diffs showed the displayed scene changed between captures.

## Kernel vs Userspace

Kernel lockup verdict: not reproduced. Timer output continued beyond the 120 second nominal test window.

Userspace lockup verdict: not reproduced. bwm continued issuing VirGL composite frames, and captures continued changing.

First-stuck process: none observed.

## Root Cause

Root cause for the reported lockup remains unconfirmed because the failure did not reproduce under the Phase 1 test conditions.

The available evidence does rule out a deterministic failure in these areas for this run:

- Timer delivery did not stop.
- bwm did not stop compositing.
- The GUI display did not freeze.
- AHCI did not emit `TIMEOUT`.
- No soft-lockup or fatal exception marker appeared.

## Next Step

Proceed with the independently measurable FPS regression. The current compositor cadence is about 70 Hz, below the 100 Hz minimum target. The F25 1 ms sleep in `userspace/programs/src/bounce.rs` remains the most direct candidate and can be removed with a 120 second validation run. If removing it reintroduces a lockup, that new failure becomes the actionable Phase 3 root cause.
