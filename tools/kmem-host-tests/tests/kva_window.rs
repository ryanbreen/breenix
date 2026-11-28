//! Kernel VA windows (E1 plan 6.5, PR E1-5): table installs, unmaps and the
//! flush before free, driven through the kernel's own `kva_map.rs` and
//! `tlb_request.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "../../../kernel/src/memory/kva_map.rs"]
mod kva_map;
// The kernel's shootdown uses the request kinds this file does not.
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/tlb_request.rs"]
mod tlb_request;

use kva_map::{Format, TableMemory, Walk, X86};

const PAGE: u64 = 4096;

/// Table pages addressed by physical address. Page 0 is never handed out,
/// so 0 is free to mean "no table".
struct Tables {
    pages: Vec<Box<[AtomicU64; 512]>>,
}

impl Tables {
    fn new() -> Self {
        let mut tables = Tables { pages: Vec::new() };
        tables.page();
        tables
    }

    /// A new page, filled with a pattern so a missing zeroing shows.
    fn page(&mut self) -> u64 {
        let pa = self.pages.len() as u64 * PAGE;
        self.pages.push(Box::new([const { AtomicU64::new(0xdead_beef) }; 512]));
        pa
    }
}

impl TableMemory for Tables {
    fn load(&self, table: u64, index: usize) -> u64 {
        self.pages[(table / PAGE) as usize][index].load(Ordering::Acquire)
    }
    fn store(&self, table: u64, index: usize, value: u64) {
        self.pages[(table / PAGE) as usize][index].store(value, Ordering::Release);
    }
    fn compare_exchange(&self, table: u64, index: usize, current: u64, new: u64) -> Result<u64, u64> {
        self.pages[(table / PAGE) as usize][index].compare_exchange(
            current,
            new,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
    }
    fn zero(&self, page: u64) {
        for entry in self.pages[(page / PAGE) as usize].iter() {
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
/// drops every entry.
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
/// leaves the global entry in each CPU's TLB, so all 4 could still reach
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
