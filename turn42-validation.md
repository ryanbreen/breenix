# Turn 42 Validation - P9 Add snapshot_counters Only

## Source Scope

`turn42-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 12 ++++++++++++
 1 file changed, 12 insertions(+)
```

Only `kernel/src/drivers/virtio/gpu_mmio.rs` was changed. The retained source
delta adds one public, uncalled `snapshot_counters()` bisect function on top of
T39's statics, T40's `get_irq()`, and T41's `handle_interrupt()`. No
`record_mmio_irq_state()`, init recording, GIC enable, exception dispatch, or
command path changes were added.

## Build Status

- Userspace aarch64 build: PASS (`turn42-artifacts/build-userspace.log`)
- aarch64 ext2 image: PASS (`turn42-artifacts/build-ext2.log`)
- aarch64 kernel: PASS (`turn42-artifacts/build-aarch64.log`)
- x86 release/test kernel: PASS (`turn42-artifacts/build-x86.log`)
- Parallels EFI image: PASS (`turn42-artifacts/build-efi.log`)
- Warning/error greps:
  - `turn42-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
  - `turn42-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Runtime Result

Single Parallels boot completed the requested 60-second test window and
continued producing serial output past 87 seconds.

- CPU0 regression scan: PASS, 0 bytes
  (`turn42-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`)
- Heartbeat: PASS, reached `uptime_ms=87239`
  (`turn42-artifacts/boot-parallels/boot-1-serial.log:651`)
- CPU0 timer ticks: PASS, reached `cpu0 ticks=50000`
  (`turn42-artifacts/boot-parallels/boot-1-serial.log:634`)
- T33 network markers preserved:
  - `NET: Network initialization complete` at line 268
  - `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)` at line 269
  - `NET: synchronously cleared virtio callback suppression` at line 270
  - `NET: pre-primed NetRx softirq for bootstrap callback re-enable` at line 271
- External ping: PASS, `1 packets transmitted, 1 packets received, 0.0% packet loss`
  (`turn42-artifacts/boot-parallels/live-ping.txt`)
- ARP resolved: `00:1c:42:5a:24:f9`
  (`turn42-artifacts/boot-parallels/live-arp.txt`)

## Hypothesis Verdict

T42 hypothesis passed: adding the uncalled `snapshot_counters()` reader to
`gpu_mmio.rs` did **not** trip the Parallels CPU0 guard.

Four PASS turns now show that the T39 statics plus the uncalled `get_irq()`,
`handle_interrupt()`, and `snapshot_counters()` functions are safe. The T38
trigger is now narrowed to `record_mmio_irq_state()` or its init-time call site.

## Proposed T43 First Step

Proceed with the directive's PASS path: add `record_mmio_irq_state()` as an
uncalled function only. If that passes, the next turn should add the init-time
call site to isolate function body versus call-site execution.
