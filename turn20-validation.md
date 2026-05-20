# Turn 20 Validation - P6 Substep 1 Budgeted NetRx

## Source Scope

Changed source files:

- `kernel/src/net/mod.rs`
- `kernel/src/tracing/providers/counters.rs`

No changes were made to `kernel/src/drivers/virtio/net_pci.rs`, `kernel/src/drivers/virtio/net_mmio.rs`, `kernel/src/main.rs`, or `kernel/src/task/softirqd.rs`.

## Implementation

- Added `PollOutcome::{Drained, BudgetExhausted}`.
- Added `process_rx_budgeted(budget: u32) -> PollOutcome` on both x86_64 and aarch64.
- Kept `process_rx()` as an unbounded wrapper using `u32::MAX`.
- Changed the NetRx softirq handler to process at most 64 packets per invocation.
- Added lock-free trace counter `NET_RX_BUDGET_EXHAUSTED`, incremented only when the NetRx softirq exhausts its packet budget.
- Left synchronous init/on-demand polling paths unchanged; existing call sites still use `process_rx()`.
- Left aarch64 PCI IRQ re-enable behavior unchanged after NetRx processing.

## Build Validation

Commands run:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error grep artifacts are all zero bytes:

- `turn20-artifacts/build-userspace-warning-error-grep.txt`
- `turn20-artifacts/build-ext2-warning-error-grep.txt`
- `turn20-artifacts/build-aarch64-warning-error-grep.txt`
- `turn20-artifacts/build-x86-warning-error-grep.txt`
- `turn20-artifacts/build-efi-warning-error-grep.txt`

## Boot Validation

Single fresh Parallels boot command:

```bash
./run.sh --parallels --test 60
```

The VM remained alive after the 60s observation window, so I copied a later serial snapshot from the same boot to satisfy the CPU0 tick threshold without starting a second boot.

Observed in `turn20-artifacts/boot-1-serial.log`:

- Gateway ARP resolved and network initialization completed.
- `bsshd` listened and `[init] bsshd started (PID 6)`.
- `[init] bounce started (PID 7)` and `[bounce] Window mode` appeared.
- Heartbeat reached `max_heartbeat_ms=147346`.
- CPU0 timer audit reached `max_cpu0_ticks=95000`.
- VirGL compositor reached `Frame #25000`.
- `bwm` produced 146 FPS samples; 120 samples were `>=160`.
- Failure scan was empty for panic, `UNHANDLED_EC`, `PC_ALIGN`, `DATA_ABORT`, soft lockup, CPU0 timer regression, and AHCI timeout markers.

The run output has `ERROR: No Parallels window found matching ...` from the screenshot helper, followed by successful `prlctl capture`; this is not a kernel failure marker.

## Trace Surface Note

The new `NET_RX_BUDGET_EXHAUSTED` TraceCounter is registered and therefore appears through the existing `/proc/trace/counters` generator.

The directive also asked for the existing `net_msi_irqs` sanity surface. Source audit shows the current tree exposes `net_msi_irqs` via `kernel/src/fs/procfs/mod.rs` using `crate::drivers::virtio::net_pci::msi_interrupt_count()`, while `/proc/trace/counters` is backed by the registered TraceCounter list in `kernel/src/fs/procfs/trace.rs`. This turn did not change either procfs surface. I did not get a live shell read of procfs from the single boot.

## Result

PASS for Substep 1 source scope, clean builds, and single-boot health gate. The procfs `net_msi_irqs` item was source-audited as unchanged, with the path discrepancy above recorded.
