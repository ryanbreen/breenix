# Turn 46 Validation

## Directive

Add exactly one source line on top of the T45 plumbing state:

```rust
record_mmio_irq_state(base, _slot);
```

The line was inserted after `flush()?;` and before the
`"[virtio-gpu] GPU device initialized successfully"` log in `init_device()`.

## Diff Scope

`turn46-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 1 +
 1 file changed, 1 insertion(+)
```

No signature rename was needed; using `_slot` compiled warning-clean.

## Builds

- `./userspace/programs/build.sh --arch aarch64`: PASS
- `./scripts/create_ext2_disk.sh --arch aarch64`: PASS
- `cargo build --release --target aarch64-breenix.json ... -p kernel --bin kernel-aarch64`: PASS
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`: PASS
- `scripts/parallels/build-efi.sh --kernel`: PASS

Warning/error grep artifacts:

- `turn46-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn46-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Boot

Single Parallels boot: FAIL, as predicted.

Evidence:

- CPU0 regression scan: `turn46-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`, 1043 bytes
- First alarm: `line 406: !!! CPU0 REGRESSION ALARM !!!`
- Alarm state: `line 407: CPU0 tick_count = 65, max peer = 30000`
- Panic site: `line 413: panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17`
- Latest heartbeat before alarm: `line 404: [heartbeat] tid=11 uptime_ms=45219`
- No regular `[timer] cpu0 ticks=...` markers were emitted before the guard fired.
- T33 bootstrap markers were reached before the alarm:
  - `line 268: NET: Network initialization complete`
  - `line 269: [virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
  - `line 270: NET: synchronously cleared virtio callback suppression`
  - `line 271: NET: pre-primed NetRx softirq for bootstrap callback re-enable`
- Ping after boot window: 1 transmitted, 0 received, 100.0% packet loss
- ARP still resolved `10.211.55.100` to `00:1c:42:e9:70:9a`

After capturing the failure, the call line was removed, restoring the T45
source state. The retained source contains the T45 slot plumbing and the
uncalled probe functions, but not the T46 call line.

## Verdict

COMPLETE (FAIL - predicted bisect endpoint). Six prior PASS turns ruled out
the dead-code scaffold and T38 slot plumbing. T46 added only the call
instruction and reproduced the CPU0 regression.

Bisect final conclusion: the trigger is the literal
`record_mmio_irq_state(base, _slot);` call line in `init_device()`. P9 should
avoid calling `record_mmio_irq_state()` from `init_device()` on this path.

Recommended next step: choose a trigger-avoiding P9 implementation strategy,
either deferred slot/base storage outside this init-time call pattern or a
handler design that does not require pre-stored MMIO slot/base state.
