# Turn 36: CPU0 regression hypotheses for T35

## Evidence baseline

T35 added two categories of code:

- An aarch64 SPI dispatch branch in `exception.rs` that calls
  `gpu_mmio::get_irq()` and, on equality, `gpu_mmio::handle_interrupt()`
  (`turn35-artifacts/source-diff.txt:1-17`).
- MMIO GPU IRQ state, counters, enablement, and a hard IRQ observer inside
  `gpu_mmio.rs` (`turn35-artifacts/source-diff.txt:37-139`), wired from
  `gpu_mmio::init_device()` after `device.driver_ok()`
  (`turn35-artifacts/source-diff.txt:157-176`).

The T35 boot was Parallels, and Parallels initialized the PCI GPU path:
`[virtio-gpu-pci] MSI-X enabled: config_spi=53 queue_spi=54` and
`[drivers] VirtIO GPU (PCI) initialized`
(`turn35-artifacts/boot-1-gpu-path-lines.txt:5-20`). It reached heartbeat
`uptime_ms=38225`, then tripped the CPU0 regression alarm with
`CPU0 tick_count = 70, max peer = 30000`
(`turn35-artifacts/boot-1-health-lines.txt:40-44`).

## Hypothesis 1: SPI dispatch branch in exception.rs

Verdict: consistent with T35 evidence and the most likely T35-specific trigger.

The branch calls `gpu_mmio::get_irq()` inside the SPI dispatch block before the
existing XHCI branch (`turn35-artifacts/source-diff.txt:5-17`). `get_irq()`
performs one atomic load of `MMIO_SLOT` and returns `None` while the slot is
uninitialized (`turn35-artifacts/source-diff.txt:52-60`). On Parallels, MMIO GPU
init does not run, so this branch should be cheap and should not call
`handle_interrupt()`. Still, it is the only T35 change that is plausibly
executed on the Parallels PCI-GPU boot path, because the rest of the added MMIO
GPU code is behind an initialization path Parallels does not take.

This does not directly explain CPU0 timer starvation: the CPU0 timer is a PPI,
not this SPI branch. The plausible mechanism is timing/code-layout sensitivity
in early interrupt/scheduler behavior, not MMIO GPU work. That matches the
T27 lesson that a dormant kthread changed early post-EL0 scheduling enough to
trip CPU0 before the intended IRQ/MSI-X experiment ran
(`turn27-validation.md:70-79`).

Isolation modification: reapply the T35 `gpu_mmio.rs` additions but remove only
the `exception.rs` dispatch branch. If Parallels boots cleanly, this branch or
its codegen/timing effect is the trigger.

## Hypothesis 2: init-time enable_mmio_irq()

Verdict: inconsistent with T35 Parallels evidence.

`enable_mmio_irq(slot)` would enable `VIRTIO_IRQ_BASE + slot` and print
`[virtio-gpu] MMIO IRQ ... enabled` (`turn35-artifacts/source-diff.txt:78-87`).
It is called only after `gpu_mmio::init_device()` marks the MMIO GPU driver OK
(`turn35-artifacts/source-diff.txt:157-168`). T35's boot log shows PCI GPU
initialization, not MMIO GPU initialization
(`turn35-artifacts/boot-1-gpu-path-lines.txt:5-20`), and the T35 MMIO IRQ
evidence file is empty. Therefore T35 does not support the idea that
`enable_mmio_irq()` enabled a Parallels SPI or conflicted with CPU0's timer.

Isolation modification: keep the dispatch branch but delete or `return` from
`enable_mmio_irq()` before `Gicv2::enable_irq()`. On Parallels this should be a
no-op test because the function is not reached; any changed result would imply
unexpected reachability.

## Hypothesis 3: atomic counter contention

Verdict: inconsistent with T35 evidence.

The added counters are atomics in `gpu_mmio.rs`
(`turn35-artifacts/source-diff.txt:37-44`). Counter increments happen only in
`handle_interrupt()` after `MMIO_BASE` is non-zero and the MMIO interrupt status
register is non-zero (`turn35-artifacts/source-diff.txt:105-139`). On the T35
Parallels boot, `MMIO_SLOT` should remain the uninitialized sentinel because
`gpu_mmio::init_device()` is not reached, so `get_irq()` returns `None` and the
handler is not called (`turn35-artifacts/source-diff.txt:52-60`).

There is one atomic load per SPI through `get_irq()`, but no hot counter
contention and no IRQ-side fetch-add contention.

Isolation modification: replace the counter increments in `handle_interrupt()`
with no-ops while leaving `get_irq()` and dispatch intact. On Parallels this
should not affect behavior because `handle_interrupt()` should not be called.

## Hypothesis 4: PCI-GPU versus MMIO-GPU SPI overlap

Verdict: inconsistent for the actual T35 Parallels boot; worth guarding for a
future QEMU/MMIO implementation.

T35's PCI GPU queue vector used SPI 54
(`turn35-artifacts/boot-1-gpu-path-lines.txt:5-13`). T35's MMIO IRQ mapping
would use `VIRTIO_IRQ_BASE + slot`, with `VIRTIO_IRQ_BASE = 48`
(`turn35-artifacts/source-diff.txt:37-38`, `turn35-artifacts/source-diff.txt:52-60`).
If an MMIO GPU were in slot 6, it would also map to SPI 54.

That overlap did not happen in T35 because Parallels did not initialize
`gpu_mmio::init_device()`, did not store `MMIO_SLOT`, and did not call
`enable_mmio_irq()` (`turn35-artifacts/source-diff.txt:71-87`,
`turn35-artifacts/source-diff.txt:157-168`). The actual T35 log is PCI-only
(`turn35-artifacts/boot-1-gpu-path-lines.txt:5-20`).

Isolation modification: make `get_irq()` return `None` on non-QEMU platforms or
use a hard `if !platform_config::is_qemu() { return None; }` guard before the
SPI comparison. If this changes Parallels behavior, the mere dispatch probe is
the trigger; if it does not, SPI overlap was not involved.

## Most likely candidate

The only T35 addition plausibly executed under Parallels is the new SPI
dispatch probe in `exception.rs`. The rest of the MMIO GPU IRQ scaffold appears
unreached on the T35 target. Therefore the most likely T35-specific root cause
is not a live MMIO interrupt handler but a small hot-path SPI-dispatch
perturbation exposing the existing CPU0 scheduler/timer fragility.

This should be treated as a bisectable trigger, not a proven architectural root
cause. The CPU0 autopsy documents the stable symptom: CPU0 tick count advances
briefly, then falls one to two orders of magnitude behind peers
(`docs/planning/cpu0-user-guard-autopsy/README.md:28-40`,
`docs/planning/cpu0-user-guard-autopsy/README.md:169-180`). It also warns that
new CPU0-specific theories need per-CPU tick parity evidence
(`docs/planning/cpu0-user-guard-autopsy/README.md:207-214`).

## Link to T22/T26/T27

T22, T26, T27, and T35 share the same failure signature but not enough evidence
to claim the same direct cause:

- T22's async TX ownership attempt built cleanly, then Parallels hit
  `CPU0 tick_count = 75, max peer = 30000` at the same panic site
  (`turn22-validation.md:1-5`, `turn22-validation.md:43-55`).
- T26's deferred ARP-primer attempt hit CPU0 at tick count 5 after network,
  softirq, tracing, primer spawn, and MSI-X enable ordering
  (`turn26-validation.md:64-91`).
- T27 narrowed that pattern: the dormant primer failed before it ran, before
  `NET_ARP_PRIMER_RAN`, and before MSI-X/IRQ enable, proving that the intended
  IRQ action was not necessary to trigger CPU0 starvation
  (`turn27-validation.md:50-79`).
- T35 repeats that shape: code intended to observe an MMIO interrupt never
  reached the MMIO path on Parallels, yet the CPU0 alarm still fired
  (`turn35-artifacts/boot-1-gpu-path-lines.txt:5-20`,
  `turn35-artifacts/boot-1-health-lines.txt:40-44`).

The shared pattern is "additive early IRQ/scheduler-adjacent changes can perturb
Parallels enough to trip the CPU0 guard before the intended experiment runs."
The revised P9 plan should avoid using Parallels as the pass/fail target for
MMIO GPU changes and should split any Parallels-visible dispatch change into its
own tiny bisect step.
