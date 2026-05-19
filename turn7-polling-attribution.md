# Turn 7 Polling Attribution

## A. Counter split and scheduler-ready predicate

Source commit: `6b588f3e feat(ahci): split post-registration polling counter by scheduler state`.

`kernel/src/drivers/ahci/mod.rs` now exports four in-memory counters:

- `ahci_polled_completion_count`: total `PORT_CI` polling completions.
- `ahci_polled_post_registration_count`: polling completions after `AHCI_IRQ` was known at command setup.
- `ahci_polled_post_reg_pre_scheduler`: post-registration polling while the scheduler-sleep predicate was false.
- `ahci_polled_post_reg_scheduler_running`: post-registration polling while the scheduler-sleep predicate was true.

The scheduler-sleep predicate is centralized in `scheduler_sleep_ready(has_irq)` at `kernel/src/drivers/ahci/mod.rs:725`. It matches the command setup decision: `has_irq && current_thread_id().is_some() && timer_is_running()` on AArch64. `setup_cmd_slot0()` uses it at `kernel/src/drivers/ahci/mod.rs:1167`, and the polling branch recomputes it at `kernel/src/drivers/ahci/mod.rs:806` before attributing a post-registration poll.

The recomputation matters: using `token.scheduler_running` inside the polling branch is tautologically false and lets LLVM remove the scheduler-running counter. The final build exports all four symbols:

```text
ahci_polled_completion_count
ahci_polled_post_registration_count
ahci_polled_post_reg_pre_scheduler
ahci_polled_post_reg_scheduler_running
```

## B. Polling callsite map

`wait_cmd_slot0()` has one combined early path and three split block-device paths.

1. `identify_device()` -> `issue_cmd_slot0()` -> `wait_cmd_slot0()`
   - `identify_device()` issues IDENTIFY at `kernel/src/drivers/ahci/mod.rs:1746`.
   - `issue_cmd_slot0()` calls `setup_cmd_slot0()` then `wait_cmd_slot0()` at `kernel/src/drivers/ahci/mod.rs:1218`.
   - This runs from `init_common()` -> `init_port()` at `kernel/src/drivers/ahci/mod.rs:946` and `kernel/src/drivers/ahci/mod.rs:1029`.
   - It is pre-IRQ-registration on Parallels because `probe_platform_irq()` runs only after `init_common()` returns, at `kernel/src/drivers/ahci/mod.rs:877` and `kernel/src/drivers/ahci/mod.rs:882`.
   - It must complete synchronously because the driver cannot expose block capacity or register a usable block device before IDENTIFY completes.

2. `probe_platform_irq()`
   - Expected by the directive, but it does not call `wait_cmd_slot0()`.
   - It manually issues an IDENTIFY command at `kernel/src/drivers/ahci/mod.rs:2167` and polls `PORT_CI` at `kernel/src/drivers/ahci/mod.rs:2169`.
   - It deliberately preserves `PORT_IS` so GIC pending-state can identify the wired SPI, then stores `AHCI_IRQ` at `kernel/src/drivers/ahci/mod.rs:2265`.
   - It is synchronous and not deferrable because later interrupt-driven completions need the SPI number.

3. `AhciBlockDevice::read_blocks()` -> `setup_read_sectors()` -> `wait_cmd_slot0()`
   - `setup_read_sectors()` issues a read at `kernel/src/drivers/ahci/mod.rs:1851`.
   - `read_blocks()` waits at `kernel/src/drivers/ahci/mod.rs:2600`.
   - Root ext2 mounting starts before scheduler initialization in `kernel/src/main_aarch64.rs:577`, after AHCI IRQ registration, so AHCI block reads here are post-registration but pre scheduler-sleep readiness.
   - `/sbin/init` preload is also before timer init at `kernel/src/main_aarch64.rs:842` through `kernel/src/main_aarch64.rs:864`; file reads go through `read_init_from_ext2()` at `kernel/src/main_aarch64.rs:44`.
   - These reads must complete synchronously because boot needs the root filesystem and init ELF before entering userspace.

4. `AhciBlockDevice::write_block()` -> `setup_write_sector()` -> `wait_cmd_slot0()`
   - `setup_write_sector()` issues at `kernel/src/drivers/ahci/mod.rs:1909`.
   - `write_block()` waits at `kernel/src/drivers/ahci/mod.rs:2668`.
   - This path is not used by the observed boot's pre-timer filesystem reads, but if an early write occurred before timer/current-thread readiness, it would be attributed by the same counters.

5. `AhciBlockDevice::flush()` -> `setup_flush_port()` -> `wait_cmd_slot0()`
   - `setup_flush_port()` issues at `kernel/src/drivers/ahci/mod.rs:1942`.
   - `flush()` waits at `kernel/src/drivers/ahci/mod.rs:2708`.
   - This was not observed in the boot path. If invoked before scheduler-sleep readiness, it would also be a synchronous early-boot poll.

## C. Linux boot-probe comparison

Linux v6.8 is not a pure "interrupt everything immediately" model during AHCI boot probe:

- AHCI driver activation requests/registers the IRQ before libata schedules async port probes. The path is `ahci_init_one()` -> `ahci_host_activate()` -> `ata_host_activate()` -> `ata_host_register()`; relevant source lines are recorded in `turn7-artifacts/linux-source-refs.txt`.
- Boot probing is driven through libata EH. `ata_port_probe()` schedules EH, `ata_eh_recover()` resets the link, and `ata_eh_revalidate_and_attach()` reads IDENTIFY for newly found devices.
- AHCI soft reset is explicitly polled: `ahci_do_softreset()` calls `ahci_exec_polled_cmd()` for the two SRST FISes.
- IDENTIFY is marked with `ATA_TFLAG_POLLING` in `ata_dev_read_id()`. For SFF this queues polling PIO work; for AHCI, `ahci_qc_issue()` itself just writes `PORT_CMD_ISSUE`, so the source distinction is that Linux uses polled AHCI reset operations and libata polling semantics for IDENTIFY, while normal AHCI queued-command completion can still be interrupt-backed once IRQs are registered.
- Turn 1's `ahci_exec_polled_cmd* = 0` was a runtime disk-load measurement, not a boot-probe measurement. It does not disprove Linux's polled reset/probe behavior.

The Breenix result is therefore aligned with the Linux pattern at the policy level: synchronous boot/device-discovery work may poll; normal post-scheduler block I/O should not.

## D. Single-boot endpoint state

Harness: `turn7-artifacts/run_polling_attribution_boot.sh`.

Fresh VM: `breenix-1779188088`.

Build gate:

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
Finished release profile with zero warnings.
```

GDB endpoint capture from `turn7-artifacts/gdb-endpoint-state.log`:

```text
ahci_irq=34
ahci_isr_count=1125
ahci_isr_last_mpidr_aff0=0
ahci_polled_completion_count=72
ahci_polled_post_registration_count=70
ahci_polled_post_reg_pre_scheduler=70
ahci_polled_post_reg_scheduler_running=0
timer_tick_count_cpu0=81894
timer_tick_count_cpu1=81703
timer_tick_count_cpu2=81774
timer_tick_count_cpu3=81824
timer_interrupt_count=655718
gdb_rc=0
```

Serial health:

- AHCI SPI discovered and enabled: SPI 34, level-triggered, CPU0.
- `/sbin/init` preloaded: 296824 bytes.
- Userspace reached: first EL0 syscall marker present.
- AHCI timeout markers: 0.
- Panic/data-abort/synchronous-exception markers: 0.
- QEMU cleanup: `All QEMU processes killed`.
- Parallels cleanup: no `breenix-*` VMs remained; only the unrelated `linux-probe` VM remained.

## E. Decision

`ahci_polled_post_reg_scheduler_running == 0`.

All 70 post-registration polls occurred while the scheduler-sleep predicate was false. This means the Turn 6 strict criterion was too broad: "no polling ever after IRQ registration" incorrectly counted the boot/pre-timer window.

Updated success criterion:

```text
ahci_polled_post_reg_scheduler_running == 0
```

That criterion captures the actual regression bar: no AHCI polling after the kernel is capable of sleeping on interrupt-driven completions.

## F. Status

COMPLETE.

Turn 7 confirms the hypothesis. The branch is ready for PR from the AHCI perspective with the success criterion redefined as no post-scheduler-running AHCI polling.
