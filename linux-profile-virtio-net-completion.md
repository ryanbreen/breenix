# Linux profile: virtio_net completion, NAPI, ARP, and NET_RX_SOFTIRQ

Turn 18 source basis: local Linux source tree at
`/Users/wrb/fun/code/backups/transcode/home/wrb/code/linux`, identified as
`v5.9-rc8-224-g6f2f486d57c4-dirty` (`VERSION=5`, `PATCHLEVEL=9`,
`SUBLEVEL=0`, `EXTRAVERSION=-rc8`). Live probe validation was not used:
non-interactive SSH to `linux-probe` fails with a host-key mismatch, so this
profile is source-only.

No Linux source text is copied here; function names and file:line citations are
used as anchors.

## 1. RX completion path

Linux virtio-net receives packets by interrupt-driven virtqueue callbacks that
schedule NAPI, not by a periodic driver timer that polls the used ring.

The PCI interrupt path enters `vp_vring_interrupt`, which walks the device
virtqueue list and delegates each queue to `vring_interrupt`
(`drivers/virtio/virtio_pci_common.c:58-74`). Legacy INTx first reads and
clears the ISR and then uses the same vring dispatch path
(`drivers/virtio/virtio_pci_common.c:82-99`). The generic vring interrupt code
first checks whether the ring actually has used buffers, treats no-work
interrupts as spurious, and invokes the virtqueue callback only when useful
work exists (`drivers/virtio/virtio_ring.c:2035-2052`).

virtio-net wires RX virtqueues to `skb_recv_done` and TX virtqueues to
`skb_xmit_done` in `virtnet_find_vqs`
(`drivers/net/virtio_net.c:2716-2769`). The RX callback calls
`virtqueue_napi_schedule` (`drivers/net/virtio_net.c:1270-1276`), and that
helper uses `napi_schedule_prep`, disables virtqueue callbacks, and schedules
NAPI (`drivers/net/virtio_net.c:319-325`). Scheduling NAPI appends the NAPI
instance to the per-CPU poll list and raises `NET_RX_SOFTIRQ`
(`net/core/dev.c:4239-4245`, `net/core/dev.c:6281-6288`).

`NET_RX_SOFTIRQ` is registered to `net_rx_action` during network core init
(`net/core/dev.c:11078-11079`). `net_rx_action` runs NAPI instances with both a
global packet budget and a time budget (`net/core/dev.c:6728-6782`). The default
global budget in this tree is `netdev_budget = 300`, and the default per-device
weights include 64-packet RX/TX weights (`net/core/dev.c:4228-4235`).

The virtio-net RX NAPI poll callback is `virtnet_poll`
(`drivers/net/virtio_net.c:1445-1475`). It first opportunistically cleans TX
for the paired queue, then calls `virtnet_receive` with the NAPI budget. The
receive loop drains at most `budget` packets from the virtqueue and hands each
buffer to `receive_buf` (`drivers/net/virtio_net.c:1346-1360`). `receive_buf`
builds the skb, records the RX queue, sets the Ethernet protocol, and hands the
skb into GRO through `napi_gro_receive`
(`drivers/net/virtio_net.c:1036-1087`). The GRO receive entry point is
`napi_gro_receive` in the network core (`net/core/dev.c:6008-6022`).

At the end of a non-exhausted RX poll, `virtnet_poll` calls
`virtqueue_napi_complete` (`drivers/net/virtio_net.c:1458-1461`). That helper
re-enables virtqueue callbacks with `virtqueue_enable_cb_prepare`, completes
NAPI with `napi_complete_done`, and uses `virtqueue_poll` to close the race
where new used buffers arrive while callbacks are being re-enabled
(`drivers/net/virtio_net.c:328-340`). The generic virtqueue helpers provide the
callback-disable, enable-prepare, and race-poll operations
(`drivers/virtio/virtio_ring.c:1918-1989`). If NAPI is missed while already
scheduled, `napi_complete_done` leaves the instance scheduled or reschedules it
through the normal NAPI path (`net/core/dev.c:6336-6404`).

The structural model is:

1. IRQ arrives.
2. virtio PCI/vring dispatch invokes the RX virtqueue callback.
3. The driver disables further callbacks and schedules NAPI.
4. `NET_RX_SOFTIRQ` runs `net_rx_action`.
5. `virtnet_poll` drains up to the NAPI budget and passes packets to GRO.
6. If the ring is drained, NAPI completion re-enables callbacks and checks the
   used-ring race.
7. If budget/time is exhausted, `net_rx_action` keeps the NAPI instance queued
   and re-raises `NET_RX_SOFTIRQ` (`net/core/dev.c:6771-6778`).

There is no virtio-net RX timer that periodically polls the used ring as the
primary completion mechanism.

## 2. TX completion path

Linux TX is also completion-driven. `start_xmit` does not wait for the device
to consume the descriptor. It may free already-completed old buffers
opportunistically, queues the new skb with `xmit_skb`, and notifies the device
when needed (`drivers/net/virtio_net.c:1579-1650`). `xmit_skb` prepares the
scatterlist and calls `virtqueue_add_outbuf`
(`drivers/net/virtio_net.c:1532-1576`).

TX completions are reclaimed by `free_old_xmit_skbs`, which drains completed
buffers from the TX virtqueue with `virtqueue_get_buf` and consumes the skb or
XDP frame (`drivers/net/virtio_net.c:1380-1414`). That cleanup is reachable
from the RX NAPI poll via `virtnet_poll_cleantx`
(`drivers/net/virtio_net.c:1426-1443`) and from the dedicated TX NAPI poll
callback `virtnet_poll_tx` (`drivers/net/virtio_net.c:1506-1530`).

The TX virtqueue callback is `skb_xmit_done`. It disables further virtqueue
callbacks and either schedules the TX NAPI context or wakes the transmit
subqueue when TX NAPI is disabled (`drivers/net/virtio_net.c:342-355`). TX NAPI
completion uses the same `virtqueue_napi_complete` callback re-enable and race
check used by RX (`drivers/net/virtio_net.c:328-340`).

The important delta for Breenix is that Linux never spins in `start_xmit` on
`used.idx` until the descriptor is consumed. Completion ownership is transferred
to NAPI and virtqueue callbacks.

## 3. Initialization

Linux device setup also avoids synchronous packet polling.

`virtnet_probe` allocates and initializes the netdev, configures feature state,
calls `init_vqs`, registers the netdev, and then marks the virtio device ready
(`drivers/net/virtio_net.c:2955-3125`). `init_vqs` allocates queues, calls
`virtnet_find_vqs`, and sets affinity (`drivers/net/virtio_net.c:2840-2857`).
Queue allocation registers RX NAPI with `virtnet_poll` and TX NAPI with
`virtnet_poll_tx` (`drivers/net/virtio_net.c:2800-2820`). `virtnet_find_vqs`
assigns RX/TX callbacks and asks the virtio transport to create the queues
(`drivers/net/virtio_net.c:2716-2769`).

When the interface opens, `virtnet_open` fills receive buffers and enables the
RX and TX NAPI contexts (`drivers/net/virtio_net.c:1477-1503`). The RX NAPI
enable helper schedules NAPI once to catch packets that may have arrived before
NAPI was enabled (`drivers/net/virtio_net.c:1278-1288`); it does not loop
synchronously waiting for ARP, ICMP, or any other packet.

Initial ARP or other traffic uses the normal neighbor, device transmit, IRQ,
and NAPI paths described in this profile. There is no virtio-net driver init
loop that sends ARP, polls RX descriptors until a reply arrives, and only then
enables interrupts.

## 4. MSI-X enablement and storm control

Linux enables MSI-X through the virtio PCI transport while creating virtqueues.
`vp_find_vqs` tries per-vqueue MSI-X, shared MSI-X, and then INTx
(`drivers/virtio/virtio_pci_common.c:391-408`). MSI-X vector allocation happens
in `vp_request_msix_vectors`, including config-vector setup and shared queue
IRQ registration when per-queue vectors are not used
(`drivers/virtio/virtio_pci_common.c:102-169`). Per-vqueue MSI-X setup assigns
vectors during `vp_find_vqs_msix` and requests a per-vqueue IRQ that directly
uses `vring_interrupt` when possible
(`drivers/virtio/virtio_pci_common.c:279-346`).

For modern virtio PCI devices, the config ops table routes `find_vqs` to
`vp_modern_find_vqs` (`drivers/virtio/virtio_pci_modern.c:447-471`). The modern
probe maps common, ISR, notify, and optional device config capabilities, then
installs those config ops (`drivers/virtio/virtio_pci_modern.c:583-712`).
`vp_modern_find_vqs` calls the common `vp_find_vqs` path and then enables each
queue in the common config area (`drivers/virtio/virtio_pci_modern.c:403-424`).

Storm/coalescing control is handled by the NAPI and virtqueue callback state
machine, not by suppressing MSI-X until a timer later re-enables it. The vring
interrupt path ignores interrupts with no used buffers
(`drivers/virtio/virtio_ring.c:2035-2052`). The virtio-net NAPI schedule helper
disables callbacks while work is scheduled (`drivers/net/virtio_net.c:319-325`),
and completion re-enables callbacks with an explicit used-ring race check
(`drivers/net/virtio_net.c:328-340`,
`drivers/virtio/virtio_ring.c:1918-1989`). If budget or time is exhausted, the
network core keeps the NAPI instance pending and re-raises `NET_RX_SOFTIRQ`
without reopening a timer-gated interrupt window (`net/core/dev.c:6735-6782`).

## 5. ARP and on-demand resolution

Linux ARP resolution is asynchronous. The IPv4 ARP neighbor ops route unresolved
output through `neigh_resolve_output` and use `arp_solicit` for probes
(`net/ipv4/arp.c:129-143`). `neigh_resolve_output` calls `neigh_event_send`; if
the neighbor is already usable it emits the packet via `dev_queue_xmit`,
otherwise it returns after queueing or dropping according to neighbor state
(`net/core/neighbour.c:1469-1500`).

For unresolved neighbors, `__neigh_event_send` transitions the entry toward
`NUD_INCOMPLETE`, arms the neighbor timer, queues the triggering skb within the
configured byte limit, and optionally sends an immediate probe
(`net/core/neighbour.c:1105-1175`). The probe path clones the queued skb if
needed and calls the neighbor `solicit` operation (`net/core/neighbour.c:1000-1011`).
For IPv4, that solicitation is `arp_solicit`, which selects source/target
information and emits an ARP request through `arp_send_dst`
(`net/ipv4/arp.c:330-390`). `arp_send` is the exported helper around that send
path (`net/ipv4/arp.c:320-328`).

Retries and failure are driven by the neighbor timer state machine, which moves
entries through reachable, delay, probe, incomplete, and failed states and calls
`neigh_probe` when appropriate (`net/core/neighbour.c:1016-1103`). This timer is
for neighbor state and retransmission. It does not drain a device RX ring while
the original sender waits.

The Linux pattern for on-demand ARP is therefore: queue or drop the triggering
skb, send probes through normal TX, receive ARP replies through the IRQ/NAPI RX
path, and let neighbor state updates release queued packets later. The original
send path does not spin on virtio RX completion.

## 6. NET_RX_SOFTIRQ contract

Linux registers `NET_RX_SOFTIRQ` to `net_rx_action`
(`net/core/dev.c:11078-11079`). The generic softirq loop snapshots pending
softirqs, runs each registered action, and either restarts within bounds or
wakes `ksoftirqd` for deferred work (`kernel/softirq.c:255-328`). Raising a
softirq sets the pending bit and wakes `ksoftirqd` when the caller is not
already in interrupt/softirq context (`kernel/softirq.c:456-488`).

For virtio-net RX completion, `NET_RX_SOFTIRQ` is raised when NAPI is scheduled:
the driver callback calls `virtqueue_napi_schedule`
(`drivers/net/virtio_net.c:1270-1276`, `drivers/net/virtio_net.c:319-325`),
and `____napi_schedule` sets the `NET_RX_SOFTIRQ` pending bit
(`net/core/dev.c:4239-4245`). The softirq action itself may re-raise
`NET_RX_SOFTIRQ` if the poll list still has work after the budget or time
window is exhausted (`net/core/dev.c:6735-6782`).

There are timers in the network stack, including neighbor retry timers and
optional NAPI/GRO deferral, but they do not form a periodic virtio-net device
poller. The Linux contract is completion-triggered NAPI plus bounded softirq
work, with virtqueue callbacks re-enabled by the NAPI completion path. A
10ms-style timer that calls the driver's RX drain function and re-enables MSI-X
afterward is structurally different from Linux's virtio_net model.
