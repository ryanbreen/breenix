# Turn 39 Validation - P9 Atomic Statics Only

## Source Scope

`turn39-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 15 +++++++++++++++
 1 file changed, 15 insertions(+)
```

Only `kernel/src/drivers/virtio/gpu_mmio.rs` was changed. The retained source
delta adds only the T39 atomic-type import, comment block, and five
`#[allow(dead_code)]` atomic statics. No functions, init recording, handler,
GIC enable, exception dispatch, or command path changes were added.

## Build Status

- Userspace aarch64 build: PASS (`turn39-artifacts/build-userspace.log`)
- aarch64 ext2 image: PASS (`turn39-artifacts/build-ext2.log`)
- aarch64 kernel: PASS (`turn39-artifacts/build-aarch64.log`)
- x86 release/test kernel: PASS (`turn39-artifacts/build-x86.log`)
- Parallels EFI image: PASS (`turn39-artifacts/build-efi.log`)
- Warning/error greps:
  - `turn39-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
  - `turn39-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Runtime Result

Single Parallels boot completed the requested 60-second test window and
continued producing serial output past 110 seconds.

- CPU0 regression scan: PASS, 0 bytes
  (`turn39-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`)
- Heartbeat: PASS, reached `uptime_ms=110249`
  (`turn39-artifacts/boot-parallels/boot-1-serial.log:711`)
- CPU0 timer ticks: PASS, reached `cpu0 ticks=70000`
  (`turn39-artifacts/boot-parallels/boot-1-serial.log:705`)
- T33 network markers preserved:
  - `NET: Network initialization complete` at line 268
  - `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)` at line 269
  - `NET: synchronously cleared virtio callback suppression` at line 270
  - `NET: pre-primed NetRx softirq for bootstrap callback re-enable` at line 271
- External ping: PASS, `1 packets transmitted, 1 packets received, 0.0% packet loss`
  (`turn39-artifacts/boot-parallels/live-ping.txt`)
- ARP resolved: `00:1c:42:0e:f8:53`
  (`turn39-artifacts/boot-parallels/live-arp.txt`)

## Hypothesis Verdict

T39 hypothesis was false: adding only five dead atomic statics to
`gpu_mmio.rs` did **not** trip the Parallels CPU0 guard.

The BSS-only/statics-only delta is Parallels CPU0 safe in this single-boot
probe. The T38 trigger is therefore more likely in one of the later scaffold
additions: `get_irq()`, init recording, or the handler body.

## Proposed T40 First Step

Proceed with the directive's PASS path: add `get_irq()` only, with no handler,
no init recording, no GIC enable, and no send-command changes, then repeat the
single Parallels CPU0/ping validation.
