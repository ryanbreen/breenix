# Turn 19 stale-contract investigation: P6/P7/P8

Turn 18 flagged a mismatch: comments in the network path describe a 10ms
timer-raised `NetRx` softirq, but timer-file grep did not find a live raise.
This document resolves the current source graph before any conversion source
change.

## 1. What actually raises `SoftirqType::NetRx` today?

Production call sites:

- `kernel/src/drivers/e1000/mod.rs:753-764`
  - x86 e1000 interrupt handler defers RX processing by raising
    `SoftirqType::NetRx`.
- `kernel/src/drivers/virtio/net_mmio.rs:716-745`
  - aarch64 VirtIO MMIO net interrupt handler acknowledges MMIO interrupt
    status and raises `SoftirqType::NetRx`.

Non-production/test call sites:

- `kernel/src/task/softirq_tests.rs:54-70`
  - shared softirq tests raise `NetRx` directly.
- `kernel/src/test_framework/registry.rs:1883-1899`
  - ARM64 softirq registration test raises `NetRx` directly.

Substrate:

- `kernel/src/task/softirqd.rs:141-145`
  - typed `raise_softirq(SoftirqType)` maps to `per_cpu::raise_softirq(nr)`.
- On aarch64, `crate::per_cpu` aliases `per_cpu_aarch64`
  (`kernel/src/lib.rs:25-29`), and `per_cpu_aarch64::raise_softirq` sets the
  architecture pending bit (`kernel/src/per_cpu_aarch64.rs:361-369`).

Negative findings:

- There is no production `SoftirqType::NetRx` raise in
  `kernel/src/drivers/virtio/net_pci.rs`.
- There is no timer-file `NetRx` raise. Grep for
  `NetRx|raise_softirq|process_rx(|re_enable_irq(` over
  `kernel/src/arch_impl/aarch64/timer_interrupt.rs`,
  `kernel/src/interrupts/timer.rs`, and timer files under
  `kernel/src/arch_impl` returned no matches.
- The only source lines tying `NetRx` to a timer are comments in
  `kernel/src/net/mod.rs:235-240`.

Conclusion: production `NetRx` is raised by x86 e1000 IRQ and aarch64 VirtIO
MMIO IRQ only. The PCI VirtIO net path does not raise `NetRx`, and the
documented 10ms timer raiser is not live in this source snapshot.

## 2. What actually invokes `net_pci::re_enable_irq()`?

Callers:

- `kernel/src/net/mod.rs:241-247`
  - the registered network softirq handler calls `process_rx()` and then, on
    aarch64 when PCI net is initialized, calls `net_pci::re_enable_irq()`.

Non-call references:

- `kernel/src/drivers/virtio/net_pci.rs:837-838` and
  `kernel/src/drivers/virtio/net_pci.rs:868`
  - comments say timer-based NetRx will call `re_enable_irq()`.
- `kernel/src/drivers/virtio/net_pci.rs:871-928`
  - the function definition.

Conclusion: `re_enable_irq()` is only reachable if something raises `NetRx`.
Because PCI net itself does not raise `NetRx`, PCI's own interrupt path cannot
reach its re-enable path.

## 3. Live execution graph

### aarch64 PCI net post-init packet arrival

1. GIC dispatch checks the PCI net MSI INTID and calls
   `net_pci::handle_interrupt()` (`kernel/src/arch_impl/aarch64/exception.rs:1301-1305`).
2. `net_pci::handle_interrupt()` increments `NET_PCI_MSI_COUNT`, suppresses RX
   virtqueue callbacks with `VRING_AVAIL_F_NO_INTERRUPT`, disables and clears
   the GIC SPI, reads ISR status, and returns
   (`kernel/src/drivers/virtio/net_pci.rs:828-868`).
3. The aarch64 IRQ tail calls `per_cpu_aarch64::irq_exit()` and then
   `task::softirqd::do_softirq()` (`kernel/src/arch_impl/aarch64/exception.rs:1371-1378`).
4. No PCI code set the `NetRx` pending bit, so `do_softirq()` has no NetRx work
   to dispatch. The registered network handler in `kernel/src/net/mod.rs:241-247`
   does not run.
5. Because the network handler does not run, `process_rx()` is not called and
   `net_pci::re_enable_irq()` is not called.

Broken link: `net_pci::handle_interrupt()` suppresses the device and GIC but
does not raise `NetRx`. The code after IRQ exit is capable of processing
pending softirqs, but PCI net never marks one pending.

### aarch64 VirtIO MMIO net packet arrival

1. GIC dispatch calls `net_mmio::handle_interrupt()`
   (`kernel/src/arch_impl/aarch64/exception.rs:1264-1268`).
2. The handler acknowledges MMIO interrupt status and raises
   `SoftirqType::NetRx` (`kernel/src/drivers/virtio/net_mmio.rs:716-745`).
3. The same aarch64 IRQ tail runs `do_softirq()`
   (`kernel/src/arch_impl/aarch64/exception.rs:1371-1378`).
4. The registered network handler runs `process_rx()`
   (`kernel/src/net/mod.rs:241-247`).

This is the working IRQ-to-softirq shape. Its shared `process_rx()` body is
still unbounded, but the IRQ scheduling edge exists.

### Init-time PCI packet reception

Parallels boots initialize PCI net before the network stack:

- `drivers::init()` enters the PCI platform branch when ECAM exists and calls
  `virtio::net_pci::init()` (`kernel/src/drivers/mod.rs:156-226`).
- `main_aarch64` calls `drivers::init()` before `kernel::net::init()`
  (`kernel/src/main_aarch64.rs:564-574`).
- `net_pci::init()` sets up RX/TX queues, posts initial RX buffers, stores
  `DEVICE_INITIALIZED`, and configures MSI/MSI-X
  (`kernel/src/drivers/virtio/net_pci.rs:444-573`).
- On the Parallels MSI-X path, the code programs MSI-X and stores `NET_PCI_IRQ`
  but intentionally does not enable the GIC SPI until after synchronous network
  init polling (`kernel/src/drivers/virtio/net_pci.rs:374-389`).
- `net::init()` detects PCI net, sends gateway ARP, loops over `process_rx()`,
  then enables the MSI-X SPI after the synchronous ARP/ICMP loops
  (`kernel/src/net/mod.rs:280-310`, `kernel/src/net/mod.rs:348-433`).

Turn 16/17 boot logs show the practical result:

- `turn17-artifacts/boot-1-serial.log:177-193` shows VirtIO PCI net initialized
  on Parallels and MSI-X configured.
- `turn17-artifacts/boot-1-serial.log:259-270` shows the gateway ARP reply was
  resolved during init polling, with RX diagnostics showing `msi_count=0`, then
  the MSI-X SPI was enabled post-init.
- `turn16-artifacts/boot-1-serial.log:177-193` and
  `turn16-artifacts/boot-1-serial.log:259-270` show the same pattern.

Conclusion: PCI net is receiving packets on Parallels during init, but those
packets are consumed by synchronous RX polling while MSI count is still zero.
Current successful boots do not prove post-init PCI RX IRQ delivery.

## 4. Hypothesis confirmation

The static source graph confirms a live post-init bug shape:

- PCI net is the selected driver on Parallels; it is not silently falling back
  to MMIO in the observed Parallels boots.
- Init-time ARP works because the network stack polls `process_rx()` before
  enabling the MSI-X SPI.
- After init, the first PCI net MSI can enter `net_pci::handle_interrupt()`,
  but that handler suppresses device and GIC interrupts without raising
  `NetRx`. The re-enable function is reachable only from the NetRx handler.

What remains unproven without a live traffic run:

- Whether routine Parallels boot traffic after `MSI-X SPI ... enabled` currently
  hits the PCI MSI handler at all. Existing Turn 16/17 logs do not show a
  post-init `net_msi_irqs` read.

Existing diagnostic surface:

- `NET_PCI_MSI_COUNT` is incremented in `net_pci::handle_interrupt()`
  (`kernel/src/drivers/virtio/net_pci.rs:842`).
- `/proc/trace/counters` exposes it as `net_msi_irqs`
  (`kernel/src/fs/procfs/mod.rs:832-838`).

No new Substep 0 source commit is required to understand the graph. If Claude
wants empirical confirmation before source conversion, use the existing
`net_msi_irqs` counter under targeted inbound traffic. If external access
remains blocked, Substep 2's boot gate should include a temporary or permanent
lock-free TraceCounter for PCI IRQ-raised NetRx, not serial logging in the IRQ
path.
