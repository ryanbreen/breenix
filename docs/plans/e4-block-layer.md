# E4 queued block layer

DEV-41(2) and DEV-29 adopt R8, the full R2 request lifecycle, and R27 from the
2026-11-27 toy-OS audit. The first implementation PR increases virtio-blk
queue depth and adds scatter-gather on the ARM64 QEMU disk. It can land before
E2 or E3. The design below was checked against Breenix at `b22a066f`.

## Starting point

- `BlockDevice` takes borrowed slices and returns completed I/O. `BufferCache`
  admits work by the identity of that wrapper, so two partitions of one disk can
  have distinct cache tickets and still contend for the same device. A cache
  batch drains synchronously. See `kernel/src/block/{mod,partition,cache}.rs`.
- The driver bottleneck sits below that. AHCI uses slot 0 and holds
  `PORT_IO_OWNER` across issue, wait and cleanup. NVMe has an I/O SQ/CQ pair but
  one request gate, pending CID and DMA data page. PCI virtio-blk has one shared
  header, payload and status buffer. ARM64 MMIO virtio-blk spends a ring of up to
  16 descriptors on one outstanding chain at a time. See
  `kernel/src/drivers/{ahci/mod,nvme/mod,virtio/block,virtio/block_mmio}.rs`.
- [Merged #3235](https://github.com/ryanbreen/breenix/pull/3235) establishes the
  non-queued AHCI baseline. It preserves ISR and task-file errors, requires
  completion evidence including the exact PRDBC for a data command, and can
  retire a lost interrupt if the registers record clean completion. For failed
  commands it holds DMA until engine stop and quiescence, runs COMRESET as
  needed, defers recovery out of a masked context, and offlines a port that
  cannot be stopped or reset. Its final production diff is AHCI-only.
- [E2.4](e2-sleepable-contexts.md#6-block-io-one-caller-rule-dev-24) owns the
  caller change. It removes `CannotPark` and its EAGAIN mapping, supplies the
  sleepable I/O context and exclusive early-boot token, and requires a working
  completion source before first dispatch. E4 changes ownership and dispatch
  beneath that contract. E2.5 removes device waits under BreenFS's mount lock.
- BreenFS still sends cold file reads through a block-at-a-time path. In
  `kernel/src/fs/breenfs/commit.rs`, `write_cached` stages native-sector cache
  entries and writes both 4 KiB superblock slots sector by sector between
  durability points. The audit's older code observations are in §2–4 of the
  synthesis and in the block I/O and BreenFS lane reports (block §§1–6,
  BreenFS F1/F2, S5 and L3).

## Request and device ownership

Provide a fallible asynchronous submit operation for a read, write or flush.
A data request covers a contiguous device range and takes an ordered vector of
owned frame leases with byte offsets and lengths. A flush seals a set of
preceding writes. A write may request FUA (Force Unit Access) when supported.
Submission returns a handle for exactly one terminal result. A full queue
returns backpressure with the unsubmitted work still owned by the caller, or a
sleepable wrapper waits for credits and retries. Define one completion for the
logical request even when it has several hardware children. Success means that
each child completed successfully for its whole range. A failed read supplies
no valid result to cache or filesystem clients. A partially issued write can
have reached the media even when the request fails.

A stable attachment object owns scheduling and the DMA limits. Partition
wrappers check bounds and write authority, translate offsets once, and submit
to their parent attachment. Cache identities can still distinguish views.
Use controller/port identity for AHCI, the virtio device for its rings, and
the NVMe controller for hardware resources shared by its namespaces. Keep NVMe
ranges and durability scoped to their namespace. Attachment generations guard
detach, resize or reprobe. Overlapping ranges and flushes from separate views
of the same medium take part in one ordering policy. Carry the authorization
of each original range through merging and retain existing mount fences.
Serialize overlapping operations when either writes, keep disjoint I/O
eligible for parallel dispatch, and coalesce only adjacent, compatible ranges
already waiting in the same ordering interval and authority domain. Completion
of a range remains separate from durability.

Bound pending and in-flight work by request, descriptor and pinned-byte credits.
Return credits through the completion service. Give latency-sensitive reads a
bounded share of dispatch opportunities and bulk writes a share of their own.
Keep enough resources to drive an admitted barrier or recovery under writeback
pressure. A latency-sensitive read may bypass eligible bulk work within the
same interval. It obeys conflicts and durability dependencies. A fence waits
for the earlier writes it covers across hardware queues before issuing a
device flush. Hold dependent operations until the fence succeeds. Fail the
dependency if any covered write or the flush fails. Submitter ownership and
task wakeup play no role in advancing the device queue. The published
multi-queue paper motivates distributing dispatch across hardware queues when
the device offers them. [Bjørling et al., SYSTOR 2013](https://kernel.dk/systor13-final18.pdf)

Keep synchronous `BlockDevice` wrappers for boot, existing filesystems and
callers that need completed results. A wrapper can use several children at once
and waits for the logical result before returning. Borrowed slices without
transferable DMA ownership use owned staging in those wrappers. Runtime
park/wake goes through E2.4 once it lands, including on platforms where a
kthread drains completions in lieu of interrupts. The early-boot token keeps
one in-flight command and bounded polling. Runtime atomic callers defer work
to a worker under E2.4. In the first virtio PR, retain the existing wait
contract and its bounded bootstrap wait. The runtime wait can still consume
a late healthy completion, as required by #3235's virtio review.

### DMA lifetime

Build and map vectors outside the queue lock. Follow the existing frame owner
when taking an exclusive read destination lease or a stable write-source lease.
Merge physically adjacent DMA ranges when the encoding allows it and split
commands at the negotiated byte, segment, address, boundary and alignment
limits. Use checked physical-to-bus mapping, with architecture-specific DMA
synchronization for the touched buffers, descriptor memory and direction.
Publish ownership and descriptors before the doorbell or available index.
Acquire device-written state before finalizing a read. The IRQ path captures
and acknowledges bounded completion state. The completion service performs
cache synchronization, callbacks, wakeups and destruction outside the
transport lock, with request ownership already installed when the device can
first see its command.

A request moves from queued to issued to retired to completed. An error can
end a caller's wait while its issued request is still with the device. After
issue, the queue retains the DMA mapping, frame pins, bounce buffers and
per-command descriptors until validated completion or a confirmed stop or reset of
the DMA engine. A signal may detach a passive waiter when another owner is
responsible for completing the operation and cleaning up. Detaching leaves
the request's result and the queue's obligations intact. A reset
puts outstanding requests in a retirement set. A software generation protects
against deferred work from an old queue. DMA retirement requires hardware
evidence.
If the stop cannot be confirmed, take the device offline and keep the affected
memory quarantined. The memory bound comes from the credits held at issue.

**Design detail for E4.4 synchronous waits:** Keep `KillCustody` and E2's
uninterruptible waits for commit and cache I/O because a signal returned
as an I/O error can latch a healthy BreenFS mount read-only.
The commit or cache operation acquires custody before it assumes work that
must be finished. Keep custody across admission, completion and cleanup,
including between requests, until the generation is installed or abandoned,
or the cache result is finalized and cache ownership released. `SIGKILL`
stays pending throughout. Device failures and recovery deadlines still
follow their normal error paths.
See the [MMIO request gate](../../kernel/src/drivers/virtio/block_mmio.rs#L41),
its [completion wait](../../kernel/src/drivers/virtio/block_mmio.rs#L410), and
the [BreenFS publication error path](../../kernel/src/fs/breenfs/vfs.rs#L2972).

Use a bounded, device-owned bounce pool only for ranges the device cannot
address or encode directly after legitimate splits, such as on a 32-bit AHCI
HBA without a suitable DMA mapping. Copy write data into bounce memory before
issue and copy read data out only on verified success. Direct DMA from owned
frames is the common path. E4.1 can use owned kernel frames allocated and held
by the driver today and split them into separately mapped segments. E3's cache
leases and E1's frame-vector interface plug into the same ownership boundary
when they land. DMA to general user pointers is outside E4.

### Attach and sector geometry

Keep protocol address units, bytes and filesystem blocks explicit. Verify
geometry and arithmetic before exposing a device or partition. ATA IDENTIFY
supplies LBA48 capacity and logical sector length (including validity of word
106 and words 117–118); physical geometry and alignment are separate hints.
NVMe uses NSZE and the active LBA format from Identify Namespace. Reject
metadata or protection-information formats that the data path cannot handle.
Virtio capacity and request sector numbers use 512-byte wire units even with a
negotiated `blk_size` or topology hint. Validate the advertised hints and keep
them separate from protocol addressing. Read virtio config consistently with
its generation counter where available. Reject invalid geometry, overflow,
misaligned requests and ranges beyond the last LBA. Verify post-reset geometry
and identity before reopening admission. [ATA/ATAPI Command Set 3](https://www.iso.org/standard/83120.html),
[NVMe NVM Command Set 1.0 §4.1](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-2021.06.02-Ratified.pdf),
[virtio 1.2 §2.5 and §5.2.4–6](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html)

The 4 KiB BreenFS block can use eight 512-byte logical sectors or one native
4 KiB logical sector. Expose 4Kn only when the backend, GPT/partition translation,
write-range policy and the target consumer agree on that size. Refuse
unsupported paths at attach or mount so legacy 512-byte consumers cannot
misaddress a disk. Validate that the writable BreenFS path can issue a whole
slot and provide its durability operation before admitting it to the new
publisher.

## Backend dispatch and recovery

### virtio-blk on MMIO and PCI

Start with many chains in queue 0. Size the split ring and per-chain storage
from the offered queue length, transport requirements and a memory budget.
Each chain owns a readable header, data segments in the appropriate direction,
and a writable status byte. Use the negotiated `SIZE_MAX` and `SEG_MAX` with the
ring's descriptor budget. Indirect descriptors and additional request rings are
available only with the corresponding negotiated features. Queue 0 is sufficient
for the first PR and the common dispatcher can use more rings when negotiated.
Keep the PCI legacy ring-layout and transport handling in their existing scope.

Track each request by used-chain head in its queue. Install the table entry
before publishing `avail.idx`. Batch a window of child requests from a
multi-block synchronous call and notify after making the window visible.
Drain the wrapping used ring entry by entry, including out-of-order heads.
Require a known outstanding head and valid terminal status. On modern
transports, require used length to cover the exact bytes expected for a successful
read including status, or just status for a write or flush. On the legacy
interface use its documented status/length compatibility rule. Unknown heads,
invalid indices and untrustworthy ownership stop queue reuse until reset.
Keep media errors distinct from broken-ring errors.

Check cache mode and negotiate FLUSH for writeback. A flush drains the
covered writes across the device's request rings before dispatch. Virtio-blk 1.2 uses
explicit flush or a verified writethrough configuration for durability. On
device failure or bootstrap abandonment, freeze dispatch, acknowledge a
negotiated queue reset or a transport device reset, and rebuild the affected
ring state. Keep buffers in custody until reset is confirmed. A late healthy
completion under the runtime wait keeps its original request identity.
[virtio 1.2 §2.4–7, §4.1–2 and §5.2](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html)

### AHCI NCQ and full R2 recovery

- Discover SATA attachments by controller and implemented port with per-port
  DMA tables sized at attach. The first NCQ path covers directly attached
  SATA devices. Negotiate NCQ from host SNCQ/NCS and valid ATA
  IDENTIFY capability and depth. Cap active tags by both limits and the
  allocated resources. Use READ/WRITE FPDMA QUEUED, put the slot tag in the ATA
  command and the transfer count in its queued-command field, and build the
  PRDT from the request's segments. Publish the command header/table and
  active request, then set only the new bit in PxSACT before setting its bit
  in PxCI. Retain the serialized READ/WRITE DMA EXT path for devices without
  NCQ. Drain NCQ before issuing a non-queued command such as FLUSH CACHE EXT,
  IDENTIFY or READ LOG EXT. Encode ATA FUA only on a write opcode and drive
  capability that provide it. [AHCI 1.3.1 §3, §4.2, §5.3 and §6.1](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/serial-ata-ahci-spec-rev1-3-1.pdf),
  [Intel and Seagate NCQ technical paper](https://www.seagate.com/docs/pdf/whitepaper/D2c_tech_paper_intc-stx_sata_ncq.pdf)
- Use the software outstanding mask minus the HBA's PxSACT mask to find
  completion candidates on a running port. The Set Device Bits FIS drives
  PxSACT updates and can retire several tags at once. Collect status with
  the mask snapshot before clearing W1C bits and resample if completion and
  error status could have crossed the snapshot. The IRQ, poller and deadline
  paths use the same rule. PxCI can clear on command acceptance before DMA
  finishes.
  **Design detail relative to #3235:** keep the exact PRDBC check for non-queued
  commands because AHCI §5.4.1 permits an invalid PRDBC for NCQ and disallows
  underflow. The NCQ path uses successful per-tag completion, an exact
  programmed transfer length and absence of command/transport errors.
  [AHCI 1.3.1 §3.3, §5.4.1, §5.5.3 and §6.1](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/serial-ata-ahci-spec-rev1-3-1.pdf)
- **Freeze and capture.** On task-file, host-bus, interface or transfer error,
  deadline without a valid completion receipt, or unsolicited COMINIT or link
  loss, stop admission and capture the outstanding tags, PxSACT, PxCI, PxIS,
  PxSERR, task file, link and relevant received FIS before clearing status or
  stopping the engine. Retain any earlier clean receipts. An error-bearing
  sample, or a mask cleared by stop/reset, cannot grant success to unresolved
  tags. Handle non-fatal indications using §6.2.1/§6.2.2 evidence. If a transfer
  is suspect, route it through recovery. Port recovery runs in the worker,
  independently of the submitting task.
- **Stop and diagnose.** Clear ST, wait boundedly for CR to clear, preserve and
  then acknowledge captured errors, and check CI/SACT and the task file. With
  an idle device and no link/device reset, restart under exclusive recovery
  ownership and issue ATA READ LOG EXT page `10h` on a separate recovery
  buffer using the verified non-queued path. Validate the log and its queued
  tag against the captured set before attributing a failure. Commands still
  unresolved are failed, with an unknown result if evidence is incomplete.
  An explicitly retryable read may be reissued with a new tag after recovery
  and identity revalidation. Report uncertain writes, flushes and FUA
  commands to the caller. A failed generation cannot be committed by silent
  replay.
- **Reset or isolate.** Use COMRESET for BSY/DRQ, a lost or reinitializing
  link, or when port recovery cannot establish readiness. A reset loses the
  old ATA error-log attribution. Before release require the #3235 quiescence
  conditions for command DMA. If reclaiming or replacing received-FIS
  memory, stop FIS reception and wait for FR to clear. A host-bus fault or
  engine that still will not stop can require a bounded controller reset
  (GHC.HR) under exclusive controller recovery. Freeze its ports and account
  for their outstanding work before resetting, establish stop for each
  affected DMA domain, and rebuild controller and port state. Re-identify the
  device, revalidate capabilities and geometry, and reopen under a new queue
  generation. A changed device goes through fresh attachment. If the stop or
  reset ends without confirmed quiescence, take the affected queues offline
  and retain their DMA memory. [AHCI 1.3.1 §5.3.2, §6.2.1–2 and §10.4](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/serial-ata-ahci-spec-rev1-3-1.pdf)

### NVMe 2.0 over PCIe

Keep the admin queue independent. Negotiate the number of I/O queues, create
CQs before their SQs, and use the returned queue and interrupt resources.
Shard dispatch across available pairs, starting with one pair that has several
commands in flight, bounded by CAP.MQES, queue credits and the chosen memory
budget. A completion is identified by its CQ phase and SQID/CID. Advance CQ
head after consuming entries. SQ head controls space for entries and can move
before those requests retire. Keep CIDs and mappings until validated CQ
completion. Use PRP1, PRP2 and request-owned PRP lists with the controller's
page alignment and MDTS. Use SGLs when supported and needed for eligible
shapes. Split or bounce an unrepresentable vector according to the DMA rules.
Respect the namespace format and its command transfer limits. Use namespace
Flush after the covered writes complete and FUA on the relevant Write.
FUA orders durability of that write only.

On timeout, first drain the CQ. An Abort command is a request to the controller.
Wait for the original I/O's terminal completion or confirm retirement through
queue deletion or controller reset. On fatal status, freeze dispatch, disable
the controller and wait boundedly for CSTS.RDY=0 before retiring the reset
queues under the PCIe reset contract. Recreate queues and revalidate namespaces
on re-enable. Unknown writes fail without replay. If shutdown cannot establish
safe DMA retirement, keep the device offline and quarantine the affected memory.
[NVMe Base 2.0 §3–5 and §7.1](https://nvmexpress.org/wp-content/uploads/NVM-Express-Base-Specification-2_0-2021.06.02-Ratified-5.pdf),
[NVM Command Set 1.0 §3.2.6](https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-2021.06.02-Ratified.pdf),
[NVMe over PCIe Transport 1.0](https://nvmexpress.org/wp-content/uploads/NVMe-over-PCIe-Transport-Specification-1_0-2021.06.02-Ratified-1.pdf)

## BreenFS consumers

**Reads.** E3 obtains a stable extent and checksum view, marks destination
frames as filling, and submits a bounded window of physically contiguous
runs across extent boundaries. Each request can span discontiguous memory.
Prefer demand work over speculative readahead within the queue's credit
policy. Completion gives E3 the filled frames. E3 verifies each block's CRC
and revalidates the inode/range version before making a page readable.
Truncate or replacement can invalidate a fill while the device still owns
its DMA lease. A failed, stale or short fill supplies no cache-valid data.
Holes are zero-filled only when the extent lookup identified a hole. E2.5 supplies
the stable-view/mount-lock handoff. Readahead and fault-around use E2.9 when
driven from a restartable fault. The measured benefit to eager mmap belongs
to the extent/fill path. Turning mmap lazy is E3 work.

**Writes.** Copy once from user memory into the E3 dirty frame. On freeze,
retain immutable frame leases for generation G and compute its checksums
from those bytes. Later `write()` calls to the same logical page get a
writable successor version. E4.8 retains a private snapshot copy for a page
with a writable `MAP_SHARED` binding. Take the copy before computing the CRC
and use that same request-owned frame for DMA. The live page stays writable
and eligible for later writeback. Installing G cannot clear its dirty state
while a writable binding can still store to it. Use a cache frame directly
only if its E3 lease excludes writable aliases for the lifetime of the lease,
including new bindings and permission upgrades. Otherwise take the same
snapshot copy.

**Design detail relative to direct writeback:** E4.8 keeps the mapped-page copy
used by `FileMap::writeback_snapshot` and BreenFS `writeback_pages` because
stores through existing writable PTEs bypass `write()`'s successor path.
See [file-map writeback](../../kernel/src/memory/file_map.rs#L875) and
[BreenFS writeback](../../kernel/src/fs/breenfs/vfs.rs#L4324).

Use the existing frozen generation, reservation and G+2 reuse rules. Prepare
disjoint physical runs with data-frame vectors and encoded metadata buffers.
Submit a bounded window across runs, bypassing native-sector cache staging,
the contiguous scratch copy and driver payload bounce on directly mappable
devices. Release mount exclusion before queue admission or waits. Coordinate
metadata cache publication and invalidation with the existing freeze/install
handoff so a late fill cannot reintroduce old bytes. The writeback worker
replenishes the window as children complete and keeps generation-level error
and durability accounting. E3 owns dirty limits and throttling. The batching
principle is described by [Rosenblum and Ousterhout, 1992](https://www2.eecs.berkeley.edu/Pubs/TechRpts/1992/6267.html).
The COW snapshot model follows [Rodeh, 2008](https://research.ibm.com/publications/b-trees-shadowing-and-clones).

**Publication.** For G, wait for the batch's data and metadata completions, then
complete the data flush before issuing either superblock ring slot. Submit
one 4 KiB request for the selected slot in each ring, preserving both copies.
With FUA, each slot's own completion establishes its durability. Otherwise
issue one ordinary write per slot, wait for both, then issue one flush for
the pair. Keep both durability points. Install G and wake its fsync waiters
only after both slots are durable. A slot can still tear on power loss.
Existing checksums and recovery validate slots and roots. If data or the first flush fails, submit no slot.
If a slot or the final flush fails, fail G and its waiters, keep G+1 from
publishing, and follow the existing read-only error path. Recovery may find a
valid G if a slot reached persistent media before the error. Acknowledgment
and recovery selection have separate evidence.

[The writer rules](../fs/breenfs-format.md#3-invariants) currently spell
W3 as an explicit trailing flush. Keep that path and its existing check.
The implementation that enables FUA also documents the equivalent W3
durability condition for that path and extends the recording device to
distinguish a completed FUA write from an ordinary write. W1, W2 and the both-rings fsync
rule still bind. Exercise the existing crash-image harness with the alternate
durability condition and keep its existing recovery expectations.

## Implementation PRs

The first three PRs can land with the present completion and frame allocator.
E4.4 adopts E2.4. E4.5 and E4.6 can proceed after E4.4 without waiting for E3.
The ARM64 consumers in E4.7 and E4.8 can proceed as soon as their E3 pieces
and E4.4 are ready, independently of the AHCI and NVMe implementation PRs.
If E2.4 lands first, the early virtio work uses its wait API. The request,
slot and DMA ownership rules are the same.

### E4.1 Virtio-blk queue depth and scatter-gather

First, can land now. MMIO and PCI, multiple chains in queue 0, multi-segment
payloads from individually owned kernel frames, bounded windows inside present
synchronous multi-block calls. Preserve runtime late completion, boot polling,
same-range exclusion and flush ordering while replacing the single-request
gate. E2/E3 unnecessary.

- **Check-first.** Drive the real split-ring submit/drain code with a small
  fake device. Publish several chains over fragmented frames, complete out of
  order across index wrap, and verify exact data, descriptor credits and
  per-head status. Force boot abandonment with an outstanding chain, then
  deliver a stale event while reset is unacknowledged. Its memory remains owned
  and no new request completes.
- **QEMU rows.** `disk.sequential-read`, `disk.sequential-write` where callers
  already issue multi-block I/O. A depth-one file caller will still feed one
  request. No direct target for one-append fsync or metadata rows.
- **Details resolved in E4.1 (DEV-39(2)).**
  - Both drivers share one engine, `kernel/src/drivers/virtio/blk_queue.rs`,
    which the host tests compile unchanged, because one ring implementation
    can be checked once against a fake device for both transports.
  - Each chain slot owns up to 16 individually allocated frames (64 KiB, the
    block cache's `RUN_BYTES`), and a device holds at most 512 KiB of them. On
    a 256-entry ring that is 8 chains, because one coalesced cache run then
    stays one request and the frame cost per disk stays bounded. A ring too
    small for a 16-frame chain gets the longest chain that fits
    (`Limits::chain_plan`: one 14-frame chain on 16 entries), because the
    single-chain driver before E4.1 ran such a disk with 3 descriptors.
  - A synchronous call is admitted for its whole sector range until its last
    chain returns, and a flush is admitted for the whole device. This is the
    same-range exclusion and flush ordering E4.1 must preserve, and E4.4
    replaces it with its conflict and fence policy.
  - A flush that has to wait holds its turn in the range table, and later
    requests wait behind it, because otherwise requests on other sectors can
    keep a flush out indefinitely. A caller that cannot sleep is admitted
    past a waiting flush, because it makes one attempt and refusing it would
    fail a disjoint boot-time read (DEV-24).
  - A submitted chain is staged until `publish` stores `avail.idx` over it,
    and only issued chains are completed, because section 2.7.8 lets the
    device return only chains it was given. A stopped queue publishes no
    staged chain, because that chain's caller has already had its error.
  - The PCI driver marks a write pending when the write is admitted, before
    it builds a chain, because another caller's publish can make the write
    visible and the writer's own publish result does not show it.
  - A wait that cannot sleep keeps a window of one chain, because the boot
    thread's bounded polling is the existing contract.
  - An abandoned wait, or a used ring that names a head the driver did not
    issue, stops the queue for good: each chain on the device keeps its
    memory and no slot is reused. E4.1 does not reset the device, because
    reset, rebuild and reopening admission are E4.4's recovery work.
  - `seg_max` bounds the data descriptors and excludes the header and status
    descriptors, because section 5.2.4 counts segments of the request's data.
  - The MMIO driver now negotiates `VIRTIO_F_VERSION_1` on version 2
    transports (section 6.1 requires accepting it), and `SIZE_MAX` and
    `SEG_MAX` on both versions. Used lengths are checked exactly only with
    `VERSION_1`, because section 5.2.6.3 tells legacy drivers to ignore them.
  - The MMIO interrupt handler only tries the queue lock and leaves a drain
    request for the holder, because the MMIO handler took no lock before.
    The PCI handler keeps its existing spin on the queue lock, which
    thread-side holders take with interrupts masked (`with_engine`). The
    MMIO queue report drains after its sample too, because it is also a
    holder an interrupt can find.
  - `reserve` takes the owner's completion token as the slot's tag, `drain`
    reports it and `finish` clears it, because a token the driver cleared
    outside the queue lock could wipe the next owner's token and leave that
    owner waiting on a chain already drained (seen in the rig's compile rows).
  - The first host test is red on the old code as a compile failure,
    because the old drivers had no multi-chain engine for it to drive, and
    so is the chain-plan check, because the old attach sizing was inline.
    Driver interleavings (the late token clear, the queue report, the PCI
    write flag, the flush turn) are checked by compiling the production
    driver functions against a fake device
    (`tests/virtio_blk_mmio_driver_test.py`,
    `tests/virtio_blk_pci_driver_test.py`), because only the driver code
    carries those races.

### E4.2 Attach geometry and limits

Can land now, separate from the queue API. Parse validated geometry and
transport limits, pass supported sizes through device/partition adapters, and
refuse units an existing consumer cannot handle. Enable a 4Kn path only with
checked native-LBA translation end to end.

- **Check-first.** Feed valid 512e and 4Kn fixtures and malformed sizes through
  actual attach and partition translation. Check an exact end-of-partition I/O,
  a one-unit overrun and checked conversion overflow. Virtio still encodes the
  correct 512-byte wire sector with a 4 KiB hint.
- **QEMU rows.** Correctness PR. No QEMU speedup expected on the current
  geometry. Use `disk.sequential-read` and `disk.sequential-write` as the
  affected-path reference.
- **Details resolved in E4.2 (DEV-39(2)).**
  - AHCI rejects a drive without valid LBA48 support because its read and
    write commands use DMA EXT, and treats drives without FLUSH CACHE EXT
    as read-only because that is its durability command.
  - AHCI compares the IDENTIFY serial, model and geometry before reopening
    after COMRESET because the existing port can resume with the same
    software identity even if the medium changed.
  - NVMe keeps its one-page transfer limit because the supported controllers
    have a 4 KiB minimum page and even the smallest nonzero MDTS exceeds one
    page.
  - GPT reads entries in the disk's validated logical-sector size and passes
    those same LBAs to the partition and write fence because the GPT range
    and the device command address the same units.
  - Native 4 KiB SATA and NVMe sectors can carry a full BreenFS block through
    the existing I/O and flush path. Ext2 and the install record keep their
    512-byte requirement because they still use fixed sector offsets.
  - Virtio reads two matching configuration samples on legacy transports
    because they lack a generation counter, and retains 512-byte capacity
    and command units even with a larger block or topology hint.
  - The protocol host fixture compiles the attach parsers and the existing
    request encoder because it can feed malformed values and force a
    configuration generation change without hardware.

### E4.3 Whole BreenFS superblock slots

Can land after E4.1 and E4.2. Keep the synchronous publication path and both
explicit flushes. Use a whole-slot write that keeps cache coherence and issues
one hardware command for supported backends. Do not enable it on an adapter
that still loops over individual sectors. No E2/E3 prerequisite.

- **Check-first.** Run the actual publication path against the recording
  device. Expect data flush, one full write per ring, one final flush. Fail a
  slot or the final flush and verify the fsync waiter fails and the prior
  durable generation is recoverable. Keep the between-rings crash case.
- **QEMU rows.** `disk.fsync` and `disk.sequential-write` through fewer
  commands per publication.
- **Details resolved in E4.3 (DEV-39(2)).**
  - The publisher uses the existing cache write-through range operation when
    the adapter's single-command limit covers a slot because `write_blocks`
    also accepts devices that split a multi-sector request.
  - The NVMe adapter splits writes at the existing one-page limit because its
    synchronous controller already accepts consecutive native LBAs in a
    single command backed by that page.
  - The recording device still saves sector events for a whole-slot command
    because power loss can persist only part of the slot.

### E4.4 Physical-device async queue

Needs E2.4 and E4.1–2, independent of E3. Expose owned-vector submit and sync
wrappers on virtio. Move device admission below partition translation, with
capacity credits, conflict ordering, cross-ring flush fences and completion
independent of the caller. Keep existing cache-coherency guards where still
needed.

- **Check-first.** A fake transport stalls an issued write while aliases of the
  same device submit an overlapping read, a disjoint read and a flush-dependent
  write. Fix the interleaving, then deliver the earlier completions in reverse
  order. Only eligible work advances and both aliases honor bounds and the
  fence.
- **Inline-commit check-first.** Drive the production synchronous wrapper
  through BreenFS's pressure-triggered inline commit and cache batch. Hold a
  data write after issue, post `SIGKILL` to the committing task, and confirm
  the batch stays pending. Complete the write successfully and let the
  durability operations finish. Verify G installs, cache ownership is
  released, the mount remains writable, and the pending kill is delivered
  after the commit has released custody.

  The host model steps through the protected block wait before and after
  the device posts its result.

  ```pycon
  >>> def wait_for_commit_data(events):
  ...     for event in events:
  ...         if event == 'SIGKILL':
  ...             continue
  ...         return event
  ...     return 'pending'
  >>> wait_for_commit_data(['SIGKILL'])
  'pending'
  >>> wait_for_commit_data(['SIGKILL', 'device success'])
  'device success'

  ```
- **QEMU rows.** `disk.sequential-read` and `disk.sequential-write` through
  available run overlap, with larger gains gated on E4.7–8. `disk.fsync` tests
  fence overhead and must retain durability.

### E4.5 AHCI NCQ and recovery

Needs E4.4 and merged #3235. Replaces the slot-0 owner for runtime NCQ, keeps
verified depth-one fallback, and implements the recovery sequence above before
enabling more than one tag. E3 unnecessary.

- **Check-first.** Extend the existing AHCI register/FIS fixture through the
  production issue and retire logic. Cover clean out-of-order tags including
  lost notification, then a queued error with one previously retired tag, one
  failed tag from log page `10h` and unresolved peers. Force stuck CR/CI/SACT
  after reset and require offline quarantine without tag or DMA reuse.
- **QEMU rows.** No affected row. The rig uses virtio-blk. AHCI affects these
  rows on the R240 only at the coordinator's checkpoint.

### E4.6 NVMe queue pairs and vectored DMA

Needs E4.4. Negotiate I/O pairs, enable per-command CID and PRP/SGL ownership,
and wire the Abort/reset retirement path and FUA. E3 unnecessary.

- **Check-first.** Drive SQ/CQ register and DMA fixtures through the production
  ring code with noncontiguous frames. Complete the same CID on two pairs, wrap
  phase and advance SQ head early. An Abort completion or stuck RDY during
  reset cannot return the original request's DMA credit.
- **QEMU rows.** No affected row. The rig's root disk uses virtio-blk.

### E4.7 E3 extent-run fill and readahead

Needs E4.4, E2.5, the Phase 1.1 extent-run work, and E3 filling-page leases and
version validation. Fault-driven integration also needs E2.9 and E3's fault
path.

- **Check-first.** Use the production fill path with adjacent and separated
  extents and fixed completion order. Truncate or replace the requested range
  before completion. Verify that a delayed or corrupt child leaves stale or
  unchecked bytes unpublished.
- **QEMU rows.** `disk.sequential-read` when data needs a fill. `disk.stat` and
  `disk.walk` are secondary observations, since their cost mostly depends on
  metadata and caching.

### E4.8 E3 generation writeback

Needs E4.3–4, E2.5 and E3 dirty/frozen frame ownership. Enable writeback on
virtio first. Native FUA becomes usable as each backend supplies its verified
FUA write path.

- **Check-first.** Drive actual copy-in, generation freeze and publication with
  a recording backend. A rewrite during the frozen write leaves G's bytes
  stable. Fail a batch child, then model power loss at the two publication
  points on flush and FUA paths. Require W1–W3, both durable slots before fsync
  success, and unchanged recovery selection.
- **Shared-mapping check-first.** Pause the recording backend after G's CRC
  is computed and before it reads the submitted frame. Store through an
  already writable `MAP_SHARED` binding to that page, then complete G. Verify
  G's persisted block and stored CRC through a cold read of the image, and
  verify a subsequent commit persists the store from the live mapping.

  The host model compares the DMA source with the bytes used for the CRC.
  Run it with `python3 -m doctest docs/plans/e4-block-layer.md`.

  ```pycon
  >>> live_frame = bytearray(b'before')
  >>> frozen_source = bytes(live_frame)
  >>> checksummed = bytes(frozen_source)
  >>> live_frame[:] = b'after!'
  >>> (bytes(frozen_source) == checksummed, bytes(live_frame))
  (True, b'after!')

  ```
- **QEMU rows.** `disk.sequential-write` and `disk.fsync` on virtio's
  write-plus-flush path. `disk.create` and `disk.unlink` can move where they
  wait on generation writeback. `disk.sequential-read` may gain from E3
  residency and is attributed separately from DMA.

## Evidence and landing

- Use the smallest host fixture or fixed-interleaving kernel check that
  reaches the production path. Commit the check first, show it red against
  old behavior, then run the focused check once at the final implementation
  head. Preserve existing checks and their expectations. Require implementation
  builds on x86-64 and ARM64 to emit no new warnings.
- DEV-30 allows a targeted ARM64 QEMU boot only for an effect the
  deterministic check cannot settle, such as real MMIO notification/DMA
  visibility or boot-device readiness. State the expected observation and
  reason in the PR, use the repository's standard boot script, and exercise
  only the reached cases once. The full effort-suite run belongs to the
  end of the foundation program. R240 delivery and hardware evidence belong
  to the coordinator. A virtio result cannot validate SATA or NVMe.
- Record the DEV-31 before/after bbench series for each implementation PR on
  the [standard rig](../../userland/bbench/RIG.md) and record even when the
  changed backend is absent. Keep VM shape and cache mode matched, alternate
  Linux and Breenix, and report medians, ranges, completed/attempted samples,
  commit IDs and filesystem labels. Under DEV-41(3), record host load if
  the rig cannot find a quiet window in 30 minutes. Use the DEV-27 parity
  band of 10% of Linux's median and rank wins by seconds saved. Attribute
  shifts with request bytes, concurrent depth, queue wait, command count,
  flush count and copy bytes. The sequential read row follows a write and
  fsync, so its cache effects and cold media timings differ.
- Reference series `qemu-20261127-28668b28-{breenix,linux}.series.json` has
  five completed samples per row. BreenFS write plus fsync took 10.378 s
  versus ext4's 0.371 s (28.0×). The following read took 3.724 s versus
  0.0416 s (89.5×). The same series measured create at 32.9×, stat at
  81.1×, unlink at 49.1×, walk at 6.1× and append fsync at 8.3×.
- DEV-39(2) lets builders resolve bit settings, fixture values, helper
  signatures and test shapes from the specifications and Breenix code.
  Record the deviation and its reason in this plan in the same PR. Stop
  for a coordinator decision when evidence requires a different data
  structure, subsystem interface, on-disk or ABI format, lock order or phase
  boundary.
- The evidence for this design is Breenix code, the merged retirement,
  existing receipts, specifications and published papers. Linux and ext4
  appear as black-box measurements. This design PR requires documentation
  checks only and lands after other-family review.
