# P6/P7/P8 conversion plan

Plan B remains the right scope: convert network RX polling, TX completion
busy-waits, and x86 hlt-loop `process_rx()` polling as one bundled campaign
split into small source turns.

Turn 19 stale-contract resolution:

- Production `NetRx` is raised by x86 e1000 IRQ and aarch64 VirtIO MMIO IRQ.
- PCI VirtIO net never raises `NetRx`.
- The documented 10ms timer-raised NetRx contract is not live.
- Parallels boots initialize PCI net and resolve init ARP via synchronous
  polling while `msi_count=0`; current boot success does not prove post-init
  PCI RX IRQ delivery.

Open-question resolution: start with budgeted `process_rx()`, not with
PCI-MSI-schedules-NetRx.

Reason: scheduling `NetRx` from the PCI MSI handler before the poll path is
budgeted would run today's unbounded `process_rx()` from IRQ-exit softirq
context and would still lack a Linux-shaped completion point for callback/GIC
re-enable. A budgeted poll API is a low-risk prerequisite because init and
on-demand callers can temporarily pass `u32::MAX` to preserve current behavior
while the IRQ path starts using a Linux-like weight such as 64.

## Substep 0 decision

No source-changing Substep 0 is required before Substep 1.

The stale contract is resolved statically: PCI net's live IRQ graph is broken
at `handle_interrupt() -> NetRx pending`. Existing `NET_PCI_MSI_COUNT` is
already exposed through `/proc/trace/counters` as `net_msi_irqs`, so empirical
confirmation can be a gate in the PCI scheduling substep rather than a separate
diagnostic source commit.

If a live traffic gate is impossible because external probe access remains
blocked, Substep 2 may add one lock-free TraceCounter for "PCI IRQ raised NetRx"
as part of the conversion commit. Do not add serial logging in the IRQ path.

## Substep 1: make `process_rx()` budgeted

Goal:

- Introduce a budgeted RX polling API, for example:
  - `pub enum PollOutcome { Drained, BudgetExhausted }`
  - `pub fn process_rx_budgeted(budget: u32) -> PollOutcome`
  - keep `process_rx()` as a compatibility wrapper if that reduces churn.
- The NetRx softirq path should use a Linux-like budget, likely 64.
- Init ARP/ICMP and on-demand ARP temporarily use `u32::MAX` to preserve
  behavior.
- x86 hlt-loop callers use budget 64.
- The return value should let later substeps re-raise NetRx or keep callbacks
  suppressed when budget is exhausted.

Dependencies:

- None; this is the structural prerequisite for all later substeps.

Gate:

- Build must be warning-free.
- Single Parallels boot first-failure-abort must preserve the current PASS
  markers.
- `git diff --stat kernel/` is expected to include only the touched network
  and x86 caller files for this substep.

Biggest risk:

- A caller accidentally gets a small budget where the old synchronous behavior
  is still required, causing init ARP or on-demand ARP to fail before the async
  neighbor work exists.

Serial failure shape:

- `NET: Gateway ARP not resolved, skipping ping test`
- `ARP lookup failed - gateway did not respond`
- bsshd/bounce not starting because boot exits network init early or stalls.

Minimal revert:

- Revert the budget API and callsite updates, likely `kernel/src/net/mod.rs`
  plus the three x86 test-only loops in `kernel/src/main.rs`.

## Substep 2: make PCI MSI schedule NetRx and complete like NAPI

Goal:

- Change `net_pci::handle_interrupt()` so PCI RX MSI marks network work pending
  instead of suppressing and stopping.
- Preserve minimal IRQ-handler work: acknowledge/clear what must be cleared,
  disable device callbacks if needed, then raise `SoftirqType::NetRx`.
- Move the `re_enable_irq()` race-check semantics into the budgeted NetRx
  completion path:
  - if the poll drained the ring, clear pending and re-enable callbacks/GIC;
  - if the poll exhausted budget, keep work pending or re-raise `NetRx` without
    reopening a storm window.
- Use existing `net_msi_irqs` plus one additional lock-free counter if needed
  to prove PCI IRQ -> NetRx scheduling. No IRQ-path logging.

Dependencies:

- Substep 1, because the PCI softirq path needs a budgeted completion point.

Gate:

- Build warning-free.
- Single Parallels boot first-failure-abort.
- Existing boot markers remain healthy.
- Under whatever inbound traffic can be generated, `net_msi_irqs` should
  increase and RX should continue after more than one PCI MSI.

Biggest risk:

- Reintroducing the GICv2m storm that the current suppression code was trying
  to avoid, or leaving the SPI disabled after the first interrupt.

Serial failure shape:

- heartbeat/timer progress stops shortly after `MSI-X SPI ... enabled`
- CPU0 timer regression or soft lockup markers
- `net_msi_irqs` increments once and then RX stops
- repeated NetRx budget-exhausted counter growth without forward progress

Minimal revert:

- Revert `kernel/src/drivers/virtio/net_pci.rs` and the NetRx completion
  changes in `kernel/src/net/mod.rs`.

## Substep 3: convert TX completion to async

Goal:

- Remove the `used.idx` busy-wait from PCI TX
  (`kernel/src/drivers/virtio/net_pci.rs:700-747`).
- Remove the `used.idx` busy-wait from MMIO TX
  (`kernel/src/drivers/virtio/net_mmio.rs:548-588`).
- Reclaim TX completions from the network poll path, following Linux's
  cleantx/TX-NAPI model.
- Do not reuse a single static TX buffer before completion. The current code's
  static TX buffers are safe only because transmit waits for completion before
  returning; async TX needs per-descriptor ownership, a small pending ring, or
  an explicit "queue full" result.

Dependencies:

- Substep 1 for budgeted network polling.
- Prefer Substep 2 first so PCI has a reliable completion scheduler.

Gate:

- Build warning-free.
- Single Parallels boot first-failure-abort.
- Gateway ARP still resolves.
- bsshd starts and the boot log shows no TX timeout markers.
- If external traffic is available, verify SSH/bounce traffic still transmits.

Biggest risk:

- Buffer lifetime corruption: a second transmit overwrites the static TX
  buffer or descriptor while the device still owns the previous packet.

Serial failure shape:

- `[virtio-net-pci] TX timeout!` should disappear; replacement failures would
  likely be ARP resolution failure, malformed packet behavior, or network
  silence after the first transmit.

Minimal revert:

- Revert TX queue ownership changes in `kernel/src/drivers/virtio/net_pci.rs`,
  `kernel/src/drivers/virtio/net_mmio.rs`, and any shared net poll cleanup hook
  in `kernel/src/net/mod.rs`.

## Substep 4: remove synchronous init ARP/ICMP polling

Goal:

- Delete the init-time gateway ARP wait loop and ICMP reply polling loop in
  `kernel/src/net/mod.rs:348-417`.
- Enable IRQs as part of network readiness rather than after the synchronous
  polling window.
- If a boot health check still needs gateway connectivity, move it to an async
  post-init diagnostic task that observes neighbor state through normal RX.

Dependencies:

- Substeps 1 and 2.
- Prefer Substep 3 first if init traffic depends on async TX ownership.

Gate:

- Build warning-free.
- Single Parallels boot first-failure-abort.
- Network stack declares ready without the old ARP/ICMP polling loops.
- Within a bounded post-init window, ARP cache populates via IRQ/NetRx when
  traffic requires it.

Biggest risk:

- Boot tasks assume the gateway MAC is already cached immediately after
  `net::init()`.

Serial failure shape:

- Later packet send paths report ARP lookup failure.
- bsshd starts but traffic to/from it cannot resolve the gateway.

Minimal revert:

- Revert the init-loop deletion and IRQ-enable ordering change in
  `kernel/src/net/mod.rs`, plus any small driver enable-order adjustment.

## Substep 5: remove synchronous on-demand ARP polling

Goal:

- Replace the 50-iteration on-demand ARP polling loop
  (`kernel/src/net/mod.rs:596-628`) with async neighbor behavior:
  - queue the triggering packet within a small bound, or
  - return a clear would-block/host-unreachable style error if queueing is not
    implemented yet.
- ARP replies must be consumed only by the normal IRQ/NetRx path.

Dependencies:

- Substeps 1, 2, and 4.
- A minimal neighbor pending queue or explicit send retry contract.

Gate:

- Build warning-free.
- Single Parallels boot first-failure-abort.
- First packet to an uncached next-hop resolves through normal ARP reply
  handling, then the original or retried packet succeeds.

Biggest risk:

- Dropping the first packet permanently with no retry path, making new
  connections flaky.

Serial failure shape:

- repeated ARP requests for the same next-hop with no queued packet release
- connection attempts fail until a later manual retry

Minimal revert:

- Revert the neighbor/on-demand resolution changes in `kernel/src/net/mod.rs`
  and any new queue state.

## Substep 6: remove x86 hlt-loop net polling

Goal:

- Delete the three x86 test-only idle-loop `net::process_rx()` calls in
  `kernel/src/main.rs`.
- Keep loopback queue draining if it is not a NIC polling workaround and still
  belongs to those tests.

Dependencies:

- x86 e1000 IRQ -> NetRx path must be reliable with budgeted `process_rx()`.

Gate:

- Build warning-free for x86.
- Relevant x86 test workflows pass without the hlt-loop packet drain.

Biggest risk:

- The x86 test harnesses were hiding a softirq scheduling bug; removing the
  poll causes DNS/blocking recv/nonblock EAGAIN tests to hang.

Serial failure shape:

- test-only boot enters idle loop and never prints the expected test pass
  marker
- e1000 interrupt count increases but NetRx processing does not run

Minimal revert:

- Revert the three call removals in `kernel/src/main.rs`.

## Final ordering

Recommended implementation sequence:

1. Substep 1: budgeted `process_rx()`.
2. Substep 2: PCI MSI raises/schedules NetRx and uses budgeted completion.
3. Substep 3: async TX completion for PCI and MMIO.
4. Substep 4: remove synchronous init ARP/ICMP polling.
5. Substep 5: remove synchronous on-demand ARP polling.
6. Substep 6: remove x86 hlt-loop net polling.

Each substep should be its own commit and Ralph turn with first-failure-abort
boot validation. If any substep fails, revert only that substep and stop for
review.
