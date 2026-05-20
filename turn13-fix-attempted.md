# Turn 13 Fix Attempted

Status: INCONCLUSIVE

Turn 13 attempted the stabilization path requested by Claude:

- Revert the Turn 8 scheduler dequeue helpers in `kernel/src/task/scheduler.rs` back to the `045dcd04` behavior.
- Keep the Turn 5 xHCI IRQ-completion conversion intact.
- Do not touch `kernel/src/drivers/usb/xhci.rs`.
- Do not touch `kernel/src/main_aarch64.rs`.
- Run exactly one fresh Parallels boot.

## Source Outcome

The scheduler revert was applied for the test and then reverted after the boot failed, per the directive. The final source tree for this diagnostic commit contains no Turn 13 source changes.

The attempted source diff was limited to `kernel/src/task/scheduler.rs`; it restored the old dequeue behavior in `schedule()` and `schedule_deferred_requeue()` and removed the Turn 8 `pop_next_dispatchable_thread*()` filtering helpers from the test build.

## Build Outcome

The build gates completed cleanly:

- `turn13-artifacts/build-userspace.log`
- `turn13-artifacts/build-ext2.log`
- `turn13-artifacts/build-aarch64.log`
- `turn13-artifacts/build-x86.log`
- `turn13-artifacts/build-efi.log`

All Turn 13 warning/error grep artifacts are empty.

## Single-Boot Result

The single boot failed the Turn 13 pass criteria.

Evidence:

- PID 1 reached userspace: `[init] Breenix init starting (PID 1)`
- xHCI IRQ completion stayed alive: `MSI_EVENT_COUNT=59`
- freeze-watch continued until `uptime_ms=40278`
- CPU0 remained stuck at `timer_ticks_cpu0=8`
- peer CPUs advanced to about `30000` ticks
- the CPU0 regression guard panicked:
  `CPU0 timer regression: tick_count=8 but peer max=30000`
- no `[heartbeat]` marker was observed
- no `bsshd started` marker was observed
- no `Boot script completed` marker was observed

Primary artifacts:

- `turn13-artifacts/boot-1-run.out`
- `turn13-artifacts/boot-1-serial.log`
- `turn13-artifacts/boot-1-key-markers.txt`
- `turn13-artifacts/boot-1-final-serial-tail.txt`
- `turn13-artifacts/boot-1-summary.txt`
- `turn13-artifacts/boot-1-screenshot.png`

## Interpretation

The failure keeps Turn 13 diagnostic-only. The scheduler dequeue revert did not restore a passing fresh-deploy baseline while retaining Turn 5 xHCI IRQ-completion.

This result does not invalidate the Turn 12 Linux profile. It says the stabilization shortcut is insufficient. The proper future scheduler work should still avoid claiming Linux equivalence for the Turn 8 dequeue filter and should instead use explicit queue ownership.

## Proposed Turn 14

Use a different approach:

1. Keep source at the current baseline.
2. Profile the CPU0 timer stall directly with nonintrusive evidence.
3. Compare the passing Variant B boot from Turn 11 against this failed Turn 13 boot at the CPU0 handoff/timer level.
4. Focus on why CPU0 stops taking timer interrupts after userspace starts even though other CPUs continue ticking and xHCI MSI delivery remains active.
5. Only after isolating the CPU0 stall, decide whether the next source change belongs in scheduler ownership, ARM64 timer routing, CPU affinity/current-thread ownership, or aarch64 interrupt return state.

No further boots were run in Turn 13 after the first failure.
