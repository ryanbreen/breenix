# Turn 22 ownership design

Chosen model: Option A, fixed TX buffer slot pool.

Rationale:

- Both transports currently expose a copy-and-return caller contract while internally using one static buffer plus a completion busy-wait (`net_pci.rs:695-744`, `net_mmio.rs:531-587`).
- Network callers use transient packet storage and drop/reuse it after `transmit()` returns (`net/mod.rs:611-619`, `net/arp.rs:302-305`, `net/tcp.rs:46-58`), so descriptors must keep pointing at driver-owned memory after `transmit()` returns.
- Both transports already have TX descriptor rings. PCI reports a 256-entry legacy ring, and MMIO caps the queue at 16. A 16-slot pool matches the MMIO ring exactly and bounds PCI in-flight TX to a small, safe subset of descriptor IDs.

Implementation shape:

- Replace each single static TX buffer with 16 static TX buffers.
- Track each slot with an atomic in-flight bit. `transmit()` claims a free slot, copies the frame into that slot, posts a descriptor whose `id` equals the slot index, notifies the device, and returns without reading `used.idx`.
- If all 16 slots are in flight, `transmit()` returns `Err("TX queue full")`. No logging is emitted in the TX hot path.
- Add `reclaim_tx_completed() -> usize` in both transports. It walks the TX used ring from `tx_last_used_idx` to the current `used.idx`, reads each used element ID, and clears the corresponding in-flight bit.
- Serialize TX ring mutation and reclaim with a small IRQ-safe lock window. The ring updates are short and contain no logging, formatting, allocation, or completion waits.
- Call TX reclaim at the top of aarch64 `process_rx_budgeted()`, before RX work, matching Linux's `virtnet_poll_cleantx` before RX processing pattern.
