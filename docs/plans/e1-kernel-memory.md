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
  - ARM64: the same inputs `platform_config::frame_pool()` reads today, with the clip to 4 GiB removed
    (section 4.4). Removing the kernel's clip is not enough on its own, because two limits upstream of it
    also drop RAM:
    - `arm64-boot-contract`'s `RangeSet` holds at most `MAX_RANGES = 32` ranges
      (`libs/arm64-boot-contract/src/phys_memory.rs:35`) and sets `truncated` when it drops one
      (lines 122-127, 168-171). A UEFI map or device tree with more fragments loses RAM before the kernel
      sees it. E1-1 feeds the kernel's growable range set straight from the firmware description: the
      device tree through the contract's existing visitor (`fdt::walk`, line 488), and the UEFI map through
      `final_uefi_conventional_ranges`'s descriptor filter (line 16), split out so it yields ranges instead
      of building the fixed set. The fixed set is then no longer on the frame-pool path. The fixed set stays for the loader's own early use, where its
      `truncated` flag becomes a boot error rather than a silent drop.
    - The output address width is fixed at 40 bits (`TCR_IPS_40BIT`, `arch_impl/aarch64/boot.S:82`, and
      the same constant in `parallels-loader/src/page_tables.rs:112`), so RAM at or above 1 TiB cannot be
      mapped. E1-2 sets TCR_EL1.IPS from ID_AA64MMFR0_EL1.PARange (Arm ARM, the ID_AA64MMFR0_EL1 and TCR_EL1
      register descriptions) at both sites. RAM above the width the CPU reports is excluded from the range
      set and counted in `/proc/breenix/kmem`, so it is reported rather than silently missing.
- There is no `MAX_REGIONS`. The range set lives in a page carved from the largest range, and grows by
  carving another page if the firmware describes more. Building it takes two passes over the firmware
  description: the first finds the largest range, and the second fills the set.
- A boot carve takes `(bytes, align, below)` and removes the highest fitting run from the range set. It
  feeds the metadata pages, section descriptors and directory (4.2), and ARM64's extra direct-map tables (4.4). It returns
  an error instead of panicking. A boot that cannot carve its metadata still stops, at an explicit
  `expect` naming the shortfall.
- After the buddy allocator is seeded (5.1), the carve is closed. The range set is not allocated from again.

### 4.2 Frame metadata sections

- Each piece of the metadata fits in one contiguous 4 KiB page, and address-space holes cost
  0 bytes below the top level. A sparse map can have ample RAM in total and still no 1 MiB run to spare, so
  a design that needs a contiguous array per section can fail to place it. A flat table indexed by
  section also grows with the highest address, holes included.
- Metadata pages: one 4 KiB page holds 4096 / `size_of::<FrameMeta>()` entries (128 at 32 bytes), so it
  describes 512 KiB of physical address space. A metadata page exists only where that 512 KiB holds
  usable RAM. Each page is carved separately and can come from any range the direct map covers (4.4).
- Sections of 2^15 frames (128 MiB, aligned): a section that holds any usable RAM has one 2 KiB descriptor,
  an array of 256 pointers to its metadata pages. A null pointer means "no RAM here".
- The directory is two-level and sparse:
  - a root with one 8-byte entry per 64 GiB of physical address space, up to the highest usable frame;
  - leaf pages of 512 section-descriptor pointers, present only for a 64 GiB span that holds RAM.
- Lookup: `root[pfn >> 24]`, then `leaf[(pfn >> 15) & 511]`, then `section[(pfn >> 7) & 255]`, then
  `page[pfn & 127]`. That is four dependent loads, O(1), each in a structure that is at most one page. A
  null at any level is `RefusedUntracked`. `frame_ordinal`'s O(regions) scan goes.
- Sizes:
  - an entry is 32 bytes, so metadata is 0.78% of the RAM it describes, rounded up to 512 KiB per range edge;
  - a section with RAM adds a 2 KiB descriptor (0.0016% of 128 MiB);
  - a 64 GiB span with RAM adds one 4 KiB leaf page;
  - holes cost 8 bytes of root per 64 GiB of address space and 0 bytes below it. A machine whose highest
    frame is at 256 TiB has a 32 KiB root.
- Worked numbers:
  - the 512 MiB QEMU rig: 4 sections, 4 MiB of metadata pages, 8 KiB of descriptors and one leaf page.
    Today's ledger and free-list capacity for those 131,072 frames take up to about 2.5 MiB of the heap;
  - the R240's 8 GiB: about 64 MiB of metadata pages and about 128 KiB of descriptors, outside the heap.
    Today it is up to 40-48 MiB inside it.
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
  `RefusedDoubleRelease`. On `ST_NEVER` it is `RefusedNeverAllocated`. A frame with no metadata page is
  `RefusedUntracked`. A frame with live leaf references is `RefusedLiveLeaf`. Each refusal leaks the frame
  and counts it, as now.
- A frame in a per-CPU cache or on a buddy list is `ST_FREE` in `state_gen`; `flags` says where.
  `LostContended` disappears: a free list threaded through the metadata needs no capacity, so each return can
  be recorded.
- A block of order n is claimed frame by frame: each frame's generation advances. The block lease records
  the head pfn, the order and the head's generation.
- `BlockLease` is neither `Copy` nor `Clone`, unlike today's `FrameLease` (`frame_allocator.rs:241`). A
  return or a split takes it by value, so one block lease authorises exactly one of them.
- A block return runs in two passes. Both apply to each of the block's 2^n frames, the head included:
  1. Check, with no writes: the head's generation matches; each tail is `ST_ALLOCATED`, flagged `TAIL`,
     with the lease's order; and each frame's `leaf_refs` is 0. A failure refuses the return
     (`RefusedLiveLeaf` for a mapped frame) and leaks the block, as a refusal does today.
  2. Transition each frame by today's per-frame rule (`return_lease`, `frame_allocator.rs:1119-1132`):
     re-read `leaf_refs`, then compare-exchange `ST_ALLOCATED` to `ST_FREE`.
     `acquire_leaf_mapping` raises `leaf_refs` before it re-checks the state (lines 524-533), so a mapping
     that races the return either sees `ST_FREE` and backs out, or is seen by the return's re-read.
     If any frame is refused in this pass, the block is leaked whole. Frames already moved stay `ST_FREE`
     with no list flag, are counted as leaked, and are not put on a list. So a later return of one of
     them is refused as a double release, and 0 of them are reused.
  The block is inserted into the buddy only when 2^n of its 2^n frames have moved.
- `split_lease(BlockLease) -> per-frame leases` consumes the block lease. Under the zone lock it clears
  `TAIL` and sets order 0 on each frame, and it advances the head's generation. The per-frame leases it
  issues carry each frame's current generation. A forged or retained copy of the block lease then fails
  the head-generation and `TAIL` checks. `split_lease` is for callers that free pages one at a time; the
  existing `allocate_contiguous_frames` keeps its per-frame contract by calling it.
- `EXTERNAL_LEAF_SPANS` is deleted (section 1.1: no code writes it). `register_external_leaf_frame`, which
  marks RAM frames, is unchanged.

The census walks the directory instead of the chunks. `/proc/breenix/frames` keeps its lines and owner set.
Zone, order and per-CPU cache counts go in a new `/proc/breenix/kmem`.

### 4.4 The direct map covers the described RAM (ARM64)

- Before the buddy is seeded, ARM64 builds direct-map entries for each usable range above the boot
  window, at `HHDM_BASE + pa`.
  - It uses 1 GiB block descriptors where a range covers an aligned GiB, 2 MiB blocks elsewhere, and 4 KiB
    pages only at unaligned edges. Table pages come from the boot carve with `below` set to the end of
    the boot window (`DIRECT_MAP_RAM_END`), because the kernel writes them through the existing direct
    map before the new entries exist. The carve's default, the highest fitting run, would otherwise pick
    pages in the RAM those tables are about to map: on E1-2's 6 GiB fixture that is near 7 GiB, above the
    window. If the boot window cannot spare the table pages, the extension stops with that shortfall named
    and the range set keeps only the RAM below the window, as today.
  - The carve's users are ordered by what they can reach: E1-2's table pages from the boot window first,
    then the metadata of 4.2 from anywhere the extended direct map covers.
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
- Fallback order: `NORMAL` falls back to `DMA32`, which keeps a fallback reserve against those fallback
  requests only. `DMA32`'s own requests see only its watermarks (5.3). `DMA32` requests do not fall forward.
  - The reserve is min(`NORMAL` frames / 256, `DMA32` frames / 4). Without the second term, a small
    `DMA32` zone under a large `NORMAL` one has a reserve larger than itself. For example, 64 MiB below
    4 GiB and 64 GiB above gives 256 MiB, and `NORMAL` could never fall back. With it, the reserve is
    16 MiB, and 32-bit devices keep 3/4 of the zone (less its `min`) against `NORMAL` demand.
- ARM64's fixed 2 MiB non-cacheable block (`KERNEL_FIXED_RANGES[1]`) is a memory-attribute requirement for
  xHCI, not an address limit. It stays outside the buddy as it is.

### 5.3 Watermarks

- Each zone has `min`, `low` and `high`:
  - `min` = min(max(128 frames, zone frames / 1024), zone frames / 16);
  - `low` = 5/4 of `min`;
  - `high` = 3/2 of `min`.
  They are computed from the zone's size at boot, with no upper constant. The zone / 16 term keeps a small
  zone usable: without it, a 1 MiB `DMA32` zone (256 frames) would hold half of itself as `min`. With it,
  `min` is 16 frames, and `high` stays under 1/10 of the zone for each zone size.
- An ordinary allocation that would take the zone below `min` enters the slow path (section 7.2).
- Allocations from interrupt context, and from inside reclaim, may use half of `min` (the emergency
  reserve) before failing.
- Crossing `low` sets the zone's pressure flag, which E3's page daemon will wait on. In E1 the slow path
  reads it.

### 5.4 Per-CPU page caches

- Each CPU keeps, per zone and per mobility class (5.5), a list of order-0 frames. This is the magazine
  idea of Bonwick and Adams 2001 applied to frames.
  - The lists are per class so that a frame refilled from a `MOVABLE` pageblock cannot be handed to an
    `UNMOVABLE` request. One mixed list per zone would let a single cache scatter slabs and page tables
    across movable pageblocks, which undoes 5.5's grouping.
  - `batch` = clamp(zone frames / (1024 × CPUs), 8, 64), shared by the zone's three lists.
  - `high` = 4 × `batch` per list.
  - Allocation pops from its own class's list; an empty list refills `batch` frames of that class from the
    buddy under one zone-lock hold.
  - A free goes to the list of the class recorded in the frame's `FrameMeta.flags`, which is the class
    of the pageblock it was allocated from, not the class the freeing caller names. Above `high`,
    `batch` frames go back to the buddy.
- Each per-CPU list has its own spin lock, taken with interrupts masked.
  - On the owner it is uncontended, one atomic.
  - It lets any CPU drain another CPU's cache without an IPI.
  - Drains happen on watermark `min`, on a failed contiguous allocation (5.6), on CPU offline, and from the
    reclaim slow path.
- Cached frames carry `PCP_CACHED`, so the buddy does not merge them and the census counts them separately.

### 5.4a Bootstrap and CPU registration (page caches and magazines)

The per-CPU layers of 5.4 and 6.3 cannot assume per-CPU state exists when the first allocation runs:

- ARM64 builds its heap and frame pool (`main_aarch64.rs:903-908`) long before `per_cpu_aarch64::init`
  sets TPIDR_EL1 (line 1258). x86 runs `per_cpu::init` (`main.rs:202`) before `memory::init` (line 228),
  so the boot CPU's id is readable there, but no AP is registered.
- The scheduler's, the tracing layer's and ARM64's per-CPU arrays are fixed at 8 CPUs
  (`task/scheduler.rs:1961`, `arch_impl/aarch64/constants.rs:514`), and x86's stack layout at 256
  (`memory/layout.rs:213`). E1 must not add another such cap.

So:

- A global `KMEM_PERCPU` slot table is sized at boot from the CPU count firmware describes (the device
  tree's `cpus` node or MADT entries on ARM64; MADT on x86), and carved from the range set like the
  metadata. It holds one pointer per described CPU to that CPU's page caches and magazines. It is not
  sized from a `MAX_CPUS` constant.
- A slot starts null. While a CPU's slot is null, or while that CPU cannot yet name itself (ARM64 before
  TPIDR_EL1 is set), its allocations take the shared path: the zone lock for frames, and the slab cache's
  depot and slabs under the cache lock for objects. That path is correct, only slower, so allocation works
  from the first heap use with no per-CPU state.
- Registration: a CPU's caches are built from the shared path and published into its slot with a release
  store, by that CPU, after its per-CPU id is readable and before it enables interrupts. For the boot CPU
  that is right after `per_cpu_aarch64::init` or `per_cpu::init` and the heap; for an AP it is in its
  bring-up, before it joins the scheduler.
- The fast path reads its own slot with an acquire load. A remote drain reads a slot only after the
  owner has published it.
- CPU offline (when it exists) drains both layers into the shared ones and clears the slot.
- The 8-CPU caps above are E9's ("CPU identity beyond 8"). Until E9 a CPU past them is not brought up, so
  the memory layer has a slot for it that stays null.
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

- `allocate_contiguous_frames(count, out)` keeps its per-frame result and gains an address limit:
  `allocate_contiguous_frames(count, limit, out)`. Today it has no limit, and its 7 callers (AHCI,
  RTL8139, xHCI, virtio `block_mmio` and `queue`, `gpu/dma.rs` and `memory/stack.rs`) take whatever run
  comes back. Drivers then test the address themselves and give up when it is above 4 GiB (see the driver list below).
  Each caller passes its limit, and drivers move to `dma::alloc`.
- The run is exact, not rounded to a power of two. RTL8139's receive buffer is exactly 3 frames
  (`RX_BUFFER_FRAMES`, `drivers/rtl8139.rs:68`). An allocator that only takes an order-2 block would refuse
  a free 3-frame run that is not inside one aligned 4-frame block, and that disables the device. So:
  1. it first takes a free block of order at least ceil(log2(count)) in the zone, `split_lease`s it, and
     returns the excess frames to the buddy at once;
  2. if the zone has no free block of that order or above, it looks for `count` frames made of adjacent free
     blocks of lower orders. For each free head below that order, it follows the frames after the block
     through `FrameMeta` while they are free heads, until `count` frames are covered, under the zone
     lock. This is the slow path, bounded by the zone's free heads times `count`. It claims the run's
     blocks in one zone-lock hold, or claims 0 of them;
  3. before failing it drains each CPU's page cache once and runs the reclaimable shrinkers (7.3), then
     repeats steps 1 and 2 once.
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
  happens to work. After E1-9 it would not. The checked `bus_address` turns that into an error at the call,
  and E1-9 converts each caller it finds, using `dma::alloc`, which E1-8 has by then (section 11).

### 5.7 Room for E3 and for swap

- `FrameMeta` grows to 64 bytes when E3 adds the page cache's owner object, index, LRU links, flags and
  reference count. The entries per metadata page and the section descriptor length follow from `size_of::<FrameMeta>()`, so that is one constant (64 entries per page and 512 pointers per descriptor at 64 bytes). The cost is
  1.56% of RAM.
- The reclaim registry (7.3) has a context argument that E2 extends with "may sleep" and "may do I/O", and
  E3 with "may write back". Swap would be one more shrinker over anonymous pages, plus its own on-disk
  format (DEV-29 Q4). No part of E1 assumes anonymous memory is unreclaimable.

## 6. The kernel heap: slab caches, magazines, span arenas and large mappings

### 6.1 Routing

An allocation's layer is chosen from its `Layout`. The rows are tested in order, and the first match wins,
so each layout has exactly one layer:

| # | Request | Layer | Backing |
|---|---|---|---|
| 1 | size <= 4096 and align <= 4096 | slab cache of the class (6.2) | order-0 frames through the direct map |
| 2 | size <= 512 KiB and align <= 4096 | span arena running `heap_index` (6.4) | 2 MiB spans in the span window |
| 3 | otherwise: size > 512 KiB, or align > 4096 at any size | vmap allocation (6.5) | discontiguous order-0 frames in the vmap window |

- Alignment above 4096 is a property of the virtual address, so it goes to vmap, whose VA allocator
  aligns the extent to max(align, 4096). The frames under it need no alignment. Span arenas get only
  align <= 4096. `heap_index` places its boundary tag in-band before each block. A 4 KiB block aligned to
  2 MiB inside a 2 MiB span would have to start at offset 0, where the tag must be, so it cannot fit.
- An alignment larger than the vmap window returns `AllocError`. There is no fourth case.
- Zero-size layouts do not reach `GlobalAlloc` (Rust's allocation APIs handle them).

A free is routed by the pointer's address, not by its layout:

- an address in the span window or the vmap window belongs to that layer (two range compares);
- otherwise it is a direct-map address, and `FrameMeta.use` names its slab (6.2).

The layout passed to `dealloc` is then checked against the layer's own record of the block: the slab's
class, `heap_index`'s block size, or the vmap extent's length. A mismatch is corruption and panics with
both values, as in 6.2. So a `realloc` that keeps a block in place while its new size would route
elsewhere cannot send the later free to the wrong layer. An example is a 1 MiB vmap block shrunk to
300 KiB. The block stays in the layer that holds it until it moves.

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
  - stack window (E1-10): reserved for kernel stacks.
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
  - It installs table pages from the page layer, zeroed and made visible with `dsb ishst` before the table
    descriptor that links them is written. Then it writes 4 KiB or 2 MiB descriptors into invalid entries,
    then `dsb ishst; isb`.
  - Unmapping writes an invalid descriptor, then `dsb ishst`, then `tlbi vaae1is` per page (any ASID, inner
    shareable), then `dsb ish; isb` (Arm ARM, TLB maintenance). Without the `dsb ishst`, the TLBI can
    complete before a peer's table walker sees the invalid descriptor, and the peer can refill the old
    translation afterwards. The mapper calls the existing `arch_stub::tlb::flush_pages`
    (`memory/arch_stub.rs:937-948`), which already issues that sequence, instead of writing its own.
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
  - On ARM64 the `flush_pages` sequence at unmap is that flush, since the TLBI is broadcast by hardware
    and the closing `dsb ish` waits for it to complete on each PE in the inner shareable domain.
  - On x86, whose kernel pages are global (Intel SDM vol. 3A §4.10, global pages and TLB invalidation),
    the batch is flushed on each CPU through the existing shootdown transport (`memory/tlb.rs`). That happens when the pending list passes
    16 MiB, when VA allocation fails, or from the shrinker.
  - That flush must remove global entries. The existing `tlb::flush_all` and its `KIND_ALL` request
    (`tlb.rs:185`, `:578`) reload CR3, and CR4.PGE is set at boot (`memory/mod.rs:123`). A CR3 load
    leaves global translations in place (SDM vol. 3A §4.10.4.1), so a CPU could acknowledge that flush and
    still translate the window page to a frame the allocator has recycled. E1-5 adds a request kind that
    does one of two things on each CPU, local and remote alike:
    - `invlpg` for each page of the batch, which removes global entries for that page, when the batch is
      at most 32 pages;
    - otherwise it clears and restores CR4.PGE, which invalidates the TLB including global entries
      (SDM vol. 3A §4.10.4.1). That is the
      same sequence as `arch_impl/x86_64/pat.rs`'s `flush_tlb`.
    `flush_all`'s existing callers are unchanged. Window frees use only the new kind.
  - Spans shrink with hysteresis and vmap frees are batched, so these flushes are rare. E9's IPI transport
    replaces the NMI underneath without changing this rule.

### 6.6 The allocator's front

- `#[global_allocator]` becomes `KernelAllocator`:
  - `alloc` routes by layout and returns null only after the slow path (7.2);
  - `dealloc` cannot fail. It routes by address (6.1);
  - `realloc` stays in place where the block's current layer allows it: the same slab class,
    `grow_in_place` or `shrink_in_place` in a span, or the vmap move. Otherwise it allocates in the new
    layout's layer, copies and frees. A shrink in place keeps the block in its layer. The layer records
    the new size, so a later `dealloc` with the new layout passes 6.1's check.
- Explicit APIs for code that can say more than `Layout` can:
  - `kmem::try_alloc(layout, Ctx)` and `kmem::alloc_pages(order, zone, Ctx)`.
- `Ctx` is `Normal`, `Atomic` or `NoReclaim`. Each is a list of steps the allocator may take, enforced in
  the allocator and not left to the caller:

  | Step | `Normal` | `NoReclaim` | `Atomic` |
  |---|---|---|---|
  | Own CPU's magazine or page-cache pop | yes | yes | yes |
  | Refill from a depot, slab list or zone, under its lock | lock | lock | `try_lock` only; busy means fail |
  | Grow a span or vmap (maps pages, may take table pages) | yes | yes | no |
  | Flush the pending-unmap list (an x86 shootdown waits for each CPU) | yes | yes | no |
  | Slow-path drains of other CPUs (`try_lock`, 7.2 step 2) | yes | yes | no |
  | Shrinkers (7.2 steps 3-4) | yes | no | no |
  | Emergency reserve (5.3, half of `min`) | no | no | yes |

  - The context is derived, not only passed: `GlobalAlloc` reads the CPU's interrupt nesting (the
    preempt count's interrupt bits, `in_interrupt()` in `per_cpu.rs:690` and `per_cpu_aarch64.rs:615`)
    and the `in_reclaim` flag. In an interrupt handler or inside a shrinker, a `Normal` request is
    lowered to `Atomic`. A caller holding a shrinker's lock passes `NoReclaim`. A request can be lowered,
    and is not raised.
  - So an interrupt-context miss reaches at most one `try_lock` per layer and the reserve. It does not
    spin on a contended refill, run reclaim or wait for a shootdown.
  - Each step checks the context in the allocator's code, and `kmem::stats()` counts refusals per
    context, so a refused step shows up in `/proc/breenix/kmem`.
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

1. Drain this CPU's page caches and magazines into the shared layers.
2. Drain the other CPUs' per-CPU memory, each under that CPU's per-CPU lock, taken with `try_lock` so an
   owner in its fast path is skipped rather than waited for:
   - their page caches, into the buddy;
   - their loaded and previous magazines, into each cache's depot. A magazine holds objects, not frames,
     so the step that frees memory is the next one;
   - their span arenas' retained empty span (6.4), which is unmapped onto the pending-flush list.
3. Return depot surplus to slabs and empty slabs to the page layer (the slab shrinker, first in 7.3), so
   that magazine rounds drained in step 2 become free frames.
4. Run the remaining registered shrinkers in cost order (7.3), asking for the shortfall plus `batch`. Stop
   as soon as the target is met.
5. Retry the allocation once.
6. Return `AllocError`.

The context limits the steps (the table in 6.6). `NoReclaim` runs steps 1, 2, 5 and 6. `Atomic` runs
step 1 with `try_lock`, then tries the emergency reserve, then step 6. It does not enter steps 2-4.

- The slow path holds no allocator lock while it runs shrinkers.
- A per-CPU `in_reclaim` flag makes any allocation a shrinker makes `Atomic`, and stops reclaim from
  recursing.
- The work is bounded: each shrinker scans at most its target plus a fixed batch.
- There is no sleeping and no waiting in E1. E2 adds a sleeping step 4b for contexts that may sleep, and E3
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
  4. BreenFS decoded-node cache, under the mount lock's `try_lock` (`fs/breenfs/mount_lock.rs:101`). It
     is registered only if its entries can be dropped from one end of an order in bounded steps. If
     `node_cache.rs`'s structure cannot do that without a full scan, it waits for E3 like the mapping
     cache below.
- The mapping cache is not an E1 shrinker. `evict` (`file_map.rs:1138`) takes its map lock with a blocking
  `lock()`, needs the filesystem write guard its caller holds, and is a full scan: `retain` over each cached
  page, testing it against each binding and population claim. That makes it O(pages × bindings). A
  shrinker built on it would block, and its work would not be bounded by its target. The mapping cache
  keeps today's eviction request at unbind (`file_map.rs:529`). E3's page cache, with an LRU that can be
  trimmed from one end under `try_lock`, is where file pages become reclaimable.
- Each registered shrinker meets two rules, tested in its own module: it takes its lock with `try_lock`
  only, and its work is bounded by its target plus a fixed batch, with no full scan. The block cache
  meets both with a new `shrink` entry point: its state is a spin `Mutex` with an LRU ordered map
  (`block/cache.rs:199`, `:276`), so it can pop clean, unpinned entries from the cold end.

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

E1 does not change these budgets. After E1-9 their comments' reason, "the smallest kernel heap", no longer
holds. E3 and E10 re-derive them from RAM with these mechanisms. Raising them before E1 would only move the
heap OOM (audit section 4).

## 9. No fixed ceilings

| Ceiling today | After E1 |
|---|---|
| 64 MiB x86 heap; 64-128 MiB ARM64 heap | The heap grows from frames until memory runs out, then fails with an error after reclaim |
| `MAX_REGIONS` = 128 memory-map regions | A range set that grows as needed (4.1) |
| ARM64 RAM above 4 GiB unused (`DIRECT_MAP_RAM_END`) | The direct map covers the described RAM (4.4) |
| `MAX_TRACKED_FRAMES` = u32::MAX frames (16 TiB) through u32 ordinals | u64 pfns and a sparse two-level directory whose only address-sized part is 8 bytes per 64 GiB (4.2) |
| Frame metadata inside the heap | Metadata from frames, 0.78% of RAM (4.2) |
| Contiguous runs only at the frontier | Buddy runs up to 1 GiB at any time (5.1, 5.6) |
| Kernel stacks: x86 fixed window (about 990), ARM64 32 MiB pool (about 480) | Stacks from the buddy in a stack window sized from RAM (E1-10) |
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
| E1-7 | `heap_index` span arenas, vmap large allocations, in-place realloc, the DMA bus-address check; built and self-checked, not yet routed | E1-5, E1-6 | M |
| E1-8 | Buddy allocator, zones, per-CPU page caches, mobility pageblocks, watermarks, `dma::alloc` with address limits; each direct frame caller in drivers converted; frontier and LIFO list deleted | E1-7 | M |
| E1-9 | Route the heap to span arenas and vmap; move heap buffers given to devices to `dma::alloc`; retire the fixed heap | E1-7, E1-8 | S |
| E1-10 | Kernel stacks from the buddy in a stack window | E1-5, E1-8 | S |
| E1-11..E1-20 | OOM conversions, one subsystem per PR (list below) | E1-4 (E1-9 for vmap-backed staging) | S each |
| E1-21 | Build `alloc` with `no_global_oom_handling`; replace infallible dependencies | E1-11..E1-20 | S |

### E1-1: one usable-RAM range set and a boot carve

- Change: a pure `memory/boot_ranges.rs` builds the normalised range set from either architecture's input,
  and `carve(bytes, align, below)` takes the highest fitting run. `frame_allocator::init` and
  `init_aarch64` consume it. `MAX_REGIONS` goes.
- x86-64: usable regions past the 128th are kept instead of dropped. Allocation behaviour is otherwise
  unchanged.
- ARM64: the frame pool's ranges go through the same type, fed from the device tree visitor and the
  split-out UEFI descriptor filter instead of the contract's 32-range `RangeSet` (4.1). On a map with at
  most 32 ranges the result is unchanged.
- Tests (host, new crate `tools/kmem-host-tests`):
  - Check first: the existing region-extraction logic, moved unchanged into the pure module, is given
    200 usable regions. It keeps 128, which is red. After the change it keeps 200 of 200.
  - Check first, ARM64: `frame_pool_from_fdt` on a device tree with 40 disjoint memory ranges keeps 32 and
    sets `truncated`, which is red. The new path keeps 40 of 40. The same holds for a UEFI map of 40
    conventional descriptors through `final_uefi_conventional_ranges`.
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
  - kernel glue writes them into the live TTBR1 tables, using table pages carved below the boot window's
    end, which the existing direct map already covers (4.4);
  - `direct_map_ram_window`'s clip goes;
  - TCR_EL1.IPS comes from ID_AA64MMFR0_EL1.PARange instead of the fixed 40-bit constant, in `boot.S` and
    in `parallels-loader`'s `page_tables.rs`. RAM above the reported width is left out of the range set and
    counted (4.1).
- x86-64: no change.
- ARM64: RAM above 4 GiB joins the frame pool, up to the CPU's physical address width.
- Tests (host):
  - Check first: the kernel's window choice (`direct_map_ram_window`) moves unchanged into
    `arm64-boot-contract` as a pure function, and `frame_pool_from_fdt` is called with it on a device tree
    describing 6 GiB at 0x4000_0000. The pool ends at 4 GiB, which is red. After the change it covers the
    6 GiB, less the fixed ranges and reservations.
  - Planner: each page of each range is covered exactly once; no descriptor covers a non-RAM address;
    blocks are used wherever alignment allows; the number of table pages is minimal for the input.
  - Table placement: on the 6 GiB fixture, the planner's table pages are carved with an unbounded carve
    (the highest fitting run) and land above 4 GiB, outside the mapped window, which is red. With `below`
    set to the window's end, each table page lies inside the window. With the window's spare RAM made
    smaller than the tables need, the extension returns the shortfall and the pool stays at the window.
  - Address width: the IPS encoding is a pure function of PARange. For each PARange value the Arm ARM
    defines, the planner accepts ranges that end at or below 2^width and leaves out those above it, and
    the excluded bytes are counted. Check first: today's choice, extracted as a function that ignores
    PARange, returns the 40-bit encoding for PARange 0b0101 (48 bits). A device tree range at 2 TiB, which
    that CPU can address, then cannot be mapped, which is red. The new choice maps it.
- Boot: one ARM64 QEMU boot with the standard boot script and more than 4 GiB of guest RAM. Host tests
  cannot settle whether the MMU accepts the tables, which is a hardware effect. Cases: the boot-path
  milestone stages, and a census read showing frames above 4 GiB allocated. The prediction is written
  before the boot.
- Rows: no row moves on the 512 MiB rig, which has no RAM above 4 GiB.

### E1-3: frame metadata sections replace the heap ledger

- Change:
  - `FrameMeta` (4.3) in separately carved 4 KiB metadata pages under the sparse directory, before the heap exists;
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
  - Sparse placement: a fixed map of 64 ranges of 768 KiB each, with the ranges spread across 1 TiB of
    address space. No range is 1 MiB long, so a layout that needs a contiguous 1 MiB array per section
    fits in 0 of the 64 ranges. The page-granular layout places the metadata (at most 192 pages), and its
    directory and descriptors total under 200 KiB: a 128-byte root, at most 16 leaf pages and at most 64
    descriptors. Each RAM pfn resolves, and each hole pfn is untracked.
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
  - the first shrinkers: the block cache (a new `shrink` from the LRU's cold end, under `try_lock`), and
    the BreenFS node cache if it meets 7.3's two rules. The mapping cache is not one (7.3);
  - `Ctx`'s step table (6.6) enforced in the allocator, with the context lowered to `Atomic` in
    interrupt context and inside reclaim;
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
  - The block-cache shrinker has its own test in `block-host-tests`: it drops only clean, unpinned
    entries from the cold end, and the entries it visits are at most its target plus the batch. With the
    cache's lock held, it returns 0 without waiting.
  - Context enforcement, under a fixed schedule: an `Atomic` request whose own-CPU layer is empty and
    whose depot lock is held by another simulated CPU makes 1 `try_lock` attempt, takes the reserve or
    fails, and runs 0 shrinkers, 0 remote drains and 0 flushes. A `Normal` request made while the
    model's interrupt count is above 0 is treated the same way. A `NoReclaim` request runs 0
    shrinkers. Check first: today's front (`heap.rs` `alloc`) given the same held lock spins on it,
    which the model counts as a wait, and that is red.
- Boot: not needed.
- Rows: no row expected to move. This is the failure path.

### E1-5: kernel VA windows

- Change: the span, vmap and stack windows (6.5), with a VA allocator per window.
  - x86-64 uses a window mapper over dedicated PML4 slots and PDPTs installed at boot (6.5), with
    compare-exchange table installs, and a pending-unmap list flushed through `memory/tlb.rs` with a new
    request kind that removes global entries (`invlpg` per page, or a CR4.PGE toggle; 6.5).
  - ARM64 gets a TTBR1 4 KiB and 2 MiB mapper. Its unmap flushes through the existing
    `arch_stub::tlb::flush_pages` (`dsb ishst`, `tlbi vaae1is` per page, `dsb ish; isb`).
  - Only this PR's own self-check at init uses the windows yet.
- x86-64: kernel-half table pages for the windows are allocated on first use.
- ARM64: a new TTBR1 mapper.
- Tests (host):
  - Check first, flush before free: a fake per-CPU TLB that follows SDM vol. 3A §4.10.4.1. A CR3 load
    drops non-global entries only, `invlpg` drops the page's entry, global or not, and a CR4.PGE toggle drops
    global and non-global entries alike. The test drives Breenix's own boundary, not a model of it:
    - `unmap_kernel_page`'s contract, which leaves the flush to the caller (`kernel_page_table.rs:282-295`);
    - then the existing flush, with the shootdown's request-to-instruction choice (`tlb.rs` `service`,
      `KIND_ALL` -> `tlb::flush_all`) extracted unchanged into a pure function.
    The test maps a window page global on 4 CPUs, unmaps it, flushes, and checks whether any CPU's TLB
    still holds the translation when the frame is offered back. On the old choice all 4 CPUs keep it,
    which is red. With the new request kind 0 of 4 do.
  - The pending list returns a frame only after a flush that covers it on each CPU that may hold it.
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
  - Bootstrap and registration (5.4a): allocations and frees made before CPU 0's slot is published go
    through the shared path and are found there afterwards; CPU 2 registers while CPUs 0 and 1 run their
    fast paths, in a fixed order, and no object is lost or handed out twice; a machine described with 12
    CPUs gets 12 slots, so CPUs past 8 are not an array overrun in this layer. The model fails the test
    if any operation touches a slot before its owner publishes it.
  - Drain: after `drain(cpu)` for each CPU and a depot trim, each slab is empty and returned except the
    one kept per cache.
  - Pressure: the page layer is empty, and the only free objects of a class are in CPU 1's magazines,
    which fill whole slabs. A page allocation on CPU 0 succeeds after the slow path drains CPU 1's
    magazines (step 2) and returns the emptied slabs (step 3). A slow path that drains only its own CPU,
    as the first draft did, returns `AllocError`, which is red.
  - Alignment: each object meets its layout's alignment, and no object crosses its slab's page.
  - Each class's objects per page and leftover tail match section 6.2's arithmetic, and each class is a
    multiple of 16.
- Boot: not needed. The heap's interface and its callers are unchanged. Slab pages are reached through the
  direct map, which the anonymous fault already writes through (`zeroed_frame`, `memory/anon_map.rs:235`),
  and the magazine layer's interrupt masking is settled by the host model.
- Rows: `process.fork-exec`, since process and thread creation allocate many small objects (the
  population path now reads into frames and one staging buffer, so the driver rows are not expected to move
  here).

### E1-7: span arenas and vmap allocations

- Change:
  - span arenas running `heap_index` (6.4), with `grow_in_place` and `shrink_in_place`;
  - vmap allocations (6.5);
  - the new `realloc`;
  - `dma::bus_address` (5.6), with its refusals.
  `GlobalAlloc` does not route to them yet; only the init self-check uses them. The fixed heap still
  serves requests above 4 KiB, so no device buffer changes here. Heap retirement waits for E1-9, after
  E1-8 provides `dma::alloc` with a 32-bit limit. Retiring it here would leave heap buffers given to
  devices with no low-address source: the frontier's contiguous runs have no address limit.
- x86-64 and ARM64: new code with no caller outside its self-check.
- Tests (host):
  - Check first: `heap_index` with a free block after an allocation. `realloc` to a larger size returns the
    same pointer, but the old `realloc` moves it, which is red.
  - The existing `tools/heap-host-tests` fixture runs unchanged against `heap_index` with the two new
    operations. The algorithm's visit bound still holds.
  - Span lifecycle: a span is mapped when no span fits and unmapped when its `used()` reaches 0, keeping
    one empty span per arena. The frames are returned only after the flush (fake TLB as in E1-5). Under
    pressure, the slow path unmaps each CPU's retained empty span, and its frames are free after the
    flush.
  - vmap: a 4 MiB allocation is backed by discontiguous frames; a `realloc` with free VA after it grows in
    place; one without moves the PTEs, and the bytes are unchanged with no copy (the model counts byte
    copies).
  - `dma::bus_address` refuses a span or vmap address and a direct-map range that crosses a discontiguity,
    and accepts a contiguous direct-map or image range.
- Boot: not needed. The window mapper's hardware effect was settled by E1-5's boot, routing is
  unchanged, and the self-check next runs in E1-8's boot.
- Rows: no row expected to move; 0 callers route here yet.

### E1-8: buddy allocator, zones and per-CPU page caches

- Change:
  - pure `memory/buddy.rs` (5.1, 5.5), `memory/zone.rs` (5.2, 5.3) and `memory/page_cache_cpu.rs` (5.4);
  - glue;
  - `allocate_frame`, `allocate_frame_for` and `allocate_frame_leased` keep their signatures and move onto
    it. `allocate_contiguous_frames` gains its address limit and its exact-run search (5.6);
  - `dma::alloc(bytes, limit)`;
  - each driver that takes frames for a device moves to `dma::alloc` with the limit its controller
    reports, in the same PR, because ordinary allocations now prefer `NORMAL` and a driver left on
    `allocate_frame` would get frames above 4 GiB while `DMA32` still has room. The drivers that call
    the frame allocator directly at f52fd83c (`rg 'allocate_frame|allocate_contiguous_frames'
    kernel/src/drivers` lists 11 files) and their limits:
    - AHCI: `Dma32` unless CAP.S64A is set (bit 31; `ahci/mod.rs:1793`, which today allocates, tests and
      gives up);
    - xHCI: `Dma32` unless HCCPARAMS1.AC64 is set (`xhci/mod.rs:243-252`, the same pattern);
    - RTL8139: `Dma32` (the driver already refuses a buffer above 4 GiB, `rtl8139.rs:173`). Its receive buffer is 3 frames, which must be one exact run
      (`rtl8139.rs:68`, `:169-176`);
    - e1000, NVMe, BCM5720 and virtio (`block`, `block_mmio`, `queue`, `net_legacy`, `sound`): the limit
      each device's specification gives, recorded in the PR next to the call;
    - `gpu/dma.rs` and `memory/stack.rs` call `allocate_contiguous_frames` too, and pass `Any`.
  - A structural host test lists each `allocate_frame*` and `allocate_contiguous_frames` call under
    `kernel/src/drivers` and fails on any that is not `dma::alloc`, so a new driver cannot skip its limit;
  - the frontier, `get_usable_frame`, the LIFO list and `contiguous.rs` go;
  - `/proc/meminfo` and `/proc/breenix/kmem` report per zone.
- x86-64: two zones on the R240 (8 GiB). DMA comes from `DMA32` by request.
- ARM64: one zone on the rig; two wherever RAM extends above 4 GiB (with E1-2, any VM given more than
  3 GiB at QEMU's and Parallels' 1 GiB RAM base).
- Tests (host):
  - Check first: `contiguous.rs` after the frontier has reached the end of a 64 MiB map, with 512 aligned
    frames freed back in random order. The old algorithm cannot return a 2 MiB run, which is red. The same
    sequence on the pure `buddy.rs`, which has no watermarks, returns the 2 MiB run. The fixture frees
    exactly 512 frames, so it runs below the zone layer: at the zone layer, taking those 512 would leave 0
    free frames, under `min`, and the watermark would correctly send it to the slow path.
  - The same 2 MiB run through the zone layer, with the fixture also freeing `min` frames elsewhere in
    the zone, succeeds without the slow path and leaves exactly `min` free.
  - Reserves: zones of 256 frames, 16,384 frames (64 MiB) and 2^20 frames (4 GiB), each under a `NORMAL`
    zone of 2^24 frames (64 GiB). For each one, a `DMA32` request succeeds until free frames reach `min`, and
    `min` and `high` stay below 1/10 of the zone and the fallback reserve at most 1/4. A `NORMAL` fallback succeeds
    until the reserve is reached. With the uncapped formulas, the 256-frame and 64 MiB zones fail one of
    these. That is red, against the design's own first draft rather than old code, because the old
    allocator has no zones.
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
  - Check first, device limits: a two-zone map with free frames in both zones. The AHCI port setup on
    a controller without S64A is driven through today's path, `allocate_contiguous_frames` and then
    its own address test, over the zoned allocator. The run comes from `NORMAL`, and the port is not
    attached, which is red. Through `dma::alloc(bytes, Dma32)` the run is below 4 GiB and the port is
    attached.
  - Check first, exact runs: a zone whose free frames are an order-1 block at pfn 0x102 and an order-0
    block at pfn 0x104, with pfn 0x101 and 0x105 allocated, and no free order-2 block. That is 3
    contiguous free frames, not inside one aligned 4-frame block. A request for 3 frames rounded to
    order 2 fails, which is red, and that is RTL8139's receive buffer. The exact-run search returns
    pfn 0x102-0x104, and the rest of the zone is unchanged.
  - Per-CPU caches: refill and drain move `batch` frames per zone-lock hold; a remote drain under the
    per-CPU lock empties another CPU's cache; cached frames are not merged. Frames allocated before a CPU's
    slot is published come from the zone directly (5.4a).
  - Classes in the per-CPU caches: on one CPU, free a `MOVABLE` frame (filling the `MOVABLE` list), then
    make an `UNMOVABLE` request. It is served from an `UNMOVABLE` pageblock, not from the cached `MOVABLE`
    frame. A frame freed by a caller naming another class returns to its own pageblock's list. With one
    list per zone, the first case hands out the `MOVABLE` frame, which is red against the design's first
    draft; the old allocator has no classes.
  - Pressure across CPUs: the zone is empty except for frames in CPU 1's page caches. An allocation on
    CPU 0 succeeds after the slow path's step 2, and with CPU 1's lock held in the fixed schedule, it
    skips CPU 1 without waiting and returns `AllocError`.
  - Watermarks: an ordinary allocation stops at `min` and enters the slow path; an `Atomic` one may use
    half of `min`.
  - OOM: with each frame allocated, `alloc_pages` returns `AllocError` after draining and shrinking, and
    does not panic.
  - The lease suite from E1-3 runs again over buddy-backed frames.
  - Block returns. Block leases are new in this PR, so there is no old code for these to be red against.
    Each case is a return the two-pass rule (4.3) must refuse:
    - an order-2 block whose third frame has `leaf_refs` = 1 is returned. It is refused with
      `RefusedLiveLeaf`, and 0 of its frames reach a free list;
    - an `acquire_leaf_mapping` on a tail is interleaved between the two passes, in the fixed order
      check (return), increment (acquire), transition (return), re-check (acquire). Exactly one side
      wins: either the return is refused with the frame mapped, or the acquire backs out. In 0 runs is
      a mapped frame on a free list;
    - after `split_lease`, a block lease rebuilt from the head generation recorded before the split is
      refused as stale, so the block cannot be returned twice.
- Boot: one ARM64 QEMU boot of the boot-path gate. This replaces the allocator the boot stages use, and
  the driver conversions change what devices are given. Cases: the boot-path milestone stages, which bring
  up the virtio block, net and input devices, the census, and E1-7's span and vmap self-check.
- Rows: `memory.anonymous-touch`, since an allocation becomes a per-CPU pop; `process.fork-exec`, for
  page-table and copy-on-write frames; `memory.driver-touch`, `memory.driver-mmap` and
  `memory.driver-map-touch`, for frames per populated page.

### E1-9: route the heap and retire the fixed heap

- Change:
  - `GlobalAlloc` routes requests above 4 KiB to E1-7's span arenas and vmap allocations (6.1);
  - each caller that passes heap memory to a device moves to a static or to `dma::alloc` with its limit,
    which E1-8 serves from the zones. `dma::bus_address` finds them: it refuses span and vmap addresses
    at the call;
  - the fixed heap is retired on both architectures (6.6). The heap init's "DMA will fail" warning goes,
    because heap memory is no longer DMA memory.
- x86-64: the 64 MiB window at `0x4444_4444_0000` is no longer mapped at boot, and the heap grows on
  demand.
- ARM64: `place_kernel_pools` keeps only the stack pool. On the rig that returns about 64 MiB of a 512 MiB
  VM to the frame pool, less what the heap actually uses.
- Tests (host):
  - Check first: an 8 KiB heap buffer under the routed heap is a span address backed by two
    non-adjacent frames. The drivers' private translation, extracted unchanged (`ahci/mod.rs:118`,
    `platform_config::kernel_va_to_ipa` on ARM64 and the physical-memory offset on x86; 8 more copies in
    virtio list the same way), returns an address for it with no error. That address is not the first
    frame's, and the second page is not at +4096, so a device given it would write to the wrong memory,
    which is red. `dma::bus_address` refuses it, and the same buffer from `dma::alloc` is accepted.
  - Routing: for each size in {1, 16, 4096, 4097, 512 KiB, 512 KiB + 1, 4 MiB} and each alignment in
    {1, 16, 4096, 8192, 2 MiB, 1 GiB}, the layout reaches exactly one layer, and the allocation succeeds
    and meets its alignment. Check first: the table in this design's first draft, applied to the same
    grid, matches two rows for (4 MiB, 8 KiB), matches 0 rows for alignment 1 GiB, and sends
    (4096, 2 MiB) to a span that cannot hold it under `heap_index`'s in-band tag. That is red.
  - Cross-layer `realloc`: a 1 MiB vmap block shrunk in place to 300 KiB and freed with the new layout
    returns to vmap. A free routed by layout would send it to a span arena. A free of a pointer with a
    layout that disagrees with the block's record panics in the model with both values.
- Boot: one ARM64 QEMU boot of the boot-path gate. Retiring the heap changes boot memory order, and the
  conversions change what devices are given. Cases: the boot-path milestone stages, which bring up the
  virtio block, net and input devices.
- Rows: `memory.driver-mmap` and `memory.driver-map-touch`. Population reserves a vector entry per
  wanted page and merges it into the sorted `PageVec` (`file_map.rs:1600-1601`, `merge_reserved` at
  `:335`); for the 262 MB driver those are multi-megabyte contiguous reservations, which become vmap
  allocations that grow in place instead of copying. `process.fork-exec` may move a little.

### E1-10: kernel stacks from the buddy

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

### E1-11 to E1-20: the OOM conversions

Each conversion PR follows the same check-first shape (7.4): its structural guard goes in red, and where
the subsystem has a host-test crate, an allocation-failure sweep. Each converts its subsystem's runtime
paths and marks boot sites `boot:`. No row is expected to move; the rows are recorded to show that.

| PR | Subsystem | Error surfaced | Host sweep in |
|---|---|---|---|
| E1-11 | Syscall entry, user copies and staging buffers (`syscall/`) | `ENOMEM` | `regular-read-host-tests` |
| E1-12 | Memory syscalls and VMAs (`syscall/mmap.rs`, `syscall/memory.rs`, `memory/vma.rs`) | `ENOMEM` | `pagetable-host-tests` |
| E1-13 | Process and thread lifecycle: fork, clone, exec, spawn (`process/`, `task/`) | `EAGAIN` or `ENOMEM` per the call | structural guard only |
| E1-14 | VFS and path walk (`fs/vfs`, `fs/namei.rs`, `ipc/fd.rs`) | `ENOMEM` | `vfs-host-tests`, `resolve-host-tests` |
| E1-15 | BreenFS and ext2 runtime (`fs/breenfs`, `fs/ext2`) | `ENOMEM` before any disk change, by the reservation rule | `breenfs-host-tests`, `ext2-host-tests` |
| E1-16 | Network and sockets (`net/`, `socket/`) | `ENOBUFS` on send; counted drop on receive | `net-host-tests`, `udp-host-tests`, `unix-host-tests` |
| E1-17 | IPC: pipes, SysV and POSIX IPC, shared memory (`ipc/`) | `ENOMEM` or `ENOSPC` per the man page | `ipc-host-tests`, `shmsem-host-tests`, `shmem-api-host-tests` |
| E1-18 | Block layer and storage drivers' runtime paths (`block/`, `drivers/ahci`, `drivers/nvme`, `drivers/virtio/block*`) | I/O error to the request; not a partial request | `block-host-tests` |
| E1-19 | Other drivers, GPU and console runtime (`drivers/`, `gpu/`, `console/`) | Counted drop or the call's errno | `gpu-host-tests`, `console-host-tests` |
| E1-20 | Diagnostics: `/proc`, census, tracing dumps (`fs/procfs`, `capture/`) | `ENOMEM` on read | structural guard only |

- x86-64 and ARM64: the same on both.
- Boot: not needed. A fallible API has the same success path, and the sweeps settle the failure paths.

### E1-21: the compile-time end state

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
- Boots: E1-2, E1-3, E1-5, E1-8, E1-9 and E1-10 each need one ARM64 QEMU boot of the standard boot script,
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
| `memory.driver-mmap` | 0.171 s | 1.09 µs | about 158,000 | E1-3, E1-8, E1-9 (a small part; the rest is E2 and E3) |
| `memory.driver-touch` | 5.17 ms | 1.34 ms | 3.87 | E1-8 |
| `memory.driver-map-touch` | 0.176 s | 1.34 ms | about 132 | as driver-mmap |
| `process.fork-exec` | 2.53 ms | 0.218 ms | 11.6 | E1-3, E1-6, E1-8, E1-9, E1-10 |
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

On the R240, the batched flash after E1-9 is followed by one R240 series (DEV-31(2)). The 8 GiB box is
where metadata leaving the heap matters most (section 1.1).

## 14. Risks and questions for the coordinator

1. x86 early boot is checked only on the R240. E1-3, E1-8, E1-9 and E1-10 change x86 boot memory order, and
   lanes have no x86 QEMU. Recommendation: batch those PRs' x86 check into one coordinator flash after
   E1-9. That is DEV-31(2)'s rule. Once DEV-32 and DEV-33's one-reboot flash with try-once rollback is in
   service (#3247), a bad kernel falls back by itself.
2. x86 kernel unmaps use the NMI shootdown until E9. Section 6.5 batches them, and spans shrink with
   hysteresis, so they are rare. A workload that frees large vmap buffers at a high rate pays an NMI round
   per 16 MiB, which is the cost to watch in E1-9's series.
3. No interim DMA source. Heap retirement (E1-9) waits for the buddy and `dma::alloc` with address limits
   (E1-8), because the frontier's contiguous runs have no address limit. The cost is that the largest
   PR, E1-8, lands before the fixed heap is retired. E1-6's slab caches still take most allocations by
   count off the heap's global lock first.
4. `BTreeMap` has no fallible insert in `alloc`. The 67 `BTreeMap::new` sites move to a slab-backed ordered
   map with `try_insert`. That map is new code, and gets its own host tests in the first conversion PR that
   needs it.
5. The scale of E1-11..E1-20. The pattern counts in 7.4 include tests and boot code. The real runtime count
   is unknown until each subsystem's guard lists it. If one subsystem's PR would be too large to review, it
   splits by module with the same guard. The guard's file list says which part is done.
6. The 32-byte `FrameMeta` costs 0.78% of RAM, against today's 12 bytes plus 8 bytes of free-list
   capacity. That buys O(1) buddy links, the slab back-pointer, and no heap dependency. E3 may grow it to
   64 bytes (5.7).
7. No step in this plan changes BreenFS's on-disk format, or anything under `compiler/` or
   `src/llvm-project`.
