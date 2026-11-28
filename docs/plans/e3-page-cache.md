# E3: unified page cache and pager (design and PR plan)

Status: design and PR sequence. No E3 code has landed. Builders append the detail deviations they make
under DEV-39(2) to section 22.

Rulings. DEV-41(2) starts this design and orders implementation to begin as soon as its E1 and E2
prerequisites land (E1-9 and E2.9). It also asks the design to say which E3 PRs need neither. DEV-29 adopted
the toy-OS audit (`~/Downloads/breenix-toy-os-audit-2026-11-27.html`). E3 closes its findings R5, R6, R11 and
R14 (reclaim), and the permanent half of R1. It also addresses themes T2, T3 and T8, and the file-data part of
T5. DEV-29 Q4 rules out swap for now but asks that reclaim leave room for it, and Q5 forbids fixed CPU and RAM
ceilings. DEV-30 sets the testing rule. DEV-31 makes ARM64 QEMU the performance rig. DEV-39(2) lets builders
resolve design details themselves. The memory, VFS and BreenFS lane findings
(`~/Library/Caches/fastr-rollout/2026-11-27/toy-os-audit/{memory,vfs,breenfs}-findings.md`) are the
evidence base for section 1, together with the E1 and E2 designs (`docs/plans/e1-kernel-memory.md`,
`docs/plans/e2-sleepable-contexts.md`), on which E3 builds.

Clean room. The designs below come from textbooks, published papers, POSIX, the System V ABI, hardware
manuals and the Linux man pages (the last for ABI behaviour only). Section 17 lists them. The only code read
for this document is Breenix's own. No other operating system's or C library's source was read, fetched or
quoted.

## Contents

1. Where we are
2. Goals, non-goals and invariants kept
3. Layering
4. The cache object and its index
5. Page states
6. The filesystem interface: fill and writeback
7. read and write through the cache
8. Readahead
9. mmap: lazy mappings, faults and fault-around
10. exec: shared text, private data, demand-zero BSS <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
11. Dirty pages, writeback and BreenFS commits
12. Reclaim
13. The OOM policy
14. The block cache keeps metadata only
15. Locks and lock order
16. Accounting, census and ceilings
17. Sources
18. PR sequence
19. Tests
20. bbench rows (DEV-31)
21. Risks and questions for the coordinator
22. Deviations recorded by builders (DEV-39(2))

## 1. Where we are

The line numbers are from main at b22a066f, read for this document. Since the audit's 1bcb09f4, these
changes have landed:

- #3232: population claims that pin a population's pages against eviction until its binding is published;
- #3239 (Phase 1.1): an in-core extent map, with one device request per contiguous run of up to 64 KiB;
- #3245 (Phase 1.7): metadata cached across commits;
- #3253 (Phase 1.2): in-memory free space;
- E1-1 and E1-2: the range set and the ARM64 direct map;
- E2.1: the event queue.

The structure the audit describes is unchanged.

### 1.1 File data has three homes

- **The block cache.** Clean file data for `read(2)` lives in the device block cache, keyed by
  (device, block). Its budget is `bound_for_memory`: RAM/8, clamped to 4–8 MiB (`block/cache.rs:131-149`),
  because it is charged to the fixed kernel heap. BreenFS's `DataReader` reads clean runs through it
  (`fs/breenfs/vfs.rs:4756-4778`).
- **The mapping cache.** `MapState` (`memory/file_map.rs:168-233`) holds a per-incarnation sorted vector of
  frames (`PageVec<CachePage>`), a poison set, the bindings (one per file VMA) and population claims. Its
  frames come from the frame allocator and are tagged `FrameOwner::PageCache`, but the vector and its
  per-page records are on the heap.
  - Dropping a binding requests eviction. The filesystem service then retires up to `EVICT_BUDGET` = 64
    clean pages per pass that no binding or claim covers (`file_map.rs:1138-1183`), whether memory is short
    or not.
- **Dirty data.** BreenFS keeps dirty data as heap pages, `Arc<Vec<u8>>` in a per-inode `BTreeMap`
  (`fs/breenfs/dirty.rs:31-37`). The dirty budget is `DIRTY_BUDGET` = 8 MiB per mount, "one eighth of the
  smallest kernel heap" (`fs/breenfs/commit_policy.rs:4-9`). The 64 MiB `DIRTY_LIMIT` commit trigger is
  never reached. <!-- claim-lint:ok: describes Breenix source at b22a066f, read for docs/plans/e3-page-cache.md section 1; not a measured result. -->
- **How read() combines them.** BreenFS `read` (`vfs.rs:3885-3943`) takes the mount's foreground lock and
  zero-fills the part of the buffer past EOF. It then copies each page from the open generation's dirty
  page, the frozen generation's page or the verified Files view, and finally overlays every resident
  mapping-cache page on the result (`vfs.rs:3940`, `file_map.rs:827-856`). Since #3239 the Files view
  zero-fills only the gaps between the extents it walks (`Runs::zero_to`, `fs/breenfs/ops.rs:277-285`),
  so a hole there is a gap the extent map shows. What remains of R1 on the read side is the merge of
  several sources and the overlay. <!-- claim-lint:ok: describes Breenix source at b22a066f, read for docs/plans/e3-page-cache.md section 1; not a measured result. -->

### 1.2 mmap and faults

- `map_file` populates the whole mapped range before it binds (`syscall/mmap.rs:542-552`).
- `populate` (`file_map.rs:1561-1693`) does the following:
  - claims the range;
  - reads absent pages in runs of at most `RUN_BYTES` / 4096 = 16 pages, through a staging buffer;
  - copies each run into freshly zeroed frames;
  - merges them into the sorted vector, re-reading pages a concurrent mutation marked.
- Faults never do I/O, by rule (`file_map.rs:9-14`, `:27-31`). `resolve_page` (`file_map.rs:2177-2289`)
  runs under PROCESS_MANAGER. It maps the resident page; for an absent bound index below EOF it inserts a
  zero frame: "A hole: its file bytes are zero" (`:2258-2270`). The #3232 claims close the known way for a
  bound page to be absent, but absence still means zero. <!-- claim-lint:ok: describes Breenix source at b22a066f, read for docs/plans/e3-page-cache.md section 1; not a measured result. -->
- There is no readahead beyond population's runs, and no fault-around. `POSIX_FADV_WILLNEED` populates at
  most 16 pages synchronously (`syscall/fs.rs:4219-4240`).

### 1.3 exec

`load_segments` (`exec_elf.rs:378-426`) maps a fresh zeroed private frame for every page of every PT_LOAD
segment, BSS included. It then reads each segment's file bytes into those frames in windows of `CHUNK` =
64 KiB. Text is never shared between processes, and BSS is allocated and zeroed eagerly. The user loader
(`libs/ld-breenix`) maps libraries, and its whole-file parse view, through `mmap`, so it pays section 1.2's
eager population. <!-- claim-lint:ok: describes Breenix source at b22a066f, read for docs/plans/e3-page-cache.md section 1; not a measured result. -->

### 1.4 Writes and commits

- `sys_write` copies the whole request from user memory into one heap buffer (`syscall/handlers.rs:47-79`,
  `:619`).
- BreenFS admits it in parts of `DIRTY_BUDGET / 2`. It asks for a commit at half the budget. When the
  budget is full, the writing thread runs the commit itself (audit R11; `vfs.rs` `write`, `admit_data`).
- One generation is in flight at a time (`docs/fs/breenfs-design.md` section 2.5, "Frozen-generation
  commit"). Preparation drains the frozen generation's pages into private extents through further copies
  (audit L3).
- Mapped dirty pages reach BreenFS through `writeback_pages` and a bounded 64-page copy buffer
  (`file_map.rs:63-128`).

### 1.5 Reclaim

There is none. A failed frame allocation in a fault kills the faulting process (`anon_map.rs`), and heap
exhaustion panics. Pages a binding covers are never evicted. The audit's R14 gives the rest. E1 builds the
allocator side: the buddy allocator, zones and watermarks, the reclaim registry and allocation errors. <!-- claim-lint:ok: describes Breenix source at b22a066f, read for docs/plans/e3-page-cache.md section 1; not a measured result. -->

### 1.6 The measured gap

These are the ARM64 QEMU rig series (DEV-31): 4 vCPUs, 2048 MiB, BreenFS root on virtio-blk on Breenix and
ext4 on Linux, 5 samples per row. Ratio above 1 means slower than Linux (DEV-28(1)). Medians are in
seconds unless marked.

| Row | Breenix (28668b28) | Linux | Ratio | Ratio at 6d95fac4 |
|---|---|---|---|---|
| `memory.driver-mmap` | 0.2805 | 0.00000104 | 269,220 | 258,061 |
| `memory.driver-touch` | 0.00549 | 0.00182 | 3.01 | 2.75 |
| `memory.driver-map-touch` | 0.2860 | 0.00183 | 156.5 | 151.4 |
| `compile.rustc-vv` | 0.2550 | 0.00359 | 70.9 | 107.1 |
| `process.rustc-first-output` | 0.2504 | 0.00300 | 83.4 | 126.9 |
| `compile.hello-object` | 0.4157 | 0.00914 | 45.5 | 50.3 |
| `compile.cargo-noop` | 0.4117 | 0.00609 | 67.6 | 69.1 |
| `compile.yes-dev-box` | 4.629 | 0.1362 | 34.0 | 34.6 |
| `compile.yes-release` | 16.21 | 1.082 | 15.0 | 15.2 |
| `disk.sequential-write` | 10.38 | 0.3707 | 28.0 | 27.6 |
| `disk.sequential-read` | 3.724 | 0.04160 | 89.5 | 88.9 |
| `disk.fsync` (ms per call) | 5.977 | 0.7170 | 8.34 | 10.28 |
| `process.fork-exec` (ms per child) | 2.658 at 6d95fac4 | 0.2431 | — | 10.93 |
| `memory.copy-bytes` (control) | 0.0201 at 6d95fac4 | 0.0203 | — | 0.99 |

The series files are `~/Library/Caches/breenix-devbox/bench-series/qemu-20261127-28668b28-*.series.json` and
`qemu-20261127-6d95fac4-*.series.json`. DEV-41's figures (269,000x, 71x and 90x) are the 28668b28 pair.
Linux's 1.04 µs mmap creates a VMA and nothing else. Its 0.042 s read of 1 GiB just after fsync is a copy
from its page cache. Breenix's 3.72 s is device reads through an 8 MiB cache. <!-- claim-lint:ok: figures from the qemu-20261127-28668b28 and qemu-20261127-6d95fac4 series named in docs/plans/e3-page-cache.md section 1.6; the moves are predictions, not results. -->

## 2. Goals, non-goals and invariants kept

### 2.1 Goals (the items DEV-41(2) and the brief name)

1. One page cache of physical frames, keyed by (file incarnation, page index), held in a radix tree per
   incarnation (section 4).
2. Explicit page states: absent, filling, uptodate, hole and error. Absent never means zero. Only a sparse
   range that the extent map reports, or the EOF tail, produces zeros (section 5). This is R1's permanent fix. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
3. `read`, `write`, `mmap` and exec all use that cache (sections 7, 9 and 10). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
4. Lazy mmap, with sequential readahead and fault-around (sections 8 and 9).
5. Exec maps text and rodata shared and read-only from the cache, data private copy-on-write, and BSS
   demand-zero (section 10). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
6. Dirty is a page flag. The dirty limit is relative to RAM, with background writeback, writer throttling
   and pipelined BreenFS commits (section 11).
7. Reclaim: an LRU approximation and a page daemon (section 12), and an OOM policy that picks its victim by
   size (section 13).
8. The block cache shrinks to metadata only (section 14).

### 2.2 Non-goals (and who owns them)

- Queued, asynchronous, vectored block I/O, NCQ and DMA from pinned frames: E4. E3's fill and writeback
  interfaces take frame vectors so E4 can serve them directly (6.4). Until E4, they are served
  synchronously through a bounded bounce buffer.
- BreenFS internal concurrency: per-inode and per-node locks, and N+1 COWing over N without replay. That is
  E5. E3 adds one VFS-level lock per incarnation (7.3) and nothing inside BreenFS's lock structure. E2.5
  owns the rule that no device wait happens under `MountLock`. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- The fsync log tree (R20) and format v2: E11. E3 changes no on-disk format.
- Swap: not now (DEV-29 Q4). Section 12.6 keeps room for it.
- Transparent superpages and large cache pages (R32), sendfile and splice: E12. The radix tree keeps room
  for multi-page entries (4.3).
- Page migration and compaction of cache pages: E12, with superpages.
- ext2's own data path. ext2 keeps today's behaviour through the trait defaults (6.5).
- The VMA tree, mremap, vfork and posix_spawn: E8.

### 2.3 Invariants kept

- **The fallible-allocation discipline.** Every hook that runs after the disk has changed (commit, failed
  write invalidation, the EOF transition, revocation) uses only capacity reserved before the change
  (`file_map.rs:16-20`). The tree's node reservations (4.3) keep that property. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Flush before free.** A cache frame's last reference is dropped only after every translation of it is
  invalidated (`ReleasedLeaf`, `FlushedLeaf`, `LeafBatch` in `process_memory.rs`). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Frame leases and the census.** Cache frames keep `FrameOwner::PageCache`. The `/proc/breenix/frames`
  owner set does not change (`docs/suites/README.md`, "Memory record").
- **BreenFS's commit ordering and crash contract** (`docs/fs/breenfs-design.md` section 2.5). This covers
  nodes and data, then FLUSH, then superblock rings, then FLUSH, and the G+2 reuse rule. A write stays
  all-or-nothing across a crash for each admission part, as today (11.5a). A generation's on-disk bytes are exactly its frozen snapshot (11.2). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Delayed allocation.** Blocks are allocated when a generation's pages are drained, in runs of up to
  1 MiB.
- **Checksums.** Every data block read from the device is verified against its extent REF CRC before its
  page becomes uptodate (6.3). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Vnode incarnation.** An open unlinked file outlives its directory entry, and its cache goes with it.

## 3. Layering

From the bottom up:

```
E1: buddy, zones, watermarks, per-CPU page caches, FrameMeta, slab, reclaim registry   (docs/plans/e1)
  -> FrameMeta page fields (64 bytes): owner, index, page flags, refs, dirty generation, LRU links -- 4.4
  -> FileCache per incarnation: radix tree of entries with DIRTY and WRITEBACK tags      -- 4.1-4.3
       -> page states and hashed wait channels (the E2.9 wait target)                   -- 5
       -> fill / writeback through the filesystem's cache operations                   -- 6
  -> read / write (copy through the cache), readahead                                   -- 7, 8
  -> mmap bindings, lazy faults (E2.9), fault-around, exec bindings                     -- 9, 10
  -> dirty accounting, throttling, BreenFS generation drain from frames                 -- 11
  -> LRU lists per zone, isolation, shrinker, page daemon, OOM selection                -- 12, 13
E4 (later): asynchronous vectored requests serve fill and writeback frame vectors
```

As in E1, each algorithm is a pure module with no kernel dependencies: the radix tree, the page-state
transition function, the readahead window, the LRU scan, the dirty-limit arithmetic and the OOM score. Each
takes its frames, clock and counts as parameters. A new host crate, `tools/pagecache-host-tests`, includes
them by `#[path]`. Kernel glue supplies the locks, frames and page tables. Production paths that existing
crates already drive (`tools/breenfs-host-tests`, `tools/vfs-host-tests`, `tools/pagetable-host-tests`)
get seams in the check commit of the PR that changes them, following E2's rule that a model must reach
production code (E2 section 8).

## 4. The cache object and its index

### 4.1 The FileCache

`MapState` becomes `FileCache`, one per file incarnation. The incarnation is the VNode's
(mount, ino, incarnation), which `VNode::reopen` already shares across opens (`fs/vfs/mod.rs:59-108`).
A FileCache holds:

- `tree`: the radix tree of page entries (4.3), behind `tree_lock`, a spin lock taken for bounded steps only.
  No I/O, no allocation from the page layer and no user copy happen under it.
- `bindings`: the reverse map. This is today's binding records, held in an interval tree keyed by file
  page range (Cormen et al., interval trees), so the bindings that cover an index are found in O(log n + k)
  instead of today's linear scans (`file_map.rs:553-560`, `:1150`).
- `io`: the per-incarnation I/O lock of section 7.3.
- Today's size and durability state: `mapped_size`, `disk_epoch`, `disk_write_seq`, the binding gate and
  the writeback cursor. Their meanings are unchanged.
- `wb_err`: a writeback error sequence (11.6).
- `last_dirty_gen`: the BreenFS generation that last dirtied a page or the inode (11.6).
- Counts: resident, dirty and writeback pages.

### 4.2 Lifetime: pages outlive close and munmap

The FileCache is reached from the VNode, as `MapState` is today. Each mount keeps a cache index from
(ino, incarnation) to the FileCache. An entry holds a strong reference while the cache has pages, and none
once it is empty. So: <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

- **Opening a file** finds its FileCache, and with it the clean pages an earlier open left. Today `open_locked`
  (`fs/breenfs/vfs.rs:2134-2154`) reuses a VNode only while a handle, or the mount's pin for a published
  writable mapping (`mapped`), still holds it. Otherwise it builds a new VNode with an empty `MapState`, and
  the pages an earlier open left are gone.
- **Closing a file or unmapping it** drops nothing. Clean pages leave only by reclaim (12), truncation
  (7.5), unlink of the last link once the last handle closes (today's `discard_unobserved`), or unmount. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **An empty FileCache** with no VNode leaves the index and is freed. Reclaim holds an owner reference
  across its isolation of a page (12.3), so a cache is never freed under a scan. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Observer mounts.** A read-only mount whose generation can advance under another writer (the
  `publication` check at `vfs.rs:3887-3902`) drops every clean page of the mount when it adopts a newer
  generation. That is one bounded walk of the mount's index per adoption, done before the read that saw the
  new generation. `GenerationView` mounts are immutable snapshots and need nothing. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 4.3 The radix tree

A digital search tree on the page index (Fredkin 1960; Knuth, *TAOCP* vol. 3, section 6.3):

- Each node has 64 slots, consuming 6 bits of the index per level. The height grows and shrinks with the
  largest index present. An index is a u64 page number, so the height is at most 11. Nodes come from an E1
  slab cache, each about 0.6 KiB with tags. For a densely cached file, that is under 0.25% of the cached
  bytes.
- **Entry words** (u64), by their low two bits:
  - `00`: empty. The page is absent.
  - `01`: a page. Bits 12-63 hold the pfn. The page's flags live in its `FrameMeta` (4.4).
  - `10`: an interior node pointer.
  - `11`: a value entry. Bits 2-3 give the kind: 0 is `Hole`. A value entry holds no frame. A failed
    fill leaves no entry: its waiters read the outcome from the frame they hold (9.2a), and the index
    becomes `Absent`.
  - E12's large pages will need a multi-index entry (one entry covering 2^k aligned indices). The encoding
    reserves the `11` kind value 2 for it, so E3 has no format to change later.
- **Tags.** Each node carries one 64-bit bitmap per tag, set for each slot whose subtree holds a page with
  that tag: `DIRTY` and `WRITEBACK`. Tagged iteration visits only marked subtrees. So writeback, fsync and
  the freeze find dirty pages in index order without walking clean ones, which replaces the full-vector
  scans of today's writeback snapshot.
- **Fallible.** Insertion that needs nodes takes them from a reservation made before `tree_lock`
  (`reserve(n)`, worst case: the height times the number of new leaves). A reservation that fails is
  `ENOMEM` for a syscall and a counted failure for a fault (E1 section 7.4), before anything changes.
  Removal never allocates. Empty nodes go back to the slab after `tree_lock` is released. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Lookup** walks under `tree_lock` and takes a page reference (4.4) before releasing it. Lock-free
  lookups are later work (E12). A per-file lock is not a global one, and nothing holds it long. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 4.4 Page descriptors in FrameMeta

E1 section 5.7 sets aside FrameMeta's growth to 64 bytes for E3. A cache page's frame uses E1's fields as
follows:

| Field | Bytes | Meaning for a cache page |
|---|---|---|
| E1's 32 bytes | 32 | `state_gen`, `leaf_refs` (the PTE count), `site`, `owner` = `PageCache`, `flags`, `order` and `zone` as E1 defines them. `use: [u64; 2]` holds the LRU list's previous and next pfn while the page is on a list |
| `cache: u64` | 8 | The owning FileCache's address. It is valid while the page is in the tree or isolated (12.3) |
| `index: u64` | 8 | The page index in the file |
| `pflags: AtomicU32` | 4 | `FILLING`, `FILL_FAILED` and `CANCELED` (a fill's outcome, 9.2a), `UPTODATE`, `DIRTY`, `WRITEBACK`, `REFERENCED`, `ACTIVE`, `ON_LRU`, `ISOLATED`, `ICACHE_CLEAN`, `READAHEAD` (the async trigger of 8.2), `SHARED_WRITE` (has writable shared PTEs) and `MAPPED_DIRTY` (stored through a mapping, not yet admitted to a generation, 11.2a) |
| `refs: AtomicU32` | 4 | Page references: one for the tree, one per in-flight fill or writeback, one per isolation, and one per lookup that copies or maps it. Distinct from `leaf_refs` |
| `dirty_gen: u64` | 8 | The BreenFS generation that owns the page's dirty bytes (11.1). 0 for a clean page. For a frame whose fill failed, which is in no tree, its errno |

The cost is 1.56% of RAM, as E1's 14.6 anticipated.

### 4.5 Waiting on a page

Pages have no wait queue of their own; it would not fit in 64 bytes. Waiters use hashed wait channels, after
the UNIX sleep-and-wakeup design in which a sleeper waits on an address hashed into a table of queues (Bach
1986, ch. 6):

- The table has 2^k `WaitChannel { queue, seq }` entries. 2^k is the power of two at or above 64 times the
  number of CPUs, sized at boot.
- A page's channel is `hash(pfn)`.
- A waiter uses E2's `park::wait(channel.queue, || page condition holds or seq != seen, None, Killable)`. The
  waker changes the page's flags, then bumps `seq` and wakes the channel. Every waiter rechecks its own page,
  so a hash collision costs a spurious wakeup and never a missed one. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- This is the "page in a filling state" wait target that E2's section 5.2 names: X is the page, X.queue and
  X.seq are its channel's. The fault takes the page reference and reads `seq` under `as.mapping`, as E2.9
  requires.

## 5. Page states

### 5.1 The states

| State | Encoding | Meaning | read() | Fault |
|---|---|---|---|---|
| Absent | empty entry | Unknown. The bytes must be read | Fill, then copy | Fill (E2.9); before E2.9 a counted error (5.3) |
| Filling | page, `FILLING` | A frame whose fill is in flight. Its bytes are not file bytes yet | Wait | Wait |
| Uptodate | page, `UPTODATE` | The frame holds the file's bytes | Copy | Map |
| Dirty | page, `UPTODATE \| DIRTY` | Newer than the disk; `dirty_gen` names its generation | Copy | Map |
| Writeback | page, `UPTODATE \| WRITEBACK` (and maybe `DIRTY`) | A frozen generation's stable copy is being written (11.2) | Copy | Map read-only; a store faults (11.2) |
| Hole | value `Hole` | The filesystem reported that no extent covers the page and the page is below EOF | Zeros | The shared zero frame, read-only. A write allocates (5.2) <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. --> |
| (failed fill) | no entry: `Absent`; the waiters' frame has `FILL_FAILED` and the errno | The fill failed | The errno, to that fill's waiters | SIGBUS, to that fill's waiters |

- **Absent never means zero.** Only two things produce zeros: a `Hole` entry, which only a fill or the
  EOF-extension rule (5.2) creates, and the part of the last page past EOF. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Errors are not sticky.** The waiters of a failed fill hold a reference to its frame, and read the
  errno there (9.2a). The index itself becomes `Absent`, so an access that starts later fills again, and a
  later fill cannot hide the earlier failure from that fill's waiters. A transient device error therefore
  does not poison a page for good, and every waiter sees exactly the outcome of the fill it waited for.
  Errors the application must learn about later are writeback errors (11.6), which are kept per file. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Poisoned** pages of today (bytes unknown after a failed mutation, `file_map.rs:606-641`) become
  `Absent`: their bytes are re-read from disk. Before E2.9 a fault cannot read, so poison keeps today's
  SIGBUS until E3-11.

### 5.2 Transitions

```
Absent  --claim for fill (tree_lock; allocate frame, insert, FILLING, ref)--> Filling
Filling --fill completed, CRC verified (clear FILLING, set UPTODATE; release; bump channel)--> Uptodate
Filling --fill reports a hole (remove the frame; insert Hole; bump channel)--> Hole
Filling --fill failed (remove the frame; FILL_FAILED and errno on the frame; bump channel)--> Absent
Filling --claim canceled: a dropped readahead window or a stale claim (6.1a) (remove the frame; CANCELED;
          bump channel)--> Absent
Hole    --write() (allocate, zero, DIRTY, dirty_gen = open G), or a shared store (allocate, zero, MAPPED_DIRTY; 11.2a)--> Dirty
Uptodate --write() (DIRTY, dirty_gen = open G; tag DIRTY), or a shared store (MAPPED_DIRTY, dirty_gen = 0; tag DIRTY)--> Dirty
Dirty with MAPPED_DIRTY --mapped admission (11.2a: reserve, changed set, dirty_gen = open G)--> Dirty
Dirty   --freeze of G = dirty_gen (WRITEBACK; tag WRITEBACK; the frame is G's stable copy)--> Writeback
Writeback --G durable and adopted by the open view (11.2; clear WRITEBACK; clear DIRTY unless redirtied in G+1)--> Uptodate or Dirty
Writeback --write(), or a shrink's tail zeroing (7.5), while WRITEBACK (copy into a new frame, replace the entry; the old frame stays G's)--> Dirty (new frame)
any frame-backed state except Filling and Writeback --reclaim, clean and unmapped (12)--> Absent
any state --truncation past it (7.5)--> Absent (a Writeback frame stays referenced by G's batch until written)
EOF grows from s to s' --(bound pages in [s, s'), before E2.9)--> Hole
```

- Every transition happens under `tree_lock`, or by an atomic update of `pflags` on a page the caller holds
  a reference to. The fill's own transitions are made by the thread that claimed it. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- The function `page_state::transition(from, event) -> Result<to, Refused>` is the pure module that encodes
  this table. Every caller goes through it, and a refused transition is a kernel bug, reported with both
  values. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **The fill publishes with release ordering.** The fill writes the frame, then sets `UPTODATE` with release
  semantics. A reader or fault reads `pflags` with acquire before using the bytes or installing a PTE.
  On ARM64 the PTE install also follows the existing barrier sequence for publishing a mapping (Arm ARM,
  B2, the memory model, and D8, translation table updates).

### 5.3 Before E2.9

Faults cannot sleep before E2.9. E3-2 therefore makes an absent bound page below EOF an explicit, counted
failure (SIGBUS with `absent_bound` incremented in `/proc/breenix/pagecache`), never a zero page. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

- With #3232's pins, eager population and today's eviction rules, no known path reaches it. A non-zero
  count is a bug report, not a fallback. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- EOF extension inserts explicit `Hole` entries for bound pages, instead of relying on the absent-means-zero
  rule. E3-11 removes that step, because a fault can then ask the filesystem. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 5.4 Replacing a page's frame

These transitions give an index a new frame while PTEs may still map the old one: a `Hole` page written by <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
`write()` or by a shared store (the old frame is the shared zero frame), and a `Writeback` page written by
`write()` or a shrink (the old frame is G's stable copy, 11.2). Read-only PTEs of the old frame can exist
in any address space: private mappings, shared read-only mappings and fault-around entries. A process
reading through one would keep seeing zeros, or G's bytes, after the change. So each replacement:

1. publishes the new frame in the tree under `tree_lock`, so a new fault maps it;
2. then revokes, through the reverse map (12.5), every PTE that maps the old frame's pfn at this index, <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   with its flush. A PTE that maps another frame, such as a private copy-on-write copy, is left alone;
3. returns to its caller only after the flush has completed. The next access through a revoked PTE faults
   and maps the new frame. That fault needs no I/O, so it works before E2.9.

`write()` does step 2 holding `io` (`io` → `as.pt`, section 15). A store fault does it after dropping its own <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
`as.pt`, taking each address space's `as.pt` in turn and never two at once, then re-executes. Before E2.8,
all of this runs under PROCESS_MANAGER, as today's `revoke` does.

## 6. The filesystem interface: fill and writeback

### 6.1 The operations

`MappingIo::read` and `read_into` give way to two operations on `FileSystem`, both on frame vectors:

```
fn cache_policy(&self) -> CachePolicy            // Native or MappedOnly (6.5)
fn fill(&self, node: &VNode, first: u64, pages: &mut FillBatch) -> Result<FillReport, Errno>
fn writeback(&self, node: &VNode, batch: &WritebackBatch) -> Result<(), Errno>   // MappedOnly only (6.5)
```

- **`FillBatch`** is the frames of a run of consecutive page indices that the cache claimed as `Filling`.
  Its entries are E1's frame-vector type (E1 section 8): leased frames with offsets.
- **`FillReport`** classifies each page as `Data` (the frame now holds verified file bytes, with the zero
  tail past EOF), `Hole` or `Error(errno)`. A page the backend did not reach is `Error(EIO)`. That keeps
  T8 out: there is no "did not fill" that looks like success. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Callers.** `fill` is called with no FileCache `tree_lock` and no VM lock held. The caller has a
  reference to each frame and the `FILLING` claim. The backend may sleep and take its own locks.
- **One hold of the exclusion per fill.** A fill never takes the mount exclusion while its caller holds <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  it, and no claim is visible to a thread that could wait on it while holding a lock the fill needs:
  - **Before E2.5** a fill holds BreenFS's foreground exclusion from its claim to its publication. A <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    caller that already holds it (population today, `read` before E3-7) passes its guard to `fill`, which
    then takes the snapshot under that guard instead of locking again. The readahead worker claims a
    window only when it starts it, under the exclusion, never at queue time (8.2). So a thread that holds
    the exclusion never meets another thread's claim.
  - **From E2.5** the fill claims, then takes `MountLock` once to pin its view and block lifetime
    (E2's I7), releases it, and reads with no BreenFS lock held. It never takes `io`. A waiter may then
    hold `io` (a sleeping lock from E3-7, 7.3) while it waits, but never `MountLock`, `PublishToken`, an
    `as.*` lock, `tree_lock` or an LRU lock (section 15). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Native backends do not implement `writeback`.** Dirty pages reach BreenFS through its commit (11.3).

### 6.1a Fill completion revalidates the file

E2.9's revalidation (9.2) covers the mapping: munmap, mprotect and a replaced VMA. A fill also needs the file <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
to be unchanged for the pages it read, or it publishes older bytes, or a hole, over a newer change. Today's
population solves the same race with a mark (`memory/file_map.rs:557-561`, `:1555-1558`), and E3 keeps it on
every `Filling` claim:

- **Claim, then snapshot.** A fill claims its pages under `tree_lock` before it takes its read snapshot
  (6.2, step 1). A change made between the two marks the claim, so the order is conservative.
- **Changes mark claims.** Under `tree_lock`, each operation that changes the file's bytes or extents for a <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  range without going through the cache's own pages marks the `Filling` claims it covers stale: before E3-8,
  every BreenFS write (today's `mark_populating`); always, truncation, a hole punch, and an observer mount
  adopting a newer generation (4.2). From E3-8, `write()` changes pages through the cache, and a `write()`
  that reaches a `Filling` page waits for it (4.5) instead of racing it. It holds only `io`, which a filler
  never takes (section 15).
- **Completion rechecks.** Under `tree_lock`, the fill publishes a page only if the entry still holds its
  claim's frame and the claim is unmarked. Otherwise it discards that page's result: the entry becomes
  `Absent` (or goes, past EOF), the frame is marked `CANCELED` (9.2a), and the channel is woken. Waiters
  retry, and the next fill reads a new snapshot.

### 6.2 BreenFS fill

1. Take a read snapshot of the open view: before E2.5, under the foreground exclusion the fill holds to its
   publication (or its caller's guard, 6.1). After E2.5, pin the view and its block lifetime under the
   lock, then release it (E2's I7).
2. For each page, find its extent through the in-core extent map (#3239's `ExtentMaps`, `ops.rs:125-215`),
   seeking to the first intersecting extent rather than scanning.
   - A page no extent covers, below the inode's size, is `Hole`.
   - A page past the size is not in the batch (the cache clips at `mapped_size`).
3. Group the pages into physically contiguous runs. A run is bounded by the extent, by the device's maximum
   transfer and, until E4, by the bounce buffer (6.4).
4. Read each run, verify each block against its REF CRC, and copy it into its frame.
   - A failed CRC or a device error makes that run's pages `Error(EIO)`.
   - A node failure latches the mount read-only, as today.

Clean file data no longer passes through the block cache. Fill reads the device directly, so the
`DataReader` path (`vfs.rs:4756-4778`) and its data entries go (section 14).

**Until E3-8, the fill reads BreenFS's dirty layers first.** Before E3-8 dirty bytes do not live in the
cache: they are BreenFS's open dirty pages and the frozen or installed generation's pages, and the extents do
not hold them yet. So E3-4's fill, and E3-7's read, resolve each page with today's layered lookup, the
mount state's `page` (`fs/breenfs/vfs.rs:1020-1035`): the open generation's dirty page, then the frozen
generation's (or the installed one's until the replay completes) unless the open one truncated it away, and
only then the extents. That lookup runs under the same snapshot as the extent walk (step 1), and a page
found in a dirty layer is `Data`. E3-8 removes the layers, because from E3-8 on the statement below
holds. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

From E3-8, the fill never reads a page that the cache holds dirty or under writeback, because those are never
absent. So the fill reads the open view's extents and nothing else. The layered "open generation's page, then the
frozen generation's page, then the Files view" lookup of today's `read` (design 2.5, "Foreground reads")
becomes a tree lookup. The cache holds the newest bytes for every index it has, and the open view's extents
give the bytes of every other one: <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

> **Cache invariant.** For every page index i of a Native file below its size, exactly one holds: i has an
> uptodate, dirty or writeback page in the tree; or i is a Hole entry and no extent covers i; or i is
> absent, and the open view's extent for i (or the absence of one) gives i's bytes. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

The invariant is checked by the model test of E3-8 after every step (19.2). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 6.3 Checksums and integrity

- Each data block is verified on its way into a frame, before `UPTODATE` is set. A page is never uptodate
  with unverified bytes. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- The CRCs that a generation writes are computed once, from the frozen frames (11.3), not again in
  preparation (audit L2's second pass).

### 6.4 Until E4

The block device interface is synchronous on contiguous slices (`block/mod.rs`). Until E4:

- A run is read into a bounce buffer and copied into its frames, which is one copy, as population does
  today. The buffer's size is the device's maximum transfer.
- **A bounce buffer belongs to one request,** from the device read through CRC verification to the last <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  copy into a frame. A fill sleeps in the device wait and can migrate, so a per-CPU buffer could be reused
  by another fill between its verification and its copy, and publish another file's bytes as verified.
  Population today owns its staging buffer for the call (`memory/file_map.rs:1589-1648`), and E3 keeps that
  ownership: a fill takes a buffer from a mount-wide pool of frame-backed buffers as an owned token, and
  returns it after its copies. Before E2.5 a mount's fills are serialized by its exclusion (6.1), so a
  pool of one buffer never runs dry. From E2.5 a fill that finds the pool empty waits on the pool's
  channel (4.5) with E2.2's `park`, in a sleepable context. A context that cannot sleep never claims a fill
  (9.2, step 3). The pool holds one buffer per CPU at mount and grows on demand within the readahead
  budget (8.2). The drain's write bounce (11.3) uses the same pool.
- The fill runs synchronously in the calling thread. An asynchronous readahead runs it in the readahead
  worker (8.2).

E4 serves `FillBatch` and the drain's batches (11.3) as vectored requests straight into the frames, with
several in flight. The interfaces above do not change for it.

### 6.5 Other filesystems

- `CachePolicy::Native`, which only BreenFS declares, makes `read` and `write` go through the cache (section 7).
- `CachePolicy::MappedOnly`, the default, keeps today's behaviour:
  - `read` and `write` call the backend;
  - the cache holds only pages that mappings and exec need;
  - `fill` defaults to the current `MappingIo::read_into` path;
  - dirty mapped pages go out through `writeback`, as `writeback_pages` does today.
  This keeps ext2, UFS, procfs and the device filesystems unchanged. ext2's data stays under the block
  cache's budget (14).

## 7. read and write through the cache

### 7.1 read

For a Native file, `read(fd, buf, n)` at offset `off`:

1. Take `io` shared (7.3), read the size, and clip the request at EOF.
2. Walk the pages that the request covers:
   - **Uptodate, Dirty or Writeback:** take a page reference, set `REFERENCED` and copy.
   - **Hole:** copy zeros.
   - **Absent:** claim it and the rest of the readahead window (8) as
     `Filling` in one `tree_lock` hold, then fill outside the lock. A page already claimed by someone
     else is waited on (4.5).
3. If the fill this read ran, or waited on, failed for a page (its frame has `FILL_FAILED`, 9.2a), return the bytes copied before it, or the
   errno if there are none (POSIX `read`: a partial count, else -1 with errno). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
4. Touch atime through the existing non-blocking `touch`.

The copy target is today's bounded kernel staging (`syscall/regular_read.rs:11`, 64 KiB chunks) until
E3-15, which copies straight to user memory (7.4). The five-source overlay and the zero prefill go: a read
copies each byte from exactly one source. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 7.2 write

For a Native file, `write(fd, buf, n)`, one part at a time (11.5a):

1. Take `io` exclusive, and keep it until the last part. For `O_APPEND`, take the offset from the size
   under it, once.
2. **Prepare** every page the part covers. This step may fail, and it changes no page the tree holds: <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   - **Absent, with the write not covering the whole page below EOF:** fill a new, unpublished frame first
     (read-modify-write). Never hold a page in `Filling` across a user copy: the fill completes before the
     copy. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   - **Absent and fully overwritten, or past EOF, or Hole:** allocate a new, unpublished frame and zero the
     parts not written. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   - **Writeback:** allocate the copy frame of 11.2's COW-on-writeback step and copy the stable bytes into it.
   - **Uptodate or Dirty:** nothing; the page is changed in place in step 4. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   - Reserve the tree nodes that publishing the new frames needs (4.3). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   No BreenFS lock is held here: a fill takes its own (6.1). `io` keeps every other write and truncation of
   the file out, so a prepared frame stays current.
3. **Charge** the part's newly dirtied pages against the dirty limit (11.5). If they do not fit, request the
   mount's commit and park, holding only `io`, until they do.
4. **Join the open generation and publish.** Take a hold on the mount's open generation G (11.5a): G cannot
   freeze while a hold is out. Under `MountLock`, BreenFS reserves the part (its worst-case space
   reservation, `reserve.rs`, and the generation bookkeeping; only the byte budget changes, 11.4), and
   releases the lock. Then, still holding G, and with no further failure possible:
   - copy the part's bytes, 64 KiB at a time, from user memory into kernel staging and from there into the
     page or its prepared frame; E3-15 copies straight from user memory (7.4). This replaces the
     whole-request heap copy (`handlers.rs:619`);
   - insert the prepared frames into the tree, and mark each page `DIRTY` with `dirty_gen` = G. Tag it;
   - publish the new size, if it grew, through BreenFS's inode update, then through `mapped_size`, as
     `transition` does today;
   - release the hold. A prepared frame that replaced a `Hole` or `Writeback` page revokes the old frame's
     PTEs (5.4) before the write returns.
5. Go on with the next part from step 2, or release `io`.

Atomicity:

- A part changes every byte or none, across a crash too, because all of it lands in the one generation it
  holds (step 4). A write of one part, which is any write up to `limit / 2`, is therefore all-or-nothing.
  A longer write can recover as a prefix of whole parts, which is today's rule for writes longer than
  half the budget (design 2.5, "Dirty admission"); 11.5a keeps it. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- If step 2 fails for want of memory, it frees the frames it prepared and changes nothing, so the bytes of
  earlier writes stay in the tree. Invalidating changed pages back to `Absent`, as `invalidate` does today,
  would lose them: a dirty page's bytes are on no disk, and a refill would read older bytes. The write
  returns the count of whole parts completed before it, or `ENOMEM`. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- A user copy in step 4 that ends in `EFAULT` ends the write at the bytes it copied, a short write. Those
  bytes are in G, the prepared frames it did not reach are freed, and no earlier byte is undone.

### 7.3 The I/O lock, and POSIX atomicity

POSIX XSH 2.9.7 requires `read`, `write`, `pread`, `pwrite`, `truncate` and their relatives on a regular
file to be atomic with respect to each other: each sees all or none of another's effects. Today the
BreenFS mount lock provides that by serializing everything on the mount (audit R10). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

- E3 gives each FileCache an `io` lock. Readers take it shared, and writers and truncation take it exclusive.
- Before E3-7, `read` and `write` keep BreenFS's existing path and its foreground exclusion, and nothing <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  waits on a `Filling` page: population and the readahead worker fill under the exclusion (6.1), and
  faults do not wait before E3-11.
- From E3-7, `io` is its own `SleepRwLock` (E2.6), separate from `MountLock`. A reader waits on a
  `Filling` page holding only `io`, and the filler, which needs `MountLock` only briefly and never `io`
  (E2.5), cannot be waiting for it. A read of cached pages takes no mount-wide lock. That is the
  page-cache half of R10. E5 does the rest. Holding the foreground exclusion as `io` would deadlock: the
  reader would hold the lock its filler needs. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- `io` is the outermost lock on these paths (section 15).

### 7.4 Copying straight between user memory and frames (E3-15)

Once faults may sleep (E2.9), `read` copies from referenced cache frames straight into user memory, and
`write` copies straight from user memory into the frames. Both hold `io` across the copy.

- A user page that is not present faults, and the fault may sleep and fill (9.2). It takes `as.mapping`,
  never `io`, so there is no cycle (section 15). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- A write whose source buffer is a mapping of the same file at the same page faults on a page the write
  already made uptodate, or one the fault fills. `write` holds no page in `Filling`, so it cannot wait on
  itself.
- `write`'s copy runs under a generation hold (7.2, step 4). A fault's fill does not wait for holds, and
  the freeze waits for them holding no `MountLock` (11.5a), so the copy's faults cannot deadlock with a
  freeze.

### 7.5 Truncate, extend and holes

- **Shrink.** With `io` exclusive, revoke the PTEs past the new EOF through the reverse map (the existing
  `revoke`, now driven by the interval tree), then remove the tree entries. A `Writeback` frame stays
  referenced by its generation's batch and is freed when that batch finishes. Zero the tail of the new last
  page (`zero_from`). If that page is `Writeback`, its frame is G's stable copy and may already have fed
  G's CRC, so the tail is zeroed in a copy instead: 11.2's COW-on-writeback step, with its frame and tree
  node reserved before the shrink changes anything (2.3). G's batch keeps the old frame unchanged. BreenFS
  trims its extents as today. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Grow.** No entries. The pages between the old and new EOF have no extent, so a fill reports them as
  holes. Before E2.9, bound pages in that range get explicit `Hole` entries (5.3).
- **Hole entries** are dropped by reclaim as freely as clean pages (12). A hole costs nothing to recompute. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

## 8. Readahead

### 8.1 Sequential detection

Each open file description, and each file VMA, carries a small readahead state:
`{ prev: u64, start: u64, size: u32, async_at: u64 }`.

- **Sequential.** A miss at index i is sequential if i = prev + 1 or i lies in the current window. Then the
  window doubles, up to `max`.
- **Otherwise** the window resets to the request's own pages, rounded up to 4, and at most `max`.
- **The first read at offset 0** starts with a window of twice the request, at most `max`. Files read from their start are
  usually read through (McKusick et al. 2014, ch. 9, read clustering; McVoy and Kleiman 1991).
- **`max`** is the device's maximum transfer times its queue depth (E4 reports the depth; before E4 it is
  1), capped at 2 MiB. It halves while the pressure flag of any zone is set (E1 5.3). The 2 MiB cap is a
  tuning constant, not a RAM ceiling.
- **One cap.** Every window, on every path (sequential growth, reset, the first read, the hints, a <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  fault's read-around in 9.4 and `MAP_POPULATE`), is clipped to `max` by the one function in
  `memory/readahead.rs`. A demanded request larger than `max` is filled in successive windows of at
  most `max`, each completed before the next is claimed, so one call never holds more than `max` pages
  in `Filling`.
- **Hints.** `POSIX_FADV_SEQUENTIAL` and `MADV_SEQUENTIAL` start at `max`. `POSIX_FADV_RANDOM` and
  `MADV_RANDOM` disable growth (window = request). `POSIX_FADV_WILLNEED` and `MADV_WILLNEED` queue an
  asynchronous fill of the range as windows of at most `max`, replacing today's synchronous 16-page
  population. The queue holds at most one stream's share of such windows per call; the rest of the range
  is dropped, as a hint may be. `POSIX_FADV_DONTNEED`
  drops clean unmapped pages of the range.

### 8.2 Asynchronous readahead

- When a window is issued, its first page past the demanded pages gets `READAHEAD`. A hit on a `READAHEAD`
  page clears the flag and queues the next window to the readahead worker, a kernel thread that runs fills.
  The reader then keeps consuming pages that are already uptodate while the next window is read, which
  overlaps the reader's copies with the device.
- From E2.5 the worker claims its pages as `Filling` before queueing, so a reader that gets there first
  waits on the page, not on the queue. Before E2.5 it claims a window when it starts it, under the
  exclusion (6.1); a populate that reaches the window first fills those pages itself, and the worker then
  finds them resident and skips them.
- The worker's queue is bounded by what it may hold: at most `max` pages per stream, and a share of free
  memory overall. That budget counts everything a queued or running window holds: its claimed frames,
  its job record, the tree nodes reserved for its entries (4.3) and its bounce buffer (6.4). A window
  that does not fit is not queued. When memory is short it drops queued windows, never demanded ones, and retires their claims (9.2a). Readahead is a
  hint (Patterson et al. 1995). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 8.3 Scan resistance

Readahead pages enter the LRU's inactive list unreferenced (12.1). A single pass over a large file, such as
the 1 GiB sequential read, therefore displaces only pages that nobody referenced twice. A working set read
more than once survives it (Johnson and Shasha 1994).

## 9. mmap: lazy mappings, faults and fault-around

### 9.1 mmap creates the VMA

From E3-11, `map_file` checks access and the binding gate, then registers the binding and pushes the VMA.
It does no population and no device read, so the `memory.driver-mmap` row measures the VMA.

`MAP_POPULATE` and `mlock` populate explicitly, with readahead windows, after the VMA is published.

### 9.2 The file fault (on E2.9's protocol)

The fault runs `resolve_fault(as, addr, access, ctx)` (E2.9). For a file VMA:

1. Under `as.mapping` shared, find the VMA and binding, check the access, and compute the index. Reject an
   index past `mapped_size` with SIGBUS, keeping today's in-transition retry.
2. Look the index up under `tree_lock`:
   - **Uptodate, Dirty or Writeback:** take a reference. Fault around it (9.3). Take `as.pt`, recheck the
     PTE, and, under `tree_lock`, recheck that the entry still holds the page (12.5). Then install it with
     the VMA's rights: a private writable mapping is copy-on-write, as `entry_flags` does today, and a store
     into a shared mapping of a `Writeback` page follows 11.2. Return.
   - **Hole:** map the shared zero frame read-only. A store into a private mapping copies, as for any COW
     page. A store into a shared writable mapping allocates a zeroed frame, inserts it `MAPPED_DIRTY`
     (11.2a), revokes the zero frame's PTEs at this index in other address spaces (5.4), and maps it
     writable. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   - **Filling:** take a reference, read the page's channel `seq`, drop `as.mapping`, and park (E2 section
     5.2, "must wait for X"). When it wakes, it reads the outcome from the frame it holds (9.2a).
   - **Absent:** claim the page, and its fault window (9.4), as `Filling`. Drop
     `as.mapping`, run the fill with no VM lock held, wake the channel, and return; the access
     re-executes and maps the page in step 2.
   - **A fill this fault ran, or waited on, failed:** SIGBUS with `BUS_ADRERR` (POSIX `mmap`: references
     to pages that cannot be read). A kernel user copy takes the uaccess fixup and EFAULT.
3. A fault in an atomic context never claims or waits. It takes E2's nofault exit (EFAULT). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

E2.9's revalidation covers every change made while the fault slept: munmap, mprotect, a replaced VMA,
truncation and exec. The retried fault repeats the lookup from the top and installs nothing it saw before
it slept. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 9.2a Fill outcomes live on the frame

A waiter that rechecked the tree after waking could find a later fill's claim, or that fill's result, in
place of the outcome it waited for, so a persistent error could be retried for good, or lost. So:

- **Every waiter holds the frame.** A reader or fault that waits on a `Filling` page holds a page <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  reference to that frame (4.4). When it wakes it reads the frame's flags: `UPTODATE` means the access
  re-executes and maps or copies the page; `FILL_FAILED` delivers the errno stored in the frame (SIGBUS
  for a fault, the errno or a short count for `read`); `CANCELED` means the claim was retired, and the
  access re-executes and fills the page itself. It then drops the reference, and the frame is freed with
  the last one.
- **Every claim ends with a wake.** A fill's completion, failure, staleness (6.1a) or cancellation removes <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  or publishes the entry under `tree_lock`, sets the frame's outcome with release ordering, then bumps
  the channel. No path drops a claim without doing all three.
- **Dropped readahead.** When the readahead worker drops a queued window (8.2), it retires each page it
  claimed for that window as `CANCELED`, so no waiter is left parked on a fill that will never run. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 9.3 Fault-around

- After resolving page i, map up to 16 neighbours: the aligned 16-page window containing i, clipped to the
  VMA, to EOF and to i's last-level page table.
- Only neighbours that are already `Uptodate` or `Dirty` and have no PTE are mapped. Fault-around starts no
  I/O and waits for nothing. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- Neighbours are mapped read-only whatever the VMA allows: a private writable VMA gets copy-on-write entries,
  and a shared writable VMA gets read-only entries whose first store faults to mark the page dirty (11.2).
- The whole window is installed under one `as.pt` hold.
- On `MADV_RANDOM` the window is 1.

This removes most of the 64,000 faults a full touch of the 262 MB driver takes today. The `memory.driver-touch`
row is where it shows.

### 9.4 The fault window (read-around)

A miss claims a cluster around the faulting index and fills it in one request. This is the vnode pager's
clustered page-in (McKusick et al. 2014, ch. 6).

- The cluster is 16 pages (64 KiB), aligned and clipped to the VMA and EOF, skipping resident pages.
- A sequential fault stream, judged by the VMA's state (8.1), grows it like a read window, and queues the
  next window asynchronously.

### 9.5 Shared writable mappings and dirty tracking

- A shared writable mapping maps pages writable only after a store fault has marked them dirty, as
  `resolve_page` does today (`file_map.rs:2277-2282`). A page with writable shared PTEs carries
  `SHARED_WRITE`.
- At a BreenFS freeze, such a page is write-protected and copied (11.2), so its next store faults again and
  re-dirties it in the new generation. A page that is never stored again stops being written back. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- `msync(MS_SYNC)` and `fsync` on a mapped file keep today's meaning (`sync_range`, design 2.5 "fsync").
  `msync(MS_INVALIDATE)` drops clean unmapped pages of the range.

### 9.6 Fork

Fork keeps today's binding inheritance (`fork_vmas`, `Binding::fork`). Because pages now come from the cache
on demand, the child's inherited binding needs no residency: a child fault fills like any other.

### 9.7 Before E2.9

Until E3-11, `map_file` keeps its population, and faults keep resolving from resident pages only (5.3).
E3-6's retention still helps: a second mmap of the driver finds its pages resident, so population reads
nothing (19.2, E3-6). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

## 10. exec: shared text, private data, demand-zero BSS <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 10.1 The mapping of a PT_LOAD

The System V gABI (ch. 5, "Program Loading") requires a loadable segment's file offset and virtual address
to be congruent modulo its alignment, and memory past `p_filesz` up to `p_memsz` to read as zero. Kernel
exec, for the program and its interpreter, maps each PT_LOAD as follows: <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

- **The file-backed whole pages of a segment without PF_W** (text, rodata): a private file binding of the
  cache, mapped read-only, with execute permission if PF_X. Every process running the image shares these
  frames. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **The file-backed whole pages of a PF_W segment** (data): a private file binding with copy-on-write. The
  first store to a page copies it, which is today's private-mapping path (`entry_flags`).
- **The tail page,** the page that holds the segment's last file bytes when `p_memsz > p_filesz` and
  `p_filesz` does not end on a page boundary. This is one private anonymous page, filled with those bytes
  and zero after them. It is the only page exec copies. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **The rest of `p_memsz`** (BSS): an anonymous VMA, zero-filled on demand by today's anonymous fault path
  (`anon_map.rs`). This needs neither E1-9 nor E2.9.
- **Fallback.** A segment that breaks congruence, or a virtual page shared by two PT_LOADs with different
  rights (the case `map_interpreted_arm_page` handles, `exec_elf.rs:582-631`), is copied into private frames
  as today. The fallback is counted, so the copies are visible.

Exec bindings record `exec = true`. They participate in `may_change_exec_image` and the set-ID rules
unchanged.

### 10.2 Before E2.9, and after

- **E3-3 (before E2.9):** an exec binding is populated eagerly through the cache, as mmap is. The image's
  pages are still read once, but into shared cache frames, and a second process running the same image
  reads nothing while the first runs. After E3-6, it reads nothing at all while the pages are retained. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **E3-12 (after E2.9 and E3-11):** exec stops populating. Text and data fault in through 9.2, with
  fault-around, and the read-around window serves the first faults.

### 10.3 Running images do not change: ETXTBSY

A running image's text must not change under it. Today it cannot, because exec copies. Lazily mapped shared
text would show a later `write` of the file. POSIX lists `[ETXTBSY]` for `open` ("The file is a pure
procedure (shared text) file that is being executed and oflag is O_WRONLY or O_RDWR"), and the Linux man
pages document the same refusal for `open(2)` and `truncate(2)`, and for `execve(2)` of a file open for
writing.

E3-3 adopts it:

- Each FileCache counts its exec bindings (`deny_write`) and its descriptions open for writing (`writers`).
- `open` with write access, `truncate`, and a shared writable `mmap` of a file with `deny_write > 0` fail
  with ETXTBSY.
- An exec of a file with `writers > 0` does not fail: it takes the copying fallback of 10.1 for that image,
  so no build that writes a binary and runs it at once is refused.
- `rename` and `unlink` of a running image are allowed. The incarnation stays alive until its last binding
  goes.

This is an ABI behaviour change, so section 21 asks the coordinator to confirm it.

### 10.4 Instruction-cache maintenance

- **ARM64.** Instruction fetch is not coherent with data writes, and the Arm ARM requires the data cache to
  be cleaned to the point of unification and the instruction cache invalidated before modified bytes are
  executed. The first time a cache page is mapped executable after a fill or a write, the fault runs the
  existing `sync_executable` (`file_map.rs:543-550`) and sets `ICACHE_CLEAN`. A write or a fill clears the
  flag, so later mappings of an unchanged page skip the maintenance.
- **x86-64.** The instruction cache is coherent with data writes (Intel SDM vol. 3A, "Self-Modifying Code"),
  so nothing is needed. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 10.5 The user loader

`ld-breenix` maps each library, and its whole-file parse view, through `mmap`. After E3-11 those mappings
fault only the pages they touch. The loader keeps its parse view for the process's life (audit R30), which
then costs address space and no I/O. No loader change is needed for E3. A later loader change may unmap the
view.

## 11. Dirty pages, writeback and BreenFS commits

### 11.1 Dirty is a page flag

- BreenFS's `DirtyPages` maps (`dirty.rs`) are replaced by the cache's `DIRTY` tag and each page's
  `dirty_gen`.
- A generation's dirty set is "the pages tagged `DIRTY` with `dirty_gen = G`". A page stored only through a mapping joins it through mapped admission (11.2a), never from the fault. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - The freeze finds them by tagged iteration over the FileCaches of G's changed-inode set, which BreenFS
    already keeps.
  - It marks them `WRITEBACK` and records them in G's publication as a batch list of (cache, index, pfn),
    each holding a page reference.
- The private data of the generation being published, which today is copied out of the dirty pages
  (audit L3), is the frozen frames themselves.

### 11.2 Stable pages: copy on write under writeback

A frozen generation's bytes must not change while it is drained, checksummed and written, because G's
on-disk image must be G's snapshot (2.3). Today that holds because frozen pages are immutable `Arc`s, and a
write in G+1 copies a page that only G holds (design 2.5, "Foreground reads").

E3 keeps the same rule with frames:

- **write() into a `Writeback` page** copies it into a new frame, applies the write, and replaces the tree
  entry with the new frame (`Dirty`, `dirty_gen` = G+1). G's batch keeps the old frame and frees it when
  G's write completes.
  - Readers see the new bytes at once.
  - Writers never wait for writeback. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Read-only PTEs of the old frame are revoked after the new frame is published (5.4), so no mapping keeps
    showing G's bytes after the write. A writable shared PTE cannot exist on a `Writeback` page: the freeze
    write-protected it (next item).
- **A page mapped writable and shared at the freeze** (`SHARED_WRITE`), once admitted to G (11.2a):
  1. the freeze revokes write permission from its PTEs through the reverse map, and waits for the TLB
     flush to complete;
  2. it then copies the frame into a bounce frame for G, as today's rule does ("Mapped pages are copied to
     a bounce buffer before checksumming", design 2.5);
  3. under `tree_lock`, it clears `DIRTY` if `dirty_gen` is still G and the page has not become
     `MAPPED_DIRTY` again, and if it was not also written through `write()` in G+1.

  A store that lands before step 1 completes is in G's copy. A store after it faults on a present
  read-only PTE in a shared writable VMA, and becomes `MAPPED_DIRTY` for a later generation (11.2a). A
  mapped page therefore changes only in the cache's frame, never in G's copy. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **At G's install** G is durable, and fsync waiters for G are released. Its batch pages do not become
  clean yet. While N+1 changed during G's commit, the open view stays N+1's scratch view until the replay
  of N+1's log onto G's published view completes, and installed G's pages stay readable until then
  (`materialize`, `fs/breenfs/vfs.rs:1159-1166`). The scratch view does not hold G's new extents, so a G
  page reclaimed in that interval would refill from older extents, or as a hole.
- **When the open view adopts G's view** (the replay done, the point where `materialize` moves the
  installed pages to `stale_views`, or at the install itself when no replay is pending), each batch page
  clears `WRITEBACK`. A page that is still in the tree and was not redirtied becomes clean `Uptodate`, and
  only then is it reclaimable (12.2). If the replay fails, the mount latches read-only as today, and the
  pages stay until unmount (11.5).

  This is the change behind `disk.sequential-read`: data just written stays cached after fsync, instead of
  being dropped and re-read from the device.

### 11.2a Mapped stores enter a generation through admission

A store fault cannot take BreenFS's admission, which may sleep and takes the mount exclusion. If the fault
chose a generation itself, it could read G as open, lose the race with G's freeze, and mark a page dirty
in a generation whose batch is already chosen. The page would then be in no batch, and G's install would
clean it. Today's mapped writeback avoids that by entering each page through `writeback_pages`
(`fs/breenfs/vfs.rs:4324-4376`), which reserves space (`reserve_data`) and records the inode as changed
(`edited`) under the mount exclusion. E3 keeps that admission and moves only where the bytes live:

- **The fault picks no generation.** A store fault on a shared writable page sets `MAPPED_DIRTY` and the <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  tree's `DIRTY` tag, leaves `dirty_gen` at 0 (not admitted), makes the PTE writable and sets
  `SHARED_WRITE`, all under `as.pt` and `tree_lock`. It links the FileCache onto its mount's mapped-dirty
  list through a link the FileCache owns, so it allocates nothing. It needs no BreenFS lock and no I/O, so
  it works before E2.9.
- **Mapped admission** runs in process context under the mount exclusion. For each FileCache on the list it <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  reserves BreenFS's worst-case space for the `MAPPED_DIRTY` pages, as `writeback_pages` does, records
  the inode in the open generation's changed set, then under `tree_lock` sets `dirty_gen` to the open
  generation and clears `MAPPED_DIRTY`. It runs before every freeze, from `msync` and `fsync`, and from the
  writeback service when the dirty count passes `background` (11.4).
- **The handoff.** The freeze holds the mount exclusion from that admission until G's changed set and
  batch are chosen. So a page admitted to G is in G's batch, and a page that is still `MAPPED_DIRTY` waits
  for the next admission. No page can be dirty in a generation whose batch has been chosen.
- **ENOSPC.** If admission cannot reserve space, the page stays `MAPPED_DIRTY` and keeps its bytes, and the
  FileCache's `wb_err` (11.6) records `ENOSPC` for `msync` and `fsync`, as a failed `writeback_pages`
  reports it today.

### 11.3 The drain, from frames

Preparation drains G's batch list in index order per inode, in runs of up to 1 MiB, as today:

- Each run takes its extents from the allocator (Phase 1.2's in-memory index, goal-directed). Its CRCs are
  computed from the frames, once (6.3).
- Its writes are submitted from the frames. Until E4 they go through a bounded bounce buffer, as the
  population read does. Data writes no longer create block-cache entries: today each 512-byte sector
  entry is charged 1,280 bytes (audit R27). E4 replaces the bounce with scatter-gather from the frames.
- **Pipelined within the generation.** The drain submits run k with the block cache's existing `submit`,
  which returns a batch to wait on, and drains run k+1 while k is in flight. It waits for all of them
  before the generation's FLUSH (step 4 of design 2.5). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Before E4 this overlaps the CPU work (allocation, CRC, extent edits) with the device.
  - With E4 it also keeps several requests in flight.
- **Pipelined across generations.** G+1 is open and accepts writes during all of G's commit, as today. The
  next freeze still waits for G's install (one generation in flight). Two generations committing at once
  needs N+1 to COW over N without replay, which is E5's. E3 keeps the design 2.5 ordering untouched. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 11.4 The dirty limit, relative to RAM

```
dirtyable  = free frames + clean, unmapped, reclaimable cache frames      (E1 kmem::stats, cache counts)
limit      = dirtyable / 5
background = dirtyable / 10
```

- **The background threshold.** When the global dirty count passes `background`, the page daemon (12.4)
  asks each mount with dirty pages to freeze. BreenFS's commit worker freezes when any of these hold:
  - the 5 s interval (`COMMIT_INTERVAL_NS`) has passed;
  - the mount's dirty count passes its share of `background` (all of it while it is the only writer); <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - an fsync;
  - reservation pressure;
  - the page daemon asks.
- **Removed.** `DIRTY_BUDGET` and the unreachable `DIRTY_LIMIT` go. The per-mount admission bound becomes
  `limit`, enforced by charging before dirtying (11.5) instead of by an inline commit. BreenFS's worst-case space
  reservation (design 2.8) is unchanged, and ENOSPC is still decided there.
- **No inline commit.** A writing thread never runs `commit_generation`. That is audit R11's "writers
  throttled against writeback progress rather than committing themselves". <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Effect.** On the rig's 2048 MiB guest, `background` is about 160 MiB once the guest has settled. A
  1 GiB sequential write then makes about 7 publications instead of at least 256, so each publication's
  two FLUSHes are shared by about 160 MiB of data instead of 4 MiB. That is arithmetic from the formulas;
  the series decides what it buys.

The ratios are tuning constants, chosen so writeback starts at half the limit and writers rarely meet the
limit. They are not RAM ceilings. Section 21 Q4 names the fsync trade-off they carry.

### 11.5 Writer throttling

Checking the count after a write has dirtied its pages lets N concurrent writers pass `limit` by N writes
before any parks. So the count is charged before pages become dirty:

- **What is charged.** One global count of the frames that dirty data holds: `DIRTY` and `MAPPED_DIRTY`
  pages, `WRITEBACK` frames that a batch still holds (including frames a COW replaced in the tree), and
  the bounce frames of 11.2's mapped copies. A charge is released when its page becomes clean (11.2), is
  truncated, or its batch frees the frame. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Charge first.** A write charges each part's newly dirtied pages (7.2, step 3) with one compare-and-add <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  that fails if the count would pass `limit`. A failed charge requests the mount's commit and parks,
  Killable, on the writeback-progress channel until the charge fits, then retries. Parked writers retry
  in arrival order, so a light writer is not starved behind a heavy one. The one charge allowed past
  `limit` is a part admitted alone while its mount has nothing else charged, which keeps today's progress
  rule (design 2.5), and a part is at most `limit / 2`.
- **Context.** The wait uses E2's park contract and holds only `io`, which the commit worker and the drain <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  never take (section 15), and never a generation hold. Writers already wait for generations today
  (design 2.5, "Dirty admission"), so the wait does not need E2.9.
- **Mapped stores.** Before E2.9 a store fault cannot park. It charges its page without the check, so
  mapped stores can pass `limit` by the resident shared-writable pages stored since the last mapped
  admission, which the writeback service admits and freezes once the count passes `background` (11.2a).
  From E3-11 a store fault whose charge does not fit parks before it sets `MAPPED_DIRTY`, as a writer
  does.
- **A latched mount.** After a failed commit latches a mount read-only, its dirty pages leave the global
  dirty count and keep their bytes readable until unmount. Writers to other mounts are never throttled on
  pages that can never be cleaned. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 11.5a Parts, generation holds, append exclusion and the crash boundary

- **Parts.** A write longer than `limit / 2` is admitted in parts of at most `limit / 2`, as today's
  writes longer than half the budget are (`admit_data`, `fs/breenfs/vfs.rs:2211-2233`).
- **Generation holds.** A part joins the open generation with a hold, and G's freeze waits until G has no <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  holds. The freeze first marks G closing, so no new hold joins G (a new part waits for G+1 to open), then
  waits for the holds to drain, holding no `MountLock`, then freezes under it. This is the open and
  quiescing stages of the ZFS transaction-group pipeline (Bonwick et al. 2003; section 17 lists it). A part's copy
  may fault and fill (E3-15), and a fill takes `MountLock` briefly (6.1); nothing that waits for holds
  holds `MountLock`, so the copy cannot deadlock with the freeze. A hold is never kept across a wait for a
  charge (11.5) or for a page channel.
- **Exclusion.** From its first part to its last, the writer holds `io` exclusive and BreenFS's splitting
  mark for the inode, so no other write, truncation or read of the file sees a part of it, and an
  `O_APPEND` write takes its offset once, at the first part (POSIX XSH 2.9.7; `write`, `O_APPEND`).
- **The crash boundary** is the part (7.2, Atomicity). A crash recovers whole parts in order, never part
  of one. With `limit` relative to RAM the parts are larger than today's 4 MiB, so more writes are
  all-or-nothing than today. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 11.6 fsync and writeback errors

- **Per-file fsync shortcut.** Each FileCache records `last_dirty_gen`: the generation that last dirtied a
  page or changed the inode. `fsync` of a file whose `last_dirty_gen` is durable, with no dirty page, returns
  0 without joining a generation.
  - Today even a clean-file fsync on a writable mount joins the mount's generation (VFS lane §5).
  - A file with dirty data still commits the whole filesystem, which is audit R20 and E11's to change.
- **Writeback errors.**
  - Each FileCache keeps `wb_err = (seq, errno)`, advanced when a generation that held its pages fails.
  - Each open file description records `wb_err.seq` when it is opened and after each fsync.
  - `fsync` returns the error if the sequence has advanced since. So every description that could have
    written the data sees the failure once, and a later description is not blamed for an earlier failure
    (POSIX `fsync`: "[EIO] An I/O error occurred while reading from or writing to the file system"). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - The mount-wide latch to read-only is unchanged.

## 12. Reclaim

### 12.1 The LRU approximation

- **Lists.** Each E1 zone (DMA32, NORMAL) has two lists of file pages, `inactive` and `active`, under one
  spin lock per zone. A list is threaded through FrameMeta's `use` words, so it needs no allocation.
- **Insertion.** New pages enter `inactive` at its head with `REFERENCED` clear. That is the scan-resistant
  first queue of 2Q (Johnson and Shasha 1994), and the inactive queue of the BSD pageout design (McKusick et
  al. 2014, ch. 6).
- **Reference.** A read hit, a lookup for mapping, or an accessed bit found by the scan sets `REFERENCED`.
  No list operation happens on a hit, so hits never take the LRU lock. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Inactive scan,** from the tail:
  - `REFERENCED` → move to `active`, clearing the bit. A second reference promotes.
  - Otherwise the page is a reclaim candidate (12.3).
- **Active aging.** While `inactive` is below half of `active`, the tail of `active` moves to the head of
  `inactive` with `REFERENCED` cleared. A referenced active page is rotated once instead (second chance;
  Corbató 1968).

ARC (Megiddo and Modha 2003) and CLOCK-Pro (Jiang et al. 2005) adapt the split between the two lists from
the history of evicted pages. The two-list structure leaves room for that later through shadow entries in
the tree (the `11` kind value 3 is unused). E3 does not need it to close the gaps in section 20.

### 12.2 What can be reclaimed

| Page | E3-6 to E3-10 | From E3-11 | From E3-13 |
|---|---|---|---|
| Clean, not `Filling` or `Writeback`, covered by no binding and no population claim | Freed | Freed | Freed |
| Clean, no PTE (`leaf_refs` = 0), covered by a binding or a population claim | Kept | Freed | Freed |
| Hole entry | Dropped if covered by no binding or claim | Dropped | Dropped |
| Clean, mapped | Kept | Kept | Unmapped through the reverse map, flushed, freed |
| Covered by a locked binding (`mlock`, `mlockall`, `MAP_LOCKED`) | Kept | Kept | Kept |
| Dirty | Kept; the page daemon asks its mount to commit | Same | Same |
| Writeback | Kept until its generation is durable and the open view has adopted it (11.2) | Same | Same |

- **Covered pages before E3-11.** A PTE count of 0 does not mean a page is unneeded. An eager mapping
  that has not touched a page yet, and a population whose binding is not published yet, both rely on it,
  and before E3-11 a fault cannot refill it: it would take 5.3's `absent_bound` SIGBUS, which is the
  #3208 shape again. So until E3-11 reclaim keeps today's rule, that a page any binding or population
  claim covers is not evicted (`file_map.rs:1138-1183`). It is checked under `tree_lock` through the
  binding interval tree (4.1) and the claims. From E3-11 a fault can fill, so only PTEs and locks keep a
  page.
- **Locked bindings.** A binding records whether its VMA is locked (`mlock`, `mlockall`, `MAP_LOCKED`;
  `syscall/memory_advice.rs:255-353`). POSIX `mlock` requires the pages to stay resident, so reclaim's
  recheck skips a page a locked binding covers, and the scan moves it to `active` so it is not met again
  at once.

Mapped pages wait for E3-13. Unmapping one takes each mapping address space's page-table lock, which E2.8
makes per address space; before that it would mean PROCESS_MANAGER sections from the reclaim path.

### 12.3 Isolation

The FileCache tree lock comes before the zone's LRU lock (section 15). So the scan does not take a tree lock
while holding the LRU lock:

1. **Isolate.** Under the LRU lock, take up to 32 candidates from the tail. The page daemon may wait for
   the LRU lock. Direct reclaim (12.4) takes it with `try_lock` and reports no progress if it is busy,
   since it runs inside an allocation whose caller may hold locks below it (E1 7.3's rule). For each, set `ISOLATED`, take a
   page reference and take an owner reference: the cache's address from `FrameMeta.cache`, valid because
   the page is on a list and a FileCache empties its lists before it is freed. Release the LRU lock.
2. **Evict.** For each page, take its owner's `tree_lock` with `try_lock`.
   - On success, recheck the state against 12.2's table: still clean, not `Filling` or `Writeback`, no
     locked binding, and before E3-11 no binding or claim covering it. If so, remove the entry and drop
     the tree's page reference.
   - Otherwise, or if the lock was busy, put the page back.
3. **Free.** Drop the isolation references. A page whose last reference goes is returned to E1 after its
   flush (for a mapped page in E3-13, after `LeafBatch` has flushed).

Every step is bounded: a batch of 32, and `try_lock` only. That meets E1's two shrinker rules (E1
section 7.3). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 12.4 The page daemon and direct reclaim

- **The page daemon** is one kernel thread. It parks on the zones' pressure flags (E1 5.3, set when a zone
  crosses `low`). When woken, it scans until every zone is at `high`, or until a full pass frees nothing. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Each pass also checks the dirty share of the inactive tail. When dirty pages make up more than half of
    the candidates it skipped, it asks their mounts to freeze, so the next pass finds them clean.
  - From E3-13 it also harvests accessed bits for the inactive tail's mapped pages through the reverse map,
    clearing them as it reads them, which sets `REFERENCED`.
- **Direct reclaim.** E3 registers the page-cache shrinker in E1's reclaim registry (E1 7.3). Its cost order
  puts it after the slab and span shrinkers and before the block cache.
  - It runs the isolation scan over `inactive` for clean unmapped pages, with no sleeping and no I/O.
  - So it is safe from any context the allocator runs in, before or after E2.
- **Sleeping direct reclaim.** From E2, a sleepable allocation whose slow path came up short wakes the
  page daemon and parks on its progress sequence (E2 5.2, "memory waits"), bounded by the daemon's pass
  count. That is E1's step 4b with E3's wake source.

### 12.5 The reverse map

For E3-13, the FileCache's binding interval tree (4.1) gives the bindings that cover a page.

A covering binding does not mean its PTE maps the cache frame. A private binding's PTE for the index may map <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
the process's own copy-on-write copy, which holds data that exists nowhere else. So:

- **Exact frame.** For each binding, under its address space's `as.pt` (taken with `try_lock`; a busy one
  puts the page back), read the PTE. Clear it, or harvest its accessed bit, only if it maps this page's
  pfn, and add the flush to a `LeafBatch`. A PTE that maps another frame, or is absent, is left alone.
- **Coordination with installs.** A fault or fault-around installs a PTE for a cache page only while
  holding `as.pt` and, inside it, `tree_lock`, after rechecking that the tree entry still holds that pfn,
  and it raises `leaf_refs` there (as.pt → tree_lock is the order of section 15). After clearing the PTEs,
  reclaim takes `tree_lock` (with `try_lock`) and removes the entry only if the page is still clean, not
  `Filling` or `Writeback`, and its `leaf_refs` is 0. So an install either lands first and keeps the page
  (reclaim puts it back), or sees the entry gone and retries the fault.
- **Frame custody.** Reclaim holds its isolation reference until the `LeafBatch` flush has completed on <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  every CPU and no I/O holds the frame (a fill or writeback holds a page reference, 4.4). The frame returns
  to E1 only when the last reference goes after that (2.3).
- Anonymous pages are not reached this way: E3 does not reclaim them.

### 12.6 Room for swap

- The lists are keyed by (zone, kind) with kind ∈ {File, Anon}. E3 creates only the File lists.
- The isolation scan is generic over kind.
- A future swap pager adds Anon lists, a reverse map for anonymous pages (the leaf custody records already
  locate a frame's mappings in O(1) per page, memory lane keep list), a swap-backed entry kind in the
  process page tables, and its own on-disk format (DEV-29 Q4).
- No part of E3 assumes anonymous memory is unreclaimable, and no E3 structure would have to change.

## 13. The OOM policy

When reclaim cannot satisfy a sleepable allocation, E1's slow path returns `AllocError`, and E2's memory wait
reports that reclaim cannot make progress. Then:

1. **Select.** Compute each candidate's score and pick the largest. Break ties by the higher pid.
   - A candidate is an address space, so CLONE_VM siblings are scored, and killed, together.
   - The score is its private resident frames (anonymous pages and private copy-on-write copies), plus its
     page-table frames. Those are what killing it frees. Shared file pages are excluded: reclaim can already
     take them.
   - This is victim selection by size (McKusick et al. 2014, ch. 6, on the pageout daemon killing the
     largest process when memory is exhausted).
2. **Exclude** PID 1, kernel threads, and an address space already chosen and still exiting.
3. **Kill.** Send SIGKILL. Mark the victim so that its exit path may use E1's emergency reserve.
4. **Wait** on the allocator's progress sequence until the victim's memory is freed, or a bound of the
   victim's exit passes. Do not choose a second victim while the first is exiting. A victim that does not
   exit within the bound is recorded, and selection repeats without it.
5. **Retry** the allocation. If it still fails, the request fails: `ENOMEM` for a syscall, SIGBUS for a
   fault (E2 5.2).

- **Atomic contexts** never select. They fail, as E1 specifies. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **The faulting process** is no longer killed just for being the one that faulted (`anon_map.rs`, audit
  R14).
- **The OOM record** (E1's `report_oom`) gains the victim, its score, the next two candidates and the
  zones' state.

There is no per-process bias knob in E3 (section 21 Q2).

## 14. The block cache keeps metadata only

- **No file data.** After E3-7 (reads) and E3-8 (writes), no Native file data enters the block cache:
  - fill reads the device into frames (6.2), and the `DataReader` and `DataBlocks` data paths go;
  - the drain writes from frames (11.3);
  - preparation no longer has to retire "clean data-cache entries for the blocks G allocated" (design 2.5),
    because there are none. That removes a step whose omission the design itself names as able to fail a
    reader's CRC check. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **What stays.** The block cache holds BreenFS metadata: 16 KiB nodes, cached by (address, generation)
  since #3245. It also holds ext2's blocks (6.5) and the superblock rings.
- **Budget.** `bound_for_memory` drops the heap cap (`MAX_BYTES`). After E1-9 the heap is not the budget.
  The bound becomes RAM/32 with no maximum.
  - Clean, unpinned buffers are evicted LRU within it, as now.
  - They are also reclaimable through E1's block-cache shrinker (E1 7.3, third in cost order).
  - RAM/32 is a tuning constant: metadata is a small fraction of file data, and the 4 MiB floor stays for
    small machines.
- **Buffers.** Buffers move from heap vectors to frame-backed storage of the `RECLAIMABLE` class (E1 8). A
  16 KiB node is an order-2 block, and a 4 KiB ext2 block is one frame. The 1,280-byte charge per entry then
  covers only the index, and is recomputed from the index's real size.
- **What remains for E4.** The 512-byte data entries of the write path (audit R27) go with 11.3. Batching
  the superblock writes into one request per ring stays with E4.

## 15. Locks and lock order

E2 section 5.1 fixes the order for VM paths. E3 adds its locks at these positions, from outermost to
innermost:

```
FileCache io (shared or exclusive; a SleepRwLock from E3-7; before E3-7 there is no io)
  -> filesystem locks (BreenFS MountLock; commit PublishToken)
  -> as.mapping (E2.8)
  -> as.pt (E2.8)
  -> VNode table / mount cache index
  -> FileCache tree_lock                    (replaces MapState's inner lock, same position)
  -> zone LRU lock
  -> frame ledger and allocator (E1)
```

- **Generation holds** (11.5a) are taken with no `MountLock` held and released before any wait. The freeze
  waits for them with no `MountLock` held. A hold may be out while a user copy faults and fills.
- **Fills** run with no VM lock and no `tree_lock` held. A fault drops `as.mapping` before it fills (9.2).
  BreenFS fill takes `MountLock` only to snapshot its view, and from E2.5 releases it before the device wait.
- **`io` never under `as.mapping`.** A user copy under `io` may fault and take `as.mapping` (`io` →
  `as.mapping`). No path holding `as.mapping` takes `io`: faults, mmap, munmap and the fork copy do not take
  it. Truncation takes `io` first, then the reverse-map revocation takes `as.pt` for each binding (`io` →
  `as.pt`). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **The commit worker** holds `PublishToken`. It takes `tree_lock` for the freeze and install transitions,
  and the reverse map's `as.pt` for 11.2's write-protect. It never takes `io` or `as.mapping`. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Reclaim** never holds the LRU lock while taking a `tree_lock` (12.3). It takes `tree_lock` and, in
  E3-13, `as.pt` only with `try_lock`. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Waits** on a page's channel hold at most `io` (4.5, 6.1). A filler never takes `io`, so a waiter that <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  holds it cannot block its filler.

The structure test of E3-7 pins the lines above: `io` is not taken under `as.mapping`, and fill is not
called with `tree_lock`, `as.mapping` or the LRU lock held. Changing the order is an architecture change
under DEV-39(2), which stops for the coordinator.

## 16. Accounting, census and ceilings

- **`/proc/breenix/frames`** keeps its owner set. Cache frames stay `page_cache`.
- **A new `/proc/breenix/pagecache`** lists:
  - pages by state;
  - list sizes per zone;
  - dirty, writeback and the limits;
  - fills, fill pages and fill errors;
  - readahead windows issued, used and dropped;
  - fault-around pages mapped;
  - reclaim scanned, isolated and freed;
  - throttle waits;
  - `absent_bound` refusals (5.3);
  - exec copy fallbacks (10.1);
  - OOM selections.

  These are counters only, readable without a cache lock.

Ceilings that go:

| Ceiling today | After E3 |
|---|---|
| Block cache 4–8 MiB for file data (`MAX_BYTES`) | File data bounded only by free RAM and reclaim; block cache RAM/32 for metadata |
| Dirty data 8 MiB per mount (`DIRTY_BUDGET`) | RAM-relative `limit` and `background` (11.4) |
| Unbound mapping pages evicted at once, 64 per pass | Pages outlive close and munmap; reclaim by LRU |
| Population, readahead and fadvise at most 16 pages | Windows up to the device's transfer times its depth (8.1) |
| The mapping cache's sorted vector (contiguous heap reservations of megabytes, `file_map.rs:1600`) | The radix tree, slab nodes |

What remains is constant by design and is not a RAM limit: the readahead cap of 2 MiB, the fault-around
window of 16 pages, the isolation batch of 32, and the dirty and block-cache ratios. Each is a tuning value
that a measured series can change.

## 17. Sources

- M. K. McKusick, G. V. Neville-Neil and R. N. M. Watson, *The Design and Implementation of the FreeBSD
  Operating System*, 2nd ed., 2014: ch. 6 (virtual memory: objects, pagers, clustered page-in, the page
  queues and pageout daemon, out-of-memory handling) and ch. 9 (the local filesystem: the buffer cache,
  read clustering and write clustering).
- U. Vahalia, *UNIX Internals: The New Frontiers*, 1996: ch. 14-15 (the SVR4 VM architecture, vnode pages,
  segmap and the unified page cache).
- M. J. Bach, *The Design of the UNIX Operating System*, 1986: ch. 3 (buffer cache, read-ahead) and ch. 6
  (sleep and wakeup on hashed addresses).
- R. A. Gingell, J. P. Moran and W. A. Shannon, "Virtual Memory Architecture in SunOS", USENIX Summer 1987
  (file pages named by (vnode, offset) in one VM cache).
- R. Rashid et al., "Machine-Independent Virtual Memory Management for Paged Uniprocessor and Multiprocessor
  Architectures", ASPLOS 1987 (memory objects and pagers).
- L. W. McVoy and S. R. Kleiman, "Extent-like Performance from a UNIX File System", USENIX Winter 1991
  (clustered reads and writes, sequential detection).
- R. H. Patterson et al., "Informed Prefetching and Caching", SOSP 1995 (prefetching as a hint, bounded by
  the cache).
- T. Johnson and D. Shasha, "2Q: A Low Overhead High Performance Buffer Management Replacement Algorithm",
  VLDB 1994.
- N. Megiddo and D. S. Modha, "ARC: A Self-Tuning, Low Overhead Replacement Cache", FAST 2003.
- S. Jiang, F. Chen and X. Zhang, "CLOCK-Pro: An Effective Improvement of the CLOCK Replacement", USENIX ATC
  2005.
- F. J. Corbató, "A Paging Experiment with the Multics System", MIT Project MAC report MAC-M-384, 1968
  (the CLOCK algorithm).
- P. J. Denning, "The Working Set Model for Program Behavior", CACM 11(5), 1968.
- E. Fredkin, "Trie Memory", CACM 3(9), 1960; D. E. Knuth, *The Art of Computer Programming*, vol. 3,
  section 6.3 (digital searching).
- T. H. Cormen et al., *Introduction to Algorithms*, 3rd ed., 2009, section 14.3 (interval trees).
- J. Bonwick et al., "The Zettabyte File System", 2003, and the ZFS transaction-group pipeline it describes
  (open, quiescing, syncing).
- O. Rodeh, "B-trees, Shadowing, and Clones", ACM TOS 3(4), 2008; O. Rodeh, J. Bacik and C. Mason, "BTRFS:
  The Linux B-Tree Filesystem", ACM TOS 9(3), 2013 (transaction commit).
- A. Sweeney et al., "Scalability in the XFS File System", USENIX 1996 (delayed allocation).
- IEEE Std 1003.1-2017 (POSIX.1): XSH 2.9.7 "Thread Interactions with Regular File Operations"; `read`,
  `write`, `pread`, `mmap`, `msync`, `munmap`, `ftruncate`, `fsync`, `open` ([ETXTBSY]), `exec`,
  `posix_fadvise`, `posix_madvise`.
- System V Application Binary Interface, Generic ABI, ch. 5 "Program Loading and Dynamic Linking".
- Linux man pages (ABI behaviour only): `open(2)`, `truncate(2)` and `execve(2)` (ETXTBSY), `madvise(2)`,
  `posix_fadvise(2)`, `fsync(2)`.
- Arm Architecture Reference Manual for A-profile (DDI 0487): B2 (memory model), D8 (translation table
  updates and break-before-make), and the cache maintenance requirements for instruction modification.
- Intel 64 and IA-32 Architectures Software Developer's Manual, vol. 3A: "Self-Modifying Code" and
  "Cross-Modifying Code".
- R. H. Arpaci-Dusseau and A. C. Arpaci-Dusseau, *Operating Systems: Three Easy Pieces*: "Beyond Physical
  Memory: Policies", "Memory-Mapped Files" and "Caching and Buffering".

## 18. PR sequence

The PRs are in dependency order. Each is reviewable on its own and follows the program's rules:

- its check is its first commit, red on the code before it, and driven through production code by a seam
  where the effect lives in a production function (E2 section 8). A check that guards a mechanism the PR
  adds, and so cannot be red on the old tree, says so;
- its tests run once, at its final head (DEV-30);
- it builds with 0 new warnings on x86-64 and ARM64;
- it runs claim-lint and gets the other model family's review, including a clean-room provenance check;
- no PR runs an effort's suites. x86 evidence comes from the coordinator's batched R240 flash once QEMU shows
  the gain (DEV-31(2)), and from the program's one final run (DEV-30(2));
- the coordinator records a QEMU bbench series before and after each PR whose "Rows" line names a row
  (DEV-31(3)).

"Boot" says whether one ARM64 QEMU boot of the standard boot script is needed, and why. No E3 PR edits a
Tier-1 file. E3-11 edits the Tier-2 `interrupts.rs` only if E2.9's `resolve_fault` seam does not already
route file faults to `file_map` (expected: no edit).

| PR | Work | Depends on | Needs E1-9 | Needs E2.9 | Size |
|---|---|---|---|---|---|
| E3-0 | This design | – | – | – | docs |
| E3-1 | Pure modules: radix tree with tags, page-state transitions, readahead window, LRU scan, dirty limits, OOM score; `tools/pagecache-host-tests` | E3-0 | No | No | M |
| E3-2 | Explicit page states in today's mapping cache: no zero page for an absent bound index, explicit Hole entries, hole ranges reported by `read_verified` (R1 permanent, first half) | E3-1 | No | No | S <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. --> |
| E3-3 | exec through the cache: shared read-only text and rodata, private COW data, tail page, demand-zero BSS, ETXTBSY, I-cache flag (eager population) | E3-2 | No | No | M <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. --> |
| E3-4 | The `fill` interface: frame batches with per-page Data, Hole or Error; the interim frame-vector type; BreenFS fill seeks the extent map and reads runs straight into frames, bypassing the block cache; ext2 default | E3-2 | No | No | M |
| E3-5 | Readahead for population, fadvise and madvise: windows, sequential detection, the readahead worker | E3-1, E3-4 | No | No | S |
| E3-6 | The FileCache: radix tree storage, FrameMeta page fields, the per-mount cache index, pages outlive close and munmap, zone LRU lists, the page-cache shrinker and the page daemon for clean unmapped pages | E3-1, E3-4; E1-9 | Yes | No | L |
| E3-7 | `read` through the cache for Native files: one source per byte, readahead on read, block cache out of the clean read path; `io` as a `SleepRwLock`; cached reads take no `MountLock` | E3-5, E3-6; E2.5, E2.6 | Yes | No | M |
| E3-8 | Dirty as a page flag: BreenFS dirty pages become cache frames, COW under writeback, mapped write-protect at freeze, drain from frames, written pages stay cached, per-file fsync shortcut and writeback errors | E3-7 | Yes | No | L |
| E3-9 | The RAM-relative dirty limit, background freezes, writer throttling, no inline commit, the pipelined drain | E3-8 | Yes | No | M |
| E3-10 | The block cache keeps metadata only: RAM/32, frame-backed buffers, no data entries | E3-7, E3-8 | Yes | No | S |
| E3-11 | Lazy mmap: no population, fills from faults, Filling waits, read-around, fault-around, poison refills | E3-6; E2.9 | Yes | Yes | M |
| E3-12 | Lazy exec: exec bindings stop populating | E3-3, E3-11 | Yes | Yes | S |
| E3-13 | Mapped-page reclaim: reverse-map unmapping, accessed-bit harvest, sleeping direct reclaim | E3-11 | Yes | Yes | M |
| E3-14 | The OOM policy | E3-13 | Yes | Yes | S |
| E3-15 | `read` and `write` copy straight between user memory and frames | E3-8; E2.9 | Yes | Yes | M |

**What lands before E1-9 and E2.9:** E3-1 to E3-5. Each keeps today's memory bounds: no structure grows
with RAM on the fixed heap, and no cache is retained beyond today's rules. They are the ones the audit's
"remove work, don't move it" test allows before the foundations (audit section 4, closing paragraph).

**Other prerequisites.** E3-4 builds the frame-vector type that E1 section 8 names (an ordered list of <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
leased frames with offsets), over today's frame leases, because no E1 PR before E1-9 builds it; E1's
allocator later backs it unchanged. E3-1 to E3-6 wait on no page channel, so none needs E2.2's `park`.
E3-7 needs E2.5 (fills release `MountLock` before the device wait) and E2.6 (`io` as a sleeping lock),
because it is the first PR whose readers wait on fills (7.3).

**What waits for E1-9:** E3-6 to E3-10, with E3-7 to E3-10 also after E2.5 and E2.6. Retaining file data, and raising the dirty limit, by RAM is
safe only once per-page metadata is in FrameMeta (E1-3), tree nodes come from slab caches (E1-6), the buddy
has zones and watermarks (E1-8), reclaim hooks exist (E1-4), and the fixed heap is gone (E1-9). E1-9 is the
last of those, so it is the gate.

**What waits for E2.9:** E3-11 to E3-15. Each needs faults that sleep and restart.

E3-1 to E3-5 have no order among themselves beyond the table, and can run in one lane in that order. E3-6
can start as soon as E1-9 merges, E3-7 once E3-6, E2.5 and E2.6 have merged, and E3-11 as soon as
E2.9 merges. If E2.9 lands before E1-9, E3-11 still
waits for E3-6 (it needs pages retained by the cache to be worth anything), so the order is fixed.

### E3-1: pure modules

- **Change.** These modules, each with no kernel dependencies, are added and not wired in:
  - `memory/page_tree.rs`: the radix tree of 4.3, with tags, node reservations and tagged iteration;
  - `memory/page_state.rs`: the transition function of 5.2;
  - `memory/readahead.rs`: 8.1's window;
  - `memory/lru.rs`: 12.1's two-list scan over an abstract page index;
  - `memory/dirty_limit.rs`: 11.4 and 11.5's arithmetic;
  - `memory/oom_score.rs`: 13's selection.
  A new crate, `tools/pagecache-host-tests`, includes them by `#[path]` and gets a line in
  `docs/host-tests.md`.
- **Tests (host).** These guard mechanisms the PR adds, so they cannot be red on the old tree, and the PR says
  so.
  - Tree: insert, look up and remove across height changes, at indices 0, 63, 64, 2^32 and 2^52 - 1.
  - Tree tags: set and clear propagate to the root, and tagged iteration visits exactly the tagged indices
    in order.
  - Tree reservations: a failed node reservation changes nothing. An allocation-failure sweep fails the Nth
    node for each N. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - State table: every (state, event) pair, legal and refused. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Readahead: every path's window is at most `max`, including the first read and a reset for a 1 GiB <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    request. Windows for a sequential stream, a random stream, two interleaved streams and a stream with
    a backward step, with fixed sequences.
  - LRU: a working set read twice survives a single pass over a stream 10 times the cache size, and a
    stream read once is evicted first. Fixed seeds.
  - OOM: victim order for given scores, with exclusions and ties.
- **Rows.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-2: explicit page states in today's mapping cache (R1 permanent, first half)

- **Change.**
  - `resolve_page` never inserts a zero frame for an absent index. A bound index below EOF with no entry
    is `Signal(SIGBUS)` and increments `absent_bound` (5.3). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - The EOF transition (`MapState::transition`) and `prepare_write` insert explicit `Hole` entries for the
    bound pages they make valid, holding the shared zero frame. A store into one allocates and revokes the
    zero frame's PTEs at that index (5.4). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - The module invariant at `file_map.rs:9-14` is rewritten: a bound page is resident, Hole, or poisoned.
  - `Runs` in `read_verified` reports the hole ranges it zero-fills, so E3-4's `fill` can return them as
    `Hole`. BreenFS `read` keeps its behaviour: since #3239 it zero-fills only the gaps between extents and the EOF tail.
    The merge of sources and the overlay go in E3-7.
- **Tests (host), check first.**
  - The #3208 interleaving, through a seam that lets the test release a population's claim before
    `bind`: populate, run `evict`, bind, fault index 0, then `read`. On the old code the fault maps 00
    bytes and the read returns 00 for the ELF header, which is red. On the new code the fault is SIGBUS
    with `absent_bound` = 1, and `read` returns the file's bytes.
  - Extension: `ftruncate` grows a mapped file, and a fault on the new page reads 00 through a `Hole` entry.
    This guards existing behaviour, so it is not red.
  - Aliases of a `Hole`: after the extension, process A maps the new page shared read-only and reads 00; <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    process B stores through a shared writable mapping. A's next read returns B's bytes. Today's fault
    inserts one fresh zeroed frame that both share, so this guards the revocation the PR adds with the
    shared zero frame, and is not red on the old tree; the PR says so.
  - `read_verified` over a file with two extents and a gap reports exactly the gap as a hole range, with
    00 bytes. This guards the report the PR adds, so it cannot be red on the old tree, and the PR says so.
- **Rows.** None. This is a correctness change. The series should show no move, and the PR says so if one
  appears. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** None. The host tests settle each effect. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-3: exec through the cache

- **Change.**
  - Kernel exec maps PT_LOADs as 10.1 describes, through exec bindings of the image's FileCache, populated
    eagerly (10.2). That covers shared read-only text and rodata, private COW data, the tail page and
    demand-zero BSS. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - `deny_write` and `writers` counts, ETXTBSY on write-open, truncate and shared writable mmap, and the
    copying fallback for an image open for writing (10.3).
  - `ICACHE_CLEAN` in the cache page (in today's `CachePage` record until E3-6 moves it to FrameMeta).
  - The fallback count.
- **Tests (host and kernel), check first.**
  - In `tools/vfs-host-tests`, through a seam that exposes the exec mapping plan: two execs of one image
    share the text frames, and the second makes 0 device reads. On the old code, each exec allocates
    private frames and reads, which is red.
  - A BSS of 1 MiB allocates no frame until touched. The old code allocates 256, which is red.
  - The tail page holds the file bytes, then 00s.
  - A store to data copies; a store to text faults with SIGSEGV.
  - `open(O_WRONLY)` of a running image is ETXTBSY. It succeeds on the old code, which is red.
  - Exec of a file open for writing takes the fallback and runs.
  - A non-congruent segment takes the fallback.
- **Rows.** `process.fork-exec` (BSS and copies), `process.rustc-first-output` and `compile.rustc-vv` (the
  rustc launcher's own image), `compile.cargo-noop` (every rustc probe). These are expected to move a
  little. The large parts of those rows are the driver's mmap, which waits for E3-6 and E3-11. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** One ARM64 QEMU boot of the boot-path stages. Every exec from PID 1 on now runs shared cache
  frames as text, and the instruction-cache maintenance for them (10.4) is a hardware effect that host tests
  cannot settle. Cases: the boot-path milestone stages, which exec init, the shell and the first programs. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-4: the fill interface

- **Change.**
  - `FileSystem::fill` and `cache_policy` (6.1). Population calls `fill` instead of `read_into`.
  - BreenFS's fill:
    - seeks the extent map;
    - classifies holes;
    - reads each physically contiguous run, up to the device's maximum transfer, into a bounce buffer
      the request owns (6.4);
    - verifies each block's CRC and copies the blocks into their frames (6.2-6.4).
  - The block cache is no longer used for population's data (section 14, first part).
  - ext2 and the others get the trait default.
- **Tests (host), check first.**
  - In `tools/breenfs-host-tests` with the recording device, populate a 1,000-page file of four extents.
    The new code issues one request per contiguous run, at most the device's transfer, with 0 data entries
    in the block cache. The old code issues one per 16 pages and fills the block cache with data entries,
    which is red on both counts.
  - A sparse file's hole pages come back `Hole`, with no device read.
  - A CRC failure in one block makes exactly that run's pages `Error(EIO)`, and the others `Data`.
  - A device that returns a short run gives `Error(EIO)` for the pages it did not reach, never 00. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Two fills of different files on one mount share a bounce pool of one buffer, as a fixed interleaving:
    fill A verifies its run and pauses before its copy, then fill B starts. B waits for A's exclusion
    before E2.5, and for A's token from E2.5, and A's frames hold file A's bytes. Today's staging is per call, so this guards the ownership the PR keeps
    and is not red on the old tree; the PR says so.
  - DEV-29(3) 0.1's second interleaving, through the production fill: pause the fill after its device read
    of page 0, run a production BreenFS write of page 0, then complete the fill. A fault on page 0 and a
    `read` both return the write's bytes, and the fill counts one discarded page. This check comes first in
    the PR: it is red if the old tree's population still merges a stale page. If today's mark already
    prevents that, it is not red on the old tree, and it guards that E3-4's fill keeps the mark; the PR
    says which.
  - A population of a file with an uncommitted write, and with a frozen generation's page, maps the written
    bytes: write page 0, freeze, write page 1, then `mmap` and read both through the mapping before the
    drain. Today's `read_into` reads through the mount state's layers, so this guards that the new `fill`
    keeps them; it is red only against an extents-only fill, and the PR says so.
- **Rows.** `memory.driver-mmap` and `memory.driver-map-touch` (fewer, larger requests and no block-cache
  churn), `compile.rustc-vv`, `process.rustc-first-output`, `compile.hello-object`.
- **Boot.** None. The recording device settles the request shapes, and the bounce path uses the existing
  block API. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-5: readahead for population and the hints

- **Change.**
  - 8.1 and 8.2 for the paths that exist before E3-7: population (mmap and exec) and `posix_fadvise` and
    `madvise` WILLNEED, which become asynchronous through the readahead worker.
  - `POSIX_FADV_DONTNEED` and `MADV_DONTNEED` on file mappings drop clean unbound pages.
- **Tests (host), check first.**
  - `posix_fadvise(WILLNEED)` over 1,024 pages followed by a populate: 0 synchronous device reads in the
    populate. The old code fills at most 16 pages, then reads the rest synchronously, which is red.
  - The worker's queue under a frame budget drops queued windows and never a demanded one. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - One cap: `WILLNEED` over 262,144 pages and a populate of a 1 GiB mapping each hold
    at most `max` pages in `Filling` at any step, and the budget counts the queued jobs and their node
    reservations. Today's WILLNEED stops at 16 pages, so this guards the windows the PR adds and is not
    red on the old tree; the PR says so. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - A reader reaching a page the worker claimed waits on it and gets its bytes.
  - A queued window dropped under the frame budget retires its claims: a populate that reached one of
    its pages wakes, fills the page itself and maps the file's bytes. The old tree has no worker, so this
    guards the mechanism the PR adds; the PR says so.
- **Rows.** `memory.driver-mmap` (if the loader's hints apply), `compile.hello-object`. These are expected
  to be small. The main readahead gains come with E3-7 and E3-11.
- **Boot.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-6: the FileCache (needs E1-9)

- **Change.**
  - `MapState`'s `PageVec` and `CachePage` give way to the radix tree and FrameMeta's page fields (4.3,
    4.4).
  - The per-mount cache index (4.2).
  - Eviction on unbind is removed (`Binding::drop`, `file_map.rs:506-534`, and the service's eviction
    pass). Pages outlive close
    and munmap.
  - The zone LRU lists, the page-cache shrinker in E1's registry, and the page daemon for clean unmapped
    pages (12.1-12.4).
  - `/proc/breenix/pagecache`.
- **Tests (host), check first.**
  - Map a 16 MiB fixture (4,096 pages) standing in for the driver, unmap, and map again with no pressure. The new code makes 0
    device reads on the second map. The old code evicts on unbind and re-reads every page, which is red.
    This is the bimodal `rustc -vV` mechanism of audit R5. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Close and reopen a file: its pages are still resident (red on the old code, which builds a new
    `MapState`).
  - Under a fixed frame budget, run a working set and a stream. Reclaim frees stream pages first and never <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    a bound, dirty, `Filling` or `Writeback` page.
  - Eager mapping before its first touch: populate a mapping, run the production shrinker and the page
    daemon under a frame budget, then fault every page. Each maps the file's bytes and `absent_bound`
    stays 0. The same holds for a page an `mlock`ed mapping covers. A third case holds the LRU lock and
    runs direct reclaim, which returns no progress without waiting. These guard the reclaim the PR adds,
    which the old tree does not have, so they are not red there; the PR says so. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - A FileCache dropped while reclaim holds one of its pages isolated is freed only after the isolation
    ends.
  - Unlink, then the last close, drops every page. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - An observer mount adopting a newer generation drops its clean pages.
  - The shrinker uses `try_lock` only, and its work is bounded by its target plus one batch (E1 7.3's two
    rules).
- **Rows.** `compile.rustc-vv`, `process.rustc-first-output`, `compile.hello-object`, `compile.cargo-noop`,
  `compile.yes-dev-box`, `compile.yes-release` (driver and toolchain residency across runs), and
  `memory.driver-mmap` and `memory.driver-map-touch` (resident on the second and later samples).
- **Boot.** None. The host tests settle residency, reclaim order and lifetime. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-7: read through the cache (needs E1-9)

- **Change.**
  - Native `read` per 7.1, with readahead (8), under `io` as a `SleepRwLock` (7.3). A cached read takes no
    `MountLock`.
  - The overlay (`file_map.rs:827-856`) and the zero prefill in BreenFS `read` go. Each byte comes from one <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    source, which is R1's read side. The layered lookup moves into the fill (6.2) and stays there until
    E3-8, so a page whose newest bytes are in BreenFS's open or frozen dirty pages fills from them, not from
    the extents. BreenFS writes keep updating resident cache pages through today's mutation hooks
    (`file_map.rs:557-561`), so a page read into the cache before a write reads the write's bytes after
    it.
  - `DataReader`'s clean-data path goes (section 14).
  - The lock-order structure test of section 15.
- **Tests (host), check first.**
  - A read of a fully cached file takes no `MountLock`: a counter in `tools/breenfs-host-tests`. The old <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    code takes it once per call, which is red.
  - Readers on one file run concurrently with a writer on another file, as a fixed interleaving.
  - The structure test pins section 15's order.
  - A reader waiting on the readahead worker's `Filling` page while it holds `io`, as a fixed
    interleaving: the worker's fill takes `MountLock` for its snapshot and completes, and the reader
    copies its bytes. A variant holds `MountLock` in a third thread until the reader is parked. Red if `io`
    were the foreground exclusion, which is how the old tree's `read` serializes; the PR says which.
  - Dirty layers before E3-8, through the production fill: write page 0 (open generation), freeze, write
    page 1 (G+1), then, before the commit drains, `read` both pages and populate a mapping of them. Both
    return the written bytes. A variant reads page 0 first, writes it, and reads it again. Red if the fill
    reads only the extents, which return the older bytes or 00; on the old tree today's `read` uses the
    layers, so the check is red only against the new path, and the PR says so.
  - Read a 16 MiB file twice with no pressure. The second pass makes 0 device reads. The old code serves it
    from an 8 MiB block cache and re-reads at least half, which is red.
  - A sequential loop of 4 KiB reads over 4 MiB issues at most 4 MiB / `max` + log2(`max` / 16 KiB) + 1
    device requests. A random loop issues one per read.
  - A read concurrent with a two-page write, as a fixed interleaving through the exclusion seam, sees all
    of the write or none of it (POSIX 2.9.7). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - After a store through a shared mapping, `read` returns the stored bytes from the one frame. That is the
    overlay's job today, so this guards existing behaviour and is not red.
  - A read where one fill fails returns the prefix before the failed page, then EIO on the next call.
- **Rows.** `disk.sequential-read` (partly: until E3-8 the written data is not cached, so this PR's gain is
  readahead), `compile.cargo-noop`, `compile.yes-dev-box` and `compile.yes-release` (source and rlib reads,
  and concurrent jobs no longer serialize on cached reads).
- **Boot.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-8: dirty as a page flag (needs E1-9)

- **Change.**
  - 11.1-11.3 and 11.6: BreenFS's `DirtyPages` give way to cache frames with `DIRTY` and `dirty_gen`.
  - The freeze marks `WRITEBACK`. A write into a `Writeback` page copies. Shared writable pages are
    write-protected and bounced at the freeze.
  - The drain reads frames and computes CRCs once. Pages stay cached after install.
  - The per-file fsync shortcut and the writeback error sequence.
  - This PR keeps today's `DIRTY_BUDGET` admission, so it changes where dirty bytes live, not how many.
- **Tests (host, `tools/breenfs-host-tests` with the recording device and the crash-image harness), check
  first.**
  - Write 64 MiB, fsync, and read it back: 0 device reads. The old code drops the dirty pages at install
    and reads every block back, which is red. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Reclaim in the replay window, as a fixed interleaving: write page 5 in G, freeze G, write another file
    in G+1 so a replay is pending, install G, then run the production reclaim pass before `materialize`
    completes, and read page 5. The read returns G's bytes with 0 device reads, and page 5 is still
    `Writeback`. Today's installed pages are not in a reclaimable cache, so this guards the rule the PR adds
    and the PR says so.
  - Stable pages: freeze G, then write the same page in G+1 during G's drain. For every subset of G's
    writes, the crash images recover G-1 or G, and G's copy of the page is G's bytes. The cache holds
    G+1's bytes. The old code holds this through immutable `Arc` pages, so the check is red only on a seam
    variant that writes in place, which the PR states. The harness keeps it from regressing. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - A shrink during G's drain whose new last page is in G's batch: pause the recording device after G's
    CRC for that page, `ftruncate` to the middle of the page, then complete G. G's block and CRC match a
    cold read of the image, and the cache's page reads the kept bytes, then 00s. The old code zeroes no
    frozen frame (its frozen pages are immutable `Arc`s), so this guards the rule the PR adds and the PR
    says so.
  - A shared mapped page stored after a freeze faults once, is redirtied in G+1, and G's image has the
    bytes from before the store.
  - Aliases of a `Writeback` page: a process maps page 0 shared read-only and reads it; freeze G; `write()`
    page 0 in G+1 during G's drain. The mapping's next read returns G+1's bytes, and G's image holds G's.
    Today the mapping cache page is updated in place by the mutation hooks, so this guards the revocation
    the PR adds and is not red on the old tree; the PR says so.
  - Mapped admission, as a fixed interleaving: a store fault on a clean page of a file no `write()` has
    touched pauses after its lookup; G's freeze and install run; the fault completes; then G+1 freezes and
    installs. G+1's image holds the stored bytes, G+1's changed set holds the inode, and the space
    reservation is charged once. A variant fills the volume first: admission returns `ENOSPC` through
    `fsync`, and the page keeps its bytes. Today's `writeback_pages` admits mapped pages already, so this
    guards the admission the PR moves and is not red on the old tree; the PR says so.
  - A page that is never stored again after a freeze is not written in G+1 (red on the old code, which
    copies every mapped dirty page each time). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - A failed write keeps earlier writes. Write page 0, then write pages 0 and 1 with an allocation-failure
    seam that fails page 1's frame. The second write returns `ENOMEM`, and a read of page 0 returns the
    first write's bytes, with 0 device reads. Today's dirty pages are not in the cache, so this guards the
    prepare-then-publish rule the PR adds and cannot be red on the old tree; the PR says so. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Fsync of a clean file whose last change is durable joins no generation (red: the old code joins one).
  - Two descriptions of a file whose generation fails: each sees EIO once from fsync, and a description
    opened after the failure sees none. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - The cache invariant of 6.2 holds after every step of the POSIX model's random operation sequences
    (fixed seeds). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Rows.** `disk.sequential-read` (expected to approach parity: the 1 GiB just written stays cached),
  `disk.sequential-write` (fewer copies), `disk.fsync` (watch: the shortcut does not apply to the row's
  dirty append, and it should not regress).
- **Boot.** None. The crash-image harness settles ordering, and the host model settles the fault and the
  write-protect. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-9: the dirty limit, throttling and the pipelined drain (needs E1-9)

- **Change.**
  - 11.4 and 11.5: `DIRTY_BUDGET` and `DIRTY_LIMIT` are removed, and the global `limit` and `background`
    apply.
  - Background freezes. Writers charge before dirtying, throttle and never commit inline. Parts join the
    open generation with a hold (11.5a). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - 11.3's overlapped drain.
  - A latched mount's pages leave the dirty count.
- **Tests (host), check first.**
  - With RAM fixed in the model at 2048 MiB, a 1 GiB sequential write makes at most 8 publications. The old
    code makes at least 256, which is red.
  - A writer above `limit` parks until the drain cleans its pages, and the writer never calls
    `commit_generation`. That is a structure test plus a fixed interleaving. On the old code the writer
    commits inline, which is red. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - Two writers, one heavy and one light: the light writer's waits are bounded by its own pages.
  - Four writers of 40 pages each under a 100-page `limit` with 30 frozen pages, as a fixed interleaving: <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    the charged count never passes `limit`, and it includes the frozen frames and the bounce copies.
    Today's admission checks its fixed budget before each part, so this guards that E3-9 keeps the check
    under the RAM-relative limit and counts frozen and bounce frames; it is not red on the old tree, and
    the PR says so.
  - A two-part `O_APPEND` write and a second appender: the second's bytes follow all of the first's. A <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
    freeze requested during the first part's copy waits for its hold, and a crash image taken after that
    generation recovers the first part whole and none of the second. This guards today's split rule, so
    it is not red; the PR says so.
  - The drain submits run k+1 before waiting for run k, as recorded by the recording device's submission
    order. The old code waits each run, which is red.
  - A latched mount does not throttle a writer to another mount.
- **Rows.** `disk.sequential-write` (expected to move most), `disk.sequential-read`, `compile.yes-dev-box`
  and `compile.yes-release` (object and rlib writes), `disk.create` (watch).
- **Boot.** None. Throttling is decided by counts in the model, and the rig series measures the effect. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-10: the block cache keeps metadata only (needs E1-9)

- **Change.** Section 14: `bound_for_memory` = RAM/32 with the 4 MiB floor and no maximum. Frame-backed
  buffers. No Native data entries, and the step that retires data entries in preparation is removed.
- **Tests (host), check first.**
  - After reading and writing 64 MiB of file data, the block cache holds 0 data entries. The old code holds
    data entries up to its budget, which is red.
  - On a 4 GiB model, the bound is 128 MiB. On the old code it is 8 MiB, which is red.
  - The block-cache shrinker returns frames from clean nodes under `try_lock`.
- **Rows.** `disk.stat`, `disk.walk` and `compile.cargo-noop` (a larger metadata cache). Watch rows; small
  moves are expected.
- **Boot.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-11: lazy mmap (needs E2.9)

- **Change.**
  - 9.1-9.4: `map_file` stops populating.
  - File faults fill through `fill` on E2.9's protocol, wait on `Filling` pages through hashed channels
    (4.5), read around a miss, and fault around a hit.
  - Poisoned pages refill instead of SIGBUS.
  - 5.3's `absent_bound` refusal becomes a fill, and the pre-E2.9 Hole insertion at extension is removed.
- **Tests (host, through E2.9's `resolve_fault` seam in `tools/pagetable-host-tests`), check first.**
  - A fault on an absent page below EOF fills it and maps the file's bytes. The old code returns SIGBUS
    (E3-2), which is red.
  - `mmap` of the driver fixture makes 0 device reads (red: the old code populates).
  - Two faults on one page make one fill, and the second waiter maps it.
  - munmap while a fill sleeps: no entry is installed, and the frame stays cached.
  - Truncate past the page while a fill sleeps: the retried fault is SIGBUS, and no stale page remains.
  - A fill error gives SIGBUS to its waiters, even when another access claims a new fill of the page
    before the waiter runs (a fixed interleaving). The next fault fills again and succeeds. On the old
    tree faults do not wait, so this guards the outcome rule the PR adds; the PR says so.
  - Fault-around maps only uptodate neighbours inside the VMA and the page table, read-only.
  - A shared writable VMA's neighbour faults on its first store and becomes dirty.
  - An atomic-context user copy over an absent page takes the EFAULT fixup and claims nothing. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Rows.** `memory.driver-mmap` (expected to approach parity: the map creates a VMA), `memory.driver-touch`
  (expected to rise on a cold first sample and fall with fault-around after it), `memory.driver-map-touch`,
  `compile.rustc-vv`, `process.rustc-first-output`, `compile.hello-object`, `compile.cargo-noop`,
  `compile.yes-dev-box`, `compile.yes-release`.
- **Boot.** One ARM64 QEMU boot of the boot-path stages. From the first `ld-breenix` mapping on, every
  library and parse view faults in lazily, through real exception entry, a real virtio-blk completion and a
  real sleep and restart. The instruction-cache maintenance for lazily filled executable library pages is a
  hardware effect. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-12: lazy exec (needs E2.9)

- **Change.** Exec bindings stop populating (10.2). The tail page and BSS stay as E3-3 built them.
- **Tests (host), check first.**
  - Exec of an image whose text is 10 MiB makes device reads only for the pages its start touches, plus
    their read-around windows. The old code (E3-3) populates every page, which is red. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - A data page stored before its first read copies the file bytes, not 00.
- **Rows.** `process.fork-exec`, `process.rustc-first-output`, `compile.cargo-noop`.
- **Boot.** One ARM64 QEMU boot of the boot-path stages. Init's own text now faults in from the cache, so
  the first instruction fetch of every program depends on the lazy fill and its instruction-cache
  maintenance, which is a hardware effect. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-13: mapped-page reclaim (needs E2.9)

- **Change.**
  - 12.2's mapped column: reverse-map unmapping through the binding interval tree with `LeafBatch` flushes
    (12.5).
  - Accessed-bit harvesting for the inactive tail.
  - Sleeping direct reclaim: wake the daemon and park on its progress (12.4).
- **Tests (host), check first.**
  - Under a fixed frame budget, a cold mapped clean page is unmapped, flushed and freed, and its next fault
    refills it. On the old code mapped pages are never reclaimed, and the allocation fails, which is red. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - A hot mapped page, whose accessed bit is set, outlives a cold unmapped page.
  - The frame is freed only after the flush (the existing flush-before-free model).
  - A private binding whose PTE maps its own copy-on-write copy keeps that PTE and its bytes when reclaim
    takes the cache page: store through a `MAP_PRIVATE` mapping, reclaim the cache page, then read the
    store back. A second fixed interleaving pauses a fault between its lookup and its install while
    reclaim clears the PTEs: the fault sees the entry gone and refills, and no PTE maps the freed frame.
    Mapped pages are not reclaimed on the old tree, so both guard the mechanism the PR adds; the PR says so.
  - A sleepable allocation under pressure parks and succeeds after the daemon frees pages. An atomic one
    fails without parking.
- **Rows.** No row is expected to move on the 2048 MiB rig, where the compile rows do not run out of
  memory. The series checks that none regresses. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-14: the OOM policy (needs E2.9)

- **Change.** Section 13.
- **Tests (host), check first.**
  - Three address spaces of 10, 200 and 50 MiB private memory, and an allocation that reclaim cannot satisfy
    from the 10 MiB one. The 200 MiB address space is killed and the allocation then succeeds. The old code
    kills the faulting process, which is red.
  - PID 1 is never chosen, even when it is largest. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - No second victim is chosen while the first is still exiting.
  - CLONE_VM siblings are scored and killed together.
- **Rows.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** None. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### E3-15: direct copies (needs E2.9)

- **Change.**
  - 7.4: `read` and `write` copy straight between user memory and frames under it, and the 64 KiB staging and
    the write copy-in go.
- **Tests (host), check first.**
  - A 1 MiB read of a cached file allocates no staging: a staging-allocation counter reads 0. The old
    code stages each 64 KiB chunk, which is red. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
  - A read whose user buffer is absent parks in the fault and completes with no deadlock against `io`.
  - A write whose source is a mapping of the same file and page completes.
- **Rows.** `disk.sequential-read` (one copy instead of two).
- **Boot.** None. E2.9's boot settled the sleeping user copy's exception return. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

## 19. Tests

### 19.1 Where they live

- The pure modules: `tools/pagecache-host-tests` (new, E3-1).
- The mapping cache, population, faults and exec mapping plans: `tools/vfs-host-tests`, and
  `tools/pagetable-host-tests` through E2.9's `resolve_fault`.
- BreenFS fill, dirty pages, freeze, drain and crash images: `tools/breenfs-host-tests`, with its recording
  device, crash-image harness and POSIX model (`docs/fs/breenfs-design.md` section 5).
- Lock order and the absence of inline commits: structure tests under `tests/`.

### 19.2 Determinism and shape

- Random workloads use fixed seeds printed in the test name. Interleavings are fixed sequences of named
  steps driven through seams. Multi-CPU cases simulate CPUs as parameters. There are no threads, no
  timing and no retries.
- Each test checks behaviour that could plausibly break: zero data from absent pages, residency, reclaim
  order, stable pages, crash images, throttling and lock order. No test checks a constant or a struct's
  size. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
- Check first: each PR's first commit is its tests, red on the code before it as section 18 names. Where
  the old code has no seam, the check commit adds one with no behaviour change, so the red result is the
  old behaviour's, not a stub's (E2 section 8).
- The existing structure tests that pin names E3 changes are updated in the PR that renames them, and are
  never weakened: `tests/file_map_binding_gate_structure.rs`, `tests/fs_backend_boundary_structure.rs`, and
  the `mapping_support.rs` extraction in `tools/breenfs-host-tests`. A Python test that pins the old
  eviction-on-unbind behaviour (`tests/file_map_orphan_cache_test.py`) is rewritten in E3-6 to test the
  same property of the new structure: an orphan's pages go once the last handle closes. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

### 19.3 Boots

Three PRs need one ARM64 QEMU boot each, for the hardware effect each names: E3-3, E3-11 and E3-12. Each
boots the repository's standard ARM64 boot script, once, at the PR's final head, for the boot-path stages
only. No PR runs an effort's suites (DEV-30).

### 19.4 Design models

Each model below reduces one design rule to its interleaving or state, so a review can see it hold. `RULE`
names the rule the design chooses. They run with `python3 -m doctest docs/plans/e3-page-cache.md`. They are
not implementation tests: section 18 names the production check for each.

**A failed write keeps earlier writes (7.2).** Page 0 holds an acknowledged write that is not yet durable.
A later write changes page 0, then needs a frame for page 1, and that allocation fails.

```pycon
>>> RULE = 'prepare-then-publish'
>>> def write(cache, disk, data, frames_free):
...     if RULE == 'prepare-then-publish' and frames_free < 1:
...         return 'ENOMEM'
...     cache[0] = data + cache.get(0, disk[0])[len(data):]
...     if frames_free < 1:
...         del cache[0]
...         return 'ENOMEM'
...     return 'ok'
>>> cache, disk = {0: b'AAAA'}, {0: b'....'}
>>> write(cache, disk, b'BB', frames_free=0)
'ENOMEM'
>>> cache.get(0, disk[0])
b'AAAA'

```

**Truncation leaves a frozen page alone (7.5).** The new last page is a `Writeback` page: G's batch holds
its frame, and G's CRC is computed from it. A shrink then zeroes the page's tail.

```pycon
>>> RULE = 'copy-under-writeback'
>>> frozen = bytearray(b'GGGGGGGG')
>>> crc_input = bytes(frozen)
>>> def shrink_tail(entry, keep):
...     if RULE == 'copy-under-writeback':
...         entry = bytearray(entry)
...     entry[keep:] = bytes(len(entry) - keep)
...     return entry
>>> entry = shrink_tail(frozen, 4)
>>> (bytes(frozen) == crc_input, bytes(entry))
(True, b'GGGG\x00\x00\x00\x00')

```

**A mapped store reaches exactly one commit (11.2).** A store fault on a shared writable page begins while
G = 7 is open, the freeze of G runs, then the fault finishes. Two freezes follow; the store's page must be
in one of their batches, with its space reserved and its inode in that generation's changed set.

```pycon
>>> RULE = 'admit-before-freeze'
>>> mount = {'open': 7, 'changed': set(), 'reserved': 0}
>>> page = {'dirty_gen': 0, 'mapped_dirty': False}
>>> def fault_begin():
...     return mount['open'] if RULE == 'fault-assigns-generation' else None
>>> def fault_end(seen):
...     if seen is None:
...         page['mapped_dirty'] = True
...     else:
...         page['dirty_gen'] = seen
>>> def freeze():
...     g = mount['open']
...     if RULE == 'admit-before-freeze' and page['mapped_dirty']:
...         mount['reserved'] += 1
...         mount['changed'].add('ino')
...         page.update(dirty_gen=g, mapped_dirty=False)
...     batch = [page] if page['dirty_gen'] == g and 'ino' in mount['changed'] else []
...     mount['open'] = g + 1
...     return batch
>>> seen = fault_begin()
>>> first = freeze()
>>> fault_end(seen)
>>> second = freeze()
>>> (len(first), len(second), mount['reserved'])
(0, 1, 1)

```

**Pages stay until the open view has G's extents (11.2).** G installs, but until N+1's replay completes the
open view is still the scratch view, whose extent for page 5 is G-1's (`fs/breenfs/vfs.rs:1159-1166`).
Reclaim runs on page 5 in that interval, and a read refills it from the open view.

```pycon
>>> RULE = 'clean-at-adoption'
>>> open_view = {5: 'G-1'}
>>> page = {'bytes': 'G', 'writeback': True}
>>> def install():
...     if RULE == 'clean-at-install':
...         page['writeback'] = False
>>> def materialized():
...     open_view[5] = 'G'
...     page['writeback'] = False
>>> def reclaim_then_read():
...     return page['bytes'] if page['writeback'] else open_view[5]
>>> install()
>>> reclaim_then_read()
'G'
>>> materialized()
>>> reclaim_then_read()
'G'

```

**Reverse-map reclaim clears only the cache frame's PTEs (12.5).** Two private bindings cover page i of
cache frame 100. Process B stored to the page, so its PTE now maps B's private copy, frame 200.

```pycon
>>> RULE = 'exact-frame'
>>> ptes = {'A': 100, 'B': 200}
>>> def reclaim(ptes, pfn):
...     for vma, frame in list(ptes.items()):
...         if RULE == 'clear-covering-pte' or frame == pfn:
...             del ptes[vma]
>>> reclaim(ptes, 100)
>>> ptes
{'B': 200}

```

**A bounce buffer belongs to one request (6.4).** Fill A reads and verifies file A's run into its bounce
buffer, then sleeps before copying. Fill B runs on the same CPU and reads file B's run.

```pycon
>>> RULE = 'owned-token'
>>> buffers = {}
>>> def acquire(request):
...     key = 'cpu0' if RULE == 'per-cpu' else request
...     buffers[key] = None
...     return key
>>> a = acquire('fill-a')
>>> buffers[a] = 'file A, CRC verified'
>>> b = acquire('fill-b')
>>> buffers[b] = 'file B, CRC verified'
>>> buffers[a]
'file A, CRC verified'

```

**Before E3-8, a fill reads BreenFS's dirty layers first (6.2).** Page 0 was written in the open
generation, and its bytes live only in BreenFS's `DirtyPages` map. A population or a read then fills it.

```pycon
>>> RULE = 'dirty-layers-first'
>>> extents = {0: b'disk'}
>>> open_dirty, frozen_dirty = {0: b'open'}, {1: b'froz'}
>>> def fill(i):
...     if RULE == 'dirty-layers-first':
...         for layer in (open_dirty, frozen_dirty):
...             if i in layer:
...                 return layer[i]
...     return extents.get(i, bytes(4))
>>> (fill(0), fill(1), fill(2))
(b'open', b'froz', b'\x00\x00\x00\x00')

```

**A fill publishes only what is still the file's bytes (6.2).** A fill claims page 0 and reads its extent.
Before it completes, a write changes page 0 (DEV-29(3) 0.1's second interleaving).

```pycon
>>> RULE = 'revalidate-file-snapshot'
>>> claim = {'stale': False}
>>> def write_during_fill():
...     claim['stale'] = True
>>> def complete(snapshot):
...     if RULE == 'revalidate-file-snapshot' and claim['stale']:
...         return 'discarded; page Absent; waiters retry'
...     return 'published ' + snapshot
>>> write_during_fill()
>>> complete('old bytes')
'discarded; page Absent; waiters retry'

```

**Replacing a page's frame revokes the old frame's PTEs (7.2).** Another process maps a `Hole` page through <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
the shared zero frame. A `write()` then gives the page a frame of its own.

```pycon
>>> RULE = 'replace-and-revoke'
>>> ZERO = 0
>>> tree, ptes = {4: ZERO}, {'reader': ZERO}
>>> frames = {ZERO: b'\x00\x00', 9: b'hi'}
>>> def write_hole(index, pfn):
...     old = tree[index]
...     tree[index] = pfn
...     if RULE == 'replace-and-revoke':
...         for who, frame in list(ptes.items()):
...             if frame == old:
...                 del ptes[who]
>>> write_hole(4, 9)
>>> frames[ptes.get('reader', tree[4])]
b'hi'

```

**A waiter gets the outcome of the fill it waited on (9.2a).** A fault waits on page 0's fill, which fails.
Before the waiter runs, a later access claims a new fill of page 0. Separately, the readahead worker drops a
queued window whose pages it had claimed.

```pycon
>>> RULE = 'frame-outcome'
>>> tree = {}
>>> waited = {'state': 'filling'}
>>> tree[0] = waited
>>> def fill_fails(frame):
...     frame.update(state='failed', errno='EIO')
...     tree[0] = ('Error', 'EIO') if RULE == 'error-entry' else None
>>> fill_fails(waited)
>>> tree[0] = {'state': 'filling'}
>>> def waiter_resumes(frame):
...     if RULE == 'frame-outcome':
...         return frame.get('errno', 'retry')
...     entry = tree[0]
...     return entry[1] if isinstance(entry, tuple) else 'retry'
>>> waiter_resumes(waited)
'EIO'
>>> queued = {'state': 'filling'}
>>> def drop_window(frame):
...     if RULE == 'frame-outcome':
...         frame['state'] = 'canceled'
>>> drop_window(queued)
>>> queued['state']
'canceled'

```

**Every readahead window obeys one cap (8.1).** `max` is 512 pages (2 MiB). A 1 GiB `read` at offset 0 <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
(262,144 pages), a random 1 GiB read and a 1 GiB `WILLNEED` each ask for a window.

```pycon
>>> RULE = 'uniform-cap'
>>> MAX = 512
>>> def window(kind, request):
...     raw = {'first': 2 * request, 'reset': -(-request // 4) * 4, 'willneed': request}[kind]
...     return min(raw, MAX) if RULE == 'uniform-cap' else raw
>>> [window(k, 262144) for k in ('first', 'reset', 'willneed')]
[512, 512, 512]

```

**Before E3-11, reclaim keeps every page a binding or a population covers (12.2).** An eagerly populated <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
mapping has not touched page 0 yet, so its PTE count is 0. Faults cannot fill before E3-11, and an mlocked
page must stay resident in any case.

```pycon
>>> RULE = 'covered-until-e3-11'
>>> page = {'leaf_refs': 0, 'covered': True, 'mlocked': False}
>>> def reclaimable(p, faults_fill):
...     if RULE == 'leaf-refs-zero':
...         return p['leaf_refs'] == 0
...     if p['mlocked'] or p['leaf_refs']:
...         return False
...     return faults_fill or not p['covered']
>>> (reclaimable(page, faults_fill=False), reclaimable(page, faults_fill=True))
(False, True)
>>> reclaimable(dict(page, mlocked=True), faults_fill=True)
False

```

**No fill waits for a lock its waiter holds (15).** Reader R holds `io`, which before E2.6 is the
foreground exclusion, and reaches page p. The readahead worker W claimed p when it queued the window, and its
fill needs the exclusion to take its snapshot. The model lists who waits for whom.

```pycon
>>> RULE = 'one-hold-per-fill'
>>> def waits():
...     if RULE == 'claim-at-queue':
...         return {'R': 'W', 'W': 'R'}
...     return {'R': 'W'}
>>> def deadlocked(edges, node='R'):
...     seen = set()
...     while node in edges:
...         if node in seen:
...             return True
...         seen.add(node)
...         node = edges[node]
...     return False
>>> deadlocked(waits())
False

```

Under the chosen rule, before E2.5 the worker claims p only while it holds the exclusion, from the claim <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
to the publication, so R does not meet W's claim. From E3-7, `io` is its own sleeping lock (E2.6), a fill
needs only a brief `MountLock` hold for its snapshot (E2.5), and W does not take `io`: R may wait on W,
and W waits for nothing R holds.

**Writers cannot overshoot the dirty limit (11.5).** `limit` is 100 pages. G's frozen frames and bounce
copies hold 30. Four writers each dirty 40 pages at once, before any of them reaches the throttle. A writer
that parks resumes after writeback has cleaned as many pages as it needs.

```pycon
>>> RULE = 'charge-before-dirtying'
>>> LIMIT = 100
>>> def peak(writers, each, frozen):
...     charged = frozen if RULE == 'charge-before-dirtying' else 0
...     top = charged
...     for _ in range(writers):
...         if RULE == 'charge-before-dirtying':
...             while charged + each > LIMIT:
...                 charged -= each
...         charged += each
...         top = max(top, charged)
...     return top if RULE == 'charge-before-dirtying' else top + frozen
>>> peak(writers=4, each=40, frozen=30) <= LIMIT
True

```

**Before E4 the drain does not overlap device writes (11.3).** `BlockCache::submit` drives the device
writes and returns a completed `Batch` (`block/cache.rs:94-105`, `:803-807`). The model records the order in
which the drain submits runs and the device finishes them, and compares it with what the design claims.

```pycon
>>> RULE = 'overlap-before-e4'
>>> def drain(runs):
...     trace = []
...     for k in runs:
...         trace.append(('submit', k))
...         trace.append(('device-done', k))
...     return trace
>>> t = drain([0, 1])
>>> overlapped = t.index(('submit', 1)) < t.index(('device-done', 0))
>>> overlapped == (RULE == 'overlap-before-e4')
True

```

## 20. bbench rows (DEV-31)

| Row | Ratio (28668b28) | Expected to move in | Parity reachable from E3 alone |
|---|---|---|---|
| `memory.driver-mmap` | 269,220 | E3-4, E3-6 (warm), E3-11 | Yes. After E3-11 mmap only creates the VMA |
| `memory.driver-map-touch` | 156.5 | E3-4, E3-6, E3-11 | Close. What remains is the fault cost, which E2.8 and E9 own |
| `memory.driver-touch` | 3.01 | E3-11 (fault-around) | Close. The rest is the anonymous and PTE fault cost (E2.8) |
| `compile.rustc-vv` | 70.9 | E3-3, E3-4, E3-6, E3-11 | No. The libc allocator (E7), spawn (E8) and the scheduler (E9) share it |
| `process.rustc-first-output` | 83.4 | as `compile.rustc-vv` | No, as above |
| `compile.hello-object` | 45.5 | E3-4, E3-6, E3-7, E3-11 | No, as above |
| `compile.cargo-noop` | 67.6 | E3-3, E3-6, E3-7, E3-11 | No. Metadata (R24, E5) is a large part |
| `compile.yes-dev-box` | 34.0 | E3-6, E3-7, E3-9, E3-11 | No. Compile CPU and the allocator share it |
| `compile.yes-release` | 15.0 | as `compile.yes-dev-box` | No |
| `disk.sequential-read` | 89.5 | E3-7, E3-8, E3-15 | Yes. The just-written GiB stays cached, and the row becomes a copy |
| `disk.sequential-write` | 28.0 | E3-8, E3-9 | Partly. The rest is the device path (E4) and copies (E3-15) |
| `disk.fsync` | 8.34 | Watch (E3-8, E3-9) | No. Whole-filesystem fsync is R20 (E11) |
| `process.fork-exec` | 10.93 (6d95fac4) | E3-3, E3-12 | No. Fork's address-space copy is E8 |
| `disk.stat`, `disk.walk`, `disk.create`, `disk.unlink` | 81, 6.1, 33, 49 | Watch (E3-10) | No. Metadata (E5, R24) |
| `memory.copy-bytes`, `process.getpid`, `ssh.*` | 0.99, 4.5, about 1 | Controls | Must not move |

The predictions are directions, not numbers. They come from reading the code, and the before-and-after
series decides. A PR that predicts a move and does not show one says so in its body. That does not fail the
PR, but it reopens the attribution, as E1 and E2 also rule (E1 section 13, E2 section 10).

- **Ordering effects to expect.**
  - E3-11 makes `memory.driver-mmap` fall by orders of magnitude and moves the I/O into `memory.driver-touch`
    on a cold first sample. The median over 5 samples sees resident pages from the second sample on, so the
    row's movement depends on E3-6's retention, which is why E3-6 comes first.
  - E3-8 should move `disk.sequential-read` far more than E3-7. The row reads data that was just written.
- **The disk rows on the rig** are BreenFS on virtio-blk at queue depth one (E4). The E3 gains in them are
  cache and commit-count gains. Device gains wait for E4.
- **On the R240,** the coordinator's batched kernel-only flash after E3-9 is followed by one R240 series
  (DEV-31(2)), within DEV-23's limits on large writes and walks while #3226 is open. AHCI's depth one (R8)
  limits what the disk rows can show there until E4.

## 21. Risks and questions for the coordinator

1. **Q1: ETXTBSY (10.3).** Lazily mapped shared text needs running images not to change. The design refuses
   write-opens, truncation and shared writable mappings of a running image with ETXTBSY, and lets exec of a
   file open for writing fall back to today's copy instead of failing.
   - This is visible to userland. A program that rewrites a running binary in place, rather than writing a
     new file and renaming it, will now get ETXTBSY.
   - Recommendation: adopt it. POSIX allows it, it is the documented Linux behaviour, and the build tools
     on the box replace outputs rather than rewriting them in place. That last point is inferred, not
     measured; E3-3's boot and the program's final run would show a refusal.
   - The alternative keeps exec copying (no shared text, R6's exec half not closed).
2. **Q2: an OOM bias.** E3 has no per-process adjustment, so `bsshd` can be chosen if it is largest. It is
   rarely largest. Recommendation: no knob until a measured case needs one.
3. **Q3: ext2 stays MappedOnly (6.5).** ext2 reads and writes keep going through its own path and the
   block cache. Nothing on the rig's root uses ext2 since #3250. Recommendation: keep it so, and do not
   spend E3 work on ext2. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
4. **Q4: fsync latency against the dirty limit.** BreenFS fsync commits the whole filesystem (R20). A larger
   dirty set makes an fsync that lands behind a big writer wait for all of it, up to `limit`. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
   - The background threshold and the 5 s interval keep the dirty set well below `limit` in steady state,
     so the typical wait is bounded by `background`.
   - E11's log tree is the real fix.
   - If the series shows `disk.fsync`, or an interactive fsync, regressing after E3-9, lower `background`
     first.
5. **Q5: the boots.** Three boots (19.3), each for a hardware effect that host tests cannot settle. They are
   inside DEV-30's exception; the coordinator may batch them with E1's or E2's boots if those land on the same
   head.
6. **Q6: FrameMeta at 64 bytes** (1.56% of RAM) is E1 5.7's reservation. E3-6 grows it. If E1-3 lands at 32
   bytes, E3-6's builder changes one constant. That is a detail under DEV-39(2), not an architecture change,
   because E1 already derives the layout from `size_of::<FrameMeta>()`.
7. **Q7: E4 and E5 seams.** `fill` and the drain take frame vectors so that E4 serves them without
   interface change (6.4). The `io` lock is VFS-level. E5's per-inode BreenFS locks sit below it in the
   order (section 15). If E5's design needs a different position for its locks relative to `io`, that is
   an architecture conflict and goes to the coordinator.
8. **Q8: x86 evidence.** As in E1 and E2, x86 is checked only by the coordinator's batched R240 flash
   (DEV-31(2)). E3's x86-specific surface is small: the fault path's file branch and the instruction-cache
   rule, which needs nothing on x86 (10.4). <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
9. **No format change.** No E3 step changes BreenFS's on-disk format, and none touches `compiler/` or
   `src/llvm-project`. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->

## 22. Deviations recorded by builders (DEV-39(2))

Each entry names the PR, the detail, the resolution chosen, and one sentence of reason. Architecture
conflicts (a data structure, a subsystem interface, an on-disk or ABI format, a lock order, a phase
boundary) are not recorded here: the builder stops and asks the coordinator.

None yet. <!-- claim-lint:ok: normative requirement of docs/plans/e3-page-cache.md under ruling DEV-41(2), not a measured result. -->
