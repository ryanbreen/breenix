# Turn 40 Validation - P9 Add get_irq Only

## Source Scope

`turn40-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 14 ++++++++++++++
 1 file changed, 14 insertions(+)
```

Only `kernel/src/drivers/virtio/gpu_mmio.rs` was changed. The retained source
delta adds one public, uncalled `get_irq()` bisect function on top of T39's
five statics. No snapshot reader, init recording, interrupt handler, GIC
enable, exception dispatch, or command path changes were added.

Directive note: existing `VIRTIO_IRQ_BASE` constants in sibling MMIO drivers
are private module-local constants, so importing the named constant would have
required touching a second file. To preserve the stricter one-file/one-function
scope, `get_irq()` uses the same existing VirtIO MMIO IRQ base value inline.

## Build Status

- Userspace aarch64 build: PASS (`turn40-artifacts/build-userspace.log`)
- aarch64 ext2 image: PASS (`turn40-artifacts/build-ext2.log`)
- aarch64 kernel: PASS (`turn40-artifacts/build-aarch64.log`)
- x86 release/test kernel: PASS (`turn40-artifacts/build-x86.log`)
- Parallels EFI image: PASS (`turn40-artifacts/build-efi.log`)
- Warning/error greps:
  - `turn40-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
  - `turn40-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Runtime Result

Single Parallels boot completed the requested 60-second test window and
continued producing serial output past 104 seconds.

- CPU0 regression scan: PASS, 0 bytes
  (`turn40-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`)
- Heartbeat: PASS, reached `uptime_ms=104230`
  (`turn40-artifacts/boot-parallels/boot-1-serial.log:699`)
- CPU0 timer ticks: PASS, reached `cpu0 ticks=65000`
  (`turn40-artifacts/boot-parallels/boot-1-serial.log:684`)
- T33 network markers preserved:
  - `NET: Network initialization complete` at line 268
  - `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)` at line 269
  - `NET: synchronously cleared virtio callback suppression` at line 270
  - `NET: pre-primed NetRx softirq for bootstrap callback re-enable` at line 271
- External ping: PASS, `1 packets transmitted, 1 packets received, 0.0% packet loss`
  (`turn40-artifacts/boot-parallels/live-ping.txt`)
- ARP resolved: `00:1c:42:aa:a0:b3`
  (`turn40-artifacts/boot-parallels/live-arp.txt`)

## Hypothesis Verdict

T40 hypothesis passed: adding `get_irq()` only to `gpu_mmio.rs` did **not**
trip the Parallels CPU0 guard.

Simple function-symbol presence in `gpu_mmio.rs` is therefore not sufficient to
reproduce the T38 CPU0 regression. The trigger is more likely in one of the
remaining heavier scaffold functions or their body content:
`snapshot_counters()`, `record_mmio_irq_state()`, or `handle_interrupt()`.

## Proposed T41 First Step

Proceed with the directive's PASS path: add exactly one remaining scaffold
function, with `handle_interrupt()` as the highest-signal next probe because it
is the heaviest T38 addition. Keep it uncalled, with no init recording, no GIC
enable, no exception dispatch, and no send-command changes.
