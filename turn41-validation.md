# Turn 41 Validation - P9 Add handle_interrupt Only

## Source Scope

`turn41-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 36 +++++++++++++++++++++++++++++++++++
 1 file changed, 36 insertions(+)
```

Only `kernel/src/drivers/virtio/gpu_mmio.rs` was changed. The retained source
delta adds one public, uncalled `handle_interrupt()` bisect function on top of
T39's five statics and T40's `get_irq()`. No snapshot reader, init recording,
GIC enable, exception dispatch, or command path changes were added.

The insertion count is above the directive's approximate 25-30 line estimate
because the full T41 comment and handler body were preserved instead of
compressing the probe.

## Build Status

- Userspace aarch64 build: PASS (`turn41-artifacts/build-userspace.log`)
- aarch64 ext2 image: PASS (`turn41-artifacts/build-ext2.log`)
- aarch64 kernel: PASS (`turn41-artifacts/build-aarch64.log`)
- x86 release/test kernel: PASS (`turn41-artifacts/build-x86.log`)
- Parallels EFI image: PASS (`turn41-artifacts/build-efi.log`)
- Warning/error greps:
  - `turn41-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
  - `turn41-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Runtime Result

Single Parallels boot completed the requested 60-second test window and
continued producing serial output past 102 seconds.

- CPU0 regression scan: PASS, 0 bytes
  (`turn41-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`)
- Heartbeat: PASS, reached `uptime_ms=102235`
  (`turn41-artifacts/boot-parallels/boot-1-serial.log:677`)
- CPU0 timer ticks: PASS, reached `cpu0 ticks=60000`
  (`turn41-artifacts/boot-parallels/boot-1-serial.log:672`)
- T33 network markers preserved:
  - `NET: Network initialization complete` at line 268
  - `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)` at line 269
  - `NET: synchronously cleared virtio callback suppression` at line 270
  - `NET: pre-primed NetRx softirq for bootstrap callback re-enable` at line 271
- External ping: PASS, `1 packets transmitted, 1 packets received, 0.0% packet loss`
  (`turn41-artifacts/boot-parallels/live-ping.txt`)
- ARP resolved: `00:1c:42:1d:b1:d7`
  (`turn41-artifacts/boot-parallels/live-arp.txt`)

## Hypothesis Verdict

T41 hypothesis passed: adding the uncalled `handle_interrupt()` body to
`gpu_mmio.rs` did **not** trip the Parallels CPU0 guard.

The heaviest uncalled handler body from T38 is Parallels CPU0 safe by itself.
The T38 trigger is now more likely in one of the remaining functions
(`snapshot_counters()` or `record_mmio_irq_state()`) or in the combination of
function additions plus the init-time call site that stores MMIO state.

## Proposed T42 First Step

Proceed with the directive's PASS path: add `snapshot_counters()` only as the
last simple reader probe, with no init recording, no GIC enable, no exception
dispatch, and no send-command changes.
