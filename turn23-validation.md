# Turn 23 validation: Outcome A

## Status

COMPLETE. The diagnostic boot of post-revert HEAD passed the CPU0 gate. This separates the Turn 22 failure from baseline instability: the post-T21/post-revert source can boot cleanly past 60s, so Turn 22's async-TX source changes are implicated in the CPU0 regression seen during that turn.

## Head state

Recorded in `turn23-artifacts/head-state.txt`:

- HEAD at turn start: `e63004b9 docs(polling): record turn22 async TX inconclusive`
- Parent: `acb62bab fix(arm64): P6 Substep 2 - PCI MSI schedules NetRx`
- `git diff --stat kernel/` was empty.

No source files were changed in this turn.

## Build validation

Required build steps completed:

- `turn23-artifacts/build-userspace.log`: userspace build passed.
- `turn23-artifacts/build-ext2.log`: ext2 image build passed.
- `turn23-artifacts/build-aarch64.log`: aarch64 kernel release build passed.
- `turn23-artifacts/build-aarch64-warning-error-grep.txt`: empty.
- `turn23-artifacts/build-x86.log`: x86 release build passed.
- `turn23-artifacts/build-x86-warning-error-grep.txt`: empty.
- `turn23-artifacts/build-efi.log`: Parallels EFI build passed.

## Single boot result

Artifacts:

- `turn23-artifacts/boot-1-run.out`
- `turn23-artifacts/boot-1-serial.log`
- `turn23-artifacts/boot-1-key-markers.txt`
- `turn23-artifacts/boot-1-fail-marker-scan.txt`

Observed:

- Gateway ARP resolved: `NET: ARP resolved gateway MAC: 00:1c:42:00:00:18`.
- `bsshd` listened on `0.0.0.0:2222` and started as PID 6.
- `bounce` started as PID 7.
- `bwm` and VirGL compositor continued rendering.
- Heartbeat passed the 60s gate (`uptime_ms=60290`) and later reached `uptime_ms=377758` before cleanup.
- CPU0 passed the required threshold: `cpu0 ticks=60000` during the boot log and later `cpu0 ticks=285000` before cleanup.
- `turn23-artifacts/boot-1-fail-marker-scan.txt` is empty for panic, CPU0 regression, `UNHANDLED_EC`, `PC_ALIGN`, `DATA_ABORT`, soft-lockup, AHCI timeout, and assertion markers.

The screenshot helper again emitted `ERROR: No Parallels window found matching ...`, then `prlctl capture` succeeded. This is not a kernel failure marker.

## Live procfs evidence

Captured in `turn23-artifacts/live-procfs.txt` via interactive SSH to `root@10.211.55.100:2222` with the development password.

Useful values:

- `/proc/stat`: `net_msi_irqs 260`
- `/proc/stat`: `interrupts 4546046`
- `/proc/stat`: `timer_ticks 2388582`
- `/proc/trace/counters`: `NET_PCI_IRQ_RAISED_NETRX: 265 (cpu0=265)`
- `/proc/trace/counters`: `NET_RX_BUDGET_EXHAUSTED: 0`
- `/proc/trace/counters`: `TIMER_TICK_TOTAL: 2395806 (cpu0=278939, ...)`

This confirms the post-revert Substep 2 IRQ delivery path is still working in this boot.

## Verdict

Outcome A: PASS.

Conclusion: Turn 22's specific source changes caused or exposed the CPU0 regression. The unchanged post-revert source booted cleanly past the CPU0 threshold in a single fresh deployment, with live SSH/procfs confirmation that PCI MSI delivery still works.

Recommendation for Turn 24: root-cause the Turn 22 diff before reattempting Substep 3. The likely suspects remain the TX ring IRQ-safe lock window, `reclaim_tx_completed()` placement at the top of `process_rx_budgeted()`, or the new in-flight atomic slot machinery.
