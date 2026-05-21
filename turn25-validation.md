# Turn 25 validation - INCONCLUSIVE

## Status

INCONCLUSIVE. The attempted Substep 4 change built cleanly and the single
Parallels boot stayed alive, but live inbound networking failed on the same
boot. Per the directive's first-failure rule, the source files were reverted and
this commit keeps diagnostics only.

Reverted source files:

- `kernel/src/net/mod.rs`
- `kernel/src/drivers/virtio/net_pci.rs`
- `kernel/src/drivers/virtio/net_mmio.rs`

The attempted source diff is preserved in
`turn25-artifacts/turn25-attempted.diff`.

## 25A audit findings

The pre-edit shared init body was `init_common()` in `kernel/src/net/mod.rs`,
lines 333-447. The architecture entry points were:

- x86_64 `net::init()`: lines 269-290, softirq registration plus E1000 MAC log,
  then `init_common()`.
- aarch64 `net::init()`: lines 292-331, softirq registration, platform config
  selection, MAC log, then `init_common()`.

The removed ARP loop was lines 375-401: `for _i in 0..100`, `process_rx()`,
`0..1_000_000` `spin_loop()` delay, aarch64 `dump_rx_state()` diagnostics, and
`arp::lookup()` to log `NET: ARP resolved gateway MAC`.

The removed ICMP loop was lines 422-429: `for _ in 0..20`, `process_rx()`, and
`0..500000` `spin_loop()` delay after `ping(gateway)`.

Before the attempted change, aarch64 IRQ enablement happened after those loops:
PCI `net_pci::enable_msi_spi()` or MMIO `net_mmio::enable_net_irq()`. Existing
comments in `net/mod.rs`, `net_pci.rs`, and `net_mmio.rs` explicitly described
the stale "enable after init polling / ARP resolved" contract.

Downstream audit found no userspace program that asserts immediate boot ARP.
`rg -l "arp|ARP" userspace/programs/src/` only matched `fart.rs` via the
substring "sharp". Kernel `main_aarch64.rs` calls `net::init()` and does not
inspect the ARP cache. Userspace services start later; bsshd's response path can
use inbound source MACs, and outbound clients still have on-demand ARP for the
future Substep 5.

## Attempted implementation

The attempted change:

- deleted the init gateway ARP polling loop.
- deleted the init ICMP ping health check and reply polling loop.
- moved aarch64 `enable_msi_spi()` / `enable_net_irq()` before the gateway ARP
  send.
- kept one asynchronous gateway ARP request as a cache-prime.
- updated stale driver comments describing the old delayed IRQ-enable contract.

No IRQ/softirq hot-path files were touched.

## Build result

All required build gates passed:

- userspace aarch64 build
- ext2 image build
- aarch64 kernel build
- x86 qemu-uefi build
- Parallels EFI build

Warning/error greps were both 0 bytes:

- `turn25-artifacts/build-aarch64-warning-error-grep.txt`
- `turn25-artifacts/build-x86-warning-error-grep.txt`

## Single boot result

The single fresh Parallels boot returned exit 0 and the fail-marker scan was
empty. Basic liveness was preserved:

- heartbeat reached `uptime_ms=105256` in the first captured serial log and
  continued past `uptime_ms=178302` before shutdown.
- CPU0 timer markers reached `cpu0 ticks=65000` in the test window and
  continued to `cpu0 ticks=115000` before shutdown.
- bwm/compositor stayed active.
- bsshd started and listened on `0.0.0.0:2222`.
- bounce started.

Substep 4 serial ordering was correct for the attempted design:

- line 266: `[virtio-net-pci] MSI-X SPI 55 enabled (post-init)`
- line 267: `NET: Sending ARP request for gateway 10.211.55.1`
- line 268: `ARP request sent successfully`
- line 269: `NET: Network initialization complete`

No init `ARP resolved gateway MAC`, `Sending ICMP echo request`, `Gateway ARP
not resolved`, RX diagnostic, `TX timeout`, or `TX queue full` markers appeared.

## Failure evidence

The live SSH proof failed on the same boot:

- SSH timed out before the password prompt.
- The guest serial log showed bsshd listening but no bsshd connection log.
- Host ARP had learned the guest MAC (`10.211.55.100 at 00:1c:42:3d:78:a0`).
- `ping -c 1 -W 1000 10.211.55.100` had 100% packet loss.

This is consistent with a receive-interrupt/callback state problem rather than
basic boot failure. The likely mechanism is that `net::init()` runs before the
scheduler and softirq subsystem are initialized on aarch64. Enabling MSI and
sending the gateway ARP request that early can deliver an RX interrupt before
ksoftirqd exists; if the PCI handler suppresses callbacks and raises NetRx, no
softirq drains/re-enables the queue at that point, so later inbound traffic can
hang even though the system remains alive.

## Next hypothesis

Turn 26 should retry Substep 4 without sending any init-time ARP packet before
the softirq subsystem exists. Let the first real outbound packet perform
on-demand ARP after boot, or add a post-softirq async ARP prime from a location
that is explicitly after `softirqd::init_softirq()`. Keeping the init ARP send
while moving IRQ enable earlier appears unsafe in the current boot ordering.
