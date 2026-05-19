# Turn 3 Experiment: CPU0 interrupt-driven AHCI

## A. Diff stats

Source commit:

- `dbb8a777 fix(ahci): restore CPU0 interrupt-driven AHCI (revert ede8246a polling gate)`
- `kernel/src/drivers/ahci/mod.rs`: removed the aarch64 polling gate, restored platform IRQ registration, and added the memory-only polling honesty counter.
- `kernel/src/main_aarch64.rs`: removed the post-SMP `enable_spi_on_cpu(ahci_irq, 2)` reroute so AHCI stays on CPU0 for this experiment.
- Source diff stat: `2 files changed, 12 insertions(+), 37 deletions(-)`.

The relevant restored platform registration is:

```rust
gic::clear_spi_pending(found_spi);
AHCI_IRQ_EDGE.store(false, Ordering::Release);
AHCI_IRQ.store(found_spi, Ordering::Release);
gic::enable_spi(found_spi);
```

The honesty counter is exported as the GDB-visible symbol `ahci_polled_completion_count` and increments once whenever `wait_cmd_slot0()` selects the polling branch.

## B. Build result

ARM64 kernel build command:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Result: clean. Exit 0, zero compiler warnings.

The Parallels harness also rebuilt/deployed through `./run.sh --parallels`; the kernel rebuild step completed cleanly. `git diff --check` was clean.

## C. Boot outcome

Single Parallels boot used fresh epoch VM `breenix-1779181010`.

Outcome: **regression branch**. AHCI IRQs did fire on CPU0, then the CPU0 timer stopped and the 30-second CPU0 regression alarm panicked.

Serial evidence:

```text
[ahci] Platform IRQ probe: discovered SPI 34
[ahci] Platform IRQ enabled: SPI 34 (wired, level-triggered, CPU0)
[drivers] AHCI initialized (platform MMIO): 2 SATA device(s)
...
[freeze-watch] ... timer_ticks_cpu0=5 timer_ticks_cpu1=27423 ...
!!! CPU0 REGRESSION ALARM !!!
CPU0 tick_count = 5, max peer = 30000
panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17:
CPU0 timer regression: tick_count=5 but peer max=30000
```

No AHCI timeout markers were present in the captured serial log. The VM was stopped and deleted after endpoint capture; only the unrelated `linux-probe` VM remained in `prlctl list --all`.

## D. GDB endpoint state

Endpoint capture succeeded (`gdb_rc=0`):

```text
ahci_irq=34
ahci_isr_count=113
ahci_isr_last_mpidr_aff0=0
ahci_polled_completion_count=72
timer_tick_count_cpu0=5
timer_tick_count_cpu1=30000
timer_tick_count_cpu2=30000
timer_tick_count_cpu3=30000
timer_tick_hw_count_cpu0=5
timer_tick_hw_count_cpu1=29999
timer_tick_hw_count_cpu2=29999
timer_tick_hw_count_cpu3=29999
timer_interrupt_count=209998
```

Interpretation:

- AHCI IRQ registration worked: `ahci_irq=34`.
- AHCI interrupt delivery worked at least partially: `ahci_isr_count=113`.
- The AHCI ISR ran on CPU0: `ahci_isr_last_mpidr_aff0=0`.
- CPU0 timer delivery regressed exactly as in the old failure class: CPU0 stayed at 5 ticks while peers reached roughly 30000.

This falsifies the "CPU0 AHCI is healthy in Breenix once polling is removed" hypothesis. It does not falsify the Linux evidence; it localizes the remaining problem to Breenix's CPU0 IRQ handling path for this level SPI.

## E. Honesty test result

`ahci_polled_completion_count=72`.

That means polling was still selected for some commands in this boot. Given the code path, the likely contributors are early AHCI setup/probe commands before `AHCI_IRQ` is stored and early boot commands before the scheduler/timer sleep path is active. Still, per the directive, the result is not a success case: polling was nonzero while AHCI ISR count was also nonzero.

The boot also independently failed the CPU0 timer criterion, so the final classification does not depend on interpreting the polling count.

Artifacts:

- `turn3-artifacts/single-boot-serial.log`
- `turn3-artifacts/gdb-endpoint-state.log`
- `turn3-artifacts/polling-counter.txt`
- `turn3-artifacts/single-boot-run/`
- `turn3-artifacts/run_ahci_cpu0_single_boot.sh`

## F. Status and Turn 4 scope

**Status: INCONCLUSIVE.**

The experiment reached the directive's regression branch: AHCI IRQs fired on CPU0, but CPU0 vtimer delivery died. The bug is not absence of AHCI IRQ routing, and not the old CPU2 IROUTER reroute failure. The next target should be the Breenix IRQ tail/nested-window path for level-triggered SPIs.

Named Turn 4 scope:

1. Inspect and compare `kernel/src/arch_impl/aarch64/exception.rs::handle_irq()` against Linux GICv3 IRQ flow, focused on `priority_drop_irq()` before device handling, `reopen_nested_irq_window`, and delayed `deactivate_irq()`.
2. Run a narrow experiment that keeps AHCI CPU0 delivery but prevents nested IRQ reopening for AHCI/level SPIs, or otherwise moves deactivate/EOI ordering to match the safe Linux flow.
3. Keep `timer_interrupt.rs`, `context_switch.rs`, and GIC redistributor gold-master regions untouched.
4. Refine the polling honesty counter if needed so it separates pre-IRQ/early-boot polling from post-registration normal command polling.
