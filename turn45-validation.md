# Turn 45 Validation

## Directive

Add only T38's slot-plumbing changes on top of T43:

- update the MMIO probe caller to pass `i as u32`
- update `init_device()` to accept `_slot: u32`

Do not call `record_mmio_irq_state(base, slot)` from `init_device()`.

## Diff Scope

`turn45-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 4 ++--
 1 file changed, 2 insertions(+), 2 deletions(-)
```

`record_mmio_irq_state` remains defined but uncalled.

## Builds

- `./userspace/programs/build.sh --arch aarch64`: PASS
- `./scripts/create_ext2_disk.sh --arch aarch64`: PASS
- `cargo build --release --target aarch64-breenix.json ... -p kernel --bin kernel-aarch64`: PASS
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`: PASS
- `scripts/parallels/build-efi.sh --kernel`: PASS

Warning/error grep artifacts:

- `turn45-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn45-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Boot

Single Parallels boot: PASS.

Evidence:

- CPU0 regression scan: `turn45-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`, 0 bytes
- Latest heartbeat: `line 598: [heartbeat] tid=11 uptime_ms=65213`
- Latest CPU0 timer marker: `line 596: [timer] cpu0 ticks=40000`
- T33 bootstrap markers preserved:
  - `line 268: NET: Network initialization complete`
  - `line 269: [virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
  - `line 270: NET: synchronously cleared virtio callback suppression`
  - `line 271: NET: pre-primed NetRx softirq for bootstrap callback re-enable`
- Ping: 1 packet transmitted, 1 packet received, 0.0% packet loss
- ARP: `10.211.55.100` resolved to `00:1c:42:93:9c:5b`

## Verdict

PASS. T38's slot-plumbing changes alone do not trip the Parallels CPU0 guard.
The remaining untested T38 edge is the `record_mmio_irq_state(base, slot);`
call line in `init_device()`.

Next expected step: T46 should add only that call line at the success-log
boundary, using the `_slot` parameter as needed by the current source state.
