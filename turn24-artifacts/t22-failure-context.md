# Turn 24: T22 failure context

Source log: `turn22-artifacts/boot-1-serial.log`.

## Failure window

- Network init completed before the failure:
  - line 262: `NET: Sending ARP request for gateway 10.211.55.1`
  - line 263: `ARP request sent successfully`
  - line 266: `NET: ARP resolved gateway MAC: 00:1c:42:00:00:18`
  - line 267: `NET: Sending ICMP echo request to gateway 10.211.55.1`
  - line 268: ICMP echo reply received
  - line 269: network initialization complete
  - line 270: PCI net MSI-X SPI 55 enabled
- Scheduler, softirq, tracing, timer, SMP, and init all came up:
  - lines 292-297: scheduler/workqueue/softirq/tracing initialized
  - lines 302-305: timer interrupt initialized
  - line 323: 8 CPUs online
  - lines 331-345: init launched and started
- Userspace progress before failure:
  - lines 346-358: heartbeat process spawned and printed its first heartbeat
  - lines 359-374: `xhci_counters` ran and exited cleanly
  - line 375: init attempted to spawn `/bin/bwm`
  - lines 376-412: heartbeat continued from `uptime_ms=7260` through `uptime_ms=43283`
- Panic:
  - line 414: `!!! CPU0 REGRESSION ALARM !!!`
  - line 415: `CPU0 tick_count = 75, max peer = 30000`
  - line 421: panic at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17`

## Other CPU activity

The gold-master alarm reports a peer CPU reached 30000 timer ticks while CPU0 remained at 75 ticks. Heartbeat kept printing for roughly 36 seconds after `/bin/bwm` spawn began, which means at least one non-CPU0 scheduler/timer path continued to run while CPU0 stopped advancing.

There are no later `/proc` counters because the boot never reached `bsshd`.

## Reached/not reached

Reached:

- Gateway ARP resolution
- ICMP echo reply
- post-init PCI net MSI enable
- ext2 root mount
- scheduler/softirq/tracing
- userspace init
- heartbeat
- `xhci_counters`
- `/bin/bwm` spawn request

Not reached:

- `bwm` startup log
- `bsshd` start/listen
- `bounce` start
- live SSH/procfs

## TX activity near failure

The only explicit TX-related serial markers are during network init, well before scheduler start:

- ARP request send at lines 262-263
- ICMP echo request at line 267

There are no `transmit`, `TX queue`, `TX queue full`, or TX timeout markers immediately before the panic. The failure happened long after init TX, during/after the `/bin/bwm` spawn window. This weakens the narrow theory that CPU0 stopped immediately after a TX attempt, but it does not rule out the T22 lock hypothesis: the failing T22 implementation added a shared TX ring lock and a TX reclaim call in the NetRx poll path, so any later RX/MSI activity could still interact with that lock without producing serial markers.
