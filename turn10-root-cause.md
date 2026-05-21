# Turn 10 Root Cause

The fresh-deploy CPU0 regression is carried by `cb73f6e3` (`fix(arm64): close scheduler dequeue race and xHCI IRQ completion`).

Evidence:

- `18c88a01` fresh deploy: pass, CPU0 reaches 63067 ticks.
- `045dcd04` fresh deploy boot 1: pass, CPU0 reaches 62948 ticks.
- `045dcd04` fresh deploy boot 2: older DATA_ABORT/soft-lockup behavior after init services start; CPU0 still advances to 26482 ticks.
- `cb73f6e3` fresh deploy: fail, CPU0 stays at 5 ticks until the regression guard panics.
- Turn 9 branch-tip fresh deploys with the same source also fail with CPU0 fixed at 5.
- The Turn 8 runner re-run observed 2/2 failures before the runner session was killed.

Within `cb73f6e3`, the strongest artifact-level signal is the xHCI IRQ transition, not the stale-queue counter path. Passing boots report `MSI_EVENT_COUNT=1` at xHCI post-activation; the failing `cb73f6e3` boot reports `MSI_EVENT_COUNT=60` before timer init, then PID1 reaches only the init-start print before CPU0 stops ticking. The scheduler stale queue counters are not observable from userspace because `/bin/xhci_counters` never runs, and the freeze-watch lines show empty ready queues rather than accumulating stale queue skips.

Working hypothesis for the next turn: the IRQ-driven xHCI command/transfer completion conversion in `cb73f6e3` creates an early Parallels xHCI MSI storm or interrupt masking condition on CPU0. That condition appears immediately after timer/userland handoff and prevents CPU0's local timer path from making progress. The scheduler dequeue validation may still need review, but it is not the first visible divergence in the failing logs.

Next diagnostic should isolate `cb73f6e3` internally without touching Tier-1 interrupt/syscall files: test one temporary build with only the xHCI changes reverted or gated, and one with only the scheduler changes reverted, then keep whichever result proves the smaller carrier.
