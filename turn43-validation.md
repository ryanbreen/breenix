# Turn 43 Validation

## Directive

Add `record_mmio_irq_state(base: u64, slot: u32)` to
`kernel/src/drivers/virtio/gpu_mmio.rs` after `snapshot_counters()`.
The function is private, marked `#[allow(dead_code)]`, and remains uncalled.

## Diff Scope

`turn43-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 17 +++++++++++++++++
 1 file changed, 17 insertions(+)
```

`rg -n "record_mmio_irq_state" kernel/src/drivers/virtio/gpu_mmio.rs`
reported only the function definition, confirming there is no call site.

## Builds

- `./userspace/programs/build.sh --arch aarch64`: PASS
- `./scripts/create_ext2_disk.sh --arch aarch64`: PASS
- `cargo build --release --target aarch64-breenix.json ... -p kernel --bin kernel-aarch64`: PASS
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`: PASS
- `scripts/parallels/build-efi.sh --kernel`: PASS

Warning/error grep artifacts:

- `turn43-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn43-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Boot

Single Parallels boot: PASS.

Evidence:

- CPU0 regression scan: `turn43-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`, 0 bytes
- Latest heartbeat: `line 631: [heartbeat] tid=11 uptime_ms=79225`
- Latest CPU0 timer marker: `line 614: [timer] cpu0 ticks=45000`
- T33 bootstrap markers preserved:
  - `line 268: NET: Network initialization complete`
  - `line 269: [virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
  - `line 270: NET: synchronously cleared virtio callback suppression`
  - `line 271: NET: pre-primed NetRx softirq for bootstrap callback re-enable`
- Ping: 1 packet transmitted, 1 packet received, 0.0% packet loss
- ARP: `10.211.55.100` resolved to `00:1c:42:6a:fd:ce`

## Verdict

PASS. Adding the uncalled `record_mmio_irq_state()` body does not trip the
Parallels CPU0 guard. This supports the body-vs-callsite hypothesis: the T38
trigger is likely the init-time call site rather than the body alone.

Next expected step: T44 should add the init-time
`record_mmio_irq_state(slot, base)` call from `init_device()` to confirm
whether the call site is the trigger.
