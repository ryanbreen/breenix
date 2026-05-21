# Turn 24 lock-free TX design

## Design goal

Re-implement Substep 3's async VirtIO net TX completion without the Turn 22 shared TX lock or IRQ-disabled critical section. The caller contract remains copy-and-return: `transmit()` must copy the frame into driver-owned memory before returning.

## Shared shape

Both PCI and MMIO transports use:

- `TX_POOL_SIZE = 16`
- a static `[TxBuffer; 16]` pool
- a static per-slot in-flight bitmap implemented with atomics
- a static atomic TX available-ring head
- `reclaim_tx_completed() -> usize`, called at the top of the aarch64 RX poll path before RX work

Descriptor ID equals TX pool slot index. Reclaim reads each TX used-ring element ID and clears that slot's in-flight bit.

## Transmit path

1. Validate frame length.
2. Read immutable device state needed for notification.
3. Claim a free TX slot by scanning the in-flight atomics:
   - `compare_exchange(false, true, Acquire, Relaxed)`
   - success means the previous `Release` clear from reclaim is visible before this slot is reused
   - if no slot can be claimed, return `Err("TX queue full")`
4. Copy caller bytes into the claimed driver-owned TX buffer.
5. Atomically reserve the next TX available-ring entry:
   - `fetch_add(1, AcqRel)` on the transport's TX avail head
   - this gives a unique ring position without a spinlock
6. Write descriptor `slot` and available-ring entry.
7. Publish the descriptor and ring entry before exposing `avail.idx`:
   - `fence(Release)`
   - volatile write to `avail.idx = reserved_idx + 1`
   - `fence(Release)` before device notify
8. Notify TX queue and return `Ok(())`.

There is no wait for `used.idx`, no spin loop, no timeout producer, no logging, no lock, and no IRQ disable in the TX path.

## Reclaim path

1. Return 0 if transport state is absent.
2. Volatile-read the device-owned TX `used.idx`.
3. Run `fence(Acquire)` before reading used-ring elements.
4. Walk from `tx_last_used_idx` to the observed `used.idx`.
5. For each used element:
   - read the element with `read_volatile`
   - if `elem.id < TX_POOL_SIZE`, clear that slot with `store(false, Release)`
   - advance `tx_last_used_idx`

The reclaim path assumes the network RX re-entrancy guard keeps only one RX poll/reclaimer active at a time on aarch64. It does not synchronize with `transmit()` via locks; slot handoff uses only the per-slot atomic bit.

## Concurrency assumptions

The network stack already treats the VirtIO TX available ring as a single-writer queue. Turn 24 preserves that assumption and uses the atomic head only to avoid a non-atomic shared counter. The pool slot state is safe against overlap because slots are claimed/freed independently via atomics.

This intentionally does not add a try-lock or a posting lock. If future callers can concurrently mutate the same TX available ring, the correct follow-up is to introduce a bounded non-spinning serialization scheme, not an IRQ-disabled spinlock.
