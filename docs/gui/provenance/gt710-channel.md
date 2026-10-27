# GK208B privileged FIFO channel (E22 M6 PR 6.1)

This step implements the plan's first privileged A06F channel. It enumerates
64 hardware TOP words and the implemented PBDMAs, selects a real copy-engine
runlist, constructs private RAMFC/USERD/GPFIFO storage and schedules one
standalone channel. It submits host semaphore releases and SET_REFERENCE,
then compares the payload and resident PBDMA IB_GET against GPPut in the
native epoch. Live REF_CNT, CHID and the USERD control address tie progress
to the owned channel; a saved USERD GPGet is not the resident cursor.
A host semaphore is PFIFO evidence; it is not an engine-ordered CE fence.

The boot-CPU bootstrap runs before native console and client publication. FIFO interrupt
delivery stays masked during this bounded polling path. This step installs
no IRQ handler. It emits failure diagnostics from the boot CPU. Subsequent
interrupt-driven submission must acknowledge/count/wake in its top half and
decode/recover in a worker.

The existing native BAR1 view gains one owned USERD page at its upper end,
with an extent check against the console. The CPU uses the existing WC alias.
Publication drains writes before GPPut. VRAM writes target owned
storage: a bound firmware channel or an occupied USERD aperture slot refuses
admission. Channel backing and tables remain pinned by the M5 residency owner
on uncertain hardware results. Successful reclamation requires stopped
scheduling, completed empty-runlist publication, preemption, PBDMA idle and
an unbound channel before the context retires.

The capture gate requires distinct semaphore payloads, native binding and
GET/PUT progression. Ordered host releases write a two-word epoch and the
payload into the completion record. The production waiter reads that record
for clean work and negative controls. A stale control plants the preceding
epoch with matching payload, GET/PUT and reference, then requires that same
waiter to expire. A withheld control prepares the ring entry without publishing
GPPut. A second control executes a reference update with its semaphore
release suppressed, requires advancing GET/PUT without success, then requires
a clean next semaphore completion after disarming. Polling is bounded by the milestone's two-second watchdog.
This step does not claim the reset budget or killable process waits.

CE full compares under complementary poisons, rectangle/overlap operations,
system-memory reads, same-VA channel isolation, remap invalidation and actual
unmapped-page faults remain PR 6.2 and issue #2514. Reusable completion is
6.3, multi-context/killable lifetime work is 6.4 and confirmed reset is 6.5.
Service power cycling does not establish in-guest reset or drain.

## Sources

The file-local MIT headers were checked before use. THIRD_PARTY_NOTICES maps
the pinned sources to consumers. The source register in the plan remains
the selection authority; the additional TOP parser and FIFO initialization
sources were individually checked under G1. The following hashes identify
the exact source bytes inspected for this step.

| Source | SHA-256 |
|---|---|
| `gk104.c` | `d9ce2a7495358c75e5cefae2ceae099d3e161ee960cba2415ba24b5c08374dae` |
| `gk208.c` | `bcefcc8f6fc1afdb4749b18d22e88d7937ab31da58dc3b96eb1614dc032317d7` |
| `gk110.c` | `581d835a89d4be2a4f1a4383efb56dc02b81f66d055020f744654968b724a9cf` |
| `gf100.c` | `049a0ff8c473d28c40b97f6482c47f974338f3df2660e8bb2dd70bfb29e49371` |
| `top-gk104.c` | `834dcf42fee7cb42442d5a60315f4bf8ab401add3ea2e0845af5b34566843b99` |
| `mc-gk104.c` | `15dd9fd27d86890a2d5cb9278903a3f95267b3d2743c626e59f6351c95ea8d7a` |
| `fifo-base.c` | `d97c42d94685b974e0bd1b3c572623fb858242a324c38c866730d7a1f700c3a3` |
| `cla06f.h` | `6c5db0562c117a76f145acbe35a6bacafe612493424078439d1a83c627fbc0b8` |
| `gf100_pfifo.xml` | `7fa832b0ab438b301370a69deadf48a8f8c350ae697011a20e82b20b10cbf065` |
| `copyright.xml` | `689484f97d5c003a6c3bb84ee09ab4b4f79266250b9b44dc3edaf859930db7a6` |
