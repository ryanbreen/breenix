# Turn 22 validation: INCONCLUSIVE

## Status

INCONCLUSIVE. The async TX ownership refactor built cleanly, but the required single Parallels boot hit a hard fail criterion: CPU0 timer regression panic before the 60s validation window completed. Per directive, the source edits to `kernel/src/drivers/virtio/net_pci.rs`, `kernel/src/drivers/virtio/net_mmio.rs`, and `kernel/src/net/mod.rs` were reverted. This commit is diagnostic-only.

## 22A ownership audit

See `turn22-artifacts/tx-ownership-audit.md`.

Findings:

- PCI TX used one static `PCI_TX_BUFFER`, copied caller data into it, posted descriptor 0, then waited for `used.idx` to advance.
- MMIO TX used one static `TX_BUFFER`, copied caller data into it, posted descriptor 0, then waited for `used.idx` to advance.
- Both completion checks were "has advanced" checks (`used_idx != state.tx_last_used_idx`), not strict equality against a specific expected index.
- Callers build transient packet storage and expect `transmit()` to copy before returning, so async TX requires driver-owned in-flight storage.

## 22B design choice

See `turn22-artifacts/ownership-design.md`.

Chosen design was Option A: a fixed 16-slot driver-owned TX buffer pool per transport, with atomic in-flight slot tracking and `reclaim_tx_completed() -> usize` called at the top of the aarch64 network poll path before RX.

## Build confirmation

All required build steps completed:

- `turn22-artifacts/build-userspace.log`: userspace build passed.
- `turn22-artifacts/build-ext2.log`: ext2 image build passed.
- `turn22-artifacts/build-aarch64.log`: aarch64 kernel release build passed.
- `turn22-artifacts/build-aarch64-warning-error-grep.txt`: empty.
- `turn22-artifacts/build-x86.log`: x86 release build passed.
- `turn22-artifacts/build-x86-warning-error-grep.txt`: empty.
- `turn22-artifacts/build-efi.log`: Parallels EFI build passed.

## Single boot result

Artifacts:

- `turn22-artifacts/boot-1-run.out`
- `turn22-artifacts/boot-1-serial.log`

Observed:

- Gateway ARP resolved: `NET: ARP resolved gateway MAC: 00:1c:42:00:00:18`.
- Heartbeat reached `uptime_ms=43283`, then the kernel panicked.
- CPU0 regression alarm fired with `CPU0 tick_count = 75, max peer = 30000`.
- Panic site: `kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17`.
- The serial log contains no `[virtio-net-pci] TX timeout!` marker and no `TX queue full` marker, but the boot did not reach the 60s PASS criteria and did not reach live SSH/procfs validation.

Fail criterion hit:

- `panic` marker present.
- CPU0 ticks far below the required threshold and explicit CPU0 timer regression alarm present.
- bsshd/bounce/compositor live markers and `net_msi_irqs` live validation were not reached before the panic.

## Polling scope confirmation

The attempted source changes targeted only Substep 3 and did not modify init ARP/ICMP polling, on-demand ARP polling, or x86 hlt-loop polling. Because the boot failed, all source changes were reverted before this diagnostic-only commit.
