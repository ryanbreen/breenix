# Turn 25 init-flow audit

## net::init entry points

`kernel/src/net/mod.rs` has architecture-specific `net::init()` entry points:

- x86_64: lines 269-290. Registers the NetRx softirq handler, logs the E1000
  MAC address when present, then calls `init_common()`.
- aarch64: lines 292-331. Registers the NetRx softirq handler, selects the
  platform network config (Parallels PCI, VMware e1000, or default MMIO/QEMU),
  logs the MAC address, then calls `init_common()`.

The shared initialization body is `init_common()`, lines 333-447 before this
turn's edits.

## Gateway ARP polling loop

The init-time gateway ARP flow starts at line 360:

- `gateway = gw`
- log `NET: Sending ARP request for gateway ...`
- call `arp::request(&gateway)` once
- log `ARP request sent successfully`

The synchronous wait is lines 375-401:

- `for _i in 0..100`
- each iteration calls `process_rx()`
- each iteration spins `0..1_000_000` with `core::hint::spin_loop()`
- on aarch64, it dumps RX state for `_i < 5 || _i % 20 == 0`
- each iteration checks `arp::lookup(&gateway)` and logs the resolved MAC if
  present

Lines 403-407 then make gateway ARP a prerequisite for the ping test by
returning if the ARP cache is still empty.

## ICMP polling loop

The init-time ICMP health check starts at line 409:

- log `NET: Sending ICMP echo request to gateway ...`
- call `ping(gateway)`

The synchronous reply wait is lines 422-429:

- `for _ in 0..20`
- each iteration calls `process_rx()`
- each iteration spins `0..500_000` with `core::hint::spin_loop()`

There is no explicit success assertion after this loop; it is only a boot-time
connectivity exercise.

## IRQ enable timing before this turn

The aarch64 interrupt enable block is lines 433-446 in `init_common()`, after
all init ARP/ICMP polling:

- PCI path calls `net_pci::enable_msi_spi()`
- MMIO path calls `net_mmio::enable_net_irq()`

The in-tree rationale is explicitly tied to the old polling contract:

- `net/mod.rs` says interrupts must be enabled after synchronous ARP/ICMP
  polling so interrupt-driven RX does not interfere with polling.
- `net_pci.rs` says `enable_msi_spi()` runs after init polling and that polling
  drains used-ring entries before interrupt-driven RX starts.
- `net_mmio.rs` says `enable_net_irq()` is called after the network stack is
  fully initialized / ARP resolved.

Those comments are stale once Substeps 1-3 provide budgeted NetRx, PCI MSI
scheduling, and lock-free TX completion.

## Downstream callers and assumptions

`kernel/src/main_aarch64.rs` calls `kernel::net::init()` at lines 572-574 after
driver init and before filesystem/devfs/userspace startup. It does not inspect
the ARP cache after `net::init()` returns.

The userspace init process starts heartbeat, xhci_counters, bwm, telnetd, bsshd,
and bounce after kernel network init. These services do not require the gateway
MAC to be cached immediately after `net::init()`:

- `bsshd` listens for inbound connections. TCP responses generated while
  processing inbound packets can use `CURRENT_PACKET_SRC_MAC` and the deferred
  TX path in `net/tcp.rs`, bypassing gateway ARP for that response path.
- Outbound userspace network clients still go through `send_ipv4()`, whose
  on-demand ARP polling remains intact for Substep 5.

The boot-stage catalog contains legacy marker checks for init ARP resolution and
ICMP reply (`xtask/src/boot_stages.rs`), but the turn explicitly forbids
gold-master edits. These are test-contract follow-ups rather than runtime
callers that require immediate gateway ARP caching.

## Userspace ARP assertions

`rg -l "arp|ARP" userspace/programs/src/` found only
`userspace/programs/src/fart.rs`, where the match is the substring "sharp" in a
comment. No userspace program asserts that ARP is resolved immediately at boot.
