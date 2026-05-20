# Turn 19 validation

Status: profile/plan-only turn, no source changes intended.

Read-only investigation commands included:

- `rg -n "raise_softirq\\(" kernel/src`
- `rg -n "raise_softirq_irq|SoftirqType::NetTx|SoftirqType::NetRx" kernel/src`
- `rg -n "re_enable_irq\\(" kernel/src`
- `rg -n "do_softirq\\(|irq_exit|softirq" kernel/src/arch_impl kernel/src/interrupts kernel/src/task kernel/src/per_cpu.rs`
- targeted reads of `kernel/src/per_cpu_aarch64.rs` and
  `kernel/src/arch_impl/aarch64/percpu.rs` for aarch64 pending-bit and
  IRQ-exit behavior
- timer-file grep for `NetRx`, `raise_softirq`, `process_rx(`, and
  `re_enable_irq(`
- targeted serial-log greps over Turn 16/17 Parallels boot artifacts

Findings:

- Production `NetRx` raises exist for x86 e1000 and aarch64 VirtIO MMIO net.
- PCI VirtIO net does not raise `NetRx`.
- The documented 10ms timer NetRx raiser is not present in timer source.
- `net_pci::re_enable_irq()` is only called by the registered network softirq
  handler, so PCI cannot re-enable itself after its own interrupt unless
  something else raises `NetRx`.
- Turn 16/17 Parallels logs show PCI net initialized and init ARP resolved by
  synchronous polling with `msi_count=0`.

Artifacts produced:

- `turn19-artifacts/stale-contract-investigation.md`
- `turn19-artifacts/kernel-diff-stat.txt`
- `turn19-conversion-plan.md`
- `turn19-validation.md`

Kernel diff sanity check:

- `turn19-artifacts/kernel-diff-stat.txt` was generated with
  `git diff --stat kernel/`.
- Expected result: empty output, meaning no changes under `kernel/`.

Recommendation:

- No source-changing Substep 0 is required.
- Start source conversion with Substep 1: budgeted `process_rx()`.
- Then make PCI MSI schedule NetRx using the budgeted completion path.
