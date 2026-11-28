# E1: kernel memory foundation (design and PR plan)

Status: E1 design and PR sequence. E1-1 records its implementation decisions below.

Rulings: DEV-37(2) orders this design before any E1 code. DEV-29 adopted the toy-OS audit
(`~/Downloads/breenix-toy-os-audit-2026-11-27.html`, findings R3 and R14, themes T3, T8 and T9) and decided Q4
(no swap now; reclaim must leave room for it) and Q5 (no fixed CPU or RAM ceiling). DEV-30 sets the testing
rule, and DEV-31 makes ARM64 QEMU the performance rig. The memory lane's full findings (F5, F6, F8, F11 and
the keep list) are the evidence base for section 1.

Amendment (2026-11-27, after E1-1's builder stopped at design fit, ledger line 124). The first version of
this plan had the kernel consume each conventional range of the final UEFI map (40 on the reference fixture), but the ARM64 handoff
carries at most 32 of them (section 1.5). Sections 1.5, 4.5 and 4.6 now specify how the complete map
reaches the kernel, and sections 2.2, 3, 4.1, 9, 10, 11 (E1-1 and E1-2), 12 and 14 follow from them.

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

### 1.5 The ARM64 loader handoff carries at most 32 ranges

Line numbers in this section, and in sections 4.5 and 4.6, are from main at 82b312d6.

- `parallels-loader` boots Parallels and VMware. ARM64 QEMU does not use it: the kernel is booted directly
  with a device tree, and `kernel_main` receives `hw_config_ptr == 0` (`main_aarch64.rs:728`).
- At exit from boot services the loader calls the `uefi` crate's `exit_boot_services(LOADER_DATA)`
  (`parallels-loader/src/main.rs:376`). It runs the final descriptors through
  `final_uefi_conventional_ranges` into the contract's fixed `RangeSet` of `MAX_RANGES = 32`
  (`phys_memory.rs:16-35`), then copies the survivors into `HardwareConfig.ram_regions`
  (`main.rs:376-387`).
- That array is a second cap of 32: `MAX_RAM_REGIONS = 32` in the loader (`hw_config.rs:7-8, 93-98`), and
  `ram_regions: [RamRegion; 32]` in the kernel's copy of the layout (`platform_config.rs:799-801`).
- The kernel builds its frame pool only from that array (`platform_config.rs:894-912`). The full
  descriptor list itself does not cross the boundary.
- The drop is silent. The `RangeSet`'s `truncated` flag has no field in `HardwareConfig`, so the kernel's
  "firmware map truncated" note (`main_aarch64.rs:893-899`) cannot fire for ranges the loader dropped.
- `HardwareConfig` is defined twice, once on each side, with a comment that the two must match
  (`platform_config.rs:773-774`).
- The kernel accepts exactly `HARDWARE_CONFIG_VERSION = 7` (`arm64-boot-contract/src/lib.rs:28`). Any
  other version fails an `assert!` (`main_aarch64.rs:729-733`) before the kernel has configured its UART
  from the config, so a mismatched loader stops the boot with no readable message.
- The loader keeps the map buffer and itself in `LOADER_DATA`, which the kernel does not free.

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
- Reclaiming ARM64 boot-services and loader memory (UEFI types 1 to 4) after devices are reset. These stay
  reserved, because firmware DMA can still be in flight after ExitBootServices
  (`phys_memory.rs:11-15`). The complete map of section 4.5 makes a later reclaim possible, but E1 keeps
  today's policy, and no effort owns that reclaim yet (14.10).

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
memory map (x86 bootloader regions; ARM64 UEFI map or DT)    -- 4.1, 4.5, 4.6
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
  - ARM64: the same firmware descriptions the kernel reads today, with the clip to 4 GiB removed
    (section 4.4). Removing the kernel's clip is not enough on its own, because three limits upstream of
    it also drop RAM:
    - `arm64-boot-contract`'s `RangeSet` holds at most `MAX_RANGES = 32` ranges
      (`libs/arm64-boot-contract/src/phys_memory.rs:35`) and sets `truncated` when it drops one
      (lines 122-127, 168-171). On the device tree path, E1-1 feeds the kernel's growable range set
      straight from the contract's existing visitor (`fdt::walk`, line 488), so the fixed set is no longer
      on the frame-pool path.
    - On the UEFI path, the loader filters the final map into that fixed set and copies the result into
      `HardwareConfig`'s 32-entry `ram_regions` array, and the kernel sees only the array (section 1.5).
      Splitting the filter in the kernel cannot recover ranges the loader did not pass. E1-1 changes the
      handoff instead: the loader passes the final map itself, and the kernel filters it (section 4.5).
    - Once neither path uses the fixed set for the frame pool, no RAM depends on it. It stays only where
      a loader-side caller still needs a bounded set, and there its `truncated` flag becomes a boot error
      rather than a silent drop.
    - The output address width is fixed at 40 bits (`TCR_IPS_40BIT`, `arch_impl/aarch64/boot.S:82`, and
      the same constant in `parallels-loader/src/page_tables.rs:112`), so RAM at or above 1 TiB cannot be
      mapped. E1-2 sets TCR_EL1.IPS from ID_AA64MMFR0_EL1.PARange (Arm ARM, the ID_AA64MMFR0_EL1 and TCR_EL1
      register descriptions) at both sites. RAM above the width the CPU reports is excluded from the range
      set and counted in `/proc/breenix/kmem`, so it is reported rather than silently missing.
- There is no `MAX_REGIONS`. The range set's storage grows by a page at a time if the firmware describes
  more ranges.
- The kernel writes the set's own pages as it builds them, so they must be RAM that the live tables
  already map writable. The set can describe RAM that is not mapped yet, but its storage cannot be in it.
  - On ARM64, each storage page, the first and each growth page alike, comes from usable RAM inside the
    boot window, `direct_map_ram_window` (below `DIRECT_MAP_RAM_END`). Each page must also pass a write
    probe, `AT S1E1W` at its `HHDM_BASE + pa` with the `PAR_EL1` checks of 4.5.4. Pages are written
    through that address.
  - The largest usable range may lie above 4 GiB, outside the map, on any machine with more RAM above
    4 GiB than below it. A rule of "a page from the largest range" would then put writable bookkeeping
    in an unmapped slot, and the first write would fault before the boot can print.
  - On x86 the bootloader's physical-memory mapping covers the described RAM (4.6), so any usable page
    qualifies.
- Building the set takes two passes over the firmware description, after the in-place sort (4.5.5) on
  the UEFI path. The first pass streams the same filtered usable intervals the second pass inserts. It
  picks the largest one inside the window, so the storage page is a page the set would hold anyway. The
  second pass fills the set. A growth page is carved from the set with `below` set to the window's end,
  and is probed the same way.
- If the window holds no usable page that passes the probe, the build stops, with that reason named, as
  `FramePoolSource::Unavailable`.
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

### 4.4 The direct map covers the described RAM

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
- On x86-64, the bootloader's dynamic physical-memory mapping covers RAM. E1-2x's move to per-slot
  PDPTs leaves the boot translations unchanged (the #3264 fixture checks two slots), so E1-3's
  carved metadata can keep using those addresses across the boot, master and process roots.
- `PhysAddrWrapper::from_kernel_virt` already uses the CPU's own translation (`AT S1E1R` on ARM64, a table
  walk on x86), so it needs no change.

### 4.5 The complete UEFI memory map reaches the kernel (ARM64)

The loader stops filtering. It passes the final map itself, as the firmware returned it, and the kernel
filters it in one place. No fixed array sits between the two.

#### 4.5.1 What the UEFI specification defines (UEFI 2.10)

- `GetMemoryMap()` (§7.2.3) fills a caller's buffer with `EFI_MEMORY_DESCRIPTOR`s and returns the map's
  size, a `MapKey`, a `DescriptorSize` and a `DescriptorVersion`.
  - The stride is `DescriptorSize`: "OS software must use the DescriptorSize to find the start of each
    EFI_MEMORY_DESCRIPTOR in the MemoryMap array". A firmware may report more than the structure's size.
  - `EFI_MEMORY_DESCRIPTOR_VERSION` is 1. Its fields are `Type` (u32 at offset 0), `PhysicalStart` (u64
    at 8), `VirtualStart` (u64 at 16), `NumberOfPages` (u64 at 24) and `Attribute` (u64 at 32), so 40
    bytes. Bytes past 40 in a longer stride are not interpreted.
  - `PhysicalStart` is aligned to 4 KiB and is not above `0xffff_ffff_ffff_f000`. `NumberOfPages` is not
    0 and describes no page that starts above that address.
  - A buffer that is too small gets `EFI_BUFFER_TOO_SMALL`. The buffer for the next call "should be bigger
    than the value returned in MemoryMapSize, since allocation of the new buffer may potentially increase
    memory map size".
  - Attribute bits used here: `EFI_MEMORY_RUNTIME` (bit 63), and `EFI_MEMORY_SP` (bit 18), which marks
    specific-purpose memory set aside for particular drivers.
- `ExitBootServices()` (§7.4.6) takes the `MapKey` of the latest `GetMemoryMap()`. A stale key returns
  `EFI_INVALID_PARAMETER`, and the loader must call `GetMemoryMap()` and `ExitBootServices()` again. The
  firmware may shut boot services down in part on the first call. After that first call, the loader may
  call only the Memory Allocation Services.
- `AllocatePages()` and `AllocatePool()` (§7.2.1, §7.2.4) take a memory type. The values
  `0x8000_0000..=0xFFFF_FFFF` are reserved for OS loaders. Pool allocations are 8-byte aligned.
- On AArch64, boot services run with an identity mapping (§2.3.6), so an address the loader holds before
  exit is a physical address.

#### 4.5.2 The loader: a buffer allocated before the final `GetMemoryMap()`

- The loader keeps using the `uefi` crate's `exit_boot_services`, with a Breenix memory type instead of
  `LOADER_DATA`: `MemoryType::custom(BOOT_MAP_MEMORY_TYPE)`, where `BOOT_MAP_MEMORY_TYPE = 0x8000_4252` is
  in the OS-loader range and no other allocation uses it. The constant lives in `arm64-boot-contract`.
- The crate's documented contract (version 0.33, the `exit_boot_services` and `MemoryMapMeta`
  documentation) is:
  - the returned map is backed by a pool allocation of the given type;
  - exiting is retried at most once;
  - the buffer is not freed, because boot services are gone.
  The crate also shuts down its own logger and allocator, which the loader relies on today.
- The ordering then follows from the specification, not from the crate's internals. Any allocation after
  a `GetMemoryMap()` changes the map, and with it the key, so `ExitBootServices()` with that key fails.
  So when the exit succeeds, the map whose key it used already contains each allocation made before it,
  the buffer included. No allocation is possible after a successful exit.
- The kernel does not trust that ordering blindly. The final map must describe its own buffer with a
  `BOOT_MAP_MEMORY_TYPE` descriptor, or the kernel refuses the map (rule 6 of 4.5.5). A map taken before
  the buffer existed cannot pass that check.
- Rejected alternative: the loader's own `GetMemoryMap`/`AllocatePages`/`ExitBootServices` sequence on the
  raw boot-services table. It would let the loader place the buffer below 4 GiB, but it duplicates the
  crate's retry, and it would have to disable the crate's logger and allocator by hand. Because a pool
  buffer can land anywhere, the loader maps it instead (4.5.4).
- Rejected alternative: a fixed array in the loader's image. That needs no allocation, but it is a fixed
  ceiling on the descriptor count, which section 9 rules out.
- Then, in place of today's code:
  - The loader's early map read, `populate_ram_regions` (`main.rs:127, 465`), keeps its own job. It finds
    the RAM base that sets `ram_base_offset` for VMware's relocation (`main.rs:124-157`), and it feeds the
    `[ram]` log lines (`main.rs:129-140, 198-203`). But it now keeps its result in a loader-local value
    instead of `HardwareConfig.ram_regions`, which the final map overwrote anyway.
    - The base it uses becomes the lowest one it found. Today the code takes the first entry
      (`main.rs:148-149`). The two differ only if the firmware lists RAM out of address order.
    - The filter loop (`main.rs:376-387`) goes, and `ram_region_count` is written as 0.
  - The loader checks the map's metadata before it reads a single descriptor. Today's runtime-region
    loop (`main.rs:388-399`) iterates `final_map.entries()`, and the crate's `MemoryMap` API hands out a
    `&MemoryDescriptor` at each multiple of `desc_size`. At a 44-byte stride the second of those
    references is misaligned, so the loader would act on a malformed map before the kernel's checks run.
    `arm64_boot_contract::uefi_map::check_meta(meta, buffer_len)` comes first, with the kernel's
    structural rules 2 to 4 (4.5.5), plus two more:
    - `meta().map_size` is at most `buffer().len()`. The wire length is `meta().map_size`, the bytes the
      firmware wrote. `buffer().len()` also counts the pool's spare capacity.
    - `meta().desc_size` converts to `u32` without loss, checked with `u32::try_from`.
  - When the check passes, the runtime-region loop reads the final map through the contract's
    stride-aware reader, which reads each field by byte offset from the slice
    `buffer()[..meta().map_size]`. It forms no `&MemoryDescriptor`, and `final_map.entries()` is not
    used.
  - When the check fails, the loader cannot go back to the firmware, because boot services have exited.
    It skips the runtime-region loop, writes `rtc_get_time = 0`, and records the failed rule in
    `UefiMapHandoff.loader_check`. The kernel then refuses the map with that rule named (4.5.5).
  - The loader records `map_phys` (`buffer().as_ptr()`), `map_size` (`meta().map_size`),
    `descriptor_size`, `descriptor_version` and `loader_check` in `HardwareConfig` (4.5.3), and leaks the
    owner with `core::mem::forget`.
  - The code between the exit and the jump does not allocate, as today.
  - The exit and these steps are one function, `handoff::exit_and_encode(config)`, which `main.rs`
    calls. E1-1's host test runs that same function over a fake firmware table.
- Before exiting, while it can still print, the loader checks that the kernel accepts v8 (4.5.3).

#### 4.5.3 `HardwareConfig` v8, and compatibility in both directions

- One definition. `HardwareConfig` moves into `arm64-boot-contract`, and both copies go
  (`hw_config.rs:49-148`, `platform_config.rs:777-823`). It is two `repr(C)` parts:
  - `HardwareConfigV7`, today's fields exactly. Its layout does not change, so `magic`, `version` and
    `uart_base_phys` stay at offsets 0, 4 and 8. The loader's handoff stub already reads
    `uart_base_phys` at `[x3, #8]` (`kernel_entry.rs:74`).
  - `UefiMapHandoff`, appended after `boot_seed`: `map_phys: u64`, `map_size: u64`,
    `descriptor_size: u32`, `descriptor_version: u32`, `loader_check: u32` (0, or the number of the rule
    the loader's check refused, 4.5.2) and `_reserved: u32`.
- `HARDWARE_CONFIG_VERSION` becomes 8. A v8 loader writes both parts, with `ram_region_count = 0`. The
  `ram_regions` array keeps its place in the layout, but v8 does not use it.
- The kernel accepts versions 7 and 8 with the magic.
  - It reads `HardwareConfigV7` by reference, and reads `UefiMapHandoff` only when the version is at
    least 8. A v7 loader's static ends at `boot_seed`, so a reference to the v8 type over it would claim
    bytes that loader did not write.
  - Version 8: the pool comes from the map (4.5.5), source `UefiMemoryMap`.
  - Version 7, from an older loader: today's path through `ram_regions`, at most 32 ranges, under a new
    source, `UefiRegionListV7`. The boot line says so. When `ram_region_count` is 32 it also says RAM may
    be missing, because a v7 loader drops ranges without saying (1.5). The older loader still boots the
    newer kernel.
- Newer loader, older kernel. Each kernel before E1-1 accepts only version 7, and fails the boot
  unreadably on anything else (1.5). So the loader decides before it exits boot services.
  - A kernel from E1-1 on exports `#[used] #[no_mangle] #[link_section = ".rodata.breenix_handoff"]
    static BREENIX_HANDOFF_VERSIONS: [u32; 2] = [7, 8]` (lowest and highest accepted).
  - The static must survive the link. The kernel's own code does not read it, and the Rust Reference says
    `#[used]` keeps an item in the object file but leaves the linker free to remove it. The ARM64 linker
    script's `.rodata` rule (`linker.ld:84-91`) has no `KEEP`. So E1-1 adds
    `KEEP(*(.rodata.breenix_handoff))` as the first line of that output section. If a future link drops
    the static anyway, the loader finds no symbol and refuses loudly rather than guess a version.
  - The existing `find_symbol` (`kernel_load.rs:329-443`) cannot find it. It skips a symbol whose type
    is not `STT_FUNC` (line 426) and returns only `st_value`, and a static is `STT_OBJECT` (gABI,
    symbol table, symbol type). `kernel_main`'s lookup keeps that function unchanged. The version gets a
    new, bounded `find_object_symbol`, which returns the 8 bytes or a named reason:
    - the symbol's type is `STT_OBJECT` and its `st_size` is 8;
    - `st_shndx` names a real section (not `SHN_UNDEF`, `SHN_ABS` or `SHN_COMMON`, and less than
      `e_shnum`), and that section is `SHT_PROGBITS`, so the bytes are in the file;
    - `[st_value, st_value + 8)` lies inside `[sh_addr, sh_addr + sh_size)`, with no overflow;
    - the bytes are read at `sh_offset + (st_value - sh_addr)`, and that range lies inside the ELF image.
    No load-address translation is involved: `vaddr_to_paddr_offset` (`kernel_load.rs:251-254`) is the
    last segment's offset, not each segment's.
  - If the symbol is missing or 8 is outside the range, the loader prints "kernel accepts HardwareConfig v7
    only; this loader writes v8" (or the range it found) on the UEFI console and the UART. Then it returns
    `EFI_INCOMPATIBLE_VERSION` from its entry point without exiting boot services. The firmware's boot
    manager has control again, and a kernel that cannot read the config has not run.
  - A stripped kernel is refused the same way, loudly. The loader already needs the symbol table to find
    `kernel_main`.
- The kernel's own backstop. A version it does not accept (9 from a future loader, for example) no
  longer fails an `assert!`. The kernel configures its UART from `uart_base_phys` and `uart_type`, which
  stay at offsets 8 and 20 in each version. It prints "HardwareConfig v9 refused: this kernel accepts
  v7..v8" and halts in a `wfe` loop.
- v7 acceptance stays until the coordinator retires it. Retiring it raises the kernel's lowest accepted
  version in its own PR.

#### 4.5.4 Where the map is mapped, and when its memory is released

- The kernel sorts the map in place (4.5.5), so each page of the buffer must be mapped writable, as
  normal write-back memory, at its own physical address. Mapped and readable is not enough:
  - The loader reserves kernel segments in 4 KiB pages (`kernel_load.rs:224-231`), but it gives each
    2 MiB block a segment touches that segment's permissions (`kernel_block_attr`,
    `page_tables.rs:245-284`). Firmware can place the pool in free pages at the end of a text or rodata
    block, which then translate read-only.
  - On VMware the low window `0x4000_0000..0x6000_0000` translates to the IPA plus `ram_base_offset`
    (`page_tables.rs:457-504`), so an address in that window that translates may still not be the buffer.
- The loader maps the buffer when it builds its tables. `build_page_tables` runs inside
  `jump_to_kernel` (`kernel_entry.rs:28`), after `ExitBootServices` has returned, so the buffer's
  address is known by then. It takes `[map_phys, map_phys + map_size)` from `UefiMapHandoff`, rounded
  out to pages, and maps it in TTBR0 only, the identity map the kernel reads it through:
  - Inside `[0, 4 GiB)`, which the loader's tables already map on both loader platforms
    (`page_tables.rs:387-574`): a 2 MiB block the buffer touches keeps its entry when the loader chose
    `RAM_DATA_BLOCK` or `NORMAL_BLOCK` for it (normal memory, writable at EL1). Any other block (kernel
    text or rodata, device, the NC DMA block, or an alias to a different physical address) is split into an L3 table in TTBR0. The buffer's pages
    get page descriptors with `RAM_DATA_BLOCK`'s attributes: writable, with PXN and UXN set. The block's
    other pages keep its own attributes, so the kernel image's W^X bits do not change.
  - Above 4 GiB: in a 1 GiB slot the loader has already mapped for its own image or table storage
    (`page_tables.rs:576-613`), the buffer is covered by that `NORMAL_BLOCK`. In any other slot the loader
    installs L1 (outside L0[0], 512 GiB and up), L2 and L3 tables that map exactly the buffer's pages,
    using 2 MiB blocks only for blocks the buffer fills. Pages beside the buffer stay unmapped.
  - The tables come from the loader's static `PageTableStorage`. No allocation is needed after the
    exit. If it runs out, `alloc_table`'s existing "out of page tables" assertion prints on the UART.
  - TTBR1 gets no entry for the buffer. A 1 GiB block there would cover holes, and E1-2 writes TTBR1
    entries only where the descriptor is invalid (4.4), so it could not later install the finer entries
    it plans. The kernel does not read the buffer at `HHDM_BASE + pa`.
  - Per DEV-39, an existing writable identity block shared with the loader retains its execute
    permissions. Where a buffer page can be mapped separately, set PXN and UXN and leave adjacent
    translations intact.
  - Secondary CPUs load CPU 0's TTBR values (`boot.S:1042-1065`), so the loader's tables stay in use
    until the kernel replaces TTBR0 for the first user address space. The buffer entries are gone then.
    By that point the kernel has finished with the buffer.
- The kernel reads the map once, in `init_from_parallels`, where it reads the array today
  (`main_aarch64.rs:728-733`). That is before another CPU starts and before TTBR0 is replaced.
  - First it probes each page of the buffer with `AT S1E1W` (Arm ARM, address translation
    instructions), and reads `PAR_EL1`. The page passes only when `F` is 0, `PA` equals the page's own
    physical address, and `ATTR` is the normal write-back encoding the loader programs into MAIR
    (index 1, `0xFF`). If a page does not pass, the kernel refuses the map with the page and the reason
    (no translation, read-only, another PA, or not normal memory). A fault this early would end the boot
    with no message.
  - The sort, and each other access to the buffer, uses the identity address the probe checked.
- Release.
  - The buffer's type is not conventional memory, so it is outside the usable set while the kernel reads
    it. The kernel treats it as reserved until it has consumed it.
  - What is released is only the buffer's proven backing: the pages of the one `BOOT_MAP_MEMORY_TYPE`
    descriptor that rule 6 of 4.5.5 found covering `[map_phys, map_phys + map_size)`. Any other
    descriptor of that type is not released. It stays reserved and is counted, because the map does not show
    what it holds. Within that one pool allocation, no other live data is in those pages. The firmware's pool
    bookkeeping in them died at exit.
  - Those pages pass through the same filters as a conventional descriptor before they enter the set.
    Being the buffer's own does not make a page usable:
    - the descriptor's attributes: `EFI_MEMORY_RUNTIME` or `EFI_MEMORY_SP` set means no page is released;
    - the overlap sweep: a page that another descriptor also describes stays out, for example one that
      overlaps `EfiACPIMemoryNVS`;
    - the kernel's fixed ranges (`kernel_fixed_ranges`, `platform_config.rs:116-119, 217-221`), relocated
      by `ram_base_offset`. They include the per-CPU stacks at `0x4300_0000..0x4400_0000`, which the
      loader does not reserve from firmware. Only the loader stack below them is reserved
      (`main.rs:449-457`), so a pool allocation can land in the per-CPU stacks. Reading the map there is
      safe, because no secondary CPU has started. Releasing those pages would let the carve or the
      buddy hand out a page that a CPU later uses as its stack;
    - the address-width window (4.1), and E1-1's 4 GiB clip until E1-2 removes it.
  - The release is an insert into the range set of what survives, in its own step after the second pass
    (4.1) and before the first carve. It comes after the last access to the buffer, the sort included. Its
    count is the released bytes, and the bytes the filters held back are counted by reason.
  - No code reads the buffer after that. The runtime ranges the kernel keeps are in `HardwareConfig`'s
    own array (`firmware_rtc::configure`), and the kernel takes each count it reports during the passes.
  - `HardwareConfig` itself stays in loader data and is not released, as today.
- When the kernel has the map, the range set no longer needs `HardwareConfig.runtime_regions` for RAM
  safety: runtime descriptors are excluded by attribute (4.5.5). The 64-entry runtime array stays for
  `GetTime`, and on overflow it still turns `GetTime` off (`main.rs:390-393`).

#### 4.5.5 Validation

There are two classes of check. A structural failure refuses the whole map: the pool becomes
`FramePoolSource::Unavailable(reason)`, and the boot stops with that reason at the existing panic
(`main_aarch64.rs:886`). A per-descriptor failure does not add RAM. It can only remove RAM, and each removal
is counted.

Structural, in order:

0. `loader_check` is 0. Otherwise the kernel refuses with the loader's rule. It then reads no descriptor,
   and it still runs its own rules below on the metadata, so the boot line names both.
1. `map_phys` is not 0 and is 8-byte aligned (the pool's alignment), and `map_phys + map_size` does not
   overflow.
2. `descriptor_version` is 1.
3. `descriptor_size` is at least 40 and a multiple of 8.
4. `map_size` is at least one `descriptor_size` and a whole multiple of it. The count is the quotient,
   and there is no upper limit.
5. Each page of `[map_phys, map_phys + map_size)` translates for a write, to its own physical address,
   as normal write-back memory (4.5.4).
6. A descriptor of type `BOOT_MAP_MEMORY_TYPE` covers `[map_phys, map_phys + map_size)`. It shows
   that the map was taken after its buffer was allocated.

Per descriptor:

- The kernel reads `Type`, `PhysicalStart`, `NumberOfPages` and `Attribute` at offsets 0, 8, 24 and 32 of
  each stride. It ignores `VirtualStart`, because the loader does not call `SetVirtualAddressMap`, and it ignores
  any bytes past 40.
- A descriptor is usable only when `Type` is 7 (`EfiConventionalMemory`) and neither `EFI_MEMORY_RUNTIME`
  nor `EFI_MEMORY_SP` is set. The first two conditions are today's rule (`phys_memory.rs:16-27`). The SP
  exclusion is new: the specification earmarks that memory for particular drivers.
- Malformed: `PhysicalStart` not aligned to 4 KiB, `NumberOfPages` of 0, or a range reaching past
  `0xffff_ffff_ffff_f000`. A malformed usable descriptor adds no RAM. A malformed other descriptor
  still reserves its bytes, rounded outward to pages with saturation.
- Overlap. The kernel sorts the descriptors in place by `PhysicalStart` with a heapsort over the stride
  (Williams 1964; Knuth vol. 3 §5.2.3). That uses no allocation and is O(n log n), and it does not depend
  on the firmware's order. The buffer is the kernel's to write until it is released. Then one sweep finds
  overlaps: a byte that more than one descriptor describes is unusable, whatever the types.
- RAM above the CPU's physical address width is excluded and counted (E1-2, 4.1).

The counts are the descriptors and the stride, the usable ranges and bytes, the malformed descriptors,
the overlapping bytes and the released bytes. They go on the boot line that already names the pool's
source (`main_aarch64.rs:878-899`), and into `/proc/breenix/kmem` once it exists.

### 4.6 x86-64: the bootloader already passes the whole map

- The bootloader and `bootloader_api` are pinned to one revision, 707db112 (`Cargo.toml:105`,
  `kernel/Cargo.toml:214, 226`), and are built into the same image as the kernel.
- `BootInfo.memory_regions` is a `MemoryRegions`, which `bootloader_api` documents as an FFI slice
  "semantically equivalent to `&'static mut [MemoryRegion]`". It is a pointer and a length, with no count
  limit in the handoff.
- Per the same documentation, the bootloader builds it from the firmware's map and marks the memory it
  uses itself, and regions marked `Usable` may be used freely. The kernel takes `Usable` only
  (`frame_allocator.rs:782-833`).
- So x86 already delivers a full map. Its one cap is the kernel's: `MAX_REGIONS = 128`
  (`frame_allocator.rs:20`, applied at lines 807-833). E1-1 removes it.
- The slice lives in memory the bootloader reports as its own, so the frame allocator does not hand it out, and the range
  set's carve cannot overlap it while the kernel is still reading it. E1 does not release it.
- No versioning question arises on x86. E1 does not change the x86 handoff, and the bootloader ships with
  the kernel from the same pinned revision. Which firmware types the bootloader reports as `Usable` is its
  documented contract, and E1 leaves it alone.

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
  VA is plentiful: the 48-bit kernel half is 128 TiB, and mapping a window consumes table pages below its boot-installed root.
  The direct map (RAM), span window (RAM) and vmap window (4 × RAM) together take about 6 × RAM of it,
  which is the bound section 9 names.
- On x86-64, E1-2x preallocates a distinct PDPT for each kernel PML4 slot. The boot, master and
  process roots share those per-slot PDPTs. E1-5 reserves VA windows on that layout:
  - each window takes ceil(size / 512 GiB) PML4 slots of its own and uses the PDPTs installed by
    E1-2x;
  - a window mapper walks from the window's own PDPT and installs only lower-level entries;
  - an intermediate table is installed by a compare-exchange of the empty entry. The loser returns
    its zeroed page and uses the winner's. Leaf entries need no lock because the VA allocator gives
    each range to one owner.
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
| ARM64 frame pool: 32 ranges (`MAX_RANGES`), a 40-bit output address width (`TCR_IPS_40BIT`) | Ranges fed straight into the growable set; IPS from the CPU's PARange (4.1) |
| ARM64 UEFI handoff: 32 entries in `HardwareConfig.ram_regions` (`MAX_RAM_REGIONS`) | The final UEFI map itself, of any length, passed by pointer and size (4.5) |
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
  translation table updates, TLB maintenance instructions, the address translation instructions
  (`AT S1E1R`, `AT S1E1W`) and `PAR_EL1`.
- Unified Extensible Firmware Interface Specification, version 2.10: §2.3.6 (AArch64 platforms), §7.2.1
  (`AllocatePages()` and the memory types reserved for OS loaders), §7.2.3 (`GetMemoryMap()` and
  `EFI_MEMORY_DESCRIPTOR`), §7.2.4 (`AllocatePool()`), §7.4.6 (`ExitBootServices()`).
- The API documentation of the loader's `uefi` crate, version 0.33 (`boot::exit_boot_services`,
  `MemoryMapMeta`, `MemoryType::custom`), and of `bootloader_api` at the pinned revision
  (`BootInfo::memory_regions`, `MemoryRegions`). These are interface documents; no bootloader or firmware
  implementation was read.
- System V Application Binary Interface, Generic ABI: the ELF section header and symbol table formats the
  loader's `find_symbol` already reads, and the symbol types, section indexes and `SHT_PROGBITS` and
  `SHT_NOBITS` section types that `find_object_symbol` checks.
- The Rust Reference, the `used` attribute (an item kept in the object file, which the linker may still
  remove), and the `uefi` crate 0.33's `table::set_system_table`, `MemoryMap` and `MemoryMapMeta`
  documentation.
- J. W. J. Williams, "Algorithm 232: Heapsort", Communications of the ACM 7(6), 1964; Knuth, *TAOCP*
  vol. 3, §5.2.3.
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
| E1-1 | One usable-RAM range set and a boot carve; the complete UEFI map handoff (`HardwareConfig` v8); no 128-region or 32-range limit | E1-0 | M |
| E1-2 | ARM64 direct map over the described RAM | E1-1 | S |
| E1-2x | x86 kernel half with per-slot preallocated PDPTs | E1-1 | S |
| E1-3 | Frame metadata sections replace the heap ledger; free list through metadata; drop the empty-span lock | E1-2, E1-2x | M |
| E1-4 | `AllocError`, allocation contexts, the reclaim registry and the first shrinkers; OOM record on both architectures | E1-3 | S |
| E1-5 | Kernel VA windows: x86 window allocator and the ARM64 TTBR1 mapper, with batched flush before free | E1-2x, E1-3 | M |
| E1-6 | Slab caches with per-CPU magazines for requests up to 4 KiB | E1-4 | M |
| E1-7 | `heap_index` span arenas, vmap large allocations, in-place realloc, the DMA bus-address check; built and self-checked, not yet routed | E1-5, E1-6 | M |
| E1-8 | Buddy allocator, zones, per-CPU page caches, mobility pageblocks, watermarks, `dma::alloc` with address limits; each direct frame caller in drivers converted; frontier and LIFO list deleted | E1-7 | M |
| E1-9 | Route the heap to span arenas and vmap; move heap buffers given to devices to `dma::alloc`; retire the fixed heap | E1-7, E1-8 | S |
| E1-10 | Kernel stacks from the buddy in a stack window | E1-5, E1-8 | S |
| E1-11..E1-20 | OOM conversions, one subsystem per PR (list below) | E1-4 (E1-9 for vmap-backed staging) | S each |
| E1-21 | Build `alloc` with `no_global_oom_handling`; replace infallible dependencies | E1-11..E1-20 | S |

### E1-1: one usable-RAM range set, the complete UEFI map handoff, and a boot carve

- Change, kernel:
  - a pure `memory/boot_ranges.rs` builds the normalised range set from either architecture's input, and
    `carve(bytes, align, below)` takes the highest fitting run. `frame_allocator::init` and
    `init_aarch64` consume it. `MAX_REGIONS` goes.
  - `boot_ranges::from_hardware_config` is the one function `init_from_parallels` calls for RAM. It
    accepts v7 and v8 (4.5.3). For v8 it validates, sorts and reads the map (4.5.5), builds the set in two
    passes, and releases the map's pages last (4.5.4).
  - It reads physical memory only through a `BootMemory` trait (read, write, and "does this page
    translate for a write, to itself, as normal memory"). The kernel implements the trait with the
    identity map and `AT S1E1W` with the `PAR_EL1` checks of 4.5.4.
  - The kernel exports `BREENIX_HANDOFF_VERSIONS = [7, 8]` in `.rodata.breenix_handoff`, and the ARM64
    linker script keeps that section (4.5.3). For a version it does not accept, it prints the refusal on
    the UART and halts, instead of the `assert!` (4.5.3).
- Change, `arm64-boot-contract`:
  - one `HardwareConfig` (`HardwareConfigV7` plus `UefiMapHandoff`), `HARDWARE_CONFIG_VERSION = 8`, and
    `BOOT_MAP_MEMORY_TYPE`;
  - a stride-aware descriptor reader with the checks of 4.5.5, which the kernel and the loader's
    runtime-region loop use, and `uefi_map::check_meta`, which the loader runs first (4.5.2);
  - `encode_uefi_map`, which the loader uses to fill `UefiMapHandoff` from the final map's `meta()` and
    buffer;
  - `handoff_decision(elf)`, which the loader uses to proceed or refuse, and the bounded
    `find_object_symbol` it reads the version with (4.5.3).
- Change, `parallels-loader`:
  - `exit_boot_services` with `BOOT_MAP_MEMORY_TYPE`;
  - `handoff::exit_and_encode`, which runs the exit, the metadata check, the runtime-region loop through
    the stride-aware reader, and `encode_uefi_map`, and writes `ram_region_count = 0`;
  - the buffer's pages mapped in TTBR0 only, writable with PXN and UXN where the granule permits (DEV-39), splitting a block whose
    attributes or output address cannot serve the buffer (4.5.4);
  - `handoff_decision` before exit;
  - the filter loop deleted, and `populate_ram_regions` writing a loader-local RAM base instead of
    `HardwareConfig.ram_regions` (4.5.2).
- x86-64: usable regions past the 128th are kept instead of dropped. Allocation behaviour is otherwise
  unchanged, and the bootloader handoff does not change (4.6).
- ARM64:
  - The device tree path feeds the growable set from `fdt::walk` (4.1).
  - The UEFI path takes the final map. Each conventional range the firmware describes reaches the pool,
    up to the 4 GiB clip that E1-2 removes. Specific-purpose memory and bytes that two descriptors
    describe are newly excluded (4.5.5), and the map buffer's pages are newly released.
  - A v7 loader still boots this kernel, with at most 32 ranges. This loader refuses a kernel older than
    E1-1 before it exits boot services.
- Tests (host, new crate `tools/kmem-host-tests`). It depends on `arm64-boot-contract` as a crate and
  includes `boot_ranges.rs` by `#[path]`.
  - The boundary tests run the code production runs on each side. They are joined only by bytes: a
    `HardwareConfig` and a map buffer in a simulated physical memory. The simulated memory implements
    `BootMemory`, and its pages translate only as the loader-built tables map them (the walker of the
    mapping check below).
    - The loader side is its whole post-exit sequence, not the encoder alone. In production, `main.rs`
      calls one function, `handoff::exit_and_encode(config)`, for everything from the exit to the
      encoded handoff. That covers `uefi::boot::exit_boot_services(MemoryType::custom(BOOT_MAP_MEMORY_TYPE))`,
      the metadata check of 4.5.2, the runtime-region loop and `encode_uefi_map`. The early map read,
      `populate_ram_regions`, runs before it, as in production.
    - The firmware under it is a fake. The test crate depends on `uefi` 0.33, without the loader's
      `panic_handler` and `global_allocator` features, and installs a fake system table with the crate's
      public `uefi::table::set_system_table`. The fake boot-services table implements the entries the
      crate calls: `GetMemoryMap`, `AllocatePool`, `FreePool` and `ExitBootServices`. Any other entry
      panics with its name. The pool it hands out is host memory, and its physical address is its host
      address, as under UEFI's AArch64 identity mapping (§2.3.6).
    - The fake keeps the specification's key rule: `ExitBootServices` accepts only the key of the latest
      `GetMemoryMap`, and each `AllocatePool` changes the key.
    - The kernel side is `from_hardware_config`, reading through `BootMemory`.
  - The first commit holds the checks. It also moves today's code, unchanged, into callable form, and
    switches the production callers to it, so each red result is the old code's:
    - the loader's filter-and-copy (`main.rs:376-387`), as `encode_v7_ram_regions(config, descriptors)`;
    - the kernel's array read (`platform_config.rs:894-912`), as the v7 arm of `from_hardware_config`;
    - the kernel's version check (`platform_config.rs:858-863`), which accepts only 7;
    - the loader's decision, which today proceeds with any kernel image.
  - The reference fixture is one final map:
    - 40 `EfiConventionalMemory` descriptors of 2 MiB, separated by 40 `EfiReservedMemoryType` descriptors
      so that no two merge;
    - loader, boot-services and runtime descriptors, and the buffer's own `BOOT_MAP_MEMORY_TYPE`
      descriptor;
    - shuffled with a fixed seed, at `DescriptorSize` 48 with nonzero bytes at offsets 40 to 47.
  - Check first, the boundary:
    - The old code gets the fixture's descriptors as tuples, as the `uefi` crate's iterator yields them.
      It passes 32 ranges, the pool is 16 MiB short, and no truncation is reported, which is red.
    - The new loader and kernel keep 40 of 40. The test compares the kernel's usable set, as a sorted
      list of `[start, end)` intervals, with the 40 intervals the fixture writes out, and the excluded
      intervals with the expected list for each reason. Counts and byte totals alone would pass with one
      interval dropped and another of the same size in its place. The same fixture at `DescriptorSize` 40
      gives the identical lists.
    - Stale key, one retry. The fake's first `ExitBootServices` returns `EFI_INVALID_PARAMETER`, so the
      exit is retried, as the specification requires. The test uses three maps that differ in their
      conventional ranges: the early map that `populate_ram_regions` reads, the map from the first exit
      attempt, and the map from the retry. The second and third both contain the buffer's
      `BOOT_MAP_MEMORY_TYPE` descriptor, so rule 6 alone cannot tell them apart. Only the retry's map is
      the reference fixture. The kernel's intervals must equal the retry map's exactly, and
      `UefiMapHandoff.map_size` must equal the retry's `meta().map_size`. A loader that encodes the early
      map or the first attempt's map fails on the interval lists. One that sends the length of the
      pool's whole `buffer()` fails the `map_size` check.
  - Check first, versions:
    - A v8 config is accepted, which is red on the old check.
    - A v7 config from `encode_v7_ram_regions` is accepted with at most 32 ranges under source
      `UefiRegionListV7`. This guards the older-loader direction.
    - A v9 config is refused with its reason.
    - On the loader side, `handoff_decision` gets minimal ELF images the test builds. One exports
      `BREENIX_HANDOFF_VERSIONS = [7, 8]` as an `STT_OBJECT` of size 8 in a `SHT_PROGBITS` section, as
      rustc emits a static, and the decision proceeds. That is red twice on the old code: the old decision
      proceeds with any image, and the old `find_symbol` skips an `STT_OBJECT`. The decision
      refuses, each with its own reason, an image with no such symbol, one exporting `[7, 7]`, one whose
      symbol is `STT_FUNC`, one whose section is `SHT_NOBITS`, and one whose `st_value` lies past its
      section's end.
    - Retention, on the real link. Symbol type and linker retention only show in a linked kernel, so
      `scripts/parallels/build-efi.sh` runs the production `handoff_decision`, as a host binary from
      `tools/kmem-host-tests`, on the ARM64 kernel ELF it has just linked, and fails the build if the
      decision refuses. The check is added in the first commit and fails on today's kernel, which has no
      such symbol. With the static, the `KEEP` line and `find_object_symbol` in place, it proceeds.
  - Check first, validation (one fixture per rule, each a single change to the reference map):
    - Structural cases are each refused with their own reason: `descriptor_version` 2; `descriptor_size`
      36 and 44; a `map_size` that is not a multiple; `map_phys` not 8-byte aligned; a buffer page that
      does not translate, one that translates read-only, one that translates to another physical address,
      and one that translates as device memory; and a buffer that no `BOOT_MAP_MEMORY_TYPE` descriptor
      covers, which is the map a `GetMemoryMap()` taken before the allocation would give.
    - Loader-side metadata. These run through `handoff::exit_and_encode` over the fake firmware. In each,
      the fake reports one bad value: `DescriptorVersion` 2, `DescriptorSize` 36 or 44, a `MapSize` that
      is not a multiple of the stride, and a `MapSize` larger than the pool it was given. Each map holds
      runtime descriptors. For each:
      - the runtime-region loop does not run, so `runtime_region_count` is 0 and `rtc_get_time` is 0;
      - `loader_check` names the rule;
      - the kernel refuses with that rule.
      Check first: today's loader runs its typed runtime-region loop on each. At the 44-byte stride it
      reads descriptors at offsets that are not multiples of 8, records runtime regions from them, and
      hands off with no refusal, which is red.
    - Per-descriptor cases each lose exactly the stated bytes and no others, and the counts match: a
      conventional descriptor that overlaps an `EfiACPIMemoryNVS` descriptor by one page, an unaligned
      start, a page count of 0, a range past `0xffff_ffff_ffff_f000`, and a conventional descriptor with
      `EFI_MEMORY_SP`.
    - The overlap and SP fixtures are red through the old path, which keeps those bytes usable.
  - Check first, the loader's buffer mapping (4.5.4). The test runs the loader's own table construction
    on the host, over a `PageTableStorage` with the buffer's range as input. The `adr` read and the UART
    output stay in the caller, so the construction is a function of its inputs. A walker in the test
    crate then resolves each buffer page through the built TTBR0 tables, following the Arm ARM's
    VMSAv8-64 descriptor formats, and checks four things: the output address is the page's own, AP allows
    an EL1 write, PXN and UXN are set, and AttrIndx is 1. Cases:
    - a buffer in the free pages at the end of a kernel rodata block;
    - one in a 1 GiB slot above 4 GiB that the loader does not map today;
    - one above 512 GiB;
    - on the VMware layout (`ram_base_offset` of `0x4000_0000`), one just above the relocated kernel.
    Today's construction leaves the rodata case read-only and the two high cases unmapped, which is red.
    With the change, each case passes. Each kernel segment page, and each page outside the buffer,
    resolves exactly as it did before, through both TTBR0 and TTBR1. That walk also confirms TTBR1 gains
    no entry. In the boundary tests, `BootMemory`'s translation is that same walker over the
    loader-built tables, so the kernel's probe sees the tables the loader built.
  - Check first, release: the simulated memory records each read and write. The
    `BOOT_MAP_MEMORY_TYPE` pages enter the set only after the last access to the buffer, and the first
    carve after the build may return them. The old code does not release them, which is red.
    - Release filters, compared by address. Each fixture lists the expected usable pages and released
      pages as exact sorted intervals, and the test compares the sets themselves, not their counts or
      byte totals:
      - the buffer's descriptor inside the per-CPU stack interval: no page of `0x4300_0000..0x4400_0000`
        is usable or released, and the boot line counts them as held back by a fixed range;
      - the buffer's descriptor overlapping an `EfiACPIMemoryNVS` descriptor by one page: that page is
        held back, and the rest are released;
      - a second `BOOT_MAP_MEMORY_TYPE` descriptor elsewhere in the map: it is not released;
      - the buffer's descriptor with `EFI_MEMORY_RUNTIME` set: no page is released.
      A release that adds each descriptor of the type after the build, as the plan first proposed, fails
      the first three. The fixtures are check-first in the sense of section 12: they are written before
      the release step and run against a release step without the filters.
  - Check first, x86: the existing region-extraction logic, moved unchanged into the pure module, is given
    200 usable regions. It keeps 128, which is red. After the change it keeps 200 of 200.
  - Check first, device tree: `frame_pool_from_fdt` on a device tree with 40 disjoint memory ranges keeps
    32 and sets `truncated`, which is red. The new path keeps 40 of 40.
  - Unsorted and overlapping firmware entries normalise to disjoint, page-aligned ranges, and the 1 MiB
    floor is applied.
  - A carve with alignment and a `below` limit returns the highest fitting run, or an error when no run fits;
    the range set loses exactly the carved bytes.
  - Range-set storage (4.1): the reference fixture with a 2 GiB conventional descriptor added above
    4 GiB, so the largest usable range lies outside the window. `BootMemory` refuses writes to pages its
    tables do not map writable. Each storage page lies inside the window, and the build completes. The
    old code has no growable set, so this tests the new rule. E1-2 repeats it with the clip removed.
- Boot: DEV-39(3) authorizes one `./run.sh --parallels` boot at the final head to verify that
  firmware describes its pool allocation with the custom type and that the MMU can write through
  the loader's TTBR0 mapping, then observe memory initialization and the rest of the boot path.
- Rows: no row expected to move.

E1-1 implementation decisions (DEV-39):

- A writable identity granule that intersects the UEFI loader image retains its execute permissions,
  because execution must survive the table switch. The loader gets the image extent from
  `EFI_LOADED_IMAGE_PROTOCOL` and applies PXN and UXN only to separate buffer pages or blocks.
- The valid check-first map uses 48-byte descriptors, since the old typed iterator needs an aligned
  fixture to deterministically expose the loss of ranges 33–40.
- The 36-byte stride, fractional and oversized lengths are exercised at the loader's production
  encoder boundary because the UEFI crate applies its own assertions and slice construction before
  returning the owned map. Version 2 and the 44-byte stride also go through the fake exit path.
- The host page walker maps the fake firmware allocation at a synthetic IPA because host pointers
  can exceed the loader's configured 40-bit physical address width. The decoder receives the wire
  bytes the loader wrote, including the original host pointer.
- The builder seeds the largest probed interval for growth and stages the release fragments in its
  range storage, because it may need more storage before the last descriptor has been read. Staged
  fragments are ineligible for storage and become allocatable only after the reader finishes.
- The existing ARM64 heap and stack sizes use the shared highest-fit carve, so their reservation
  remains driven by the usable map and their placement follows the carve's alignment and limit.
- The early loader reports the descriptor count and lowest RAM base because relocation needs that
  base alone. No bounded early RAM array is used in production.

E1-1 review evidence (PR #3259):

- The follow-up checks in `62fb595e` were replayed against `079583f2` before the final-head run
  because the original implementation commit `83345629` was already on the published branch.
- The x86 test feeds the same 200 disjoint usable spans to the old allocator's extracted `init`
  and to the current range builder because their entry points accept different types. The old
  allocator retained 128. The same flattened device tree yielded 32 in-window spans and
  `truncated=true` in the old `frame_pool_from_fdt`, against 40 expected spans.
- The same 48-byte descriptors went through the old loader's `exit_and_encode` and the old kernel
  decoder. They admitted the NVS-overlap page at `0x6000_8000` and the specific-purpose page at
  `0x6020_0000`. The current check compares the exact surviving intervals.
- The old TTBR0 builder was replayed with synthetic loader and table-storage IPAs and cache
  maintenance omitted because host pointers are not guest physical addresses. It left the buffer
  read-only at `0x4420_4000` and `0x8420_4000`, and unmapped at `0x1_2200_0000` and
  `0x80_2000_0000`. The current check also walks neighboring mappings and TTBR1.

### E1-2: ARM64 direct map over the described RAM

- Change:
  - a pure planner turns ranges above the boot window into a minimal list of 1 GiB, 2 MiB and 4 KiB
    descriptors;
  - kernel glue writes them into the live TTBR1 tables, using table pages carved below the boot window's
    end, which the existing direct map already covers (4.4);
  - `direct_map_ram_window`'s clip goes;
  - TCR_EL1.IPS comes from ID_AA64MMFR0_EL1.PARange instead of the fixed 40-bit constant, in `boot.S` and
    in `parallels-loader`'s `page_tables.rs`. RAM above the reported width is left out of the range set and
    counted (4.1). The same width bounds where the loader can map E1-1's map buffer (4.5.4). The loader
    computes the width once and uses it for both.
  - The order at boot is fixed. First the range set is built from the firmware description (E1-1). That
    includes reading the UEFI map through the loader's mappings and releasing its buffer. Then the table
    pages are carved, and then the direct map is extended. So a table page may come from the released
    buffer, and no code reads the map through the extended direct map.
  - Removing the clip lets the set describe RAM above 4 GiB before the direct map covers it. The set's
    own storage pages stay inside the boot window and pass the write probe (4.1), so building the set
    writes only to RAM the boot tables map. The set may describe unmapped RAM while E1-2 builds the
    tables for it.
- x86-64: no change.
- ARM64: RAM above 4 GiB joins the frame pool, up to the CPU's physical address width. That holds on both
  paths: the device tree, and E1-1's UEFI map.
- Tests (host):
  - Check first: the kernel's window choice (`direct_map_ram_window`) moves unchanged into
    `arm64-boot-contract` as a pure function, and `frame_pool_from_fdt` is called with it on a device tree
    describing 6 GiB at 0x4000_0000. The pool ends at 4 GiB, which is red. After the change it covers the
    6 GiB, less the fixed ranges and reservations.
  - Check first, the UEFI boundary. This is E1-1's reference fixture with its conventional descriptors
    moved to span 0x4000_0000 to 0x1_C000_0000 (6 GiB), and with the map buffer placed above 4 GiB. It goes
    through `encode_uefi_map` and `from_hardware_config`, as in E1-1.
    - On E1-1's code the pool ends at 4 GiB, which is red, and the buffer's released pages are clipped
      away with the rest.
    - After the change, the pool covers the 6 GiB less the fixed ranges, the non-conventional
      descriptors and the overlaps, and the released buffer pages are in it.
    - The buffer is read at its own address above 4 GiB, through `BootMemory`. Its readability does not
      depends on the direct-map window.
  - Planner: each page of each range is covered exactly once; no descriptor covers a non-RAM address;
    blocks are used wherever alignment allows; the number of table pages is minimal for the input.
  - Range-set storage: a variant of the 6 GiB fixture whose largest conventional descriptor lies above
    4 GiB, outside the window. `BootMemory` refuses a write to a page its tables do not map writable, as
    the MMU would fault. Each storage page the build uses, the first and each growth page alike, lies
    inside the window, and the build completes. The fixture describes enough
    ranges that the set grows at least once. Check first: the same build with storage taken from the
    largest range writes above 4 GiB and is refused, which is red.
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
  - That boot reaches the planner and the MMU through the device tree path only. The loader's
    PARange-derived IPS and the UEFI path above 4 GiB run on Parallels and VMware, which the final run
    covers (14.8).
- Rows: no row moves on the 512 MiB rig, which has no RAM above 4 GiB.

E1-2 implementation decisions (DEV-39):

- Both boot paths cap IPS at 48 bits with the current 4 KiB descriptors and `TCR_EL1.DS=0`, since wider output addresses need another descriptor format. The kernel uses the installed IPS so an older loader's tables retain their configured output width.
- The kernel also excludes and counts RAM whose direct-map VA would overlap the existing ECAM and MMIO window, because the boot mapper must keep those virtual addresses available for devices.
- The QEMU assembly leaves the old high ECAM slot empty because the existing device-tree path already installs ECAM at its dedicated virtual address, and the obsolete 1 GiB Device block could obstruct RAM.
- The QEMU watchdog uses `pci_ecam_virt_base()` for its native-width configuration accesses because the removed boot alias no longer covers high ECAM.
- The UEFI loader maps its high image in TTBR0 and only its exact page-table storage in TTBR1, so the kernel can reach both roots through their live aliases and install finer leaves in neighboring slots without replacing valid translations.
- Older loaders' existing writable identity blocks are retained, because changing valid translations would require break-before-make and could remove the running roots' aliases.
- The existing low heap and stack reservations precede the table carve, so a table shortfall can retain a bootable low-memory pool. Heap sizing still uses the RAM in its low placement window.
- The host FDT fixture runs the production growable visitor and the UEFI fixture uses the production stride-aware encoder and table builder. These are the paths that make RAM allocatable after E1-1.

### E1-2x: x86 kernel half with per-slot preallocated PDPTs

- DEV-42 and #3264. Boot allocates 256 independent 4 KiB PDPT frames (1 MiB) and copies an existing
  PDPT only into the new table for its corresponding PML4 slot. The master inherits the boot root,
  and each process copies the master's 256 kernel entries. Later kernel mappings edit the shared
  tables below PML4. The frame allocator retains its boot order because the migration keeps its
  direct-map addresses backed by the same frames.
- DEV-42(2) offered two orders. E1-2x takes the translation-preserving migration rather than moving
  `kernel_page_table::init` ahead of `frame_allocator::init`: the migration needs 256 frames from that
  allocator, and leaving the boot translations unchanged makes the order irrelevant to direct-map
  pointers.
- After `build_master_kernel_pml4`, the kernel writes PML4 kernel entries only into a process root
  that `ProcessPageTable::new` has not yet published. The old `map_kernel_page` re-pointed slots at
  the shared PDPT and fell back to the active root before the master existed. That fallback is gone:
  the 3 of 3 `map_kernel_page` call sites (`kernel_stack`, `per_cpu_stack`, `gpu::dma`) run after
  `memory::init` publishes the master, and a call before then now returns an error.
- Implementation decisions (DEV-39(2)):
  - A copied PML4 entry's read-only or execute-disable restriction moves to its populated PDPTEs so
    existing access rights survive and later mappings can use empty PDPTEs without changing the root
    permissions. See [Intel SDM, Vol. 3A, paging and access rights](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
    XD is copied down only from a root entry that already sets it, which the CPU accepts only with
    `IA32_EFER.NXE` set.
  - The master uses the migrated stack and IST PDPTs (slots 402 and 403) instead of allocating
    fresh ones. The old fresh allocation existed only to undo the shared-PDPT aliasing, and replacing
    a per-slot PDPT would discard any boot mappings in those slots. `main.rs` sets the bootloader's
    physical-memory mapping to `Mapping::Dynamic` and keeps the default for the others, and the
    measured physical-memory and kernel offsets are lower-half. So these slots are expected to arrive
    empty, matching the old fresh tables. The next R240 flash shows it: a present PTE in the stack
    range would make `map_kernel_page` refuse a stack mapping and count it in
    `pte_overwrite_refusals`. Their stack ranges still receive the lower-level tables built at boot.
  - The master no longer aliases boot PML4[0]'s PDPT at slot 511. That alias did not serve
    `layout::high_alias_from_low`: `0xffffffff80000000` falls in PDPT index 510 of the aliased table,
    the low VA `0x7f80000000`, not `KERNEL_LOW_BASE`, and `syscall::entry_address` and the IDT setup
    use the high alias only for kernels linked in `[0x100000, 0x40000000]`, while the bootloader
    loads the PIE above that range (`0x8000000000` and `0x10000000000` observed). The alias also put
    the boot root's slot-0 hierarchy at slot 511 of each process root, which copies 256 to 511.
  - A process requires the master root: `memory::init` publishes it before `heap::init`, which
    process creation depends on, and the active root at process creation can belong to another
    process.
  - `memory::init` no longer calls `paging::init` before the CR3 switch. That call's only lasting
    effect was to fill the never-read `PAGE_TABLE_MAPPER` cell with a mapper over the boot root;
    the remaining call after the switch now fills it with the master.
  - Exec no longer clears PML4 slots 32 and 170 of its new root (`clear_user_entries`, removed).
    This defect predates E1-2x and was raised in the #3270 review. The root is fresh from
    `ProcessPageTable::new`, which on x86 fills those slots only from the master's supervisor-only
    entries, and the dynamic physical-memory mapping can place the direct map there. So the clear
    could only erase a kernel mapping. On ARM64 the fresh root is empty and the clear had no effect.
- Check first, exec root. The fixture `exec_root_keeps_a_direct_map_at_pml4_32` puts the master's
  direct map at PML4[32] and runs the production slot policy (`inherit_master_slots`) and exec's
  slot clear. It fails on the commit that moves them into `x86_kernel_half.rs` unchanged: low
  physical RAM does not translate under the exec root. The fix removes the clear.
- Check first. The #3264 fixture drives the production migration loop with slots 256 and 257
  sharing a PDPT index. Before and after migration, the expected translations are
  `0xffff800010000000` to `0x10000000` and `0xffff808010000000` to `0x8010000000`.
  The old loop sends both addresses to `0x8010000000`.
  The access-rights assertions (slot 256's root entry read-only and XD) were added after the
  migration change. They fail on a copy of the new helper with the push-down removed.
- Boot. The next coordinator R240 one-reboot flash supplies x86 hardware evidence under DEV-42.

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
  - `FaultOutcome::OutOfMemory` with the existing SIGKILL policy, counted;
  - the caller inventory, `docs/plans/e1-oom-inventory.json`, and its scan `tests/kmem_oom_inventory.rs`
    (see E1-11 to E1-20): each runtime site with its owning PR, error surfaced and rollback point.
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
  - Check first, inventory: the scan with an empty inventory reports each runtime site as unowned, which
    is red. With the filled inventory it passes, and a row with no owner, error or rollback point fails.
- Boot: not needed.
- Rows: no row expected to move. This is the failure path.

### E1-5: kernel VA windows

- Change: the span, vmap and stack windows (6.5), with a VA allocator per window.
  - x86-64 uses a window mapper over the slots and PDPTs preallocated by E1-2x (6.5), with
    compare-exchange table installs, and a pending-unmap list flushed through `memory/tlb.rs` with a new
    request kind that removes global entries (`invlpg` per page, or a CR4.PGE toggle; 6.5).
  - ARM64 gets a TTBR1 4 KiB and 2 MiB mapper. Its unmap flushes through the existing
    `arch_stub::tlb::flush_pages` (`dsb ishst`, `tlbi vaae1is` per page, `dsb ish; isb`).
  - Only this PR's own self-check at init uses the windows yet.
- x86-64: page directories and page tables for the windows are allocated on first use.
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
  - Address space: use E1-2x's per-slot PDPTs, and check that a window of 4 × 128 GiB uses four
    slots with independent translation trees.
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

The caller inventory comes first, in E1-4, so no runtime caller is left without an owner:

- `tests/kmem_oom_inventory.rs` scans `kernel/src` for the infallible allocation APIs of 7.4, outside
  `#[cfg(test)]`. Each site must match a row of `docs/plans/e1-oom-inventory.json`. A row holds the
  file and function, the owning PR, the error surfaced, and the rollback point: the last state change
  that must be undone, or that has not happened yet, when the allocation fails. A site with no row, or
  a row with an empty field, fails the test.
- Check first: with an empty inventory, the scan over f52fd83c reports each site as unowned, which is
  red. E1-4 fills the inventory until it passes. Each conversion PR then marks its rows converted, and
  its guard fails while one of its rows still names an infallible call.
- The table below assigns each top-level part of `kernel/src`. A rough count at f52fd83c of the patterns
  in 7.4 found runtime sites in each of these; the inventory, not the count, is the list.

| PR | Subsystem | Error surfaced and rollback point | Host sweep in |
|---|---|---|---|
| E1-11 | Syscall entry, user copies and staging buffers (`syscall/` except the files other rows name), `time/`, `random/`, `interrupts/` (interrupt handlers may not allocate; a site there moves to preallocated state) | `ENOMEM`, before any user-visible effect | `regular-read-host-tests` |
| E1-12 | Memory syscalls and VMAs (`syscall/mmap.rs`, `syscall/memory.rs`, `memory/vma.rs`) and the rest of `memory/` | `ENOMEM`, before the VMA or page-table change is published | `pagetable-host-tests` |
| E1-13 | Process and thread lifecycle: fork, clone, exec, spawn (`process/`, `task/`, `arch_impl/` runtime paths) and signals (`signal/`, `syscall/signal.rs`) | `EAGAIN` or `ENOMEM` per the call. `sigqueue` returns `EAGAIN` (POSIX: no resources to queue), before the signal is queued. Kernel-generated signals use per-thread preallocated slots and do not allocate | structural guard only |
| E1-14 | VFS and path walk (`fs/vfs`, `fs/namei.rs`, `ipc/fd.rs`) and the other in-memory filesystems and locks (`fs/devfs`, `fs/devptsfs`, `fs/ufs`, `fs/flock*.rs`, `fs/locks.rs`) | `ENOMEM`, before the dentry, fd or lock is published | `vfs-host-tests`, `resolve-host-tests` |
| E1-15 | BreenFS and ext2 runtime (`fs/breenfs`, `fs/ext2`) | `ENOMEM` before any disk change, by the reservation rule | `breenfs-host-tests`, `ext2-host-tests` |
| E1-16 | Network and sockets (`net/`, `socket/`) | `ENOBUFS` on send, before the segment is queued; counted drop on receive | `net-host-tests`, `udp-host-tests`, `unix-host-tests` |
| E1-17 | IPC: pipes, SysV and POSIX IPC, shared memory (`ipc/` except `fd.rs`) | `ENOMEM` or `ENOSPC` per the man page, before the object is linked | `ipc-host-tests`, `shmsem-host-tests`, `shmem-api-host-tests` |
| E1-18 | Block layer and storage drivers' runtime paths (`block/`, `drivers/ahci`, `drivers/nvme`, `drivers/virtio/block*`) | I/O error to the request, before it is submitted; not a partial request | `block-host-tests` |
| E1-19 | Other drivers, GPU, console and windows (`drivers/`, `gpu/`, `console/`, `tty/`, `graphics/`, `window/` with its commit path, `input/`, `keyboard/`) | A window commit returns `ENOMEM` before the new buffer is published, so the last committed frame stays on screen. An input batch is dropped whole and counted, before it is published to readers. Other drivers: counted drop or the call's errno | `gpu-host-tests`, `console-host-tests` |
| E1-20 | Diagnostics (`fs/procfs` including `/proc/kmsg`, `capture/`, `tracing/`, `log_buffer.rs`, `serial/`, `test_control/`) | A `/proc/kmsg` read builds its buffer before it consumes from the log ring, so a failure returns `ENOMEM` with the read position unchanged. Logging itself does not allocate | structural guard only |

- `boot/` and the boot-time parts of other directories are `boot:` sites (7.4). The inventory lists them
  with owner E1-4 and the boot rule.
- `contracts/` (`testing` on x86), `proof/` (`coreproof` on ARM64) and `test_framework/` (`boot_tests` or
  `btrt`) are built only with those features (`kernel/src/lib.rs:143-167`, `:263`). Their sites are
  listed with the owner `test` and are outside the runtime rule, so they do not block E1-21's flip for
  production builds. They are converted when a test feature builds `alloc` with the cfg.
  `test_control/` is built in each configuration (`lib.rs:83`), so it is a runtime caller and belongs to
  E1-20.

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
- Boots: E1-1 uses one Parallels boot (DEV-39). E1-2, E1-3, E1-5, E1-8, E1-9 and E1-10 each need one ARM64 QEMU boot of the standard boot script,
  for the reason given in each PR, and only for the cases named. The other PRs need no boot. No PR runs an
  effort's suites (DEV-30).
- The ARM64 QEMU boot script does not run `parallels-loader` (1.5). So the loader-to-kernel boundary is
  tested on the host: the loader's post-exit sequence runs over a fake firmware table, and the kernel's
  decoder reads the bytes it produced (E1-1).
  A QEMU boot is not evidence for it.
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
   is known once E1-4's inventory (`docs/plans/e1-oom-inventory.json`) lists each site with its owner. If
   one subsystem's PR would be too large to review, it splits by module with the same guard. The
   inventory's converted rows say which part is done.
6. The 32-byte `FrameMeta` costs 0.78% of RAM, against today's 12 bytes plus 8 bytes of free-list
   capacity. That buys O(1) buddy links, the slab back-pointer, and no heap dependency. E3 may grow it to
   64 bytes (5.7).
7. No step in this plan changes BreenFS's on-disk format, or anything under `compiler/` or
   `src/llvm-project`.
8. The UEFI handoff runs only on Parallels and VMware. E1-1's host tests settle the byte boundary: layout,
   stride, validation, versions and release order. They cannot settle two firmware effects: that real
   firmware describes the pool buffer with its own type, and that the MMU walks the loader's buffer
   entries as the host walker does.
   - DEV-39(3) resolves the earlier testing question with one final-head Parallels boot. The VMware
     firmware and MMU remain covered by the program's final run.
9. Version skew. `scripts/parallels/build-efi.sh` builds the loader with the kernel from one tree, so a
   fresh image pairs matching versions. Skew comes only from an older disk image, or a loader
   built from another tree.
   - A v7 loader with a newer kernel boots with at most 32 ranges, and the boot line says so (4.5.3).
   - A v8 loader with an older kernel is refused by the loader, before it exits boot services.
   - v7 acceptance in the kernel stays until the coordinator retires it.
10. Reclaiming boot-services and loader memory after device reset (2.2) is not in E1, and no effort owns
    it. The complete map makes it possible. On a VM whose firmware keeps large boot-services ranges, it is
    RAM the kernel does not use. Recommendation: record it for E3's reclaim work rather than open anything
    now.
