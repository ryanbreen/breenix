# Breenix vs Linux virtio-net gap analysis

Linux source basis: local tree
`/Users/wrb/fun/code/backups/transcode/home/wrb/code/linux`, identified as
`v5.9-rc8-224-g6f2f486d57c4-dirty`.

This is a Turn 18 profile artifact only. It intentionally does not implement a
conversion.

| Breenix site | Pattern | Linux equivalent | Delta | Conversion shape |
|---|---|---|---|---|
| `kernel/src/net/mod.rs:235-247` | Timer-driven device polling contract | virtio-net RX callback schedules NAPI (`drivers/net/virtio_net.c:1270-1276`, `drivers/net/virtio_net.c:319-325`); NAPI scheduling raises `NET_RX_SOFTIRQ` (`net/core/dev.c:4239-4245`) | Breenix documents a timer-raised NetRx handler that drains rings and re-enables PCI MSI-X. Linux raises NetRx from NAPI scheduling and re-raises only for remaining NAPI work, not from a periodic device poller. Current timer grep found no live timer raise, which makes the PCI re-enable contract internally inconsistent. | Replace the timer-shaped NetRx role with an IRQ-scheduled, budgeted network poll context. PCI IRQ should schedule work; completion should re-enable callbacks after the budgeted drain/race check. |
| `kernel/src/drivers/virtio/net_pci.rs:828-868` | Timer-gated MSI suppression | `vp_vring_interrupt` delegates to `vring_interrupt` (`drivers/virtio/virtio_pci_common.c:58-74`); `vring_interrupt` calls the virtqueue callback when used buffers exist (`drivers/virtio/virtio_ring.c:2035-2052`) | Breenix PCI MSI handler suppresses device and GIC interrupts and does not raise NetRx. Linux uses the interrupt to call the virtqueue callback, which schedules NAPI. | PCI MSI handler should acknowledge/dispatch minimally and schedule NetRx/NAPI-style work, not leave the device suppressed until an unrelated timer/softirq path runs. |
| `kernel/src/drivers/virtio/net_pci.rs:871-928` | Timer-gated callback re-enable | `virtqueue_napi_complete` re-enables callbacks and uses `virtqueue_poll` to close the race (`drivers/net/virtio_net.c:328-340`, `drivers/virtio/virtio_ring.c:1941-1970`) | Breenix re-enable is a separate function called after a full ring drain. Linux ties callback re-enable to NAPI completion after bounded work. | Move the re-enable/race-check semantics into the budgeted poll completion path. |
| `kernel/src/net/mod.rs:467-518` | Unbounded RX ring drain primitive | `virtnet_receive` stops at NAPI budget (`drivers/net/virtio_net.c:1346-1360`); `net_rx_action` also applies global budget/time bounds (`net/core/dev.c:6735-6782`) | Breenix `process_rx()` drains until no packet remains and is shared by init polling, on-demand ARP polling, and NetRx. Linux bounds each poll. | Split or wrap RX processing with an explicit budget and completion result so the interrupt path can behave like NAPI while init/on-demand paths stop synchronously draining rings. |
| `kernel/src/net/mod.rs:348-389` | Synchronous init polling for gateway ARP | `virtnet_probe` sets up queues and registers netdev without waiting for packets (`drivers/net/virtio_net.c:2955-3125`); `virtnet_open` fills RX buffers and enables NAPI (`drivers/net/virtio_net.c:1477-1503`) | Breenix sends ARP during network init and polls RX 100 times before proceeding. Linux initializes the driver and lets ARP resolution happen asynchronously through neighbor state and NAPI. | Remove init-time ARP wait from driver bring-up. If a boot health check needs gateway resolution, make it an async network task or a bounded test outside driver readiness. |
| `kernel/src/net/mod.rs:397-417` | Synchronous init polling for ICMP | Linux ping/ICMP traffic uses normal TX and RX completion after the device is ready; the virtio driver does not block init on an ICMP reply | Breenix sends a ping during init and manually drains RX for replies. Linux does not use packet success as a prerequisite to enabling interrupts. | Move ping validation out of init, or make it an asynchronous diagnostic after IRQ/NAPI is active. |
| `kernel/src/net/mod.rs:421-433`; `kernel/src/drivers/virtio/net_pci.rs:379-389`; `kernel/src/drivers/virtio/net_pci.rs:959-984`; `kernel/src/drivers/virtio/net_mmio.rs:253-284`; `kernel/src/drivers/virtio/net_mmio.rs:690-704` | IRQ enable delayed until after synchronous init polling | Linux allocates MSI-X/request_irq during `vp_find_vqs_msix` and enables modern queues in `vp_modern_find_vqs` (`drivers/virtio/virtio_pci_common.c:279-346`, `drivers/virtio/virtio_pci_modern.c:403-424`) | Breenix delays GIC-level enable to avoid storms during polling. Linux handles storms/coalescing by disabling virtqueue callbacks while NAPI is scheduled and re-enabling them at completion. | Enable IRQs as part of queue readiness, but use the callback-disable/NAPI completion race-check pattern to control storms. |
| `kernel/src/net/mod.rs:596-628` | Synchronous on-demand ARP resolution | `neigh_resolve_output` calls `neigh_event_send` and either transmits, queues, or drops (`net/core/neighbour.c:1469-1500`); unresolved state queues skb and arms timers (`net/core/neighbour.c:1105-1175`); ARP request send is `arp_solicit`/`arp_send_dst` (`net/ipv4/arp.c:330-390`) | Breenix sends ARP and drains RX in the caller until the reply arrives. Linux returns control after queuing/dropping and lets the IRQ/NAPI path update neighbor state. | Introduce async neighbor resolution: queue the original packet or return would-block/host-unreachable, send ARP through normal TX, and consume replies through normal RX completion. |
| `kernel/src/drivers/virtio/net_pci.rs:700-747` | TX completion busy-wait | `start_xmit` queues the skb and notifies without waiting (`drivers/net/virtio_net.c:1579-1650`); completions are freed in `free_old_xmit_skbs` via RX poll cleanup or TX NAPI (`drivers/net/virtio_net.c:1380-1414`, `drivers/net/virtio_net.c:1426-1443`, `drivers/net/virtio_net.c:1506-1530`) | Breenix PCI TX spins on `used.idx` after every send. Linux transfers completion to callbacks/NAPI. | Convert PCI TX to asynchronous completion: enqueue descriptor, notify, return, and reclaim in the network poll/TX completion path. |
| `kernel/src/drivers/virtio/net_mmio.rs:548-588` | TX completion busy-wait | Same Linux TX model as above | Breenix MMIO TX also spins on `used.idx`, even though its RX IRQ handler already raises NetRx. | Convert MMIO TX completion together with PCI TX so the shared net stack has one async TX completion contract. |
| `kernel/src/main.rs:805-816`; `kernel/src/main.rs:871-882`; `kernel/src/main.rs:940-951` | x86 idle/hlt-loop device polling | Linux does not require idle loops to call a NIC ring drain function; NIC interrupts schedule NAPI and `NET_RX_SOFTIRQ` runs `net_rx_action` (`net/core/dev.c:4239-4245`, `net/core/dev.c:6735-6782`) | Breenix test-only x86 idle loops call `net::process_rx()` after `hlt` as a workaround for softirq timing. | Remove hlt-loop net polling after NetRx scheduling is reliable for x86 e1000 and any virtio-net path used by those tests. |
| `kernel/src/drivers/virtio/net_mmio.rs:716-745` | MSI-driven completion | Linux driver IRQ callbacks schedule NAPI (`drivers/net/virtio_net.c:1270-1276`, `drivers/net/virtio_net.c:319-325`) | Breenix MMIO already acknowledges interrupt and raises NetRx. It still depends on shared `process_rx()` semantics that are unbounded and coupled to init/on-demand polling. | Preserve the IRQ-to-softirq shape, but make the softirq handler budgeted and remove synchronous callers. |

## Bundle-scope decision

Recommendation: Plan B - convert P6 + P7 + P8 together.

Reasoning from the Linux model:

- Linux RX and TX completion are separate callbacks but not independent
  contracts. RX NAPI runs `virtnet_poll_cleantx`, and TX completion uses the
  same callback disable/enable and race-check pattern
  (`drivers/net/virtio_net.c:1426-1443`, `drivers/net/virtio_net.c:1506-1530`,
  `drivers/net/virtio_net.c:328-340`).
- Breenix has a single shared `process_rx()` drain primitive that is called by
  the NetRx handler, init ARP/ICMP loops, on-demand ARP, and x86 hlt-loop test
  workarounds (`kernel/src/net/mod.rs:235-247`,
  `kernel/src/net/mod.rs:348-417`, `kernel/src/net/mod.rs:596-628`,
  `kernel/src/main.rs:805-816`, `kernel/src/main.rs:871-882`,
  `kernel/src/main.rs:940-951`).
- Leaving TX busy-waits in place while converting only RX would still violate
  Linux's completion model, and leaving hlt-loop `process_rx()` calls in place
  would preserve a second polling path that can mask broken softirq scheduling.
- The PCI MSI suppression/re-enable path cannot be made Linux-shaped without a
  budgeted poll completion path, and that path is also the right place to
  reclaim TX completions.

Plan B should still be implemented in small commits or substeps, but the
intermediate state should preserve one coherent completion contract:
IRQ/callback schedules NetRx/NAPI-style work, bounded poll processes RX and TX
completion, poll completion re-enables callbacks after a used-ring race check,
and ARP/init callers stop draining the device ring synchronously.
