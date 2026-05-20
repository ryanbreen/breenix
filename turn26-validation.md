# Turn 26 validation - INCONCLUSIVE

## Status

INCONCLUSIVE. The deferred ARP-primer implementation built cleanly, but the
single Parallels boot hit the CPU0 regression alarm before the required host
ping test could be run. Per first-failure protocol, all source files were
reverted and this commit keeps diagnostics only.

Reverted source files:

- `kernel/src/net/mod.rs`
- `kernel/src/drivers/virtio/net_pci.rs`
- `kernel/src/drivers/virtio/net_mmio.rs`
- `kernel/src/main_aarch64.rs`
- `kernel/src/tracing/providers/counters.rs`

The attempted source diff is preserved at
`turn26-artifacts/turn26-attempted.diff`.

## 26A boot-order findings

The Turn 25 boot-order hypothesis was confirmed statically:

- `kernel::net::init()` is called from `kernel/src/main_aarch64.rs` lines
  572-574, immediately after driver init.
- Scheduler setup is later, lines 792-795.
- Workqueue setup is lines 799-801.
- `kernel::task::softirqd::init_softirq()` is lines 805-807.
- Tracing counters are initialized and enabled at lines 832-837.
- SMP reaches the `[smp] ... CPUs online` marker at lines 979-982.
- Userspace init launches much later, beginning around lines 1126-1132.

This means Turn 25 really did enable network IRQs before softirqd existed.

## Attempted implementation

The attempted Turn 26 source change:

- made `init_common()` traffic-free: ARP cache/config only, no gateway ARP
  request, no ICMP ping, no synchronous `process_rx()` polling.
- added `net::spawn_arp_primer()` on aarch64.
- spawned a one-shot `net_arp_primer` kthread after softirqd and tracing were
  initialized, before timer/SMP/userspace.
- made the kthread increment `NET_ARP_PRIMER_RAN`, enable PCI MSI-X SPI or MMIO
  net IRQ, send one gateway ARP request, and exit without waiting.
- updated stale driver comments for post-softirq IRQ enablement.

## Build result

All required build gates passed:

- userspace aarch64 build
- ext2 image build
- aarch64 kernel build
- x86 qemu-uefi build
- Parallels EFI build

Warning/error greps were both 0 bytes:

- `turn26-artifacts/build-aarch64-warning-error-grep.txt`
- `turn26-artifacts/build-x86-warning-error-grep.txt`

## Single boot result

The serial log confirms the intended ordering:

- line 262: `NET: Network initialization complete`
- line 287: `[boot] Softirq subsystem initialized`
- line 289: `[boot] Tracing subsystem initialized and enabled`
- line 290: `[boot] net_arp_primer spawned (tid=3)`
- line 314: MSI-X SPI 55 enabled from the primer path
- line 317: `[smp] 8 CPUs online`

The boot then failed before service liveness or host ping validation:

- line 341: `!!! CPU0 REGRESSION ALARM !!!`
- line 349: `CPU0 timer regression: tick_count=5 but peer max=30000`
- repeated panic reports followed at lines 352-415.

Because the first failure was the CPU0 regression, the Turn 26 ping/SSH gate was
not run.

## Interpretation

Deferring ARP priming until after softirqd fixed the specific Turn 25 ordering
bug in theory, but the actual kthread ran too early in the boot window: the
MSI-X enable happened interleaved with SMP bring-up, before userspace had made
meaningful CPU0 timer progress. This reproduces the same CPU0 starvation class
that Turn 22 exposed, though with an even lower CPU0 tick count (`5` instead of
`75`).

Turn 27 should not guess at another placement. It should instrument the PCI MSI
handler and NetRx softirq chain with counters only, or defer the primer until a
known-safe post-SMP/post-userspace milestone, then prove exactly whether callback
suppression, NetRx dispatch, or CPU0 scheduling is the break point.
