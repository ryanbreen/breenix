# E1: kernel memory foundation (design and PR plan)

Status: design, docs only. No code changes ship with this document.

Rulings: DEV-37(2) orders this design before any E1 code. DEV-29 adopted the toy-OS audit
(`~/Downloads/breenix-toy-os-audit-2026-11-27.html`, findings R3 and R14, themes T3, T8 and T9) and decided Q4
(no swap now; reclaim must leave room for it) and Q5 (no fixed CPU or RAM ceiling). DEV-30 sets the testing
rule, and DEV-31 makes ARM64 QEMU the performance rig. The memory lane's full findings (F5, F6, F8, F11 and
the keep list) are the evidence base for section 1.

Clean room. The designs below come from textbooks, published papers and hardware manuals, listed in
section 10. The only code read for this document is Breenix's own. No other operating system's or C
library's source was read, fetched or quoted.

## Contents

1. Where we are
2. Goals, non-goals and invariants kept
3. Layering
4. Physical memory: memory map, direct map, frame metadata
5. The buddy allocator, zones and per-CPU page caches
6. The kernel heap: slab caches, magazines, span arenas and large mappings
7. Allocation failure, reclaim hooks and the conversion of panicking callers
8. What E3 and E10 build on
9. No fixed ceilings
10. Sources
11. PR sequence
12. Tests
13. bbench rows (DEV-31)
14. Risks and questions for the coordinator

## 1. Where we are

The line numbers are from main at f52fd83c, read for this document. Phase 0 and Phase 1 changed several
of these files after the audit's 1bcb09f4: the R1 population pins, the failed-translation error, ARM64
ASIDs (4b2177f7), the heap placed from the memory map (e052deef) and multi-page population (332db244).
Where the audit's line numbers moved, the ones below are the current ones.

### 1.1 Physical frames

- `kernel/src/memory/frame_allocator.rs` hands out frames from two sources.
  - First a LIFO free list, `FREE_FRAMES: Mutex<Vec<PhysFrame>>` (line 289), taken with interrupts masked
    through `with_free_frames` (line 306).
  - When the list is empty, a bump frontier `NEXT_FREE_FRAME` walks the usable regions in order
    (`get_usable_frame`, line 373, O(regions)).
- Regions past `MAX_REGIONS = 128` (line 20) are dropped at init (lines 807-833).
- The generation ledger (`FrameLedger`, lines 127-189) is the part the audit calls production-grade.
  - Each frame has a 12-byte slot: `allocation` (2 state bits and a 30-bit generation), `leaf_refs` and
    `site` (lines 90-96).
  - A `FrameLease` (lines 241-252) is the only authority to return a generation.
  - `return_lease` (lines 1089-1152) refuses double, stale, unallocated, untracked and live-leaf returns.
- The ledger is built from heap `Vec`s:
  - a chunk directory, plus 64 Ki-frame chunks created on demand from inside the allocation path
    (`ensure_chunk`, line 146; `prepare_frame_for_allocation`, line 710);
  - a free-list capacity reservation of 8 bytes per frame up to the frontier (`ensure_free_frame_capacity`,
    line 693).
- A return that finds the list at capacity is dropped and counted as `LostContended` (lines 1143-1145).
- `frame_ordinal` (line 412) is a linear scan over the regions. It runs on each claim, return, leaf
  acquire, leaf decref and refcount query.
- `acquire_leaf_mapping` (line 494) and `leaf_mapping_refcount` (line 574) take the global
  `EXTERNAL_LEAF_SPANS` spin mutex (lines 281, 425-431). They do it for each user leaf mapping, so on each
  anonymous fault and each page fork shares. No code in the tree writes that table: `rg EXTERNAL_LEAF_SPANS`
  finds only its declaration and the one reader. Each call takes a global lock to scan an empty array.
- Contiguous runs exist only at the frontier (`allocate_contiguous_frames`, lines 985-1034, and
  `contiguous.rs`). Once the frontier reaches the end of RAM, no run of more than one frame can be allocated
  again.
- There are no zones. x86 heap init logs "DMA will fail" for frames above 4 GiB (`heap.rs:243-248`).

Arithmetic for the R240's 8 GiB (about 2.0 M frames) at full frontier: ledger about 24 MiB plus free-list
capacity at least 16 MiB, 40-48 MiB of a 64 MiB heap (memory lane F6). On 16 GiB they would not fit.

### 1.2 The kernel heap

- `kernel/src/memory/heap.rs` is one region behind one spin mutex, `IrqSafeLockedHeap` (line 101). Each
  operation masks local interrupts.
  - x86-64: a fixed 64 MiB window at `0x4444_4444_0000` (lines 40-44), mapped page by page from frames at
    boot (lines 228-262).
  - ARM64: one physically contiguous block in the direct map, sized one eighth of the frame pool within
    64-128 MiB (`libs/arm64-boot-contract/src/phys_memory.rs:262-284`, placed by `place_kernel_pools`,
    line 309). On the QEMU rig's 512 MiB VM that is 64 MiB, one eighth of RAM, held for the whole boot.
- The algorithm, `heap_index.rs`, is size classes over address radix trees with boundary tags. Allocation
  looks at no more than two class heads, and a free coalesces in O(1). Offsets are u32, so one region is
  below 4 GiB (line 35). The audit keeps it.
- `realloc` allocates, copies and frees on each call (lines 193-204).
- `alloc_error_handler` panics (line 368). The OOM record that names the size and callers is x86-only
  (`report_oom`, line 298).
- Fixed budgets elsewhere are derived from "the smallest kernel heap":
  - the block cache's `MAX_BYTES = 64 MiB / 8` (`block/cache.rs:135`);
  - BreenFS `DIRTY_BUDGET = 8 MiB` (`fs/breenfs/commit_policy.rs:8`).
- `memory/slab.rs` has two fixed bitmap caches (fd tables and signal tables, 64 slots each) carved from the
  heap. They fall back to `Box` when full.

### 1.3 The direct map and other ceilings

- ARM64's direct map covers RAM only below `DIRECT_MAP_RAM_END = 0x1_0000_0000`
  (`platform_config.rs:164`), and the frame pool is clipped to that window (`direct_map_ram_window`,
  line 199). ARM64 does not use RAM above 4 GiB.
- x86-64's bootloader is configured to map the complete physical memory at a dynamic offset
  (`main.rs:47`, `Mapping::Dynamic`).
- Kernel stacks have fixed windows:
  - x86 production uses 128 KiB stacks mapped eagerly in a fixed VA window (`kernel_stack.rs:376`);
  - ARM64 uses a fixed 32 MiB physical pool with 64 KiB stacks (`kernel_stack.rs:788-810`).
  - The memory lane's arithmetic: about 990 and about 480 kernel threads system-wide.

### 1.4 Failure today

- When a page fault cannot get a frame, the faulting process gets SIGKILL (`memory/anon_map.rs:166-168`).
- A heap allocation that cannot be met panics. #3226 and DEV-23 are that panic; #3228's fallible buffers are
  point repairs around it.
- There is no reclaim path: no shrinkers, no watermarks and no OOM policy (memory lane F8).

## 2. Goals, non-goals and invariants kept

### 2.1 Goals (the six items DEV-37(2) and the brief name)

1. Frame metadata is one array per memory section, allocated from frames at boot, sized from the memory map,
   outside any heap. It keeps the ledger's lease semantics exactly (section 4.3).
2. The kernel heap is frame-backed and grows and shrinks:
   - size-class slab caches with per-CPU magazines in front;
   - `heap_index.rs`'s algorithm behind them for mid-size requests, over frame-backed spans;
   - large requests as mappings of discontiguous frames (section 6).
3. Allocation failure is an error. It first runs reclaim hooks; it does not panic by itself. Each panicking
   caller is converted (section 7).
4. A buddy allocator with zones (DMA32 and NORMAL on both architectures) and per-CPU page caches replaces
   the frontier and the global LIFO list. Large contiguous runs are available after boot (section 5).
5. E1 exposes what E3 (page cache, dirty data, staging copies, reclaim) and E10 (socket and packet buffers)
   need to move off the fixed heap (section 8).
6. No fixed RAM ceiling anywhere in this design (section 9).

### 2.2 Non-goals (and who owns them)

- Page cache, lazy mmap, readahead, writeback, the page daemon and OOM victim selection: E3.
- Sleepable allocation contexts and direct reclaim that sleeps: E2. Until E2, E1's paths do not sleep.
- Page migration and compaction: E3 for page-cache pages, E12 with superpages.
- Transparent superpages and large file-cache pages: E12.
- Libc malloc: E7. The `memory.malloc-*` bbench rows measure libc, not E1.
- PCID and IPI shootdowns: E9. ARM64 ASIDs landed in Phase 1.6 (4b2177f7).
- Swap: not now (DEV-29 Q4). Section 5.7 keeps room for it.
- NUMA placement. The section layout of section 4.2 leaves room for per-node arrays, but E1 does not
  implement nodes.

### 2.3 Invariants kept (the audit's keep list)

- Frame generation leases: double, stale and unallocated returns are refused, and a refusal leaks rather
  than reuses (section 4.3).
- Flush before free: a frame's last reference is dropped only after its translation is invalidated
  (`ReleasedLeaf`, `FlushedLeaf`, `LeafBatch` in `process_memory.rs`). The new kernel mappings of
  section 6.5 follow the same rule.
- Leaf custody records and `leaf_refs` semantics, including `LEAF_EXTERNAL`.
- The fallible-allocation discipline: hooks that run after the disk has changed use only capacity reserved
  before it (`file_map.rs:16-20`).
- `heap_index.rs`'s algorithm: two class heads per allocation and O(1) coalescing.
- The `/proc/breenix/frames` census and its owner rows. The suites' memory record reads them
  (`docs/suites/README.md`, "Memory record"), so the owner set does not change; new detail goes in a new
  file.

## 3. Layering

From the bottom up:

```
memory map (x86 bootloader regions; ARM64 frame pool)        -- 4.1
  -> boot carve (metadata arrays, direct-map tables)           -- 4.1, 4.2
  -> frame metadata sections (FrameMeta per 4 KiB frame)        -- 4.2, 4.3
  -> buddy allocator per zone, mobility-grouped pageblocks     -- 5
       -> per-CPU page caches (order 0)                         -- 5.4
  -> kernel VA windows: span window, vmap window, stack window  -- 6.5
  -> slab caches (16 B .. 4 KiB) + per-CPU magazines           -- 6.2, 6.3
  -> span arenas running heap_index (4 KiB .. 512 KiB)          -- 6.4
  -> vmap allocations (> 512 KiB, discontiguous frames)         -- 6.5
  -> GlobalAlloc + explicit fallible APIs + DMA API             -- 6.6, 5.6, 7
  -> reclaim registry (shrinkers), watermarks, OOM record       -- 7
```

Each algorithm is a pure module with no kernel dependencies. Each takes its frames, CPU id and clock as
parameters, as `heap_index.rs` and `contiguous.rs` already do. That lets host test crates include it with
`#[path]`, as `tools/heap-host-tests` and `tests/frame_allocator_contiguous.rs` do today. The kernel side is
glue: locks, interrupt masking, the direct map and page tables.

## 4. Physical memory: memory map, direct map, frame metadata

### 4.1 One range set and a boot carve

- Both architectures normalise their memory description into one sorted, disjoint, page-aligned set of
  usable RAM ranges.
  - x86: the bootloader's `MemoryRegions`, less the 1 MiB floor and the AP trampoline run.
  - ARM64: `platform_config::frame_pool()`, with the clip to 4 GiB removed (section 4.4).
- There is no `MAX_REGIONS`. The range set lives in a page carved from the largest range, and grows by
  carving another page if the firmware describes more.
- A boot carve takes `(bytes, align, below)` and removes the highest fitting run from the range set. It
  feeds the metadata arrays (4.2), the section table, and ARM64's extra direct-map tables (4.4). It returns
  an error instead of panicking. A boot that cannot carve its metadata still stops, at an explicit
  `expect` naming the shortfall.
- After the buddy allocator is seeded (5.1), the carve is closed. The range set is not allocated from again.

### 4.2 Frame metadata sections

- Physical memory is described in sections of 2^15 frames (128 MiB, aligned).
- A section that holds any usable RAM gets one metadata array of 2^15 entries, carved from that section's
  own RAM where the section can spare it, and from the nearest larger section otherwise.
- A section table holds one pointer per 128 MiB of physical address space, up to the highest usable frame.
  It is carved at boot.
- Lookup is `pfn >> 15` into the section table, then `pfn & 0x7fff` into the array: two loads, O(1).
  `frame_ordinal`'s O(regions) scan goes.
- Sizes:
  - an entry is 32 bytes, so metadata is 0.78% of RAM;
  - a section costs 1 MiB;
  - holes cost 8 bytes of section table per 128 MiB of address space;
  - a section that is only partly RAM still costs its full 1 MiB.
- Worked numbers:
  - the 512 MiB QEMU rig: 4 sections, 4 MiB of metadata. Today's ledger and free-list capacity for those
    131,072 frames take up to about 2.5 MiB of the heap;
  - the R240's 8 GiB: about 64 MiB of metadata outside the heap. Today it is up to 40-48 MiB inside it.
  - 32 bytes is more than today's 12-byte slot plus 8 bytes of list capacity. It buys O(1) buddy links and
    a slab back-pointer, and it scales with RAM instead of with a fixed heap.

### 4.3 FrameMeta and the lease semantics

The entry layout, 32 bytes:

| Field | Bytes | Meaning |
|---|---|---|
| `state_gen: AtomicU32` | 4 | Today's encoding unchanged: 2 state bits (`ST_NEVER`, `ST_ALLOCATED`, `ST_FREE`) and a 30-bit generation that only increases. |
| `leaf_refs: AtomicU32` | 4 | Today's mapping count, including `LEAF_EXTERNAL`. |
| `site: AtomicU16` | 2 | Today's allocation-site index for the census. |
| `owner: AtomicU8` | 1 | The `FrameOwner` tag `allocate_frame_for` sets, kept per frame instead of per site. |
| `flags: AtomicU8` | 1 | `BUDDY_HEAD`, `PCP_CACHED` (in a per-CPU cache), `TAIL` (inside an allocated block), and the mobility class (5.5). |
| `order: u8`, `zone: u8`, pad | 4 | Buddy order of a free head or allocated block, and the zone index. |
| `use: [u64; 2]` | 16 | A union. Free: the buddy list's previous and next pfn. Slab: the slab descriptor's address and the cache id. Span: the span's address. Otherwise 0. |

The lease rules carry over word for word:

- A claim moves `ST_NEVER` or `ST_FREE` to `ST_ALLOCATED` and increments the generation in one CAS, then issues
  `FrameLease { pfn, generation }`. The index becomes the pfn (u64) instead of the ordinal (u32).
- A return with a lease whose generation does not match is `RefusedStale`. A return on `ST_FREE` is
  `RefusedDoubleRelease`. On `ST_NEVER` it is `RefusedNeverAllocated`. A frame outside any section is
  `RefusedUntracked`. A frame with live leaf references is `RefusedLiveLeaf`. Each refusal leaks the frame
  and counts it, as now.
- A frame in a per-CPU cache or on a buddy list is `ST_FREE` in `state_gen`; `flags` says where.
  `LostContended` disappears: a free list threaded through the metadata needs no capacity, so each return can
  be recorded.
- A block of order n is claimed frame by frame: each frame's generation advances. The block lease records
  the head pfn, the order and the head's generation. A block return checks the head's generation, then that
  each tail is still `ST_ALLOCATED`, flagged `TAIL`, with the same order. Anything else is a refused return that leaks the
  block. `split_lease` turns a block lease into per-frame leases, for callers that free pages one at a time.
  The existing `allocate_contiguous_frames` keeps its per-frame contract by calling it.
- `EXTERNAL_LEAF_SPANS` is deleted (section 1.1: no code writes it). `register_external_leaf_frame`, which
  marks RAM frames, is unchanged.

The census walks the sections instead of the chunks. `/proc/breenix/frames` keeps its lines and owner set.
Zone, order and per-CPU cache counts go in a new `/proc/breenix/kmem`.

### 4.4 The direct map covers the described RAM (ARM64)

- Before the buddy is seeded, ARM64 builds direct-map entries for each usable range above the boot
  window, at `HHDM_BASE + pa`.
  - It uses 1 GiB block descriptors where a range covers an aligned GiB, 2 MiB blocks elsewhere, and 4 KiB
    pages only at unaligned edges. Table pages come from the boot carve.
  - The attributes are those of the existing normal-memory entries.
  - Entries are written only where the descriptor was invalid, so no break-before-make sequence is needed
    (Arm ARM, translation table updates). Then `dsb ishst; isb`.
- `direct_map_ram_window`'s clip goes, and the frame pool covers the described RAM.
- x86-64 needs no change: the bootloader's dynamic physical-memory mapping covers it.
- `PhysAddrWrapper::from_kernel_virt` already uses the CPU's own translation (`AT S1E1R` on ARM64, a table
  walk on x86), so it needs no change.

## 5. The buddy allocator, zones and per-CPU page caches

### 5.1 The buddy allocator

- A binary buddy system (Knowlton 1965; Knuth, TAOCP vol. 1 §2.5) per zone, with orders 0..=18 (4 KiB to
  1 GiB). The allocator itself puts no limit below a 1 GiB block, which E12's superpages need.
- Free lists are doubly linked through `FrameMeta.use`, not through the free pages. A free page is not
  written, so frames need not be mapped to be freed.
- Seeding: each usable range left after the boot carve is cut into the largest naturally aligned blocks
  that fit, inserted as free heads. Frames the kernel image, boot stacks and loader own are not seeded, so
  they stay `ST_NEVER`.
- Allocation of order n takes the smallest non-empty order k >= n from a per-zone non-empty bitmap (one
  `trailing_zeros`, as `heap_index.rs` finds its class), then splits down, pushing the upper halves.
- Free of order n finds the buddy at `pfn ^ (1 << n)`. If it is a free head of the same order, in the same
  zone and not `PCP_CACHED`, it is unlinked, merged, and the step repeats. Cost: O(max order) per free.
- One lock per zone, taken with interrupts masked. It is held only for list edits, not across zeroing,
  mapping or shrinkers.
- The frontier, `NEXT_FREE_FRAME`, `get_usable_frame`, `FREE_FRAMES`, `BOOTSTRAP_FREE_FRAMES` and the
  ledger chunks are deleted. `init_frame_ledger` goes because the metadata exists before the heap.

### 5.2 Zones

- Zones are defined by device addressing, not by platform. Both architectures have:
  - `DMA32`: frames whose device-visible address is below 4 GiB. On ARM64 VMware the device-visible
    address is the IPA (`platform_config::kernel_va_to_ipa`), so the boundary is computed in that space.
  - `NORMAL`: the rest.
- When RAM ends below 4 GiB (the 512 MiB rig), only `DMA32` exists. `NORMAL` requests are served from it.
- No 16 MiB ISA zone: no supported device declares an address limit below 32 bits. A driver whose limit is
  narrower fails its own init with an error instead of getting a special case (DEV-27(2)).
- Fallback order: `NORMAL` falls back to `DMA32`, which keeps a fallback reserve of 1/256 of the higher
  zone's pages on top of its own watermark. That way `NORMAL` demand cannot use up the memory 32-bit
  devices need. `DMA32` requests do not fall forward.
- ARM64's fixed 2 MiB non-cacheable block (`KERNEL_FIXED_RANGES[1]`) is a memory-attribute requirement for
  xHCI, not an address limit. It stays outside the buddy as it is.

### 5.3 Watermarks

- Each zone has `min`, `low` and `high`:
  - `min` = max(128 frames, zone frames / 1024);
  - `low` = 5/4 of `min`;
  - `high` = 3/2 of `min`.
  They are computed from the zone's size at boot, with no upper constant.
- An ordinary allocation that would take the zone below `min` enters the slow path (section 7.2).
- Allocations from interrupt context, and from inside reclaim, may use half of `min` (the emergency
  reserve) before failing.
- Crossing `low` sets the zone's pressure flag, which E3's page daemon will wait on. In E1 the slow path
  reads it.

### 5.4 Per-CPU page caches

- Each CPU keeps, per zone, a list of order-0 frames. This is the magazine idea of Bonwick and Adams 2001
  applied to frames.
  - `batch` = clamp(zone frames / (1024 × CPUs), 8, 64).
  - `high` = 4 × `batch`.
  - Allocation pops; an empty list refills `batch` frames from the buddy under one zone-lock hold.
  - A free pushes; above `high`, `batch` frames go back to the buddy.
- Each per-CPU list has its own spin lock, taken with interrupts masked.
  - On the owner it is uncontended, one atomic.
  - It lets any CPU drain another CPU's cache without an IPI.
  - Drains happen on watermark `min`, on a failed contiguous allocation (5.6), on CPU offline, and from the
    reclaim slow path.
- Cached frames carry `PCP_CACHED`, so the buddy does not merge them and the census counts them separately.
- Cost on the anonymous fault: an allocation is one per-CPU pop and the lease CAS. Today it is a global
  IRQ-masked lock, an O(regions) ordinal scan, and the empty-span mutex.

### 5.5 Mobility grouping

Fragmentation comes from unmovable allocations spread across large regions (Wilson et al. 1995 survey
the general problem). So the buddy keeps 2 MiB pageblocks tagged with one of three classes:

- `UNMOVABLE`: slabs, page tables, kernel stacks, DMA.
- `RECLAIMABLE`: clean cache memory a shrinker can drop.
- `MOVABLE`: user anonymous pages, and page-cache pages once E3 can migrate them. Until then they behave as
  pinned, but they are still grouped.

An allocation takes from a pageblock of its own class. It takes a whole free pageblock before splitting
someone else's, and when it must take from another class it takes the largest free block there and
re-tags the pageblock when more than half of it is now its own. So unmovable kernel memory stays in few
pageblocks. That is what leaves contiguous runs available long after boot.

### 5.6 Contiguous runs after boot, and the DMA API

- `allocate_contiguous_frames(count, out)` keeps its signature and its per-frame result.
  - It allocates order ceil(log2(count)) from the zone the caller's address limit selects, `split_lease`s
    it, and returns the excess frames to the buddy at once.
  - Before failing it drains each CPU's page cache once and runs the reclaimable shrinkers (7.3).
  - With no migration in E1, a request that still cannot be met fails, as today, but long after the
    frontier is gone.
- A new `memory::dma` API replaces the drivers' private `virt_to_phys` helpers (AHCI and most virtio
  drivers carry their own; `rg 'fn virt_to_phys'` lists them):
  - `dma::alloc(bytes, limit) -> DmaBuffer { virt, bus, len }` gives physically contiguous, zeroed frames,
    within the address limit (`limit` is `Dma32` or `Any`), with the CPU address and the device address;
    it does the IPA translation on VMware;
  - `dma::bus_address(virt, len) -> Result<u64, DmaError>` accepts only direct-map or static-image addresses
    whose whole range is physically contiguous. It refuses heap span or vmap addresses (6.5), which are not.
- Today ARM64's heap is one physically contiguous direct-map block, so a heap buffer handed to a device
  happens to work. After E1-7 it would not. The checked `bus_address` turns that into an error at the call,
  and E1-7 converts each caller it finds (section 11).

### 5.7 Room for E3 and for swap

- `FrameMeta` grows to 64 bytes when E3 adds the page cache's owner object, index, LRU links, flags and
  reference count. The sections are sized by `size_of::<FrameMeta>()`, so that is one constant. The cost is
  1.56% of RAM.
- The reclaim registry (7.3) has a context argument that E2 extends with "may sleep" and "may do I/O", and
  E3 with "may write back". Swap would be one more shrinker over anonymous pages, plus its own on-disk
  format (DEV-29 Q4). No part of E1 assumes anonymous memory is unreclaimable.

## 6. The kernel heap: slab caches, magazines, span arenas and large mappings

### 6.1 Routing

`GlobalAlloc` gets the `Layout` on both `alloc` and `dealloc`, so the layer is chosen from the layout
alone, with no lookup:

| Request | Layer | Backing |
|---|---|---|
| size <= 4096 and align <= 4096 | slab cache of the class (6.2) | order-0 frames through the direct map |
| 4096 < size <= 512 KiB, or align > 4096 up to 2 MiB | span arena running `heap_index` (6.4) | 2 MiB spans in the span window |
| size > 512 KiB | vmap allocation (6.5) | discontiguous order-0 frames in the vmap window |

The size limits are constants of the layout, not of RAM. The windows' sizes scale with RAM (section 9).

### 6.2 Slab caches

The design is Bonwick 1994's object caches over slabs, with single-page slabs, so no slab needs contiguous
frames and the heap works before the buddy lands.

- Classes, in bytes:
  - 16 to 160 in 16-byte steps;
  - 192, 224, 256, 288, 336, 400, 448, 512;
  - 576, 672, 816, 1024, 1360, 2048 and 4096.
  Above 128, each class is the largest multiple of 16 that fits a whole number of objects in a page (for
  example 816 is 5 per page and 1360 is 3). Each slab is one 4 KiB frame, reached through the direct map.
  The slab's bookkeeping is off-page (next bullet), so the whole page holds objects. Arithmetic: the tail
  left over in a page is at most 96 bytes (2.3%), for the 160 and 400 classes; it is 0 for each
  power-of-two class.
- A request with alignment above 16 bytes goes to the smallest power-of-two class at least
  max(size, align), up to 4096.
- The slab descriptor holds: cache, frame, the free-object list head (threaded through free objects), the
  in-use count, and partial/full/empty list links. Descriptors come from a self-hosting descriptor cache
  whose slabs hold their own descriptors in-page. `FrameMeta.use` points at the descriptor, so a free finds
  its slab from `pfn(ptr)` in O(1).
- Per cache: lists of partial, full and empty slabs under the cache lock. At most one empty slab is kept;
  the rest go back to the page layer. The shrinker returns that last one too.
- Named caches with fixed object sizes and an optional constructor replace `memory/slab.rs`'s two bitmap
  caches. `SlabBox` keeps its API and loses its 64-slot limit.
- On free, a mismatch between the layout's class and the frame's cache is memory corruption, and it panics
  with both values. That is integrity, not allocation failure.

### 6.3 Per-CPU magazines

Bonwick and Adams 2001.

- Each CPU has, per cache, a loaded and a previous magazine of M rounds.
  - Allocation pops from the loaded one. If it is empty and the previous one is full, the two swap.
    Otherwise the CPU exchanges its empty magazine for a full one from the cache's depot. If the depot is
    empty, it fills from slabs.
  - Free is the mirror image.
  - M starts at 15, and grows when the depot lock is contended, as the paper describes.
- The magazine operation runs with local interrupts masked for a few instructions. On a CPU that is
  migration safety and interrupt reentry in one, and it allows allocation from interrupt handlers.
  - The fast path takes only its own CPU's magazine lock, which is uncontended unless a remote drain is
    running (as for the page caches in 5.4), and writes no cache line another CPU writes in normal
    operation.
  - It does not log, format or walk page tables, so it stays inside the interrupt-path rules in CLAUDE.md.
- Magazines come from a magazine cache that has no magazine layer of its own, which ends the recursion.
- Draining:
  - a CPU's magazines are flushed into the depot by its owner on request, or by any CPU under the per-CPU
    lock;
  - the depot's surplus beyond its recent working set is returned to slabs by a periodic trim from an
    existing kernel worker, not from the timer interrupt (a Tier-1 file), and by the shrinker.
  - Retained memory is bounded per CPU by 2 × M × object size per cache, plus the depot's working set.
    Hoard (Berger et al. 2000) names the failure such a bound prevents: memory freed on one processor that
    no other processor can reuse.

### 6.4 Span arenas running heap_index

- The span window is a kernel VA range divided into 2 MiB spans (6.5). A span is backed by 512 order-0
  frames mapped 4 KiB at a time. When the buddy can give an order-9 block (after E1-8), it is backed by that
  block with one 2 MiB mapping.
- Each span runs one `heap_index::Heap` (u32 offsets fit). Each CPU has an arena: a set of spans, a lock,
  and the spans ordered by free space.
- An allocation uses the arena of the CPU it runs on. A free goes to the span's owning arena, found from
  the address: `(va - window base) >> 21` indexes a span table. Taking another arena's lock for a
  cross-CPU free is correct; it is only slower.
- Growth: when no span in the arena has room, a new span is mapped. Shrink: a span whose `used()` drops to
  0 is unmapped, keeping one empty span per arena. The flush-before-free rule (6.5) applies to its
  frames.
- `heap_index.rs` gains two operations that use its boundary tags:
  - `grow_in_place(ptr, new)` succeeds when the next block is free and large enough;
  - `shrink_in_place(ptr, new)` splits off a free tail.
  This is the same algorithm. The two-class-head bound and O(1) coalescing still hold.

### 6.5 Kernel VA windows and the large-allocation layer

- Three windows in the kernel half, sized from RAM at boot:
  - span window: RAM size, rounded up to 1 GiB;
  - vmap window: 4 × RAM;
  - stack window (E1-9): reserved for kernel stacks.
  VA is plentiful: the 48-bit kernel half is 128 TiB, and a window costs no memory until it is mapped.
  The direct map (RAM), span window (RAM) and vmap window (4 × RAM) together take about 6 × RAM of it,
  which is the bound section 9 names.
- x86-64 does not map the windows through `map_kernel_page` (`kernel_page_table.rs:132`). That function
  points each upper-half PML4 slot it touches, other than 402 and 403, at one shared `KERNEL_PDPT_FRAME` (lines 170-191),
  so addresses that differ only in their PML4 index alias each other and the whole upper half has 512 GiB
  of unique space. The vmap window alone fills that at 128 GiB of RAM. It also walks and fills the tables
  with no lock, so two first mappings under one empty PD entry can each install a table and one is lost.
  E1-5 instead gives the windows their own slots:
  - each window takes ceil(size / 512 GiB) PML4 slots of its own, each with its own PDPT, installed in the
    master PML4 at boot. That happens before the first process is created, because
    `ProcessPageTable::new` copies PML4[256..512] from the master once (`process_memory.rs:1098-1107`), so
    a slot added later would be missing from existing address spaces;
  - a window mapper walks from the window's own PDPT and does not write PML4 entries after boot;
  - an intermediate table is installed by a compare-exchange of the empty entry. The loser returns its
    zeroed page and uses the winner's. Leaf entries need no lock: the VA allocator gives each range to
    one owner.
  `map_kernel_page`'s aliasing stays for its existing callers, and E1-5 does not widen its use.
- ARM64 has no TTBR1 page mapper today (`arch_impl/aarch64/mmu.rs` holds the boot-time block tables only).
  E1-5 adds one.
  - It installs table pages from the page layer, then 4 KiB or 2 MiB descriptors into invalid entries, then
    `dsb ishst; isb`.
  - Unmapping writes an invalid descriptor, then `tlbi vaae1is` per page (any ASID, inner shareable), then
    `dsb ish; isb` (Arm ARM, TLB maintenance).
  - Kernel mappings are global and need no ASID, so this is independent of Phase 1.6's user ASIDs.
- vmap allocation:
  - VA comes from the vmap window's free-extent tree (ordered by start and by size). Its nodes come from a
    preallocated reserve, so it does not recurse into vmap.
  - Each allocation is followed by an unmapped guard page.
  - Frames are order-0 from the per-CPU caches and need not be contiguous.
  - `realloc` grows in place when the following VA is free. Otherwise it moves the PTEs to a new range
    without copying the bytes.
- Flush before free, for kernel mappings. Unmapped ranges go on a pending list, and their frames are not
  returned until one flush covers the batch.
  - On ARM64 the `tlbi vaae1is` at unmap is that flush, since it is broadcast by hardware.
  - On x86, whose kernel pages are global (Intel SDM vol. 3A §4.10, global pages and TLB invalidation),
    the batch is flushed on each CPU through the existing shootdown transport (`memory/tlb.rs`). That happens when the pending list passes
    16 MiB, when VA allocation fails, or from the shrinker.
  - Spans shrink with hysteresis and vmap frees are batched, so these flushes are rare. E9's IPI transport
    replaces the NMI underneath without changing this rule.

### 6.6 The allocator's front

- `#[global_allocator]` becomes `KernelAllocator`:
  - `alloc` routes by layout and returns null only after the slow path (7.2);
  - `dealloc` cannot fail;
  - `realloc` stays in place where the layer allows: same slab class, `grow_in_place`, or the vmap move.
- Explicit APIs for code that can say more than `Layout` can:
  - `kmem::try_alloc(layout, Ctx)`, where `Ctx` is `Normal`, `Atomic` (interrupt or reclaim; may use the
    emergency reserve) or `NoReclaim` (must not run shrinkers, for callers that hold a shrinker's lock);
  - `kmem::alloc_pages(order, zone, Ctx)`.
- The fixed heap reservation goes on both architectures:
  - x86's 64 MiB window at `0x4444_4444_0000` is no longer mapped;
  - ARM64's `place_kernel_pools` no longer carves a heap, and `KERNEL_HEAP_MIN` and `KERNEL_HEAP_MAX` are
    deleted.
- `heap::free_bytes`, `heap_size` and `high_water_bytes`, which the console, procfs and `gpu/heap.rs` read,
  become `kmem::stats()`: frames in use by the heap, high water, per-class counts, and free frames per zone.

## 7. Allocation failure, reclaim hooks and the conversion of panicking callers

### 7.1 The error

- `kmem::AllocError` has a cause: `NoFrames { zone, order }`, `NoVa { window }` or `Refused` (a context
  that may not take more).
- Each layer returns it; no layer panics.
- `GlobalAlloc::alloc` returns null on it, which reaches `alloc_error_handler` only through Rust's
  infallible APIs. Section 7.4 removes those from the runtime paths.
- The OOM record (`report_oom`) moves to the common path, so ARM64 gets it too, and gains the zone, order
  and watermark state.

### 7.2 The slow path

The slow path is entered when a zone would go below `min`, or when a layer cannot be refilled:

1. Drain this CPU's page cache and magazines into the shared layers.
2. Drain other CPUs' page caches, under their per-CPU locks.
3. Run the registered shrinkers in cost order (7.3), asking for the shortfall plus `batch`. Stop as soon as
   the target is met.
4. Retry the allocation once.
5. If the context is `Atomic`, allow the emergency reserve. Otherwise return `AllocError`.

- The slow path holds no allocator lock while it runs shrinkers.
- A per-CPU `in_reclaim` flag makes any allocation a shrinker makes `Atomic`, and stops reclaim from
  recursing.
- The work is bounded: each shrinker scans at most its target plus a fixed batch.
- There is no sleeping and no waiting in E1. E2 adds a sleeping step 3b for contexts that may sleep, and E3
  adds waking the page daemon.

### 7.3 The reclaim registry

- `kmem::register_shrinker(&'static Shrinker)`, where
  `Shrinker { name, cost, count: fn() -> usize, scan: fn(target, ReclaimCtx) -> usize }` and both counts are
  in frames.
- Each E1 shrinker is non-blocking:
  - it takes its structure's lock with `try_lock` and skips when the lock is busy;
  - it does not sleep, wait for I/O or wake a thread.
  That makes it safe from any context the allocator runs in, including the x86 syscall path, which still
  runs with IF clear until E2 (T1). It also cannot deadlock against a caller that holds the scheduler's lock
  while it allocates: that existing scheduler -> heap edge is documented at `heap.rs:86-89`.
- The shrinkers E1 registers, in cost order:
  1. slab: empty slabs and depot surplus (cheapest);
  2. span arenas: empty spans; vmap: the pending-flush list;
  3. block cache: clean, unpinned entries. This needs a new non-blocking entry point in `block/cache.rs`;
     the cache bounds itself today but cannot be asked to shrink;
  4. mapping cache: clean pages no binding covers, using the eviction `evict` already does
     (`file_map.rs:1138`), but on demand. Unbinding still requests eviction at once (`file_map.rs:529`);
     with pressure now having a path, that request can wait for the shrinker (memory lane F3's interim
     step). Populated pages that are not yet bound stay pinned (cfec5a00);
  5. BreenFS decoded-node cache.
- Each shrinker is a few lines in its owning module, with its own host test.

### 7.4 Converting the panicking callers

The panicking callers are:

- each use of Rust's infallible allocation APIs: `Box::new`, `Vec::push`, `vec!`, `format!`,
  `String::from`, `Arc::new`, `BTreeMap::insert`, `collect`, `to_vec` and so on;
- the explicit `.expect` and `unwrap` calls on allocation results.

A pattern count over `kernel/src` at f52fd83c, which includes tests, gives a sense of scale:

- `.push(` 1,302 lines in 223 files;
- `format!(` 336;
- `.collect` 269;
- `.insert(` 250;
- `String::from(` 238;
- `Arc::new(` 165;
- `vec![` 139;
- `Box::new(` 94;
- `.to_vec()` 82;
- `BTreeMap::new` 67.
- `try_reserve` already appears on 533 lines in 142 files.
- The explicit frame-allocation panics are 11 lines. Ten are in `kernel_page_table.rs`'s
  boot-time setup, `init` and `build_master_kernel_pml4` with its kernel-stack and IST tables (lines
  70-713); the eleventh is a test.

The conversion rules, by context:

| Context | Rule | Error surfaced |
|---|---|---|
| Boot, before PID 1 runs | May stay as it is, but each site becomes an explicit `try_*().expect("boot: <what>")`. A boot that cannot allocate its own structures has no system to keep. | Panic with the OOM record |
| Syscall paths | Fallible. `AllocError` maps to the errno the call's POSIX or man page lists for resource exhaustion. | `ENOMEM` (mmap, brk, open, exec, most); `EAGAIN` for fork, clone and thread creation (POSIX fork: "lacked the necessary resources"); `ENOBUFS` for sockets |
| Page faults | Fallible. The fault returns a distinct `FaultOutcome::OutOfMemory`. In E1 it still delivers SIGKILL to the faulting process, the existing policy, now counted and recorded; E3 replaces that with victim selection by size. | SIGKILL (E1); E3 policy |
| Interrupt and receive paths | Fallible with the emergency reserve. On failure the unit of work is dropped and counted (a received frame, a refill), not half-applied. | Counter |
| Kernel workers (breenfs-commit, workqueues) | Fallible. Retry with backoff from the worker loop, keeping the operation's own reservation discipline (`file_map.rs:16-20`). | Retry; error to the waiter |
| Diagnostics (`/proc`, census, logging) | Fallible. `format!` becomes a bounded writer over a `try_reserve`d buffer. A `/proc` read that cannot allocate returns ENOMEM; logging does not allocate. | `ENOMEM` on read |

Mechanics:

- Fallible helpers in `kmem`: `try_box`, `try_arc` (the kernel already has `allocator_api` and uses
  `Box::try_new` on 13 lines), a `TryPush` extension for `Vec` and `VecDeque`, and `try_format!`. A
  `try_insert` for ordered maps is not offered by `alloc`'s `BTreeMap`. Those 67 sites move to a
  slab-backed ordered map with a fallible insert, or to a structure preallocated by the caller.
- One conversion PR per subsystem (section 11). Each is check-first: its structural guard, a host test
  that fails while the subsystem still names an infallible allocation API outside a `boot:` site, goes in
  red, then the conversion turns it green. Where the subsystem has a host-test crate, an allocation-failure
  sweep also runs: a counting `#[global_allocator]` that fails the Nth allocation, for each N in turn, through the
  converted path. An infallible allocation left on the path aborts the test process, which is a red result.
- The last PR builds `alloc` with the `no_global_oom_handling` configuration Rust's `alloc` crate provides.
  The infallible APIs then do not exist, so no new caller can come back. The textual guards are deleted as
  redundant. This passes a `--cfg` to the existing `-Z build-std=core,alloc` build and needs no change
  under `compiler/` or `src/llvm-project`, so it is not an O5 step.
  - Before the flip, the dependencies that use `alloc` infallibly are replaced or given fallible paths:
    `crossbeam-queue` and `futures-util` (both built with their `alloc` feature in `kernel/Cargo.toml`)
    and the in-tree `gpu-abi` with its `alloc` feature.

## 8. What E3 and E10 build on

The fixed heap's tenants move in their own efforts. E1 provides the mechanisms:

| Tenant today | Moves in | Where it goes | What E1 must expose |
|---|---|---|---|
| Block cache file data (`block/cache.rs`, `MAX_BYTES` = 8 MiB, "one eighth of the smallest kernel heap") | E3 | File data into the page cache (frames). The block cache keeps metadata only, sized as a fraction of RAM. | `alloc_pages` with the `RECLAIMABLE` or `MOVABLE` class; `FrameMeta` room for owner, index and LRU (5.7); the shrinker registry; `kmem::stats()` for RAM-relative sizing |
| Dirty data (`fs/breenfs/dirty.rs`, `DIRTY_BUDGET` = 8 MiB per mount) | E3 | Dirty becomes a page flag. The limit is a fraction of RAM, with background writeback below it (audit R11). | Zone pressure flags and watermarks (5.3); the context argument E2 extends to "may sleep" and "may write back" (7.3) |
| BreenFS PENDING quarantine loaded whole at commit | Phase 1.2 (still building at DEV-37) coalesces it; E3 sizes it | Slab and span memory, with failure as an error | Fallible APIs (7.4) |
| Syscall staging copies (write copy-in, read staging, pipe staging) | E3 (file I/O), E12 (pipes as page rings) | Copies straight between user memory and page-cache frames. Until then, chunked staging in vmap allocations, not the fixed heap. | vmap allocations (6.5); a frame-vector type (an ordered list of leased frames with offsets) for scatter-gather, which E4's DMA from pinned pages also uses |
| Mapping-cache vectors (`PageVec`, `file_map.rs:246`, a sorted `Vec` of the cached pages; population's own `read` vector, `file_map.rs:1600`; the 64 KiB staging buffer) | E3 | The page cache's radix tree of frames | Slab-backed tree nodes; `grow_in_place` until E3 lands (6.4) |
| Socket buffers (a fixed default, `BUF_DEFAULT` in `socket/opts.rs:49`, with no autotuning; on the heap) | E10 | Page-fragment-backed packet buffers, autotuned under a global TCP budget that is a fraction of RAM | A page-fragment allocator: sub-page pieces of order-0..3 frames with a reference count, per-CPU, interrupt safe. Named slab caches with constructors for descriptors. `kmem::stats()` and pressure flags for the budget. |
| Device rings and receive buffers | E4 and E10 | The DMA API | `dma::alloc` with address limits, `dma::bus_address` (5.6); the emergency reserve for receive refill from interrupt context |

E1 does not change these budgets. After E1-7 their comments' reason, "the smallest kernel heap", no longer
holds. E3 and E10 re-derive them from RAM with these mechanisms. Raising them before E1 would only move the
heap OOM (audit section 4).

## 9. No fixed ceilings

| Ceiling today | After E1 |
|---|---|
| 64 MiB x86 heap; 64-128 MiB ARM64 heap | The heap grows from frames until memory runs out, then fails with an error after reclaim |
| `MAX_REGIONS` = 128 memory-map regions | A range set that grows as needed (4.1) |
| ARM64 RAM above 4 GiB unused (`DIRECT_MAP_RAM_END`) | The direct map covers the described RAM (4.4) |
| `MAX_TRACKED_FRAMES` = u32::MAX frames (16 TiB) through u32 ordinals | u64 pfns and a section table sized from the highest frame (4.2) |
| Frame metadata inside the heap | Metadata from frames, 0.78% of RAM (4.2) |
| Contiguous runs only at the frontier | Buddy runs up to 1 GiB at any time (5.1, 5.6) |
| Kernel stacks: x86 fixed window (about 990), ARM64 32 MiB pool (about 480) | Stacks from the buddy in a stack window sized from RAM (E1-9) |
| Two 64-slot fd and signal-table slabs | Ordinary slab caches |

What remains is constant by design and is not a RAM limit:

- the slab, span and vmap size thresholds;
- the 30-bit generation, which wraps exactly as today (`& 0x3fff_ffff`);
- the per-CPU batch bounds.

The 48-bit kernel VA half (128 TiB) bounds the windows. The direct map, span and vmap windows take about
6 × RAM of it (6.5), which is about 21 TiB of RAM; x86 also needs one PML4 slot per 512 GiB of window.
Supporting more than that is a 5-level paging (x86)
or 52-bit VA (ARM64) question, outside E1.

## 10. Sources

- Jeff Bonwick, "The Slab Allocator: An Object-Caching Kernel Memory Allocator", USENIX Summer 1994.
- Jeff Bonwick and Jonathan Adams, "Magazines and Vmem: Extending the Slab Allocator to Many CPUs and
  Arbitrary Resources", USENIX Annual Technical Conference 2001.
- Kenneth C. Knowlton, "A Fast Storage Allocator", Communications of the ACM 8(10), 1965.
- Donald E. Knuth, *The Art of Computer Programming*, vol. 1, §2.5, "Dynamic Storage Allocation".
- Paul R. Wilson, Mark S. Johnstone, Michael Neely and David Boles, "Dynamic Storage Allocation: A Survey
  and Critical Review", IWMM 1995.
- Emery D. Berger, Kathryn S. McKinley, Robert D. Blumofe and Paul R. Wilson, "Hoard: A Scalable Memory
  Allocator for Multithreaded Applications", ASPLOS 2000.
- Juan Navarro, Sitaram Iyer, Peter Druschel and Alan Cox, "Practical, Transparent Operating System Support
  for Superpages", OSDI 2002 (for what E12 will need from the buddy).
- Uresh Vahalia, *UNIX Internals: The New Frontiers*, ch. 12, "Kernel Memory Allocation".
- Marshall Kirk McKusick, George V. Neville-Neil and Robert N. M. Watson, *The Design and Implementation of
  the FreeBSD Operating System*, 2nd ed., ch. 6 (the textbook; no source).
- Intel 64 and IA-32 Architectures Software Developer's Manual, vol. 3A, ch. 4 (paging; global pages and
  invalidation, §4.10).
- Arm Architecture Reference Manual for A-profile (DDI 0487): VMSAv8-64 translation table descriptors,
  translation table updates, and TLB maintenance instructions.
- POSIX.1-2017 (The Open Group Base Specifications, Issue 7) and the Linux man pages, for errno values only.
- The Rust `alloc` crate's documentation of its allocation APIs and of the `no_global_oom_handling`
  configuration.

## 11. PR sequence

The PRs are in dependency order. Each is reviewable on its own and ships its tests check-first: the test
commit goes in first, red on the old code, then the change. The host tests run once at the PR's final head
(DEV-30). Each PR builds with 0 new warnings on x86-64 and ARM64. "x86-64" and "ARM64" say what the PR
changes there. "Boot" says whether one targeted ARM64 QEMU boot is needed, and why.

No PR runs an effort's suites. x86 evidence comes from the coordinator's batched R240 flash once QEMU shows
the gain (DEV-31(2)), and from the program's one final run (DEV-30(2)).

| PR | Title | Depends on | Size |
|---|---|---|---|
| E1-0 | This design | - | docs |
| E1-1 | One usable-RAM range set and a boot carve; no 128-region limit | E1-0 | S |
| E1-2 | ARM64 direct map over the described RAM | E1-1 | S |
| E1-3 | Frame metadata sections replace the heap ledger; free list through metadata; drop the empty-span lock | E1-1, E1-2 | M |
| E1-4 | `AllocError`, allocation contexts, the reclaim registry and the first shrinkers; OOM record on both architectures | E1-3 | S |
| E1-5 | Kernel VA windows: x86 window allocator and the ARM64 TTBR1 mapper, with batched flush before free | E1-3 | M |
| E1-6 | Slab caches with per-CPU magazines for requests up to 4 KiB | E1-4 | M |
| E1-7 | `heap_index` span arenas, vmap large allocations, in-place realloc, the DMA bus-address check; retire the fixed heap | E1-5, E1-6 | M |
| E1-8 | Buddy allocator, zones, per-CPU page caches, mobility pageblocks, watermarks, `dma::alloc`; frontier and LIFO list deleted | E1-7 | M |
| E1-9 | Kernel stacks from the buddy in a stack window | E1-5, E1-8 | S |
| E1-10..E1-19 | OOM conversions, one subsystem per PR (list below) | E1-4 (E1-7 for vmap-backed staging) | S each |
| E1-20 | Build `alloc` with `no_global_oom_handling`; replace infallible dependencies | E1-10..E1-19 | S |

### E1-1: one usable-RAM range set and a boot carve

- Change: a pure `memory/boot_ranges.rs` builds the normalised range set from either architecture's input,
  and `carve(bytes, align, below)` takes the highest fitting run. `frame_allocator::init` and
  `init_aarch64` consume it. `MAX_REGIONS` goes.
- x86-64: usable regions past the 128th are kept instead of dropped. Allocation behaviour is otherwise
  unchanged.
- ARM64: the frame pool's ranges go through the same type. Behaviour is unchanged.
- Tests (host, new crate `tools/kmem-host-tests`):
  - Check first: the existing region-extraction logic, moved unchanged into the pure module, is given
    200 usable regions. It keeps 128, which is red. After the change it keeps 200 of 200.
  - Unsorted and overlapping firmware entries normalise to disjoint, page-aligned ranges, and the 1 MiB
    floor is applied.
  - A carve with alignment and a `below` limit returns the highest fitting run, or an error when no run fits;
    the range set loses exactly the carved bytes.
- Boot: not needed. Boot memory order is unchanged.
- Rows: no row expected to move.

### E1-2: ARM64 direct map over the described RAM

- Change:
  - a pure planner turns ranges above the boot window into a minimal list of 1 GiB, 2 MiB and 4 KiB
    descriptors;
  - kernel glue writes them into the live TTBR1 tables, using table pages from the carve;
  - `direct_map_ram_window`'s clip goes.
- x86-64: no change.
- ARM64: RAM above 4 GiB joins the frame pool.
- Tests (host):
  - Check first: the kernel's window choice (`direct_map_ram_window`) moves unchanged into
    `arm64-boot-contract` as a pure function, and `frame_pool_from_fdt` is called with it on a device tree
    describing 6 GiB at 0x4000_0000. The pool ends at 4 GiB, which is red. After the change it covers the
    6 GiB, less the fixed ranges and reservations.
  - Planner: each page of each range is covered exactly once; no descriptor covers a non-RAM address;
    blocks are used wherever alignment allows; the number of table pages is minimal for the input.
- Boot: one ARM64 QEMU boot with the standard boot script and more than 4 GiB of guest RAM. Host tests
  cannot settle whether the MMU accepts the tables, which is a hardware effect. Cases: the boot-path
  milestone stages, and a census read showing frames above 4 GiB allocated. The prediction is written
  before the boot.
- Rows: no row moves on the 512 MiB rig, which has no RAM above 4 GiB.

### E1-3: frame metadata sections replace the heap ledger

- Change:
  - `FrameMeta` (4.3) in per-section arrays from the carve, before the heap exists;
  - the lease and refusal logic moves verbatim into a pure `memory/frame_meta.rs` over the array;
  - `frame_ordinal` becomes a pfn split;
  - the LIFO free list is threaded through `FrameMeta.use`, so the `Vec`, its capacity reservation and
    `LostContended` go;
  - `EXTERNAL_LEAF_SPANS` and its lock go;
  - `init_frame_ledger` goes;
  - the census walks sections.
  The frontier stays until E1-8. This PR moves storage and removes scans; it changes no allocation policy.
- x86-64 and ARM64: the same code on both. Up to 40-48 MiB of the R240's heap (at full frontier) and up to
  about 2.5 MiB of the rig's go back to other heap users. Boot memory order changes: the metadata now
  exists before the heap.
- Tests (host):
  - Lease equivalence: one property suite, with fixed seeds and a fixed operation sequence, runs against
    both the extracted old ledger and the new metadata. Double, stale, unallocated, untracked and
    live-leaf returns are refused identically, and generations only increase.
  - Check first: a return when the free list's reserved capacity is exactly full is `LostContended` on the
    old code, which is red. The new code records the return.
  - Check first: the old ledger built under a host allocator that fails returns `Err` (red). The new arrays
    come from carved ranges and need no allocator.
  - Lookup: a pfn in a section hole is untracked; each pfn in RAM maps to exactly one entry.
- Boot: one ARM64 QEMU boot of the boot-path gate (`--features testing`). The change reorders boot memory
  initialisation, which is publication order that host tests cannot settle. Cases: the boot-path milestone
  stages and the frame census.
- Rows: `memory.anonymous-touch` and `process.fork-exec`. Each fault and each shared fork page loses the
  empty-span mutex and two or three O(regions) scans. The free list's global lock stays until E1-8. `memory.driver-mmap` and
  `memory.driver-map-touch` lose the same per populated page.

### E1-4: allocation errors, contexts and reclaim hooks

- Change:
  - `kmem::AllocError`, `Ctx`, and the `in_reclaim` flag;
  - the shrinker registry and the slow path (7.2) wrapped around the existing fixed heap: on failure the
    heap's `alloc` runs the shrinkers and retries before returning null;
  - the first shrinkers: block cache (a new non-blocking `shrink`), mapping cache (on-demand eviction) and
    the BreenFS node cache;
  - `report_oom` on both architectures;
  - `FaultOutcome::OutOfMemory` with the existing SIGKILL policy, counted.
- x86-64 and ARM64: the same on both. A heap exhaustion that clean caches can cover is now survived instead
  of panicking. That is the #3226 and DEV-23 class.
- Tests (host):
  - Check first: a model heap filled to capacity, with a registered shrinker holding enough bytes. The old
    front (`alloc` then null) fails, which is red; the new one succeeds after one scan.
  - A shrinker that allocates gets an `Atomic` allocation and no recursive reclaim.
  - A shrinker whose lock is held is skipped, and the slow path returns `AllocError` without waiting, under
    a fixed interleaving in which the caller holds that lock.
  - Shrinkers run in cost order and stop at the target.
  - The block-cache and mapping-cache shrinkers have their own tests in `block-host-tests` and the
    mapping-cache fixture: only clean, unpinned, unbound entries are dropped. That includes the R1 case:
    pages populated but not yet bound are pinned and not dropped.
- Boot: not needed.
- Rows: no row expected to move. This is the failure path.

### E1-5: kernel VA windows

- Change: the span, vmap and stack windows (6.5), with a VA allocator per window.
  - x86-64 uses a window mapper over dedicated PML4 slots and PDPTs installed at boot (6.5), with
    compare-exchange table installs, and a pending-unmap list flushed through `memory/tlb.rs`.
  - ARM64 gets a TTBR1 4 KiB and 2 MiB mapper with `tlbi vaae1is` on unmap.
  - Only this PR's own self-check at init uses the windows yet.
- x86-64: kernel-half table pages for the windows are allocated on first use.
- ARM64: a new TTBR1 mapper.
- Tests (host):
  - A pure window model, with a fake TLB that records flushes per CPU. A frame of an unmapped range is
    not returned before a flush that covers it (check first: the model's naive "free on unmap" variant
    fails this).
  - A mapping that runs out of table pages part way unwinds, leaving no partial mapping.
  - Check first, address space: the existing x86 index selection (`map_kernel_page`'s PML4/PDPT choice,
    lines 165-191) is extracted unchanged into a pure function and given two addresses that differ only in
    their PML4 index (any two slots other than 402 and 403). It returns the same PDPT and PDPT index for
    both, which is red: the two windows alias. The window mapper resolves them to distinct entries, and a
    window of 4 × 128 GiB gets four slots.
  - Check first, table install: two simulated CPUs make first mappings under the same empty PD entry, in
    the fixed order read-empty (CPU 0), read-empty (CPU 1), install (CPU 0), install (CPU 1). On the
    extracted old walk CPU 0's page table is overwritten and its leaf is lost, which is red. With the
    compare-exchange install both leaves are present and one table page is returned.
  - Free-extent tree: allocations do not overlap and each keeps a guard page; freed extents coalesce;
    exhausting the window returns `NoVa`.
- Boot: one ARM64 QEMU boot of the boot-path gate. That the new TTBR1 mapper's descriptors and TLB maintenance work on the CPU
  is a hardware effect. Case: the init self-check maps, writes, unmaps and remaps a page in each window, and
  reports through the boot-path stages.
- Rows: no row expected to move.

### E1-6: slab caches with per-CPU magazines

- Change:
  - pure `memory/slab_cache.rs` (classes, slabs, descriptors) and `memory/magazine.rs` (per-CPU layer and
    depot);
  - glue to frames through the direct map;
  - `GlobalAlloc` routes requests up to 4 KiB there, and the fixed heap serves only larger requests;
  - `memory/slab.rs`'s bitmap caches become named caches;
  - `/proc/breenix/kmem` gains per-class rows.
- x86-64 and ARM64: the same on both. Most allocations by count leave the heap's global lock.
- Tests (host):
  - Check first: 4 simulated CPUs, each allocating and freeing 64 B objects under a fixed schedule. The
    old heap front takes its global lock on each operation; the model counts acquisitions, which is red
    against the requirement of 0 on the fast path. The magazine layer takes 0 until a magazine
    empties.
  - Conservation: across any sequence of allocations and frees on any CPUs, live + magazine rounds + depot
    rounds + free slab slots = total slots. No object is handed out twice.
  - Cross-CPU free: allocate on CPU 0 and free on CPU 1; the object returns to a slab of the right cache.
  - Drain: after `drain(cpu)` for each CPU and a depot trim, each slab is empty and returned except the
    one kept per cache.
  - Alignment: each object meets its layout's alignment, and no object crosses its slab's page.
  - Each class's objects per page and leftover tail match section 6.2's arithmetic, and each class is a
    multiple of 16.
- Boot: not needed. The heap's interface and its callers are unchanged. Slab pages are reached through the
  direct map, which the anonymous fault already writes through (`zeroed_frame`, `memory/anon_map.rs:235`),
  and the magazine layer's interrupt masking is settled by the host model.
- Rows: `process.fork-exec`, since process and thread creation allocate many small objects (the
  population path now reads into frames and one staging buffer, so the driver rows are not expected to move
  here).

### E1-7: span arenas, vmap allocations, and retiring the fixed heap

- Change:
  - span arenas running `heap_index` (6.4), with `grow_in_place` and `shrink_in_place`;
  - vmap allocations (6.5);
  - the new `realloc`;
  - `dma::bus_address` (5.6) replaces each driver's private `virt_to_phys`, and each caller that passes
    heap memory to a device moves to a static, a frame allocation, or `dma::alloc`'s E1-7 interim, which
    is the frontier's contiguous allocation;
  - the fixed heap is retired on both architectures (6.6).
- x86-64: the 64 MiB window at `0x4444_4444_0000` is no longer mapped at boot, and the heap grows on
  demand.
- ARM64: `place_kernel_pools` keeps only the stack pool. On the rig that returns about 64 MiB of a 512 MiB
  VM to the frame pool, less what the heap actually uses.
- Tests (host):
  - Check first: `heap_index` with a free block after an allocation. `realloc` to a larger size returns the
    same pointer, but the old `realloc` moves it, which is red.
  - The existing `tools/heap-host-tests` fixture runs unchanged against `heap_index` with the two new
    operations. The algorithm's visit bound still holds.
  - Span lifecycle: a span is mapped when no span fits and unmapped when its `used()` reaches 0, keeping
    one empty span per arena. The frames are returned only after the flush (fake TLB as in E1-5).
  - vmap: a 4 MiB allocation is backed by discontiguous frames; a `realloc` with free VA after it grows in
    place; one without moves the PTEs, and the bytes are unchanged with no copy (the model counts byte
    copies).
  - `dma::bus_address` refuses a span or vmap address and a direct-map range that crosses a discontiguity,
    and accepts a contiguous direct-map or image range.
- Boot: one ARM64 QEMU boot of the boot-path gate. Retiring the heap changes boot memory order, and the
  driver conversions change what devices are given. Cases: the boot-path milestone stages, which bring up
  the virtio block, net and input devices.
- Rows: `memory.driver-mmap` and `memory.driver-map-touch`. Population reserves a vector entry per
  wanted page and merges it into the sorted `PageVec` (`file_map.rs:1600-1601`, `merge_reserved` at
  `:335`); for the 262 MB driver those are multi-megabyte contiguous reservations, which become vmap
  allocations that grow in place instead of copying. `process.fork-exec` may move a little.

### E1-8: buddy allocator, zones and per-CPU page caches

- Change:
  - pure `memory/buddy.rs` (5.1, 5.5), `memory/zone.rs` (5.2, 5.3) and `memory/page_cache_cpu.rs` (5.4);
  - glue;
  - `allocate_frame`, `allocate_frame_for`, `allocate_frame_leased` and `allocate_contiguous_frames` keep
    their signatures and move onto it;
  - `dma::alloc`;
  - the frontier, `get_usable_frame`, the LIFO list and `contiguous.rs` go;
  - `/proc/meminfo` and `/proc/breenix/kmem` report per zone.
- x86-64: two zones on the R240 (8 GiB). The heap init's "DMA will fail" warning goes, because heap memory
  is no longer DMA memory, and DMA comes from `DMA32` by request.
- ARM64: one zone on the rig; two wherever RAM extends above 4 GiB (with E1-2, any VM given more than
  3 GiB at QEMU's and Parallels' 1 GiB RAM base).
- Tests (host):
  - Check first: `contiguous.rs` after the frontier has reached the end of a 64 MiB map, with 512 aligned
    frames freed back in random order. The old algorithm cannot return a 2 MiB run, which is red. The buddy
    can, and does.
  - Invariants, checked after each operation of a seeded random workload of 10^5 operations, with fixed
    seeds:
    - each free block is naturally aligned, in one zone, and on exactly one list;
    - no two blocks overlap;
    - free + cached + allocated = total;
    - no free head's buddy is a free head of the same order (full coalescing).
  - Fragmentation:
    - after freeing everything in a seeded random order, the free lists hold only maximal blocks;
    - with a mixed workload of 10% unmovable allocations held for its whole length, the number of 2 MiB
      pageblocks containing an unmovable frame stays within twice the minimum. The ungrouped variant fails
      this, as the check for 5.5.
  - Zones: a `NORMAL` request falls back to `DMA32` but does not take it below its reserve; a `DMA32` request
    does not get a `NORMAL` frame.
  - Per-CPU caches: refill and drain move `batch` frames per zone-lock hold; a remote drain under the
    per-CPU lock empties another CPU's cache; cached frames are not merged.
  - Watermarks: an ordinary allocation stops at `min` and enters the slow path; an `Atomic` one may use
    half of `min`.
  - OOM: with each frame allocated, `alloc_pages` returns `AllocError` after draining and shrinking, and
    does not panic.
  - The lease suite from E1-3 runs again over buddy-backed frames.
- Boot: one ARM64 QEMU boot of the boot-path gate. This replaces the allocator the boot stages use.
  Cases: the boot-path milestone stages and the census.
- Rows: `memory.anonymous-touch`, since an allocation becomes a per-CPU pop; `process.fork-exec`, for
  page-table and copy-on-write frames; `memory.driver-touch`, `memory.driver-mmap` and
  `memory.driver-map-touch`, for frames per populated page.

### E1-9: kernel stacks from the buddy

- Change:
  - kernel stacks become buddy blocks mapped into the stack window, each with an unmapped guard page
    below it;
  - x86 maps them on allocation instead of from a fixed eager window;
  - ARM64's 32 MiB physical pool and `ARM64_KERNEL_STACK_POOL_SIZE` go.
- x86-64 and ARM64: the thread limits of about 990 and 480 become memory limits.
- Tests (host):
  - Check first: the model allocates 1,000 stacks. The old ARM64 pool's bitmap refuses the 481st, which is
    red; the new window does not.
  - Each stack's guard page is unmapped. A freed stack's frames are returned only after the flush.
- Boot: one ARM64 QEMU boot of the boot-path gate. Stack memory is used from the first thread on, and its
  guard-page placement is an MMU effect. Cases: the boot-path stages.
- Rows: `process.fork-exec` (one stack per thread); recorded to show it does not regress.

### E1-10 to E1-19: the OOM conversions

Each conversion PR follows the same check-first shape (7.4): its structural guard goes in red, and where
the subsystem has a host-test crate, an allocation-failure sweep. Each converts its subsystem's runtime
paths and marks boot sites `boot:`. No row is expected to move; the rows are recorded to show that.

| PR | Subsystem | Error surfaced | Host sweep in |
|---|---|---|---|
| E1-10 | Syscall entry, user copies and staging buffers (`syscall/`) | `ENOMEM` | `regular-read-host-tests` |
| E1-11 | Memory syscalls and VMAs (`syscall/mmap.rs`, `syscall/memory.rs`, `memory/vma.rs`) | `ENOMEM` | `pagetable-host-tests` |
| E1-12 | Process and thread lifecycle: fork, clone, exec, spawn (`process/`, `task/`) | `EAGAIN` or `ENOMEM` per the call | structural guard only |
| E1-13 | VFS and path walk (`fs/vfs`, `fs/namei.rs`, `ipc/fd.rs`) | `ENOMEM` | `vfs-host-tests`, `resolve-host-tests` |
| E1-14 | BreenFS and ext2 runtime (`fs/breenfs`, `fs/ext2`) | `ENOMEM` before any disk change, by the reservation rule | `breenfs-host-tests`, `ext2-host-tests` |
| E1-15 | Network and sockets (`net/`, `socket/`) | `ENOBUFS` on send; counted drop on receive | `net-host-tests`, `udp-host-tests`, `unix-host-tests` |
| E1-16 | IPC: pipes, SysV and POSIX IPC, shared memory (`ipc/`) | `ENOMEM` or `ENOSPC` per the man page | `ipc-host-tests`, `shmsem-host-tests`, `shmem-api-host-tests` |
| E1-17 | Block layer and storage drivers' runtime paths (`block/`, `drivers/ahci`, `drivers/nvme`, `drivers/virtio/block*`) | I/O error to the request; not a partial request | `block-host-tests` |
| E1-18 | Other drivers, GPU and console runtime (`drivers/`, `gpu/`, `console/`) | Counted drop or the call's errno | `gpu-host-tests`, `console-host-tests` |
| E1-19 | Diagnostics: `/proc`, census, tracing dumps (`fs/procfs`, `capture/`) | `ENOMEM` on read | structural guard only |

- x86-64 and ARM64: the same on both.
- Boot: not needed. A fallible API has the same success path, and the sweeps settle the failure paths.

### E1-20: the compile-time end state

- Change:
  - replace or wrap `crossbeam-queue`, `futures-util` and `gpu-abi`'s infallible `alloc` uses;
  - pass `--cfg no_global_oom_handling` to the kernel's `alloc` build on both architectures;
  - delete the textual guards.
- Tests:
  - Check first: the kernel build with the cfg fails while any infallible use remains, which is red; it
    passes once no such use is left.
  - No runtime change, so no boot.
- Rows: no row expected to move.

## 12. Tests

- The allocator algorithms are pure modules, tested in one new crate, `tools/kmem-host-tests`. It includes
  them by `#[path]`, as `tools/heap-host-tests` includes `heap_index.rs`. The crate gets a line in
  `docs/host-tests.md`.
- Determinism:
  - random workloads use fixed seeds printed in the test name;
  - multi-CPU tests simulate CPUs as explicit parameters under a fixed schedule;
  - there are no threads, no timing and no retries.
- Each test checks behaviour that could plausibly break: lease refusal, conservation, coalescing,
  fragmentation, drain, flush before free, the OOM path and region limits. There is no test of a constant
  or of a struct's size.
- Check first: each PR's first commit is its tests, failing on the old code as named in section 11. The
  change follows. Where the old code has no pure form to test against, the first commit also extracts it
  unchanged, so the red result is the old behaviour's and not a stub's.
- Boots: E1-2, E1-3, E1-5, E1-7, E1-8 and E1-9 each need one ARM64 QEMU boot of the standard boot script,
  for the reason given in each PR, and only for the cases named. The other PRs need no boot. No PR runs an
  effort's suites (DEV-30).
- Kernel `boot_tests` that assume the old allocator are rewritten in the PR that removes what they test:
  `frame_allocator_tests.rs`, `retire_with_free_list_contended` and `free_list_len_for_gate`. They are
  not weakened to pass. Each rewrite tests the same property of the new structure, or is removed with the
  structure when the property no longer exists. `LostContended` is one; it cannot happen any more.

## 13. bbench rows (DEV-31)

- The kernel rows E1 can move are `memory.anonymous-touch`, `memory.driver-mmap`, `memory.driver-touch`,
  `memory.driver-map-touch` and `process.fork-exec`.
- Two rows are controls that E1 should not move:
  - `memory.copy-bytes` (already at parity on QEMU);
  - `process.getpid` (no allocation).
- The `memory.malloc-*` rows measure libc (E7).

The QEMU rig's last pair of series, 731f1d00, recorded 2026-11-27 22:49 UTC (5:49 PM ET), 4 vCPUs and
512 MiB, with medians from 5 samples each:

| Row | Breenix | Linux | Breenix/Linux | Expected to move in |
|---|---|---|---|---|
| `memory.anonymous-touch` | 0.0795 s | 0.0309 s | 2.57 | E1-3, E1-8 |
| `memory.driver-mmap` | 0.171 s | 1.09 µs | about 158,000 | E1-3, E1-7, E1-8 (a small part; the rest is E2 and E3) |
| `memory.driver-touch` | 5.17 ms | 1.34 ms | 3.87 | E1-8 |
| `memory.driver-map-touch` | 0.176 s | 1.34 ms | about 132 | as driver-mmap |
| `process.fork-exec` | 2.53 ms | 0.218 ms | 11.6 | E1-3, E1-6, E1-7, E1-8, E1-9 |
| `memory.copy-bytes` | 18.8 ms | 18.3 ms | 1.03 | control |
| `process.getpid` | 455 ms | 109 ms | 4.19 | control |

The predictions are directions, not numbers. The coordinator records a QEMU series before and after each
PR (DEV-31(3)), and the ratio is the judgement.

- E1 removes allocator work from these rows. It does not remove the larger parts:
  - the anonymous fault's two PROCESS_MANAGER acquisitions and `find_process_by_cr3` (E2 and E8);
  - driver-mmap's eager whole-file populate (E2 and E3);
  - fork's full address-space copy (E8).
- So E1 is not expected to reach parity on any row. A row named here that does not move after its PR is a
  finding: it means the allocator was not the cost this design assumes, and it is reported with the
  series, not explained away.

On the R240, the batched flash after E1-8 is followed by one R240 series (DEV-31(2)). The 8 GiB box is
where metadata leaving the heap matters most (section 1.1).

## 14. Risks and questions for the coordinator

1. x86 early boot is checked only on the R240. E1-3, E1-7, E1-8 and E1-9 change x86 boot memory order, and
   lanes have no x86 QEMU. Recommendation: batch those PRs' x86 check into one coordinator flash after
   E1-8. That is DEV-31(2)'s rule. Once DEV-32 and DEV-33's one-reboot flash with try-once rollback is in
   service (#3247), a bad kernel falls back by itself.
2. x86 kernel unmaps use the NMI shootdown until E9. Section 6.5 batches them, and spans shrink with
   hysteresis, so they are rare. A workload that frees large vmap buffers at a high rate pays an NMI round
   per 16 MiB, which is the cost to watch in E1-7's series.
3. Interim DMA in E1-7: drivers converted off heap memory before the buddy lands use the frontier's
   contiguous allocation for one PR's span. E1-8 replaces it. The alternative, landing the buddy first, puts
   the largest PR ahead of the heap work the audit says goes first.
4. `BTreeMap` has no fallible insert in `alloc`. The 67 `BTreeMap::new` sites move to a slab-backed ordered
   map with `try_insert`. That map is new code, and gets its own host tests in the first conversion PR that
   needs it.
5. The scale of E1-10..E1-19. The pattern counts in 7.4 include tests and boot code. The real runtime count
   is unknown until each subsystem's guard lists it. If one subsystem's PR would be too large to review, it
   splits by module with the same guard. The guard's file list says which part is done.
6. The 32-byte `FrameMeta` costs 0.78% of RAM, against today's 12 bytes plus 8 bytes of free-list
   capacity. That buys O(1) buddy links, the slab back-pointer, and no heap dependency. E3 may grow it to
   64 bytes (5.7).
7. No step in this plan changes BreenFS's on-disk format, or anything under `compiler/` or
   `src/llvm-project`.
