# Turn 2 Blocker: CPU0 Timer I/O Hot Path Demolition

Status: INCONCLUSIVE

## Scope attempted

Turn 2 implementation attempted to remove the CPU0 timer I/O polling root:

- Removed timer ISR callouts for xHCI HID polling, EHCI keyboard polling, VirtIO input polling, and timer-raised network RX softirq.
- Deleted `xhci::poll_hid_events()` and moved its load-bearing pieces toward IRQ/workqueue paths.
- Removed xHCI SPI disable-before-`try_lock()` in the IRQ handler and added lock-contention tracing.
- Moved xHCI endpoint reset recovery out of the timer path into process context.
- Requeued xHCI transfer events inline while waiting for command completion.
- Added tracing counters for xHCI IRQ entry, MSI event delivery, and lock contention.
- Moved VirtIO net RX softirq scheduling to the IRQ path.

The implementation was reverted per the Turn 2 directive after runtime criteria failed.

## Build verification

Both build gates completed cleanly during the attempted implementation:

```text
/tmp/breenix-turn2-aarch64-build.log: no warning/error lines
/tmp/breenix-turn2-x86-build.log: no warning/error lines
```

Commands used:

```bash
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

## Runtime evidence

First Parallels attempt failed early because the new xHCI recovery thread was spawned during driver init before the scheduler was initialized:

```text
panicked at kernel/src/task/scheduler.rs:2311:13:
Scheduler not initialized
```

That was fixed locally by starting the recovery thread from `activate_msi_if_ready()` instead of xHCI init.

Second Parallels attempt reached userspace PID 1:

```text
[boot] USB HID input active via XHCI IRQ
[xhci] post-activation: MSI_EVENT_COUNT=0 EVENT_COUNT=0 XHCI_MSI_EVENT_TOTAL=0 XHCI_IRQ_ENTRY_TOTAL=0 XHCI_LOCK_CONTENDED_TOTAL=0 SPI_ACTIVATED=true
Breenix ARM64 Boot Complete!
[boot] Launching init from pre-loaded ELF...
manager.create_process_with_argv [ARM64]: Generated PID 1
EL0_SYSCALL: First syscall from userspace (SPSR confirms EL0)
[init] Breenix init starting (PID 1)
```

The serial log did not contain `poll_hid_events` or `XHCI_DIAG` output during the second run.

However, the run failed the "no new kernel panics" criterion because the protected CPU0 regression alarm fired:

```text
!!! CPU0 REGRESSION ALARM !!!
CPU0 tick_count = 71, max peer = 30000
panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17:
CPU0 timer regression: tick_count=71 but peer max=30000; read docs/planning/cpu0-user-guard-autopsy/README.md before touching anything
```

It also did not satisfy the xHCI MSI evidence criterion before the panic. The only available post-activation counter line still showed:

```text
XHCI_MSI_EVENT_TOTAL=0 XHCI_IRQ_ENTRY_TOTAL=0 XHCI_LOCK_CONTENDED_TOTAL=0
```

## Blocker

Turn 2 cannot be called complete under the current runtime gate because Parallels reaches userspace but then trips the gold-master CPU0 regression alarm before the 90-second evidence window can complete. The alarm is explicitly protected by the directive and was not modified.

A secondary evidence gap remains: the implementation needs a later, explicit xHCI trace-counter read after userspace starts and after an interrupt-generating stimulus. The current boot-time post-activation print is too early to prove sustained MSI delivery.

## Proposed Turn 3

Use one of these narrower next-turn contracts:

1. CPU0 gate first: investigate the existing CPU0 regression alarm with GDB/nonintrusive tracing and decide whether it is pre-existing, newly exposed, or caused by removing timer I/O work. Do not modify the protected alarm without explicit approval.
2. Short runtime gate: rerun the polling removal under a shorter pass window that stops immediately after PID1 and a forced xHCI interrupt/counter read, before the known CPU0 alarm window.
3. Evidence gate: add an on-demand, non-periodic xHCI counter dump reachable after userspace starts, then rerun Turn 2 so `MSI_EVENT > 0` is proved by a concrete counter read rather than by the early boot diagnostic.

Artifacts from the failed attempt:

- `/tmp/breenix-turn2-aarch64-build.log`
- `/tmp/breenix-turn2-x86-build.log`
- `/tmp/breenix-turn2-parallels-run.log`
- `/tmp/breenix-turn2-parallels-run2.log`
- `/tmp/breenix-parallels-serial.log`
- `/tmp/breenix-screenshot.png`
