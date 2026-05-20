# Turn 21 Validation - PCI MSI Schedules NetRx

## Source Scope

Changed source files:

- `kernel/src/drivers/virtio/net_pci.rs`
- `kernel/src/net/mod.rs`
- `kernel/src/tracing/providers/counters.rs`

No changes were made to `net_mmio.rs`, `main.rs`, `softirqd.rs`, TX completion code, init/on-demand ARP polling removal, x86 hlt-loop polling, scheduler, GIC, exceptions, or syscall code.

## Implementation

- `net_pci::handle_interrupt()` now:
  - increments the existing `NET_PCI_MSI_COUNT`;
  - reads the legacy VirtIO ISR status to acknowledge/clear the device interrupt;
  - sets `VRING_AVAIL_F_NO_INTERRUPT` on the RX queue to suppress device callbacks;
  - leaves the GIC SPI enabled;
  - raises `SoftirqType::NetRx`;
  - increments the new `NET_PCI_IRQ_RAISED_NETRX` TraceCounter.
- Replaced the old GIC-disable based `re_enable_irq()` path with `reenable_and_check_race() -> bool`.
- `reenable_and_check_race()` clears `VRING_AVAIL_F_NO_INTERRUPT`, publishes the clear with a memory fence, reads `used.idx`, and returns true if `used.idx != rx_last_used_idx`.
- `net_rx_softirq_handler()` now completes PCI RX in a NAPI-shaped match:
  - `Drained`: re-enable callbacks and re-raise NetRx only if the race check sees new work.
  - `BudgetExhausted`: keep callbacks suppressed, increment the existing T20 budget counter once, and re-raise NetRx.
- Added and registered `NET_PCI_IRQ_RAISED_NETRX` in `kernel/src/tracing/providers/counters.rs`.

## Build Validation

Commands run:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep artifacts are all zero bytes:

- `turn21-artifacts/build-userspace-warning-error-grep.txt`
- `turn21-artifacts/build-ext2-warning-error-grep.txt`
- `turn21-artifacts/build-aarch64-warning-error-grep.txt`
- `turn21-artifacts/build-x86-warning-error-grep.txt`
- `turn21-artifacts/build-efi-warning-error-grep.txt`

## Single Boot Validation

Single fresh Parallels boot command:

```bash
./run.sh --parallels --test 60
```

Observed in `turn21-artifacts/boot-1-serial.log` and `turn21-artifacts/boot-1-summary.txt`:

- Gateway ARP resolved and network initialization completed.
- MSI-X SPI enabled post-init.
- `bsshd` listened and `[init] bsshd started (PID 6)`.
- `[init] bounce started (PID 7)` and `[bounce] Window mode` appeared.
- Heartbeat reached `max_heartbeat_ms=169388`.
- CPU0 timer audit reached `max_cpu0_ticks=110000`.
- VirGL compositor reached `Frame #28500`.
- `bwm` produced 167 FPS samples; 134 samples were `>=160`.
- Failure scan was empty for panic, `UNHANDLED_EC`, `PC_ALIGN`, `DATA_ABORT`, soft lockup, CPU0 timer regression, AHCI timeout, and assertion markers.

The screenshot helper again emitted `ERROR: No Parallels window found matching ...`, then `prlctl capture` succeeded. This is not a kernel failure marker.

## PCI IRQ / NetRx Evidence

Live procfs was read over `bsshd` on the same boot via OpenSSH to `10.211.55.100:2222`.

Baseline from serial:

- `msi_count=0` before post-init external traffic.

`/proc/stat` reads:

- First read: `net_msi_irqs 119`
- Second read: `net_msi_irqs 179`

`/proc/trace/counters` read:

- `NET_PCI_IRQ_RAISED_NETRX: 187 (cpu0=187)`
- `NET_RX_BUDGET_EXHAUSTED: 0`

This confirms PCI MSI delivery advanced from zero, scheduled NetRx, and did not enter the budget-exhaustion storm failure mode.

## Non-Changes Confirmed

- TX completion still busy-waits in `net_pci::transmit()`; Substep 3 remains.
- Init ARP/ICMP polling still calls `process_rx()`; Substep 4 remains.
- On-demand ARP polling still calls `process_rx()`; Substep 5 remains.
- x86 hlt-loop polling was not touched; Substep 6 remains.

## Result

PASS. PCI MSI now schedules NetRx and completion is handled through the budgeted softirq path with callback re-enable/race-check semantics.
