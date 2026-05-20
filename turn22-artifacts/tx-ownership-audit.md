# Turn 22 TX ownership audit

## PCI transport

- `kernel/src/drivers/virtio/net_pci.rs:125-130` defines a single `TxBuffer`.
- `kernel/src/drivers/virtio/net_pci.rs:218-228` allocates exactly one static `PCI_TX_BUFFER`.
- `kernel/src/drivers/virtio/net_pci.rs:695-704` copies each caller-provided frame into that static buffer and builds descriptor 0 against the static buffer address.
- `kernel/src/drivers/virtio/net_pci.rs:716-724` posts descriptor 0 to the TX available ring and notifies queue 1.
- `kernel/src/drivers/virtio/net_pci.rs:726-744` then busy-waits until `used.idx != state.tx_last_used_idx`. This is an "has advanced" check, not strict equality to a specific expected index. On advancement, it assigns `tx_last_used_idx = used_idx`; it does not walk individual used entries.

Current ownership consequence: because `transmit()` blocks until the device reports at least one TX completion, the caller may reuse or drop its source buffer immediately after `Ok(())`. The static buffer is also safe from reuse only because the function does not return until completion.

## MMIO transport

- `kernel/src/drivers/virtio/net_mmio.rs:119-124` defines a single `TxBuffer`.
- `kernel/src/drivers/virtio/net_mmio.rs:213-223` allocates exactly one static `TX_BUFFER`.
- `kernel/src/drivers/virtio/net_mmio.rs:531-547` copies each caller-provided frame into that static buffer and builds descriptor 0 against the static buffer address.
- `kernel/src/drivers/virtio/net_mmio.rs:559-567` posts descriptor 0 to the TX available ring and notifies queue 1.
- `kernel/src/drivers/virtio/net_mmio.rs:569-587` busy-waits until `used.idx != state.tx_last_used_idx`. Like PCI, this is an "has advanced" check and it collapses any advancement to the latest used index.

Current ownership consequence matches PCI: `transmit()` is a copy-and-complete API today. The caller can immediately reuse/free its source buffer because the driver copies into its static buffer and blocks until completion before returning.

## Caller contract

- `kernel/src/net/mod.rs:122-135` routes `driver_transmit()` directly to PCI or MMIO on aarch64.
- `kernel/src/net/mod.rs:611-619` builds an Ethernet frame in a local `Vec` and passes a borrowed slice to `driver_transmit()`. That local frame is dropped immediately after `send_ethernet()` returns.
- `kernel/src/net/mod.rs:690-693`, `kernel/src/net/arp.rs:302-305`, `kernel/src/net/icmp.rs:205-213`, and `kernel/src/net/tcp.rs:46-58` all follow the same shape: build transient packet storage, call into the network transmit path, and then allow that storage to be reused or dropped.

Therefore the async design must preserve the copy-and-return contract. It cannot make descriptors point at caller-owned slices after `transmit()` returns.

## Design implication

Option A is the right fit: a fixed driver-owned TX buffer pool with one buffer per in-flight descriptor. Option B would require changing caller ownership/lifetime semantics or heap-owning every frame until completion, neither of which is necessary for the current queue sizes.
