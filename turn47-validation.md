# Turn 47 Validation

## Directive

Move the `record_mmio_irq_state(base, slot)` call site out of
`init_device()` and into the MMIO probe loop after `init_device()` returns
successfully.

## Diff Scope

`turn47-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 5 ++++-
 1 file changed, 4 insertions(+), 1 deletion(-)
```

The attempted source kept `init_device()`'s body unchanged and placed the call
in the MMIO probe loop.

## Builds

- `./userspace/programs/build.sh --arch aarch64`: PASS
- `./scripts/create_ext2_disk.sh --arch aarch64`: PASS
- `cargo build --release --target aarch64-breenix.json ... -p kernel --bin kernel-aarch64`: PASS
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`: PASS
- `scripts/parallels/build-efi.sh --kernel`: PASS

Warning/error grep artifacts:

- `turn47-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn47-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Parallels Boot

Single Parallels boot: FAIL.

Evidence:

- CPU0 regression scan: `turn47-artifacts/boot-parallels/boot-1-cpu0-regression-scan.txt`, 780 bytes
- First alarm: `line 349: !!! CPU0 REGRESSION ALARM !!!`
- Alarm state: `line 350: CPU0 tick_count = 5, max peer = 30000`
- Panic site: `line 356: panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17`
- No regular `[timer] cpu0 ticks=...` markers were emitted before the guard fired.
- T33 bootstrap markers were reached before the alarm:
  - `line 268: NET: Network initialization complete`
  - `line 269: [virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
  - `line 270: NET: synchronously cleared virtio callback suppression`
  - `line 271: NET: pre-primed NetRx softirq for bootstrap callback re-enable`
- Ping after boot window: 1 transmitted, 0 received, 100.0% packet loss
- ARP still resolved `10.211.55.100` to `00:1c:42:e2:49:ff`

After capturing the failure, the probe-loop restructure was removed, restoring
the T45 source state.

## Verdict

INCONCLUSIVE (FAIL). Relocating the call one stack frame up still trips the
Parallels CPU0 guard. The trigger is broader than an `init_device()` body call:
calling `record_mmio_irq_state()` from the compiled init-time MMIO path also
triggers the regression.

Next step: pivot to the statics-free path. Avoid calling
`record_mmio_irq_state()` anywhere from the init-time path; instead, make the
MMIO interrupt handler discover or read the MMIO transport state directly, or
otherwise avoid this function call pattern entirely.
