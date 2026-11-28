# E10 network stack

DEV-41(2) starts the network design under DEV-29 and covers audit R21, R22, R23
and R28. The implementation order starts with **NewReno, legacy and modern
virtio-net, and hashed TCP lookups**, independently of E1 and R240 delivery. The shared packet
data path follows E1's frame-backed allocations. Protocol behavior comes from
the RFCs, device behavior from the hardware specifications, and current behavior
from Breenix and saved benchmark receipts. Linux is a measured black-box peer.

## Starting point and target

| Finding | Breenix path | Consequence to measure |
| --- | --- | --- |
| R21 | [TCP output, ACK processing and retransmission](../../kernel/src/net/tcp.rs) limit new sends by the peer window and recover through the retransmission timer. The receiver emits SACK blocks. | Sender bursts and timeout recovery can cost throughput. Per-run timeout deltas help attribute the loss. |
| R22 | [Virtio-net MMIO](../../kernel/src/drivers/virtio/net_mmio.rs) posts four RX buffers and caps split queues at 16 entries. The feature request is MAC and STATUS. The [TX queue](../../kernel/src/drivers/virtio/net_tx_queue.rs) serializes publication and reclaims completions on submit. | RX refill, ring exhaustion and notification overhead can constrain both directions. |
| R23 | [TCP](../../kernel/src/net/tcp.rs) scans vectors for connections, listeners and binds under one lock. It scans connections for timers and TX retries. [IPv4](../../kernel/src/net/ipv4.rs) and [Ethernet](../../kernel/src/net/ethernet.rs) build fresh contiguous packets. | Per-flow work grows with table size. Serialization, allocation and copying limit throughput. |
| R28 | [Socket options](../../kernel/src/socket/opts.rs) feed fixed TCP buffer caps. ACKs wake writers as bytes are freed. [Loopback](../../kernel/src/net/mod.rs) uses shared queues and syscall-side drains. The [BCM5720 driver](../../kernel/src/drivers/bcm5720/mod.rs) uses one active send ring and one return ring, with 63 RX buffers and per-packet mailbox updates. | Write wakes, local queue contention, receive-window limits and device overruns are distinct mechanisms. BCM5720 overruns need its chip counters. |

The saved ARM64 QEMU series pair `qemu-20261127-28668b28-{breenix,linux}` gives
the brief's roughly **6× to 11× raw TCP** and **2× to 3× SCP** gaps. The medians
below are historical, with five completed samples of five in each guest and
row. The 10% parity target is measured against the stock Linux guest on the
same rig under DEV-27 and DEV-31.

| bbench row | Breenix | Linux | Breenix slowdown |
| --- | ---: | ---: | ---: |
| `network.tcp-guest-to-host` | 57.4 MB/s | 354.3 MB/s | 6.17× |
| `network.tcp-host-to-guest` | 76.9 MB/s | 857.9 MB/s | 11.15× |
| `network.scp-upload` | 0.182 s | 0.085 s | 2.15× |
| `network.scp-download` | 0.305 s | 0.106 s | 2.87× |
| `ssh.round-trip` | 0.040 s | 0.066 s | 0.62× |

The raw rows transfer 32 MB between host and guest without file or crypto work.
SCP transfers 8 MB and includes SSH and file work. Treat SCP changes as secondary
evidence and record the cipher and cache conditions. `network.bsync-upload`
includes inventory and publication and is deferred on the QEMU rig. Historical
receipts and provenance are under
`~/Library/Caches/breenix-devbox/bench-series/`. Row definitions and the rig are
in [tables.py](../../userland/bbench/tables.py) and
[rig.py](../../userland/bbench/rig.py).

The audited R240 baseline at `bbb69a99` has 2.628 s for 8 MB SCP download
and 8.947 s for bsync upload. The lane's concerns about RX starvation and
SSH crypto are hypotheses. Use saved BCM5720 drop counters and the raw TCP
directions to isolate the kernel contribution. The upload and download
times include different work. [R240 bbench baseline](../../userland/bbench/BASELINE.md).

## TCP transmission and recovery

The TCP core owns sequence numbers and wire validation, the peer window,
retransmission ranges, timers and output. A per-connection congestion policy
receives acknowledged bytes, flight size, MSS, clock/RTT samples, and
idle/recovery/timeout events. It supplies the congestion window, loss target
and, later, a pacing rate. Keep dispatch internal, with NewReno selected when a
connection is created. Recovery chooses **which range** to send and how much
repair traffic to admit. The congestion policy determines **the window and
loss response**. CUBIC will use the same interface as NewReno.

- **NewReno first.** Track actual sent sequence space independently of the
  retransmit cursor. Admit new data only within the peer's advertised right edge
  and the congestion allowance. Use RFC 5681's MSS-dependent initial window,
  slow start, byte-counted additive growth, idle restart and flight-based
  reduction. Count qualifying duplicate ACKs only after sequence and ACK
  validation, excluding window updates. Use SMSS for the sender's maximum
  segment size. At the third qualifying duplicate, enter recovery only if the
  ACK covers a sequence beyond `recover` (compare `ACK - 1` with `recover`
  using wrap-safe sequence arithmetic). Initialize `recover` to the initial
  send sequence at connection setup. On entry, snapshot the highest sequence
  sent in `recover`, set `ssthresh = max(FlightSize / 2, 2 * SMSS)` using
  outstanding data excluding any Limited Transmit segments, retransmit the
  first unacknowledged segment, and set `cwnd = ssthresh + 3 * SMSS`. Each
  further qualifying duplicate in recovery adds one SMSS to cwnd and permits
  a new segment if the window allows.
  [RFC 5681 §§2, 3.1–3.2](https://www.rfc-editor.org/rfc/rfc5681.html),
  [RFC 6582 §3.2, steps 1–2](https://www.rfc-editor.org/rfc/rfc6582.html#section-3.2).

  On a partial ACK, retransmit the first unacknowledged segment, subtract
  newly cumulatively acknowledged bytes from cwnd and add back one SMSS if
  the ACK advances by at least one SMSS. Remain in recovery, send new data if
  permitted, and restart the retransmit timer on the first partial ACK.
  On a full ACK through `recover`, set
  `cwnd = min(ssthresh, max(remaining FlightSize, SMSS) + SMSS)` and exit
  recovery. [RFC 6582 §3.2, step 3](https://www.rfc-editor.org/rfc/rfc6582.html#section-3.2).

  On data RTO, set cwnd to one SMSS and update ssthresh by the flight-size
  rule on the first timer retransmission of that segment. Hold ssthresh
  constant on subsequent timeouts for that segment. Set `recover` to the
  highest sequence already sent, clear the duplicate-ACK count, exit fast
  recovery, retransmit just the oldest unacknowledged segment, and resume slow
  start. Three duplicate ACKs covering at most `recover` cannot trigger a
  second fast retransmit or ssthresh reduction. Keep SYN/FIN sequence
  accounting and zero-window persist separate. A failed local admission
  retains the range for retry and leaves flight accounting unchanged.
  [RFC 5681 §3.1](https://www.rfc-editor.org/rfc/rfc5681.html#section-3.1),
  [RFC 6582 §3.2 step 4 and §4](https://www.rfc-editor.org/rfc/rfc6582.html#section-4).
- **Retransmission clock.** Keep SRTT and RTTVAR, update variance with the old
  SRTT, apply Karn's rule, restart for newly cumulatively acknowledged data,
  and back off on timeout. Use RFC 6298's recommended one-second floor for
  computed RTO and its handshake exception. Local TX backpressure and persist
  have distinct retry events. Retain `/proc/net/tcp_stats` timeout attribution
  and count actual retransmitted segments by cause. A timeout count and a
  transmitted repair count answer different questions.
  [RFC 6298 §§2–5](https://www.rfc-editor.org/rfc/rfc6298.html).
- **SACK scoreboard.** Parse bounded SACK blocks after normal ACK validation
  if our SYN advertised SACK-Permitted. Send SACK blocks if the peer advertised
  permission. Validate ranges in wrap-safe
  outstanding sequence space. Merge overlap and count delivered bytes once.
  Track sent ranges, transmissions and selectively delivered/lost ranges
  separately from cumulative ACK. Retain payload until the cumulative ACK.
  Reset or revalidate selective state on RTO where reneging is possible. Use
  RFC 6675's pipe and loss accounting until RACK supplies the loss decision.
  Run NewReno's recovery path on connections without usable SACK feedback.
  [RFC 2018 §§2–5](https://www.rfc-editor.org/rfc/rfc2018.html),
  [RFC 6675 §§3–5](https://www.rfc-editor.org/rfc/rfc6675.html).
- **RACK-TLP and PRR.** Record monotonic transmit time and an end-sequence
  tie-breaker per transmitted range. RACK uses ACK/SACK delivery of a later
  transmission plus the reordering window and a reordering timer to declare
  older transmissions lost. Implement the RFC's RTT and reordering rules,
  including DSACK feedback. TLP schedules a probe before RTO when eligible,
  allowing one outstanding probe and preferring permitted new data to a tail
  retransmission.
  Preserve the RTO safety net. PRR meters recovery sends using newly delivered
  bytes, recovery flight size, bytes sent during recovery and the selected
  `ssthresh`. Keep one recovery episode across partial ACKs. Charge against
  peer-window and device admission limits as well as PRR allowance.
  [RFC 8985 §§5–7](https://www.rfc-editor.org/rfc/rfc8985.html),
  [RFC 6937](https://www.rfc-editor.org/rfc/rfc6937.html),
  [RFC 9937 §§3–5](https://www.rfc-editor.org/rfc/rfc9937.html).
- **Timestamps, CUBIC and pacing.** Negotiate timestamps and window scaling on
  SYN, budget TCP option space for timestamps and SACK, and apply TS.Recent and
  PAWS with wrap and idle expiry. Use valid echoes for RTT measurement under
  RFC 7323, keeping local transmit time for RACK. Negotiate a scale large enough
  for the maximum receive window at connection setup and account for option
  bytes in effective segment payload size. Add CUBIC with integer arithmetic,
  elapsed-time cubic growth, the Reno-friendly region, fast convergence and
  app-limited epoch handling. Keep NewReno selectable internally and make CUBIC
  the default after the controlled comparison. Pace new data and recovery
  output with per-flow send deadlines and a bounded byte burst, based on
  window, SRTT and the recovery allowance. ACK/control output still follows
  protocol deadlines. Cap an offload batch by the same burst budget.
  [RFC 7323 §§2–5](https://www.rfc-editor.org/rfc/rfc7323.html),
  [RFC 8312 §§4–5](https://www.rfc-editor.org/rfc/rfc8312.html),
  [RFC 9438 §§4–5](https://www.rfc-editor.org/rfc/rfc9438.html).

Nagle holds a short data segment when sent data remains unacknowledged. A full
effective MSS or an ACK draining the flight makes held data eligible, subject to
normal send limits. Default to Nagle on, carry the socket's `TCP_NODELAY` into
the connection, and schedule held bytes promptly when it is enabled on a live
socket. Retransmissions and required control traffic have their own admission
rules. Ensure orderly shutdown flushes queued data in sequence before the FIN.
[RFC 9293 §3.7.4](https://www.rfc-editor.org/rfc/rfc9293.html#section-3.7.4).

In PR 4, set `TCP_NODELAY` with an integer value of 1 on the connected
[bssh](../../userspace/programs/src/bssh.rs) fd and on each accepted
[bsshd](../../userland/bsshd/src/bsshd.rs) fd before the first SSH write.
Report option failures through the respective connection error path. The choice
applies to the transport for its lifetime so the handshake, short exec
exchanges and interactive channels all get it, including on reused transports.
Check `ssh.round-trip` for a regression in the matched before/after run.

## Packet ownership and socket concurrency

### Packet buffers and DMA

Use a descriptor with a writable linear head, bounded reserved headroom for
headers, and reference-counted frame fragments described by offset and length.
Carry checksum state, segmentation parameters and receive device metadata
explicitly. A shared fragment is immutable. A writer gets private header
storage when it needs to change a shared header. The send queue keeps payload
fragments through cumulative ACK, while each device submission holds separate
DMA references until verified completion or quiesced reset. One validated
user-to-kernel copy fills the TX fragments. Add TCP, IP and link headers in
headroom and submit a scatter-gather chain within the device's limits.
Coalesce or segment in software when a device cannot accept the chain.

RX transfers ownership of completed buffers into the stack. Replenish with
distinct frames before exposing a new descriptor to hardware. Hand fragment
references through in-order and out-of-order receive queues, then copy directly
to validated userspace destinations. Reuse the frame only after both device and
stack have released it. Bounded descriptor pools and fallible refill keep
failure local. A failed TX reservation leaves bytes with the caller or socket.
RX exhaustion accounts for each drop and leaves capacity for refill and control
progress. Keep device visibility and cache synchronization in the DMA
contract, including correct barriers before handing over descriptors and after
consuming completion indices.

**E1 dependency.** The common path needs the frame leases, direct map, per-CPU
page cache/zone allocation and checked DMA addresses in
[E1](e1-kernel-memory.md), notably the E1-8 allocation and DMA interface after
its prerequisites. Coordinate the fallible network allocation work with
E1-16. Device-local copy pools in the first virtio PR use existing DMA storage.
They do not introduce the shared frame-fragment lifetime ahead of E1.

### Hash tables, ownership and deadlines

Hash established sockets by local and remote address/port, listeners by local
address/port with exact-before-wildcard match, and bind owners by local port
with address and reuse checks within the bucket. Use keyed hashing, full key
comparisons, and fallible growth. Keep the current lock for the first
conversion so state and memberships publish together. Preserve active open,
listener children, reuse rules, simultaneous close and TIME_WAIT reservations,
and removal of closed orphan entries. Publish an index change only after all
associated space has been secured.

Then place protocol state under a per-socket lock. Table shards publish
reference-counted socket identities and membership records, with generations
to reject stale work after close or tuple reuse. Cross-index changes reserve
and publish under shard locks in a fixed table/shard order. An operation
releases those locks before taking a socket lock and revalidates the pinned
identity under that lock. Close marks the generation dead before unlinking
memberships. Per-listener SYN and ready-to-accept queues carry explicit bounds.
Connection admission and listener close must settle each child exactly once.

Softirq gets a socket reference from demux and enqueues to a bounded socket
backlog when task context owns state. The backlog gate protects both enqueue
and the owner's empty-check/release handoff. A producer drops the gate before
trying the state lock or scheduling a worker. Pending work remains published
when a budgeted drain yields. The socket owner prepares output under state
ownership, then releases it for routing, device queueing, doorbells and wait
notifications. Keep one ordered output owner per flow. Bounded egress
admission reserves the range once, a full device queue retains owned work for
retry, and dispatch stamps become visible before packets can produce ACKs.
A completion frees DMA references and wakes device-space waiters. A TCP ACK
retires stream bytes independently. Hard IRQs only publish bounded work.
Use the durable wake path for work and socket notifications.

Replace `ktcptimerd`'s table scan with a hierarchical timer wheel. Embed
entries for retransmit, persist, delayed ACK, keepalive, FIN_WAIT_2/TIME_WAIT,
and later RACK, TLP and pacing. Update an entry on deadline change and kick its
worker if an earlier deadline is published. Pop bounded due batches outside
the socket lock, drop the wheel lock, then check socket incarnation, timer
generation and absolute deadline under state ownership. Round coarse deadlines
up and keep a high-resolution next deadline for pacing. Use the monotonic
clock and the platform's deadline facility. Measure pacing resolution before
making it the default. [Varghese and Lauck, Hashed and Hierarchical Timing
Wheels](https://www.cs.columbia.edu/~nahum/w6998/papers/sosp87-timing-wheels.pdf).

### Socket memory, readiness and local delivery

Charge actual frame-backed capacity and descriptor metadata to per-socket and
global network budgets, including retained out-of-order data and retransmit
storage. Shared references count the backing once, with separate accounting for
the descriptors. Bound growth by a RAM-relative budget and reserve progress
capacity for control traffic. Receive growth follows bytes drained by the
application per measured RTT with bounded headroom. Send growth follows
observed flight, including congestion window and application demand. Explicit
`SO_SNDBUF` and `SO_RCVBUF` choices cap autotuning under the same global
budget. Reduce unused capacity under pressure. Preserve the advertised right
edge, accounting for already promised space before granting new window
credit. Allocation denial yields backpressure, and release reopens growth.

Use one write-space readiness predicate for blocking sends and poll/epoll.
Start from the existing half-free policy, capped by the effective buffer and
raised enough for a segment when capacity allows. Wake writers on transition
to writable, and on shutdown/error. Reevaluate after ACK, buffer resize and
budget release. Readers have their own threshold and wake path. Retain the
existing publish/recheck wait contract and readiness generation so a waiter
registering on the threshold crossing sees it. Keep short writes, nonblocking
progress, and terminal events correct. Stream socket `write()` through
bounded copy-in and the send path so partial progress is returned on fault or
pressure, using the Phase 1.8 work if present.

Make loopback a device with a 64 KiB interface MTU and the same packet handoff.
Respect the IPv4 total-length limit when forming IP packets.
Deliver through per-CPU receive backlogs and preserve ordering within each
flow. Replace the global drain spin with targeted close/bind publication and
delivery ordering. A local close publishes its terminal state and port
ownership before returning, and a reused endpoint rejects stale queued work.
For UDP, publish a read-side-safe hashed port table with immutable bucket
snapshots and a read-side grace period. Preserve broadcast fan-out and local
address matching. Contention in bind/close is decoupled from receive lookup.

## Device paths

### Virtio-net

Use virtio 1.2 split queues on both MMIO transports in PR 2. Derive
ring offsets, alignment and allocation from the selected power-of-two depth.
Start at 256, support up to 1024 if advertised and within the DMA budget, and
honor smaller device maxima. Post RX capacity proportional to ring size.
Both transports negotiate advertised `MRG_RXBUF`, `EVENT_IDX`, `CSUM`,
`HOST_TSO4` with `CSUM`, and `CTRL_VQ` with `MQ` when supported. Both also
get the new TX completion path. A device offering only MAC/STATUS still gets
the larger queues when space and `QueueNumMax` permit.

On MMIO version 1, negotiate the supported legacy feature word during reset
and initialization without `VERSION_1` or `FEATURES_OK`. Set `GuestPageSize`
after reset and before queue setup. Each queue uses a zeroed, physically
contiguous split-ring allocation, with descriptors and the available ring
first and the used ring padded to `QueueAlign`. Configure `QueueNum` and
`QueueAlign`, then publish the base page through `QueuePFN`. Use guest-endian
ring and header fields and the legacy framing rules when selecting descriptor
chains. The legacy packet header is 10 bytes without `MRG_RXBUF` and 12 bytes
with it. On MMIO version 2, require `VERSION_1`, negotiate features and verify
`FEATURES_OK`, then publish the individual queue addresses and set
`QueueReady`. Both paths set `DRIVER_OK` after queues are configured.
[OASIS Virtio 1.2 §§2.7.2–2.7.4, 3.1, 4.2.3–4.2.4, 5.1.6.1](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html).

Size
queue pairs from online CPUs, device maximum and memory budget. Negotiate
`CTRL_VQ` with `MQ`, confirm the control command setting the active queue-pair
count, and keep a one-pair configuration if the feature or activation is
unavailable. Use a stable flow-to-queue selection and a per-queue lock around
reservation, descriptor fill and avail-index publication.

With `MRG_RXBUF`, parse the header from the first buffer and consume the
specified number of used buffers as a single frame. Validate descriptor IDs,
lengths, the merge count and total frame size before passing it up. Handle
wrap and a drain budget that stops at a packet boundary. For `EVENT_IDX` use
16-bit wrap-safe event accounting, barriers before checking suppression, and
a recheck after rearming notifications. Reclaim on TX submit and from bounded
used-ring processing even without RX. MMIO shares an interrupt, so dispatch
work for the relevant RX and TX queues. A transition from full to available
retries deferred TX.

Implement negotiated `CSUM` and `HOST_TSO4` with valid offsets, header lengths,
MSS and pseudo-header checksum seed. Accept `GUEST_CSUM` only with a path that
completes or verifies the checksum before the stack uses the packet. Keep
`GUEST_TSO4` disabled until the receive path can safely consume or segment a
coalesced packet. The first PR can send a bounded eligible TCP batch through
the existing copy path, leaving ordinary frames and software checksums as
fallbacks. Debit flight for the payload and track logical on-wire segments,
even when the device gets one request. Flush promptly for control traffic,
changed options or a short tail. Adopt scatter-gather from the shared packet
descriptor after E1. Keep feature-specific transport work with the driver.
[OASIS Virtio 1.2 §§2.6–2.7, 3.1, 4.2, 5.1](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html).

### Broadcom BCM5720

Use 512-entry send, standard RX producer and RX return rings in standard-frame
mode. Reserve the empty slot on producer rings and size return capacity for
the enabled producer rings. Enforce the documented DMA address boundaries and
ring alignment. The return descriptor identifies a completed RX buffer. Consume
it before returning the index, and refill and publish in a batch. Batch TX
producer and RX producer/consumer mailbox writes with a bounded flush on
idle or backpressure. Use tagged status and host coalescing, with
`COAL_NOW` reserved for an explicit flush. Preserve the mask, drain, rearm and
recheck sequence. Separate TX from RX locking and give the shared RX producer
one refill owner.

Use the documented IP and TCP/UDP checksum descriptor flags on TX and qualify
RX checksum state from return flags and error evidence. Use LSO fields for
eligible IPv4 TCP batches (header length and MSS) and consecutive send BDs
ending in `PACKET_END`. Validate lengths, flags and supported headers before
handoff and use software fallback otherwise. Use RSS indirection for stable
IPv4 flow placement across up to four receive return rings in RSS mode.
Configure the control/status vector and per-return-ring MSI-X vectors,
status blocks and interrupt mailboxes as specified. Cap active data queues by
online CPUs and usable device vectors. Fall back to single-queue MSI with
the existing RX/TX contract. Keep the reset and bootcode handshake and leave
APE access outside the work. Compare existing `no_rx_bd`, `in_discards` and
`tx_ring_full` plus interrupts, batches and per-queue distribution at the
coordinator's R240 checkpoint.
[Broadcom BCM5718 Family Programmer's Reference Guide, 5718-PG107-R,
2013-07-17, pp. 70–106, 122–124, 228–259](https://docs.broadcom.com/doc/1211168564147).

## Implementation PRs, in landing order

Each row names the behavioral check to commit first and the expected movement
in the QEMU series. In the last column, **TX** is `network.tcp-guest-to-host`,
**RX** is `network.tcp-host-to-guest`, and **SCP up/down** are
`network.scp-upload` and `network.scp-download`. **SSH** is `ssh.round-trip`.
Record these five rows using the rig's `network,tcp-send,tcp-recv` subset.
Smaller gains on a single stream can coexist with larger benefits under the
concurrency, loss or RTT named in the prediction.

| PR | Scope and dependency | Deterministic check | Expected ARM64 QEMU rows |
| --- | --- | --- | --- |
| 1 | NewReno policy interface, send admission and RTO. **No E1.** | Drive the production send/ACK path with a fixed clock and an eight-SMSS flight with two losses and no more application data. Check admission, the third duplicate ACK's first repair and `(cwnd, ssthresh) = (7, 4) SMSS`, another duplicate raising cwnd to 8 SMSS, a three-SMSS partial ACK lowering it to 6 SMSS and repairing the next hole while restarting the timer, and the full ACK lowering cwnd to 2 SMSS and exiting. In the timeout trace check one oldest-range repair, backoff, cwnd of one SMSS and the highest-sent recovery marker. Repeat the same segment's timeout and inject three qualifying duplicate ACKs within the marked flight. Check the unchanged first-timeout ssthresh and no fast retransmit, then allow a fresh recovery after ACK progress beyond the marker. | TX and SCP down benefit when sender loss or queue bursts dominate. A clean path can show slower startup. RX and SCP up are controls. |
| 2 | Legacy and modern virtio-net rings, merged RX, checksum/TSO, EVENT_IDX, TX completion and multiqueue. **No E1.** | Run the production setup with fake version-1 MMIO advertising a 256-entry maximum and the PR's feature bits. Check 256 entries, more than four posted RX buffers, exact guest-page/PFN/used-ring offsets, the accepted low feature word and the 10-/12-byte legacy headers with merge disabled/enabled. Check modern VERSION_1 and FEATURES_OK separately. Drive merge and event-index wrap, malformed merges, offload header bytes, control-command rejection and TX full-to-available without RX traffic. | TX and RX are primary on the existing legacy rig. SCP up/down may improve if packet service limits either direction. Record which features were offered and enabled.
| 3 | Hashed established, listener and bind indexes. **No E1.** | Collision fixture with unrelated connections drives production demux and bind. Check exact/wildcard selection, TIME_WAIT and reuse conflicts, and that a failed reservation preserves the established, listener and bind indexes. A fixed comparison counter shows lookup work stays within the affected bucket. | TX/RX and SCP up/down can gain with many active sockets. Little movement is expected on the standard single-flow rows. |
| 4 | Nagle default, live `TCP_NODELAY`, and SSH transport opt-outs. **No E1.** | With an outstanding small write, verify that the next small write waits, an ACK or full MSS permits output, and enabling NODELAY flushes eligible data. Drive both SSH transport setup paths with a fixture that records NODELAY on the connected or accepted fd before the first protocol write. Withhold the first ACK and verify two short writes leave on the opted-out TCP connection. | Use `ssh.round-trip` as an explicit regression guard against the matched pre-PR Breenix run. SCP up/down guard short control writes. TX/RX are bulk controls.
| 5 | Frame-backed packet descriptors, receive ownership and virtio scatter-gather. **E1-8 and prerequisites**, with E1-16 coordination. | Force descriptor exhaustion and an ACK preceding DMA completion. Check reference retirement and exact bytes across odd fragment boundaries, software checksum and fallback. | TX/RX gain from fewer copies and allocations. SCP up/down can follow. |
| 6 | Per-socket state, listener queues, backlog and ordered deferred output. Depends on 3 and 5 plus durable wakes. | Force RX between an owner's last drain and release, and close/reuse while queued RX and TX still hold the old reference. Verify one delivery, one retirement and progress on a different socket during the pause. | TX/RX and SCP up/down can gain from less lock hold time. Largest movement is expected with concurrent flows. |
| 7 | Timer wheel. Depends on 6. | With fake time and one due timer, run the actual timer worker pass with 16 and then 4096 unrelated sockets, including sockets with later deadlines. Count connection visits across deadline selection, expiry/reaping and notification collection. In each run require one visit to the due socket, zero visits to unrelated sockets, and one event. Also arm an earlier deadline during the worker's wait and cancel an entry after dequeue before tuple reuse. Check the wake and stale-event rejection. | TX and SCP down can gain when many deadlines compete with real work. SSH and the single-flow rows act as latency and throughput controls. |
| 8 | SACK scoreboard and range-based recovery. Depends on 1 and 5. | Drive a cumulative ACK plus overlapping SACK blocks across sequence wrap, then RTO and reneging. Check unique delivery credit, hole-only repair before timeout, and retention of selectively acknowledged bytes. | TX and SCP down gain if multiple losses share a flight. RX and SCP up are controls. |
| 9 | RFC 7323 timestamps, RACK-TLP and PRR. Depends on 7 and 8. | Replay reordering followed by real tail loss with fake time. Check the reordering window, probe before RTO, one repair of the lost range, bounded PRR sends and stale timestamp rejection across wrap. | TX and SCP down gain on loss or tail stalls. SSH may gain if its short exchange previously hit RTO. |
| 10 | CUBIC and pacing. Depends on 9. | Feed fixed ACK times, loss and an idle interval to both policies. Check CUBIC's window trajectory and restart, pacing deadlines, burst limit and recovery credit under delayed device service. | TX and SCP down should gain on long, fast paths or from fewer bursts. Record ordinary QEMU rows even when the short path shows little change. |
| 11 | Socket autotuning, the global budget and write-space wakes. Depends on 5, 6 and timestamp/scale handling in 9. | Feed fixed RTT and drain samples, then force global pressure and a writer registering across the free-space threshold. Check budget conservation, the advertised right edge and a single readiness transition. | TX/RX improve where small windows or wake frequency bind. SCP up/down are secondary. |
| 12 | Loopback device and UDP port publication. Depends on 5 and 6. | Fix the interleaving of local close/rebind and backlog delivery, then hold a UDP bucket writer while delivering to an existing binding. Check in-order local delivery, stale packet rejection and receive progress. | SSH and SCP up/down are regression controls on the external rig, as are TX/RX. The current rig has no guest-loopback throughput or UDP row. |
| 13 | BCM5720 512-entry rings, batch publication and coalescing. Depends on E1's DMA allocation for multi-page ring storage. | Fake status and MMIO drive wrap, batch flush and an update during rearm. Check exact indices, reserved slot and visible descriptors before mailbox write. | No QEMU network row exercises this device. TX/RX and SCP up/down are controls. Coordinator checks real-device RX/TX and drops at the R240 checkpoint. |
| 14 | BCM5720 scatter-gather offload and RSS/MSI-X. Depends on 5 and 13. | Use the manual's descriptor formats in a fixture with a wrapped multi-BD send, checksum result/error cases, and RSS-selected completions on distinct vectors. Check terminal BD and retirement, then exact return-ring/mailbox mapping. | No QEMU network row exercises this device. TX/RX and SCP up/down are controls. Coordinator checks both directions, interrupts and per-queue distribution on the R240. |

## Implementation evidence

- Start from the affected Breenix paths and the saved receipts. Commit the few
  deterministic checks separately and show red on the old behavior. Prefer
  host tests driving production code with fake time, DMA or a fixed interleaving.
  Run affected checks once at the final head. Builds on x86-64 and ARM64
  must introduce no warnings. DEV-30 reserves the full suite run for the end
  of the program.
- If the deterministic checks leave timing, hardware or publication ambiguity,
  state the prediction and evidence to collect in the PR. Use the standard
  ARM64 QEMU boot script once at the final head, covering only reachable cases.
  The implementation lane does no x86 QEMU, R240 access, metal-run or flash.
  BCM5720 live evidence belongs to the coordinator.
- Per DEV-31 and DEV-41, record a scoped before/after QEMU bbench series per
  implementation PR. Match QEMU binary, accelerator, VM shape, network
  backend, workload and peer for the stock Linux comparison. Alternate the
  samples and retain median, range, completed/attempt counts, configuration,
  kernel/userland revisions and receipts. Record negotiated features and TCP
  timeout/retransmit deltas. Record deferred/failed rows with reasons. If the
  host stays busy for 30 minutes, record measured load and proceed. Keep any
  controlled loss/RTT or concurrent attribution cases distinct from the
  baseline row IDs. The saved QEMU baseline presents legacy MMIO (device
  version 1 in the saved serial). PR 2 measures both guests on that same
  profile and records the transport version and enabled features. Any
  additional modern-MMIO attribution uses a separate matched profile, marking
  a base that cannot attach as failed with its reason. Report a flat or worse
  row alongside the observed counters. These series measure performance under
  DEV-41 and complement the deterministic behavior checks.
- DEV-39(2) gives builders ownership of fit details. Record a detail
  deviation and one sentence of reason in this plan in the same PR. Escalate
  changes to a data structure, subsystem interface, ABI/on-disk format, lock
  order or phase boundary for a coordinator ruling. Land through other-family
  review.

## Design details resolved against the current tree

- Snapshot the highest transmitted sequence independently of the resend cursor
  because today's RTO rewinds `send_next` to `send_unack` and resends through
  `push_output`, losing the boundary the post-timeout duplicate-ACK guard needs.
- Keep the Phase 0 TX publication lock, TX-submit reap, ISN/port selection and
  existing timeout counter as the starting point because they have already
  landed since the audit.
- Apply the same ring and feature policy on legacy MMIO because the bbench rig
  and standard ARM64 QEMU launcher use the same device profile as the saved
  version-1 baseline.
- Require `VERSION_1` when using modern MMIO because the existing helper
  intersects the requested mask and net currently requests only MAC/STATUS.
- Use the RFC 6298 one-second computed RTO floor in PR 1 because the existing
  200 ms floor predates this standards-based recovery design.
- Opt out at SSH transport setup because both programs start the handshake on
  the connected fd before they know which channels the transport will carry.
- Use RFC 9438 and RFC 9937 for implementation details where they supersede
  RFC 8312 and RFC 6937 because those newer texts specify the current CUBIC and
  PRR behavior.
- Describe `COAL_NOW` as a forced send-consumer update because the guide makes
  the resulting interrupt conditional on the interrupt-avoidance state.
