//! Kernel VA windows (E1 plan 6.5, PR E1-5): table installs, unmaps, the
//! flush before free, the free-extent trees and window placement, driven
//! through the kernel's own `kva_map.rs`, `kva_extent.rs`, `kva_window.rs`,
//! `tlb_request.rs` and E1-2x's `x86_kernel_half.rs`.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "../../../kernel/src/memory/kva_extent.rs"]
mod kva_extent;
// The ARM64 encoding and the kernel's thresholds and names are not used here.
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kva_map.rs"]
mod kva_map;
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/kva_window.rs"]
mod kva_window;
// The kernel's shootdown uses the request kinds this file does not.
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/tlb_request.rs"]
mod tlb_request;
#[path = "../../../kernel/src/memory/x86_kernel_half.rs"]
mod x86_kernel_half;

use kva_extent::{Extent, ExtentError, Extents};
use kva_map::{Format, MapError, TableMemory, TablePages, Walk, X86};
use kva_window::{FlushPlan, FlushPolicy, Placement, Window, WindowId};

const PAGE: u64 = 4096;

#[repr(C, align(4096))]
struct Page([AtomicU64; 512]);

/// Table pages addressed by physical address, held in one allocation so
/// that its base works as a physical-memory offset. Page 0 is not handed
/// out, so 0 is free to mean "no table".
struct Tables {
    pages: Vec<Page>,
}

impl Tables {
    fn new() -> Self {
        let mut tables = Tables { pages: Vec::with_capacity(2048) };
        tables.page();
        tables
    }

    /// A new page, filled with a pattern so a missing zeroing shows.
    fn page(&mut self) -> u64 {
        assert!(self.pages.len() < self.pages.capacity(), "fixture out of pages");
        let pa = self.pages.len() as u64 * PAGE;
        self.pages.push(Page([const { AtomicU64::new(0xdead_beef) }; 512]));
        pa
    }

    /// The host address of physical address 0.
    fn offset(&self) -> u64 {
        self.pages.as_ptr() as u64
    }
}

impl TableMemory for Tables {
    fn load(&self, table: u64, index: usize) -> u64 {
        self.pages[(table / PAGE) as usize].0[index].load(Ordering::Acquire)
    }
    fn store(&self, table: u64, index: usize, value: u64) {
        self.pages[(table / PAGE) as usize].0[index].store(value, Ordering::Release);
    }
    fn compare_exchange(&self, table: u64, index: usize, current: u64, new: u64) -> Result<u64, u64> {
        self.pages[(table / PAGE) as usize].0[index].compare_exchange(
            current,
            new,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
    }
    fn zero(&self, page: u64) {
        for entry in self.pages[(page / PAGE) as usize].0.iter() {
            entry.store(0, Ordering::Relaxed);
        }
    }
    fn publish_table(&self) {}
}

const X86_GLOBAL: u64 = 1 << 8;
/// A 4 KiB kernel data leaf: present, writable, global, no-execute.
fn x86_leaf(pa: u64) -> u64 {
    pa | 1 | (1 << 1) | X86_GLOBAL | (1 << 63)
}

/// A root whose kernel slot `slot` links an empty PDPT, as E1-2x leaves each
/// upper-half slot of the master PML4.
fn x86_root(tables: &mut Tables, slot: usize) -> u64 {
    let root = tables.page();
    tables.zero(root);
    let pdpt = tables.page();
    tables.zero(pdpt);
    tables.store(root, slot, X86::table(pdpt));
    root
}

/// Map `va` to `pa` as a global 4 KiB leaf, installing tables as a CPU would.
fn map(tables: &mut Tables, root: u64, va: u64, pa: u64) {
    loop {
        match kva_map::walk::<X86, _>(tables, root, va, 3) {
            Walk::Table(table) => {
                tables.store(table, kva_map::index(va, 3), x86_leaf(pa));
                return;
            }
            Walk::Missing { parent, index, .. } => {
                let page = tables.page();
                kva_map::install_table::<X86, _>(tables, parent, index, page);
            }
            Walk::Blocked { .. } => panic!("block in the way"),
        }
    }
}

/// The physical address `va` translates to through the tables, if any.
fn translate(tables: &Tables, root: u64, va: u64) -> Option<(u64, bool)> {
    let Walk::Table(table) = kva_map::walk::<X86, _>(tables, root, va, 3) else {
        return None;
    };
    let leaf = tables.load(table, kva_map::index(va, 3));
    X86::is_valid(leaf).then(|| (X86::address(leaf), leaf & X86_GLOBAL != 0))
}

/// One CPU's TLB, following Intel SDM vol. 3A §4.10.4.1 with CR4.PGE set
/// and no PCIDs: a CR3 load drops non-global entries only, `invlpg` drops the
/// page's entry whether global or not, and clearing and restoring CR4.PGE
/// drops each entry.
#[derive(Default)]
struct FakeTlb {
    entries: HashMap<u64, (u64, bool)>,
}

impl FakeTlb {
    /// Access `va`, filling the TLB from the tables on a miss.
    fn access(&mut self, tables: &Tables, root: u64, va: u64) -> Option<u64> {
        if let Some(&(pa, _)) = self.entries.get(&va) {
            return Some(pa);
        }
        let (pa, global) = translate(tables, root, va)?;
        self.entries.insert(va, (pa, global));
        Some(pa)
    }

    fn holds(&self, va: u64) -> bool {
        self.entries.contains_key(&va)
    }
}

impl tlb_request::TlbOps for FakeTlb {
    fn invlpg(&mut self, va: u64) {
        self.entries.remove(&(va & !(PAGE - 1)));
    }
    fn reload_cr3(&mut self) {
        self.entries.retain(|_, &mut (_, global)| global);
    }
    fn toggle_global_pages(&mut self) {
        self.entries.clear();
    }
    fn release_root(&mut self, _root: u64) {}
}

/// Check first (E1 plan, E1-5): a kernel window page is global, so the flush
/// that precedes its frame's release must remove global translations. The
/// page is mapped and used on 4 CPUs, removed through `unmap_kernel_page`'s
/// walk, which leaves the flush to its caller, and then each CPU, the
/// initiator included, executes the kernel-unmap request. A CR3 reload
/// leaves the global entry in each CPU's TLB, so 4 of 4 could still reach
/// the frame the allocator is about to hand out again.
#[test]
fn kernel_unmap_flush_leaves_no_cpu_holding_a_global_translation() {
    let mut tables = Tables::new();
    let slot = 257;
    let root = x86_root(&mut tables, slot);
    let va = 0xffff_8080_0020_3000u64;
    assert_eq!(kva_map::index(va, 0), slot);
    let frame = 0x5000_0000u64;
    map(&mut tables, root, va, frame);

    let mut cpus: Vec<FakeTlb> = (0..4).map(|_| FakeTlb::default()).collect();
    for cpu in cpus.iter_mut() {
        assert_eq!(cpu.access(&tables, root, va), Some(frame));
    }

    let unmapped = kva_map::unmap_page::<X86, _>(&tables, root, va).unwrap();
    assert_eq!(unmapped, Some(frame));
    assert_eq!(translate(&tables, root, va), None);

    let pages = [va];
    let request = tlb_request::kernel_unmap_request(&pages);
    let (kind, arg) = tlb_request::encode(request);
    for cpu in cpus.iter_mut() {
        // SAFETY: `pages` outlives each call.
        unsafe { tlb_request::execute(kind, arg, cpu) };
    }

    let holding: Vec<usize> = (0..4).filter(|&cpu| cpus[cpu].holds(va)).collect();
    assert_eq!(
        holding,
        Vec::<usize>::new(),
        "CPUs still translating {va:#x} to frame {frame:#x} when it is offered back",
    );
}

/// Check first (E1 plan, E1-5): two CPUs make first mappings under the same
/// empty PD entry in the fixed order read-empty (CPU 0), read-empty (CPU 1),
/// install (CPU 0), install (CPU 1), and then each writes its leaf in the
/// table its install returned. The old install overwrites CPU 0's page table
/// with CPU 1's, and CPU 0's leaf is lost.
#[test]
fn first_mappings_racing_under_one_empty_entry_keep_both_leaves() {
    let mut tables = Tables::new();
    let slot = 258;
    let root = x86_root(&mut tables, slot);
    // Both VAs need the same PT, under PD entry 0 of a PD that exists.
    let va0 = 0xffff_8100_0000_0000u64;
    let va1 = va0 + PAGE;
    let pdpt = X86::address(tables.load(root, slot));
    let pd = tables.page();
    kva_map::install_table::<X86, _>(&tables, pdpt, 0, pd);

    let Walk::Missing { parent: p0, index: i0, level: 2 } = kva_map::walk::<X86, _>(&tables, root, va0, 3)
    else {
        panic!("CPU 0 should find the PD entry empty");
    };
    let Walk::Missing { parent: p1, index: i1, level: 2 } = kva_map::walk::<X86, _>(&tables, root, va1, 3)
    else {
        panic!("CPU 1 should find the PD entry empty");
    };
    let (page0, page1) = (tables.page(), tables.page());
    let (table0, spare0) = kva_map::install_table::<X86, _>(&tables, p0, i0, page0);
    let (table1, spare1) = kva_map::install_table::<X86, _>(&tables, p1, i1, page1);
    tables.store(table0, kva_map::index(va0, 3), x86_leaf(0x6000_0000));
    tables.store(table1, kva_map::index(va1, 3), x86_leaf(0x6000_1000));

    assert_eq!(translate(&tables, root, va0).map(|t| t.0), Some(0x6000_0000), "CPU 0's leaf");
    assert_eq!(translate(&tables, root, va1).map(|t| t.0), Some(0x6000_1000), "CPU 1's leaf");
    let returned: Vec<u64> = [spare0, spare1].into_iter().flatten().collect();
    assert_eq!(returned, vec![page1], "the losing CPU gives its table page back");
}

/// Table pages for a mapping from the fixture, at most `limit` of them, with
/// the pages given back recorded.
struct FixturePages<'a> {
    tables: &'a mut Tables,
    limit: usize,
    taken: Vec<u64>,
    freed: Vec<u64>,
}

impl TablePages for FixturePages<'_> {
    fn allocate(&mut self) -> Option<u64> {
        if self.taken.len() == self.limit {
            return None;
        }
        let page = self.tables.page();
        self.taken.push(page);
        Some(page)
    }
    fn free(&mut self, page: u64) {
        self.freed.push(page);
    }
}

/// Map `[va, va + len)` 4 KiB at a time to `frames`, with as many table
/// pages as it needs.
fn map_window_range(tables: &mut Tables, root: u64, va: u64, len: u64, frames: &[u64]) {
    // The pages live in `tables`, which the mapping also reads, so take them
    // first and hand them over by address.
    let needed = kva_map::tables_needed::<X86, _>(tables, root, va, len, 3).unwrap();
    let pages: Vec<u64> = (0..needed).map(|_| tables.page()).collect();
    struct Given(Vec<u64>);
    impl TablePages for Given {
        fn allocate(&mut self) -> Option<u64> {
            self.0.pop()
        }
        fn free(&mut self, _page: u64) {}
    }
    kva_map::map_range::<X86, _>(tables, root, va, len, 3, |offset| frames[(offset / PAGE) as usize], &mut Given(pages))
        .unwrap();
}

/// A mapping short of the table pages it needs links no table and writes
/// no leaf, and each page it took goes back. (The plan's "unwinds" is met
/// by reserving the pages before the first write, so no leaf needs undoing.)
#[test]
fn a_mapping_short_of_table_pages_leaves_no_partial_mapping() {
    let mut tables = Tables::new();
    let slot = 300;
    let root = x86_root(&mut tables, slot);
    let pdpt = X86::address(tables.load(root, slot));
    // 3 MiB across a 2 MiB boundary: one PD and two PTs.
    let va = kva_window::slot_va(slot) + 0x4000_0000 + 0x10_0000;
    let len = 3 << 20;
    assert_eq!(kva_map::tables_needed::<X86, _>(&tables, root, va, len, 3), Ok(3));

    let before: Vec<u64> = (0..512).map(|index| tables.load(pdpt, index)).collect();
    let mut pages = FixturePages { tables: &mut tables, limit: 2, taken: Vec::new(), freed: Vec::new() };
    // The provider lives in `tables`, so the mapping reads the tables through
    // a second view of the same pages.
    let view: &Tables = unsafe { &*(pages.tables as *const Tables) };
    let result = kva_map::map_range::<X86, _>(view, root, va, len, 3, |offset| 0x7000_0000 + offset, &mut pages);
    assert_eq!(result, Err(MapError::NoTables { needed: 3, got: 2 }));
    let mut freed = pages.freed.clone();
    freed.sort();
    assert_eq!(freed, pages.taken, "each table page taken goes back");

    let after: Vec<u64> = (0..512).map(|index| tables.load(pdpt, index)).collect();
    assert_eq!(after, before, "no table was linked");
    for page in (va..va + len).step_by(PAGE as usize) {
        assert_eq!(translate(&tables, root, page), None, "{page:#x} is mapped");
    }
}

/// Free extents: allocations do not overlap, keep their guard page inside
/// their own extent and meet their alignment; each byte is either free or
/// allocated; freeing each allocation coalesces back to the one window extent; a
/// full window returns `NoVa`; and a double free is refused.
#[test]
fn free_extents_keep_guards_coalesce_and_run_out_with_no_va() {
    let base = kva_window::slot_va(300);
    let size = 1 << 30;
    let (granule, before, after) = WindowId::Vmap.shape();
    let mut extents = Extents::new(granule);
    let mut nodes = vec![0u64; 64 * 1024 / 8];
    unsafe { extents.add_nodes(nodes.as_mut_ptr() as *mut u8, nodes.len() * 8) };
    extents.free(Extent { start: base, len: size }).unwrap();

    // A fixed sequence (seed 0x5eed): allocate 1 to 64 pages at 4 KiB,
    // 64 KiB or 2 MiB alignment, or free a live allocation.
    let mut seed = 0x5eedu64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut live: BTreeMap<u64, kva_extent::Allocation> = BTreeMap::new();
    for _ in 0..4000 {
        if live.is_empty() || next() % 3 != 0 {
            let len = (next() % 64 + 1) * PAGE;
            let align = [PAGE, 64 << 10, 2 << 20][(next() % 3) as usize];
            let allocation = extents.allocate(len, align, before, after).unwrap();
            assert_eq!(allocation.start % align, 0);
            assert_eq!(allocation.extent.start + before, allocation.start);
            assert_eq!(allocation.extent.end(), allocation.start + len + after, "guard page after");
            assert!(allocation.extent.start >= base && allocation.extent.end() <= base + size);
            live.insert(allocation.extent.start, allocation);
        } else {
            let key = *live.keys().nth((next() % live.len() as u64) as usize).unwrap();
            let allocation = live.remove(&key).unwrap();
            extents.free(allocation.extent).unwrap();
        }
        let mut end = base;
        for allocation in live.values() {
            assert!(allocation.extent.start >= end, "allocations overlap");
            end = allocation.extent.end();
        }
        let held: u64 = live.values().map(|allocation| allocation.extent.len).sum();
        assert_eq!(extents.free_bytes() + held, size);
    }

    let some = *live.values().next().unwrap();
    for allocation in std::mem::take(&mut live).into_values() {
        extents.free(allocation.extent).unwrap();
    }
    let mut free = Vec::new();
    extents.for_each_free(|extent| free.push(extent));
    assert_eq!(free, vec![Extent { start: base, len: size }], "freed extents coalesce");
    assert_eq!(extents.free(some.extent), Err(ExtentError::Overlap), "double free");

    let whole = extents.allocate(size - after, PAGE, before, after).unwrap();
    assert_eq!(extents.allocate(PAGE, PAGE, before, after), Err(ExtentError::NoVa));
    extents.free(whole.extent).unwrap();
    assert!(extents.allocate(PAGE, PAGE, before, after).is_ok());
}

/// A window whose placement is slot `slot` of `root`, open and with nodes.
fn open_window(id: WindowId, slot: usize, nodes: &mut Vec<u64>) -> Window {
    let placement = Placement { id, first_slot: slot, slots: 1, base: kva_window::slot_va(slot), bytes: 1 << 30 };
    let mut window = Window::new(placement, id.shape().0);
    unsafe { window.extents.add_nodes(nodes.as_mut_ptr() as *mut u8, nodes.len() * 8) };
    window.open().unwrap();
    window
}

/// The pending list (6.5): an unmap's frames and VA come back only from a
/// flushed batch. Each of 4 CPUs caches the window's global leaves; the
/// unmap invalidates them, and they are not offered back, nor is their VA
/// handed out, until the batch's flush has run on each CPU, after which no
/// CPU holds a translation to any frame offered back. An unmap after the
/// batch was taken waits for the next one. Run with 3 pages, which the
/// flush names one by one, and with 40, which is past the 32-page list and
/// clears and restores CR4.PGE.
#[test]
fn pending_unmaps_give_frames_back_only_after_a_flush_on_each_cpu() {
    for pages in [3u64, 40] {
        let mut tables = Tables::new();
        let slot = 310;
        let root = x86_root(&mut tables, slot);
        let mut nodes = vec![0u64; 512];
        let mut window = open_window(WindowId::Vmap, slot, &mut nodes);

        let allocation = window.reserve(pages * PAGE, PAGE).unwrap();
        let frames: Vec<u64> = (0..pages).map(|page| 0x8000_0000 + page * 0x1_0000).collect();
        map_window_range(&mut tables, root, allocation.start, allocation.len, &frames);
        let mut cpus: Vec<FakeTlb> = (0..4).map(|_| FakeTlb::default()).collect();
        for cpu in cpus.iter_mut() {
            for page in 0..pages {
                let va = allocation.start + page * PAGE;
                assert_eq!(cpu.access(&tables, root, va), Some(frames[page as usize]));
            }
        }

        assert_eq!(window.unmap::<X86, _>(&tables, root, &allocation), Ok(pages as usize));
        for page in 0..pages {
            assert_eq!(translate(&tables, root, allocation.start + page * PAGE), None);
        }
        // The VA is held: a new allocation does not overlap it.
        let other = window.reserve(PAGE, PAGE).unwrap();
        assert!(other.extent.start >= allocation.extent.end() || other.extent.end() <= allocation.extent.start);

        let batch = window.take_pending();
        let late = window.reserve(PAGE, PAGE).unwrap();
        map_window_range(&mut tables, root, late.start, late.len, &[0x9000_0000]);
        window.unmap::<X86, _>(&tables, root, &late).unwrap();

        let mut plans = 0;
        let flushed = batch.flush::<X86, _>(&tables, root, FlushPolicy::Batched, |plan| {
            plans += 1;
            let request = match plan {
                FlushPlan::Pages(list) => {
                    assert!(pages <= 32);
                    tlb_request::kernel_unmap_request(list)
                }
                FlushPlan::All { leaves } => {
                    assert_eq!(leaves as u64, pages);
                    tlb_request::Request::GlobalAll
                }
            };
            let (kind, arg) = tlb_request::encode(request);
            for cpu in cpus.iter_mut() {
                // SAFETY: the list outlives the call.
                unsafe { tlb_request::execute(kind, arg, cpu) };
            }
        });
        assert_eq!(plans, 1, "one shootdown per batch");

        let mut offered = Vec::new();
        flushed.reclaim::<X86, _>(&tables, root, |pa, bytes| {
            assert_eq!(bytes, PAGE);
            for (index, cpu) in cpus.iter().enumerate() {
                assert!(
                    !cpu.entries.values().any(|&(cached, _)| cached == pa),
                    "CPU {index} still translates to frame {pa:#x} when it is offered back",
                );
            }
            offered.push(pa);
        });
        offered.sort();
        assert_eq!(offered, frames);
        window.release(flushed).unwrap();
        assert_eq!(window.extents.retired_bytes(), late.extent.len, "the late unmap waits");
        let again = window.reserve(allocation.len, PAGE).unwrap();
        assert_eq!(again.start, allocation.start, "the flushed VA is free again");
    }
}

/// Address space (E1-2x): the vmap window for 512 GiB of RAM is 4 × 512 GiB,
/// so it takes four PML4 slots of its own, each with its own PDPT from the
/// migration. Pages mapped at the same offset in each slot translate to
/// their own frames, through the master root and through a process root
/// that copied the master's kernel half before the pages were mapped.
#[test]
fn a_vmap_window_of_four_slots_has_independent_trees_shared_by_each_root() {
    use x86_64::structures::paging::{PageTable, PhysFrame};
    use x86_64::{PhysAddr, VirtAddr};

    let mut tables = Tables::new();
    let master = tables.page();
    tables.zero(master);
    let process = tables.page();
    tables.zero(process);
    let pdpts: Vec<u64> = (0..256).map(|_| tables.page()).collect();
    let offset = VirtAddr::new(tables.offset());
    let table_at = |pa: u64| (tables.offset() + pa) as *mut PageTable;
    let mut next = pdpts.iter();
    unsafe {
        x86_kernel_half::migrate_kernel_half(&mut *table_at(master), offset, || {
            PhysFrame::containing_address(PhysAddr::new(*next.next().unwrap()))
        });
        x86_kernel_half::inherit_master_slots(&*table_at(master), &mut *table_at(process));
    }

    let ram = 512u64 << 30;
    let free = |slot: usize| {
        let pdpt = X86::address(tables.load(master, slot));
        X86::is_valid(tables.load(master, slot)) && (0..512).all(|index| tables.load(pdpt, index) == 0)
    };
    let placed = kva_window::place(ram, 257, 402, free).unwrap();
    let vmap = placed[WindowId::Vmap as usize];
    assert_eq!((vmap.bytes, vmap.slots), (4 * ram, 4));
    assert_eq!(vmap.first_slot, placed[WindowId::Span as usize].first_slot + 1);

    let within = 0x40_2030_0000u64;
    let mut roots_seen = Vec::new();
    for (k, slot) in (vmap.first_slot..vmap.first_slot + 4).enumerate() {
        let va = kva_window::slot_va(slot) + within;
        let frame = 0x1_0000_0000 * (k as u64 + 1);
        map_window_range(&mut tables, master, va, PAGE, &[frame]);
        roots_seen.push(X86::address(tables.load(master, slot)));
        assert_eq!(translate(&tables, master, va).map(|t| t.0), Some(frame));
        assert_eq!(translate(&tables, process, va).map(|t| t.0), Some(frame), "process root");
    }
    roots_seen.sort();
    roots_seen.dedup();
    assert_eq!(roots_seen.len(), 4, "each slot has its own PDPT");
}

/// 2 MiB leaves, in both encodings: a mapping in an empty GiB needs one
/// directory and no page table, writes a block (not a table) that maps the
/// block's base, and is invalidated and reclaimed whole.
fn two_mib_leaves_map_and_reclaim_whole<F: Format>() {
    let mut tables = Tables::new();
    let slot = 320;
    let root = tables.page();
    tables.zero(root);
    let top = tables.page();
    tables.zero(top);
    tables.store(root, slot, F::table(top));
    let va = kva_window::slot_va(slot) + 0x8040_0000;
    let pa = 0x1_2340_0000u64 & !((2 << 20) - 1);
    assert_eq!(kva_map::tables_needed::<F, _>(&tables, root, va, 2 << 20, 2), Ok(1));

    let page = tables.page();
    struct One(Option<u64>);
    impl TablePages for One {
        fn allocate(&mut self) -> Option<u64> {
            self.0.take()
        }
        fn free(&mut self, _page: u64) {
            panic!("the one page was needed");
        }
    }
    assert_eq!(
        kva_map::map_range::<F, _>(&tables, root, va, 2 << 20, 2, |_| pa, &mut One(Some(page))),
        Ok(1)
    );
    let Walk::Table(directory) = kva_map::walk::<F, _>(&tables, root, va, 2) else {
        panic!("no directory");
    };
    let block = tables.load(directory, kva_map::index(va, 2));
    assert!(F::is_valid(block) && !F::is_table(block, 2), "a block, not a table: {block:#x}");
    assert_eq!(F::address(block), pa);
    assert_eq!(
        kva_map::tables_needed::<F, _>(&tables, root, va, 2 << 20, 3),
        Err(MapError::Blocked { va, level: 2 }),
        "4 KiB leaves cannot go under a block"
    );

    let mut invalidated = Vec::new();
    assert_eq!(kva_map::invalidate_range::<F, _>(&tables, root, va, 2 << 20, |leaf| invalidated.push(leaf)), 1);
    assert_eq!(invalidated, vec![va]);
    assert!(!F::is_valid(tables.load(directory, kva_map::index(va, 2))));
    let mut reclaimed = Vec::new();
    kva_map::reclaim_range::<F, _>(&tables, root, va, 2 << 20, |frame, bytes| reclaimed.push((frame, bytes)));
    assert_eq!(reclaimed, vec![(pa, 2 << 20)]);
    assert_eq!(tables.load(directory, kva_map::index(va, 2)), 0);
}

#[test]
fn two_mib_leaves_map_and_reclaim_whole_on_x86() {
    two_mib_leaves_map_and_reclaim_whole::<X86>();
}

#[test]
fn two_mib_leaves_map_and_reclaim_whole_on_arm64() {
    two_mib_leaves_map_and_reclaim_whole::<kva_map::Arm64>();
}
