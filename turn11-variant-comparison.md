# Turn 11 Variant Comparison

Turn 11 split `cb73f6e3` into its two logical changes:

- xHCI IRQ-driven completion conversion: `kernel/src/drivers/usb/xhci.rs`, `kernel/src/main_aarch64.rs`
- Scheduler dequeue validation: `kernel/src/task/scheduler.rs`, `kernel/src/tracing/providers/counters.rs`, `kernel/src/fs/procfs/xhci.rs`

Each variant used a detached temporary worktree at `cb73f6e3`, restored only the requested files from `cb73f6e3^` (`045dcd04`), then ran exactly one fresh Parallels boot with `./run.sh --parallels --test 45`.

| Variant | xHCI source | Scheduler source | Boot outcome | CPU0 ticks at panic/uptime |
|---|---|---|---|---|
| baseline `cb73f6e3` | Turn 5 IRQ-driven | Turn 8 stale filters | FAIL (panic) | 5 / 30000 |
| Variant A | reverted (pre-Turn-5 polling) | Turn 8 stale filters | FAIL (panic) | 5 / 30000 |
| Variant B | Turn 5 IRQ-driven | reverted (pre-Turn-8) | PASS (1 sample) | no panic; CPU0 reached at least 45000 ticks |
| `045dcd04` | reverted (pre-Turn-5 polling) | reverted (pre-Turn-8) | PASS (boot 1) / DIFFERENT FAIL (boot 2) | 62948 / 26482 |

## Evidence

### Variant A: xHCI reverted, scheduler filters kept

Artifacts:

- `turn11-artifacts/variant-a-xhci-reverted/boot-1-run.out`
- `turn11-artifacts/variant-a-xhci-reverted/boot-1-serial.log`
- `turn11-artifacts/variant-a-xhci-reverted/boot-1-screenshot.png`
- `turn11-artifacts/variant-a-xhci-reverted/boot-1-cpu0-final.txt`
- `turn11-artifacts/variant-a-xhci-reverted/build-warning-error-grep.txt`

Result:

- Build warning/error grep was empty.
- xHCI post-activation marker returned to the pre-Turn-5 shape:
  `MSI_EVENT_COUNT=1 EVENT_COUNT=0 POLL_COUNT=0 SPI_ACTIVATED=true`.
- Userland reached only `[init] Breenix init starting (PID 1)`.
- Freeze-watch repeatedly reported `timer_ticks_cpu0=5` while peers advanced.
- The guard panicked with `CPU0 tick_count = 5, max peer = 30000`.

This rules out the xHCI IRQ conversion as a necessary condition for the fresh-deploy CPU0=5 regression.

### Variant B: scheduler reverted, xHCI conversion kept

Artifacts:

- `turn11-artifacts/variant-b-sched-reverted/boot-1-run.out`
- `turn11-artifacts/variant-b-sched-reverted/boot-1-serial.log`
- `turn11-artifacts/variant-b-sched-reverted/boot-1-screenshot.png`
- `turn11-artifacts/variant-b-sched-reverted/boot-1-cpu0-final.txt`
- `turn11-artifacts/variant-b-sched-reverted/build-warning-error-grep.txt`

Result:

- Build warning/error grep was empty.
- xHCI kept the Turn 5 IRQ-driven behavior:
  `MSI_EVENT_COUNT=60 EVENT_COUNT=0 POLL_COUNT=0 SPI_ACTIVATED=true`.
- Init spawned `heartbeat`, `xhci_counters`, `bwm`, `telnetd`, `bsshd`, and `bounce`.
- Freeze-watch showed CPU0 advancing normally:
  - `uptime_ms=45316 timer_ticks_cpu0=29509`
  - `uptime_ms=50318 timer_ticks_cpu0=32928`
  - `uptime_ms=60326 timer_ticks_cpu0=39637`
  - `[timer] cpu0 ticks=45000`
- No CPU0 regression panic occurred in the single requested sample.

This confirms the carrier is the scheduler dequeue validation, not the xHCI conversion. The xHCI `MSI_EVENT_COUNT=60` observation is noisy and worth later cleanup, but it is not sufficient to reproduce the fresh-deploy CPU0=5 failure.

## Interpretation

The decisive split is:

- A fails with xHCI reverted and scheduler filters kept.
- B passes with scheduler filters reverted and xHCI kept.

Carrier identified: `kernel/src/task/scheduler.rs` dequeue validation added in `cb73f6e3`.

Confidence is high for carrier identification because the two variants isolate opposite halves of the same commit and produce opposite outcomes. Variant B is still only one boot sample, so it should be treated as "PASS (1 sample)", not a full stability claim.
