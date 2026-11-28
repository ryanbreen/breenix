//! The kernel VA windows (E1 plan, 6.5, PR E1-5): the span, vmap and stack
//! windows in the kernel half, mapped through the kernel's own tables.
//!
//! The algorithms are in `kva_map.rs`, `kva_extent.rs` and `kva_window.rs`,
//! which the host tests include. This file supplies the tables (through the
//! direct map), the frames, the locks and the TLB flush.
//!
//! - x86-64: each window takes PML4 slots of its own between 257 and 401,
//!   whose PDPTs E1-2x preallocated and each root shares, so a mapping
//!   below them is visible in each process at once. Window leaves are
//!   global. An unmap's frames and VA wait on a pending list until one
//!   `tlb::KernelUnmapFlush` (`tlb_request::KernelUnmap`) covers the batch: past
//!   16 MiB pending, when the window has no VA left, or from the shrinker.
//! - ARM64: each window takes TTBR1 L0 entries above the direct map and
//!   below the ECAM reservation, whose L1 tables `init` installs. An unmap
//!   is flushed at once with `tlb::KernelUnmapFlush` (`dsb ishst`,
//!   `tlbi vaae1is` per page, `dsb ish; isb`), which completes the broadcast
//!   on each CPU, and its frames and VA come back then.
//!
//! The window takes ownership of the frames a caller maps, and returns each
//! one to the frame allocator after the flush that follows its unmap.
//! Only `self_check` uses the windows in E1-5.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use spin::Mutex;

use super::kmem::{self, AllocError, Ctx};
use super::kva_extent::{Allocation, ExtentError};
use super::kva_map::{self, MapError, TableMemory, TablePages};
use super::kva_window::{self, FlushPolicy, KernelTlb, Placement, PlaceError, Window};
use super::tlb::KernelUnmapFlush;
pub use super::kva_window::WindowId;

#[cfg(target_arch = "x86_64")]
type Fmt = kva_map::X86;
#[cfg(target_arch = "aarch64")]
type Fmt = kva_map::Arm64;

const POLICY: FlushPolicy = <KernelUnmapFlush as KernelTlb>::POLICY;

/// The kernel's page tables, reached through the direct map.
pub(crate) struct KernelTables;

impl KernelTables {
    fn entry(&self, table: u64, index: usize) -> &AtomicU64 {
        let offset = super::physical_memory_offset().as_u64();
        // SAFETY: page-table frames are RAM the direct map covers, and
        // `index` is below 512, so this is one aligned entry.
        unsafe { &*((offset + table + 8 * index as u64) as *const AtomicU64) }
    }
}

impl TableMemory for KernelTables {
    fn load(&self, table: u64, index: usize) -> u64 {
        self.entry(table, index).load(Ordering::Acquire)
    }
    fn store(&self, table: u64, index: usize, value: u64) {
        self.entry(table, index).store(value, Ordering::Release);
    }
    fn compare_exchange(&self, table: u64, index: usize, current: u64, new: u64) -> Result<u64, u64> {
        self.entry(table, index)
            .compare_exchange(current, new, Ordering::AcqRel, Ordering::Acquire)
    }
    fn zero(&self, page: u64) {
        for index in 0..512 {
            self.entry(page, index).store(0, Ordering::Relaxed);
        }
    }
    fn publish_table(&self) {
        // x86: stores are seen in program order (SDM vol. 3A §9.2.2), and the
        // table walker reads the coherent tables. ARM64: a DSB makes table
        // stores visible to each PE's walker (Arm ARM, translation table
        // updates).
        #[cfg(target_arch = "x86_64")]
        core::sync::atomic::fence(Ordering::Release);
        #[cfg(target_arch = "aarch64")]
        // SAFETY: a barrier only.
        unsafe {
            core::arch::asm!("dsb ishst", options(nostack, preserves_flags))
        };
    }
    fn publish_leaves(&self) {
        self.publish_table();
        #[cfg(target_arch = "aarch64")]
        // SAFETY: a barrier only.
        unsafe {
            core::arch::asm!("isb", options(nostack, preserves_flags))
        };
    }
}

/// Page-table frames for window mappings.
struct TableFrames;

impl TablePages for TableFrames {
    fn allocate(&mut self) -> Option<u64> {
        let frame = super::frame_allocator::allocate_frame_for(
            super::frame_allocator::FrameOwner::PageTables,
        )?;
        TABLE_PAGES.fetch_add(1, Ordering::Relaxed);
        Some(frame.start_address().as_u64())
    }
    fn free(&mut self, page: u64) {
        TABLE_PAGES.fetch_sub(1, Ordering::Relaxed);
        free_frame(page);
    }
}

fn free_frame(pa: u64) {
    super::frame_allocator::deallocate_frame(super::PhysFrame::containing_address(
        super::PhysAddr::new(pa),
    ));
}

/// Table pages the windows hold, and node pages their extent trees hold.
static TABLE_PAGES: AtomicUsize = AtomicUsize::new(0);
static NODE_PAGES: AtomicUsize = AtomicUsize::new(0);

struct Windows {
    root: u64,
    windows: [Mutex<Window>; 3],
    /// Each window's place, fixed at `init`, read with no lock.
    placements: [Placement; 3],
}

static WINDOWS: spin::Once<Windows> = spin::Once::new();

/// Why the windows could not be set up.
#[derive(Clone, Copy, Debug)]
pub enum InitError {
    /// The root page table is not known yet.
    NoRoot,
    Place(PlaceError),
    /// No frame for a window's top table or first extent node.
    NoFrames,
}

/// The page-table root whose upper half the windows live in.
fn root() -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    return super::kernel_page_table::master_kernel_pml4().map(|frame| frame.start_address().as_u64());
    #[cfg(target_arch = "aarch64")]
    {
        let root: u64;
        // SAFETY: reads a system register.
        unsafe { core::arch::asm!("mrs {}, ttbr1_el1", out(reg) root, options(nomem, nostack)) };
        Some(root & 0x0000_ffff_ffff_f000)
    }
}

/// The root entries the windows may take, `first..end`, and whether one is free.
fn candidate_slots(root: u64) -> (usize, usize, impl Fn(usize) -> bool) {
    let mem = KernelTables;
    #[cfg(target_arch = "x86_64")]
    {
        // Slot 256 is the layout's HHDM_BASE; 402 and 403 hold the kernel and
        // IST stacks, and the MMIO, fixmap, per-CPU and kernel-image slots lie
        // above them. Each slot already links its own PDPT (E1-2x); a free one
        // has no entry yet.
        let free = move |slot: usize| {
            let pdpt = mem.load(root, slot);
            <Fmt as kva_map::Format>::is_valid(pdpt)
                && (0..512).all(|index| mem.load(<Fmt as kva_map::Format>::address(pdpt), index) == 0)
        };
        (257, 402, free)
    }
    #[cfg(target_arch = "aarch64")]
    {
        // Above the direct map's RAM (and the first 4 GiB, which the boot
        // tables map), below the ECAM reservation just under MMIO_BASE.
        let ram_end = crate::platform_config::frame_pool()
            .ranges
            .iter()
            .map(|range| range.end)
            .max()
            .unwrap_or(0)
            .max(4 * kva_window::GIB);
        let first = ram_end.div_ceil(kva_window::SLOT_BYTES) as usize;
        let ecam = crate::arch_impl::aarch64::constants::MMIO_BASE - 0x1000_0000;
        let end = kva_map::index(ecam, 0);
        (first, end, move |slot: usize| mem.load(root, slot) == 0)
    }
}

/// Place and open the three windows, sized from the RAM the frame allocator
/// holds. Runs once, on the boot CPU, after the frame allocator and (x86)
/// the master kernel PML4.
pub fn init() -> Result<[Placement; 3], InitError> {
    let root = root().ok_or(InitError::NoRoot)?;
    let ram = super::frame_allocator::memory_stats().total_bytes;
    let (first, end, free) = candidate_slots(root);
    let placements = kva_window::place(ram, first, end, free).map_err(InitError::Place)?;

    #[cfg(target_arch = "aarch64")]
    for placement in placements.iter() {
        for slot in placement.first_slot..placement.first_slot + placement.slots {
            let page = TableFrames.allocate().ok_or(InitError::NoFrames)?;
            let (_, spare) = kva_map::install_table::<Fmt, _>(&KernelTables, root, slot, page);
            if let Some(page) = spare {
                TableFrames.free(page);
            }
        }
    }

    let windows = placements.map(|placement| {
        Mutex::new(Window::new(placement, placement.id.shape().0))
    });
    for window in windows.iter() {
        let mut window = window.lock();
        add_node_page(&mut window).map_err(|_| InitError::NoFrames)?;
        window.open().expect("a new window has a spare node and no extent");
    }
    WINDOWS.call_once(|| Windows { root, windows, placements });
    #[cfg(target_arch = "x86_64")]
    kmem::register_shrinker(&PENDING_SHRINKER);
    Ok(placements)
}

fn windows() -> Option<&'static Windows> {
    WINDOWS.get()
}

fn add_node_page(window: &mut Window) -> Result<(), AllocError> {
    let frame = super::frame_allocator::allocate_frame_for(super::frame_allocator::FrameOwner::Other)
        .ok_or(AllocError::NoFrames)?;
    let memory = (super::physical_memory_offset().as_u64() + frame.start_address().as_u64()) as *mut u8;
    // SAFETY: a new frame, reached through the direct map, owned by this
    // window for the rest of the boot.
    unsafe { window.extents.add_nodes(memory, kva_window::PAGE as usize) };
    NODE_PAGES.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Run `f` on a window with this CPU's interrupts masked.
fn with_window<R>(id: WindowId, f: impl FnOnce(&mut Window) -> R) -> Option<R> {
    let windows = windows()?;
    Some(crate::arch_without_interrupts(|| f(&mut windows.windows[id as usize].lock())))
}

/// `ctx` as the allocator lowers it: `Atomic` in an interrupt handler and
/// inside reclaim.
pub(super) fn lowered(ctx: Ctx) -> Ctx {
    use kmem::Cpu;
    let cpu = kmem::KernelCpu;
    ctx.lowered(cpu.in_interrupt(), kmem::RECLAIM.held_by(cpu.id()))
}

/// VA for `len` bytes aligned to `align` in window `id`. A window with no
/// room flushes its pending unmaps and tries once more. The allocation holds
/// the extent node its unmap will need, so `release` does not allocate; a
/// window short of nodes gets a node page here, where failing is an error.
pub fn reserve(id: WindowId, len: u64, align: u64, ctx: Ctx) -> Result<Allocation, AllocError> {
    let ctx = lowered(ctx);
    if !ctx.may_map() {
        return Err(kmem::STATS.refuse(ctx));
    }
    let mut flushed = false;
    loop {
        let result = with_window(id, |window| window.reserve(len, align))
            .ok_or(AllocError::NoVa { window: id as u8 })?;
        match result {
            Ok(allocation) => return Ok(allocation),
            Err(ExtentError::NoNode) => {
                let frame_added = with_window(id, add_node_page);
                frame_added.unwrap_or(Err(AllocError::NoFrames))?;
            }
            Err(ExtentError::NoVa) if !flushed => {
                flushed = true;
                flush(id);
            }
            Err(_) => return Err(AllocError::NoVa { window: id as u8 }),
        }
    }
}

/// Map the first `len` bytes of `allocation` (a multiple of 4 KiB, at most
/// its length), the 4 KiB page at offset `o` to frame `frame_at(o)`, which the
/// window then owns.
pub fn map(
    allocation: &Allocation,
    len: u64,
    frame_at: impl FnMut(u64) -> u64,
    ctx: Ctx,
) -> Result<(), AllocError> {
    map_at(allocation, 0, len, frame_at, ctx)
}

/// Map `len` bytes of `allocation` from `offset` (each a multiple of 4 KiB,
/// inside its length), the page at `offset + o` to frame `frame_at(o)`, which
/// the window then owns. A shortfall of table pages leaves the range unmapped.
pub fn map_at(
    allocation: &Allocation,
    offset: u64,
    len: u64,
    frame_at: impl FnMut(u64) -> u64,
    ctx: Ctx,
) -> Result<(), AllocError> {
    let ctx = lowered(ctx);
    if !ctx.may_map() {
        return Err(kmem::STATS.refuse(ctx));
    }
    assert!(
        offset.checked_add(len).is_some_and(|end| end <= allocation.len),
        "kernel window mapping past its allocation"
    );
    let windows = windows().ok_or(AllocError::NoFrames)?;
    let mut pages = TableFrames;
    let va = allocation.start + offset;
    match kva_map::map_range::<Fmt, _>(&KernelTables, windows.root, va, len, 3, frame_at, &mut pages) {
        Ok(_) => Ok(()),
        Err(MapError::NoTables { .. }) => Err(AllocError::NoFrames),
        Err(error) => panic!("kernel window mapping at {:#x}: {:?}", va, error),
    }
}

/// Grow `allocation` by `extra` bytes into the free VA after it in window
/// `id`, if there is that much; its guard page moves past them.
pub fn extend(id: WindowId, allocation: &Allocation, extra: u64) -> Option<Allocation> {
    with_window(id, |window| window.extend(allocation, extra).ok()).flatten()
}

/// Map `to` whole: its first `moved` bytes to the frames `from` maps at the
/// same offsets, and the rest to `frame_at(o)`. The window then owns the new
/// frames. A shortfall of table pages leaves `to` unmapped.
pub fn remap(
    from: &Allocation,
    to: &Allocation,
    moved: u64,
    frame_at: impl FnMut(u64) -> u64,
    ctx: Ctx,
) -> Result<(), AllocError> {
    let ctx = lowered(ctx);
    if !ctx.may_map() {
        return Err(kmem::STATS.refuse(ctx));
    }
    let windows = windows().ok_or(AllocError::NoFrames)?;
    let mut pages = TableFrames;
    match kva_window::remap::<Fmt, _>(&KernelTables, windows.root, from, to, moved, frame_at, &mut pages) {
        Ok(_) => Ok(()),
        Err(MapError::NoTables { .. }) => Err(AllocError::NoFrames),
        Err(error) => panic!("kernel window remap to {:#x}: {:?}", to.start, error),
    }
}

/// Unmap `allocation` after `remap` moved its pages: its leaves wait for the
/// flush like `release`'s, and its frames stay with the range they moved to.
pub fn forward(id: WindowId, allocation: Allocation, ctx: Ctx) {
    let Some(windows) = windows() else { return };
    let pending = with_window(id, |window| {
        window
            .forward::<Fmt, _>(&KernelTables, windows.root, &allocation)
            .map(|_| window.extents.retired_bytes())
    })
    .expect("windows are set up")
    .unwrap_or_else(|error| panic!("kernel window {} forward: {:?}", id.name(), error));
    flush_if_due(id, pending, ctx);
}

/// The frame the window page at `va` maps, if it is mapped.
pub fn frame_of(va: u64) -> Option<u64> {
    let windows = windows()?;
    kva_map::leaf_frame::<Fmt, _>(&KernelTables, windows.root, va)
}

/// The window `va` lies in, if any.
pub fn window_of(va: u64) -> Option<WindowId> {
    let windows = windows()?;
    windows
        .placements
        .iter()
        .find(|placement| va >= placement.base && va - placement.base < placement.bytes)
        .map(|placement| placement.id)
}

/// Where window `id` sits, once the windows are set up.
pub fn placement(id: WindowId) -> Option<Placement> {
    Some(windows()?.placements[id as usize])
}

/// Unmap `allocation` and give its VA and frames back after the flush that
/// covers them: at once on ARM64, and on x86 when the window's pending list
/// passes 16 MiB and `ctx` may wait for the shootdown.
pub fn release(id: WindowId, allocation: Allocation, ctx: Ctx) {
    let Some(windows) = windows() else { return };
    // The allocation's own node retires it, so this takes no memory; an
    // error is a release of an allocation this window did not hand out.
    let pending = with_window(id, |window| {
        window
            .unmap::<Fmt, _>(&KernelTables, windows.root, &allocation)
            .map(|_| window.extents.retired_bytes())
    })
    .expect("windows are set up")
    .unwrap_or_else(|error| panic!("kernel window {} release: {:?}", id.name(), error));
    flush_if_due(id, pending, ctx);
}

/// Flush window `id` now on ARM64, and on x86 once its pending list passes
/// 16 MiB and `ctx` may wait for the shootdown.
fn flush_if_due(id: WindowId, pending: u64, ctx: Ctx) {
    let flush_now = POLICY == FlushPolicy::Chunked
        || (pending > kva_window::PENDING_FLUSH_BYTES && lowered(ctx).may_map());
    if flush_now {
        flush(id);
    }
}

/// `release` that takes the window's lock with one `try_lock`, for 7.2's
/// step 2, which may run while this CPU holds that lock (a frame request
/// made under it). A busy lock hands `allocation` back untouched. Returns
/// the frames given back now: the flushed frames on ARM64, and 0 on x86,
/// whose pending list the `kva-pending` shrinker flushes.
pub fn try_release(id: WindowId, allocation: Allocation) -> Result<usize, Allocation> {
    let Some(windows) = windows() else { return Err(allocation) };
    let mutex = &windows.windows[id as usize];
    crate::arch_without_interrupts(|| {
        let Some(mut window) = mutex.try_lock() else {
            return Err(allocation);
        };
        window
            .unmap::<Fmt, _>(&KernelTables, windows.root, &allocation)
            .unwrap_or_else(|error| panic!("kernel window {} release: {:?}", id.name(), error));
        Ok(())
    })?;
    Ok(if POLICY == FlushPolicy::Chunked { flush_window(id, true) } else { 0 })
}

/// Flush window `id`'s pending unmaps on each CPU and give back their frames
/// and VA. Returns the frames given back.
pub fn flush(id: WindowId) -> usize {
    flush_window(id, false)
}

/// Take the window's pending batch, flush it, give its frames back and
/// return its VA, in one hold of the window's lock with interrupts masked.
/// `try_only` takes the lock with one `try_lock` and returns 0 when it is
/// busy. An x86 shootdown waited for with interrupts masked still completes:
/// its requests are NMIs.
fn flush_window(id: WindowId, try_only: bool) -> usize {
    let Some(windows) = windows() else { return 0 };
    let mutex = &windows.windows[id as usize];
    crate::arch_without_interrupts(|| {
        let mut window = if try_only {
            match mutex.try_lock() {
                Some(window) => window,
                None => return 0,
            }
        } else {
            mutex.lock()
        };
        let batch = window.take_pending();
        if batch.is_empty() {
            return 0;
        }
        let flushed = batch.flush::<Fmt, _, _>(&KernelTables, windows.root, &mut super::tlb::kernel_unmap_flush());
        let mut frames = 0;
        flushed.reclaim::<Fmt, _>(&KernelTables, windows.root, |pa, bytes| {
            for page in (pa..pa + bytes).step_by(kva_window::PAGE as usize) {
                free_frame(page);
                frames += 1;
            }
        });
        window
            .release(flushed)
            .unwrap_or_else(|error| panic!("kernel window {} flush: {:?}", id.name(), error));
        frames
    })
}

/// The x86 pending lists as a shrinker: what they hold, and a flush that
/// skips a window whose lock is busy.
#[cfg(target_arch = "x86_64")]
static PENDING_SHRINKER: kmem::Shrinker = kmem::Shrinker::new("kva-pending", 20, pending_frames, flush_pending);

#[cfg(target_arch = "x86_64")]
fn pending_frames() -> usize {
    let Some(windows) = windows() else { return 0 };
    crate::arch_without_interrupts(|| {
        windows
            .windows
            .iter()
            .filter_map(|window| window.try_lock().map(|window| window.extents.retired_bytes()))
            .map(|bytes| (bytes / kva_window::PAGE) as usize)
            .sum()
    })
}

/// Flush the pending lists until `target` frames are back, skipping a window
/// whose lock is busy. A window's batch is flushed whole, so this visits at
/// most its target plus one window's pending list.
#[cfg(target_arch = "x86_64")]
fn flush_pending(target: usize, _ctx: kmem::ReclaimCtx) -> usize {
    let mut frames = 0;
    for id in WindowId::ALL {
        if frames >= target {
            break;
        }
        frames += flush_window(id, true);
    }
    frames
}

/// What `self_check` did.
pub struct SelfCheck {
    pub mappings: usize,
    pub table_pages: usize,
}

/// Map, write, unmap and remap one page in each window, checking the
/// translation the CPU uses (`PhysAddrWrapper`: `AT S1E1R` on ARM64, which
/// consults the TLB, and a table walk on x86) before and after each flush.
pub fn self_check() -> Result<SelfCheck, &'static str> {
    let mut mappings = 0;
    for id in WindowId::ALL {
        for round in 0..2u64 {
            let allocation = reserve(id, kva_window::PAGE, kva_window::PAGE, Ctx::Normal)
                .map_err(|_| "no VA in a window")?;
            let frame = super::frame_allocator::allocate_frame_for(super::frame_allocator::FrameOwner::Other)
                .ok_or("no frame for the self-check")?
                .start_address()
                .as_u64();
            if map(&allocation, kva_window::PAGE, |_| frame, Ctx::Normal).is_err() {
                free_frame(frame);
                release(id, allocation, Ctx::Normal);
                return Err("no table pages for a window mapping");
            }
            let va = allocation.start;
            if super::PhysAddrWrapper::from_kernel_virt(va as usize) != Ok(frame) {
                return Err("a window page does not translate to its frame");
            }
            let pattern = 0x6b76_6100_0000_0000 | (id as u64) << 8 | round;
            let direct = (super::physical_memory_offset().as_u64() + frame) as *const u64;
            // SAFETY: `va` maps `frame` writable, and the direct map covers it.
            let seen = unsafe {
                core::ptr::write_volatile(va as *mut u64, pattern);
                core::ptr::write_volatile((va + 4088) as *mut u64, !pattern);
                (core::ptr::read_volatile(direct), core::ptr::read_volatile(direct.add(511)))
            };
            if seen != (pattern, !pattern) {
                return Err("a window write did not reach its frame");
            }
            release(id, allocation, Ctx::Normal);
            flush(id);
            if super::PhysAddrWrapper::from_kernel_virt(va as usize).is_ok() {
                return Err("an unmapped window page still translates");
            }
            mappings += 1;
        }
    }
    Ok(SelfCheck { mappings, table_pages: TABLE_PAGES.load(Ordering::Relaxed) })
}

/// The windows' counts as `name value` lines for `/proc/breenix/kmem`.
pub fn write_stats(out: &mut impl core::fmt::Write) -> core::fmt::Result {
    let Some(windows) = windows() else { return Ok(()) };
    for window in windows.windows.iter() {
        let Some(window) = window.try_lock() else { continue };
        let name = window.placement.id.name();
        writeln!(out, "kva_{name}_base {:#x}", window.placement.base)?;
        writeln!(out, "kva_{name}_bytes {}", window.placement.bytes)?;
        writeln!(out, "kva_{name}_free_bytes {}", window.extents.free_bytes())?;
        writeln!(out, "kva_{name}_pending_bytes {}", window.extents.retired_bytes())?;
    }
    writeln!(out, "kva_table_pages {}", TABLE_PAGES.load(Ordering::Relaxed))?;
    writeln!(out, "kva_node_pages {}", NODE_PAGES.load(Ordering::Relaxed))
}
