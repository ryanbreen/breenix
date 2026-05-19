# Turn 5: Narrow level-SPI nested IRQ fix

**Status: COMPLETE for the narrow fix single-boot gate.**

The Turn 5 change fixed the Turn 3 failure mode: AHCI interrupts still fire on
CPU0, and CPU0 timer delivery no longer collapses. The polling counter remains
nonzero at `72`, matching Turn 3's value; I am treating that as an honesty-test
caveat and recommending Turn 6 include either the 5-boot gate alone or a
pre/post-IRQ-registration counter split if strict zero-polling proof is needed.

## A. Code diff

Source commit:

- `6e837ff4 fix(ahci): defer nested-IRQ unmask until level SPI is serviced`

Files changed:

- `kernel/src/arch_impl/aarch64/gic.rs`
- `kernel/src/arch_impl/aarch64/exception.rs`

The GIC helper added beside the SPI trigger configuration helpers:

```rust
pub fn is_spi_level_triggered(irq: u32) -> bool {
    if irq < 32 {
        return false;
    }

    let reg_index = irq / 16;
    let field = (irq % 16) * 2;
    let current = gicd_read(GICD_ICFGR + (reg_index as usize * 4));

    (current & (0b10 << field)) == 0
}
```

The IRQ tail gate changed from unconditional reopen based only on interrupted
PSTATE:

```rust
let reopen_nested_irq_window = interrupted_context_had_irqs_unmasked(frame);
```

to:

```rust
let reopen_nested_irq_window =
    interrupted_context_had_irqs_unmasked(frame) && !gic::is_spi_level_triggered(irq_id);
```

That preserves existing behavior for edge SPIs and SGI/PPI interrupts, but
keeps regular IRQs masked while a level-triggered external SPI handler clears
its device source.

## B. Build result

Command:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Result: clean. Exit 0, zero compiler warnings.

`git diff --check` was clean before the source commit.

## C. Boot outcome

Single fresh Parallels VM:

- VM name: `breenix-1779183335`
- Harness: `turn5-artifacts/run_ahci_level_spi_single_boot.sh`
- Wait window: 90 seconds after VM start
- Cleanup: VM stopped/deleted; only `linux-probe` remained in `prlctl list --all`
- Required QEMU cleanup reported `All QEMU processes killed`

Outcome: **single-boot success for the nested IRQ fix.**

Evidence:

- AHCI platform IRQ registered: serial shows SPI 34 discovered and enabled as
  wired level-triggered CPU0.
- AHCI ISR fired on CPU0: `ahci_isr_count=1125`,
  `ahci_isr_last_mpidr_aff0=0`.
- CPU0 timer stayed healthy: `timer_tick_count_cpu0=57474`, peer max `57507`.
  CPU0 was at 99.94% of peer max.
- No AHCI timeout markers.
- No `CPU0 REGRESSION ALARM`, panic, data abort, or synchronous exception
  markers.
- Freeze-watch/BWM continued through the capture window with normal frame
  progress; late serial showed `timer_ticks_cpu0=56485` alongside peers around
  `56433..56542`.

The harness `cpu0_timer_alarm_markers=29` count is a false positive from its
broad grep matching ordinary `freeze-watch` lines containing `timer_ticks_cpu0`.
Direct grep for `REGRESSION`, `ALARM`, `panicked`, `KERNEL PANIC`, `Data Abort`,
`Synchronous exception`, and AHCI timeout patterns returned no matches.

## D. GDB endpoint state

Endpoint capture succeeded with `gdb_rc=0`:

```text
ahci_irq=34
ahci_isr_count=1125
ahci_isr_last_mpidr_aff0=0
ahci_polled_completion_count=72
timer_tick_count_cpu0=57474
timer_tick_count_cpu1=57497
timer_tick_count_cpu2=57507
timer_tick_count_cpu3=57404
timer_tick_hw_count_cpu0=57474
timer_tick_hw_count_cpu1=57497
timer_tick_hw_count_cpu2=57507
timer_tick_hw_count_cpu3=57404
timer_interrupt_count=459554
gdb_rc=0
```

This is the inverse of Turn 3: AHCI still interrupts on CPU0, but CPU0 timer
delivery stays in lockstep with the other CPUs instead of freezing at 5 ticks.

## E. Honesty test result

`ahci_polled_completion_count=72`.

This is not zero, so I am not claiming strict zero-polling proof. It is the same
counter value observed in Turn 3, while AHCI ISR count rose from `113` to `1125`
and the CPU0 timer recovered fully. That strongly suggests the remaining 72
polls are early setup/probe or pre-scheduler operations, not the mechanism
keeping normal AHCI IO alive after IRQ registration.

Turn 6 should decide whether to:

- run the planned 5-boot gate immediately, accepting this as the known early
  polling caveat; or
- first split the polling counter into pre-IRQ-registration and
  post-IRQ-registration buckets for strict proof.

Do not restore polling and do not weaken the honesty criterion.

## F. Status and Turn 6 scope

**Status: COMPLETE for Turn 5's narrow fix.**

The load-bearing divergence was the regular nested IRQ window before servicing
level SPIs. Gating that window fixed the CPU0 timer collapse without disabling
AHCI interrupts.

Named Turn 6 scope:

1. Run a 5-boot Parallels stress gate on commit `6e837ff4`.
2. Require every boot to show `ahci_isr_count > 100`, CPU0 timer ticks within
   10% of peer max, no AHCI timeout, and no CPU0 regression panic.
3. Track `ahci_polled_completion_count` per boot. If Claude/operator requires
   strict zero proof, split the counter into early/pre-registration and
   post-registration counters before or during the gate.

Artifacts:

- `turn5-artifacts/run_ahci_level_spi_single_boot.sh`
- `turn5-artifacts/single-boot-serial.log`
- `turn5-artifacts/serial-signals.log`
- `turn5-artifacts/gdb-endpoint-state.log`
- `turn5-artifacts/polling-counter.txt`
- `turn5-artifacts/single-boot-run/`

