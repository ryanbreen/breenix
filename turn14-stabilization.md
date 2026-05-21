# Turn 14 Stabilization

Status: COMPLETE

Turn 14 tested the exact three-file revert shape used by Turn 11 Variant B.

## Applied Change

Restored these files to `045dcd04` state:

- `kernel/src/task/scheduler.rs`
- `kernel/src/tracing/providers/counters.rs`
- `kernel/src/fs/procfs/xhci.rs`

Kept the Turn 5 xHCI IRQ-completion conversion intact:

- `kernel/src/drivers/usb/xhci.rs`
- `kernel/src/main_aarch64.rs`

The source diff against `HEAD` is exactly the three intended files:

```text
kernel/src/fs/procfs/xhci.rs             |  10 +-
kernel/src/task/scheduler.rs             | 212 ++++++++++++++-----------------
kernel/src/tracing/providers/counters.rs |  24 ----
3 files changed, 98 insertions(+), 148 deletions(-)
```

## Build Outcome

The build gates completed cleanly:

- `turn14-artifacts/build-userspace.log`
- `turn14-artifacts/build-ext2.log`
- `turn14-artifacts/build-aarch64.log`
- `turn14-artifacts/build-x86.log`
- `turn14-artifacts/build-efi.log`

All Turn 14 warning/error grep artifacts are empty.

## Single-Boot Result

The single fresh Parallels boot passed the Turn 14 criteria.

Evidence:

- PID 1 reached userspace: `[init] Breenix init starting (PID 1)`
- xHCI IRQ completion stayed alive: `MSI_EVENT_COUNT=60`
- heartbeat spawned and emitted repeated `[heartbeat]` lines through `uptime_ms=113320`
- bwm started and compositor activity flowed through `Frame #20000`
- boot script completed: `[init] Boot script completed`
- bsshd started: `[init] bsshd started (PID 6)`
- bounce started and BWM discovered the Bounce window
- CPU0 reached `timer_ticks_cpu0=72843` in freeze-watch and `[timer] cpu0 ticks=75000`
- no `CPU0 timer regression` marker
- no `END SOFT LOCKUP DUMP` marker
- no `KERNEL PANIC` marker

Primary artifacts:

- `turn14-artifacts/boot-1-run.out`
- `turn14-artifacts/boot-1-serial.log`
- `turn14-artifacts/boot-1-key-markers.txt`
- `turn14-artifacts/boot-1-final-serial-tail.txt`
- `turn14-artifacts/boot-1-summary.txt`
- `turn14-artifacts/boot-1-screenshot.png`

## Interpretation

The three-file revert restores the one-sample passing behavior seen in Turn 11 Variant B. This is a valid stabilization signal for the polling-elimination gate, but it is not deterministic proof that the underlying race is gone.

This commit is stabilization only. It does not claim that the restored scheduler behavior is Linux-shaped. The proper future fix remains the explicit queue-ownership design described by `linux-profile-scheduler-dequeue.md`: represent queue ownership directly instead of inferring stale entries at dequeue time.
