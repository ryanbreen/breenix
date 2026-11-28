//! Allocation contexts, allocation errors, the reclaim registry and the slow
//! path (E1 plan, 6.6 and 7.1 to 7.3), with no kernel dependencies. The
//! kernel supplies the pools, the CPU and the interrupt masking
//! (`memory/kmem.rs`); `tools/kmem-host-tests` supplies simulated ones.
//!
//! Requests up to a page go to the slab caches and their per-CPU magazines
//! (E1-6), with the remote drains of 7.2 step 2. Until E1-9 larger requests
//! go to the fixed kernel heap, and both layers share its emergency reserve.
//! The layers E1-7 and E1-8 add (per-CPU page caches, span and vmap growth,
//! the pending-unmap flush) bring their own steps of the context table, and
//! each checks the context the same way.
use core::alloc::Layout;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

const PAGE: usize = 4096;

/// Frames a reclaim asks for beyond the shortfall: the floor of the per-CPU
/// batch (5.4), since no per-CPU cache exists before E1-8.
pub const RECLAIM_BATCH_FRAMES: usize = 8;

/// What an allocation may do to meet a request (the table in 6.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctx {
    /// May wait for a refill lock and run the shrinkers.
    Normal,
    /// May wait for a refill lock; runs no shrinker. A caller holding a
    /// shrinker's lock asks for this.
    NoReclaim,
    /// Takes each refill lock with one `try_lock`, runs no reclaim, and may
    /// use the emergency reserve. Interrupt handlers and shrinkers get this.
    Atomic,
}

impl Ctx {
    /// The context a request runs in: lowered to `Atomic` in an interrupt
    /// handler or inside reclaim. A lowered context is not raised again.
    pub fn lowered(self, in_interrupt: bool, in_reclaim: bool) -> Ctx {
        if in_interrupt || in_reclaim {
            Ctx::Atomic
        } else {
            self
        }
    }

    /// Whether a busy refill lock may be waited for.
    pub fn may_wait(self) -> bool {
        self != Ctx::Atomic
    }

    /// Whether the shrinkers may run (7.2 steps 3 and 4).
    pub fn may_shrink(self) -> bool {
        self == Ctx::Normal
    }

    /// Whether a kernel VA window may map pages, taking table pages, or flush
    /// its pending unmaps, which on x86 waits for each CPU (6.6's table).
    pub fn may_map(self) -> bool {
        self != Ctx::Atomic
    }

    /// Whether the emergency reserve may be used (5.3).
    pub fn may_use_reserve(self) -> bool {
        self == Ctx::Atomic
    }

    fn index(self) -> usize {
        match self {
            Ctx::Normal => 0,
            Ctx::NoReclaim => 1,
            Ctx::Atomic => 2,
        }
    }
}

/// Why an allocation failed (7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocError {
    /// The fixed heap had no extent for the request after the slow path.
    /// E1-9 retires the fixed heap, and this cause becomes `NoFrames` or
    /// `NoVa` of the layer that failed.
    HeapExhausted,
    /// The context may not take the step the request needed: an `Atomic`
    /// request found the heap lock busy and the reserve could not serve it,
    /// or asked a kernel VA window to map pages or flush.
    Refused,
    /// The kernel VA window numbered `window` (`kva_window::WindowId`) has
    /// no free extent for the request, after its pending unmaps were flushed.
    NoVa { window: u8 },
    /// No frame for a slab, or for a kernel window's page tables or extent
    /// nodes, after the slow path. E1-8's zones add the zone and order.
    NoFrames,
}

impl AllocError {
    pub fn name(self) -> &'static str {
        match self {
            AllocError::HeapExhausted => "heap_exhausted",
            AllocError::Refused => "refused",
            AllocError::NoVa { .. } => "no_va",
            AllocError::NoFrames => "no_frames",
        }
    }

    fn code(self) -> u64 {
        match self {
            AllocError::HeapExhausted => 1,
            AllocError::Refused => 2,
            AllocError::NoVa { window } => 3 | u64::from(window) << 8,
            AllocError::NoFrames => 4,
        }
    }

    fn from_code(code: u64) -> Option<AllocError> {
        Some(match code & 0xff {
            1 => AllocError::HeapExhausted,
            2 => AllocError::Refused,
            3 => AllocError::NoVa { window: (code >> 8) as u8 },
            4 => AllocError::NoFrames,
            _ => return None,
        })
    }
}

/// Why one pool attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// The pool's lock was held and the attempt did not wait.
    Busy,
    /// The pool has no extent for the request.
    Exhausted,
}

/// One locked pool: the fixed heap, or its emergency reserve.
pub trait Pool {
    /// One allocation. With `wait` false the pool's lock is taken with one
    /// `try_lock`, and a held lock is `Miss::Busy`.
    fn take(&self, layout: Layout, wait: bool) -> Result<NonNull<u8>, Miss>;
    /// Whether `ptr` lies in this pool.
    fn contains(&self, ptr: NonNull<u8>) -> bool;
    /// Free a block `take` gave.
    ///
    /// # Safety
    ///
    /// `ptr` came from this pool's `take` and is not freed twice.
    unsafe fn give(&self, ptr: NonNull<u8>);
    /// Resize a block `take` gave to hold `new_size` bytes without moving
    /// it (`heap_index`'s `grow_in_place` and `shrink_in_place`). Returns
    /// false, with the block unchanged, when it cannot.
    ///
    /// # Safety
    ///
    /// `ptr` came from this pool's `take` and is live.
    unsafe fn resize(&self, ptr: NonNull<u8>, new_size: usize) -> bool;
}

/// The CPU an allocation runs on.
pub trait Cpu {
    /// Whether this CPU is in an interrupt handler (hard, NMI or softirq).
    fn in_interrupt(&self) -> bool;
    /// This CPU's number.
    fn id(&self) -> usize;
    /// Run `f` with this CPU's interrupts masked, so it neither migrates nor
    /// takes an interrupt.
    fn masked<R>(&self, f: impl FnOnce() -> R) -> R;
}

/// The context a shrinker runs in. E2 adds "may sleep" and "may do I/O", and
/// E3 adds "may write back" (5.7); an E1 shrinker may do 0 of the 3.
#[derive(Clone, Copy, Debug)]
pub struct ReclaimCtx {
    _e1: (),
}

/// A registered reclaim hook (7.3). Both counts are in 4 KiB frames.
///
/// Each E1 shrinker takes its structure's lock with `try_lock` and returns 0
/// when it is busy, does not sleep, wait for I/O or wake a thread, and visits
/// at most its target plus a fixed batch.
pub struct Shrinker {
    pub name: &'static str,
    /// Lower runs first.
    pub cost: u32,
    /// Frames the shrinker could give back now, at most.
    pub count: fn() -> usize,
    /// Give back up to `target` frames; returns the frames given back.
    pub scan: fn(target: usize, ctx: ReclaimCtx) -> usize,
    next: AtomicPtr<Shrinker>,
}

impl Shrinker {
    pub const fn new(
        name: &'static str,
        cost: u32,
        count: fn() -> usize,
        scan: fn(usize, ReclaimCtx) -> usize,
    ) -> Self {
        Self { name, cost, count, scan, next: AtomicPtr::new(core::ptr::null_mut()) }
    }
}

/// A layer's share of 7.2 step 2 beyond the magazines: the per-CPU memory
/// it holds as whole frames, given back from each CPU under that CPU's lock
/// taken with `try_lock`, skipping a CPU whose lock is busy. The span arenas'
/// kept empty spans are one (6.4). `run` returns the frames free now; frames
/// a flush still has to cover are counted by the shrinker that flushes them.
pub struct Drain {
    pub name: &'static str,
    pub run: fn() -> usize,
    next: AtomicPtr<Drain>,
}

impl Drain {
    pub const fn new(name: &'static str, run: fn() -> usize) -> Self {
        Self { name, run, next: AtomicPtr::new(core::ptr::null_mut()) }
    }
}

/// The shrinkers in cost order, and the step-2 drains. Registration links a
/// hook in with a compare-exchange, and a linked hook stays linked, so a walk
/// needs no lock.
pub struct Registry {
    head: AtomicPtr<Shrinker>,
    drains: AtomicPtr<Drain>,
}

impl Registry {
    pub const fn new() -> Self {
        Self { head: AtomicPtr::new(core::ptr::null_mut()), drains: AtomicPtr::new(core::ptr::null_mut()) }
    }

    /// Link a step-2 drain in. A drain is registered once.
    pub fn register_drain(&self, drain: &'static Drain) {
        let new = drain as *const Drain as *mut Drain;
        let mut head = self.drains.load(Ordering::Acquire);
        loop {
            drain.next.store(head, Ordering::Relaxed);
            match self.drains.compare_exchange(head, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return,
                Err(current) => head = current,
            }
        }
    }

    /// Run each step-2 drain; returns the frames they gave back.
    pub fn drain_cpus(&self) -> usize {
        let mut freed = 0;
        let mut next = self.drains.load(Ordering::Acquire);
        // SAFETY: linked drains are `&'static` and stay linked.
        while let Some(drain) = unsafe { next.as_ref() } {
            freed += (drain.run)();
            next = drain.next.load(Ordering::Acquire);
        }
        freed
    }

    /// Link `shrinker` in after each registered shrinker of equal or lower
    /// cost. A shrinker is registered once.
    pub fn register(&self, shrinker: &'static Shrinker) {
        let new = shrinker as *const Shrinker as *mut Shrinker;
        'retry: loop {
            let mut link = &self.head;
            loop {
                let next = link.load(Ordering::Acquire);
                assert!(next != new, "shrinker {} registered twice", shrinker.name);
                // SAFETY: linked shrinkers are `&'static` and stay linked.
                match unsafe { next.as_ref() } {
                    Some(current) if current.cost <= shrinker.cost => link = &current.next,
                    _ => {
                        shrinker.next.store(next, Ordering::Relaxed);
                        if link.compare_exchange(next, new, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                            return;
                        }
                        continue 'retry;
                    }
                }
            }
        }
    }

    /// Run the shrinkers in cost order until `target` frames are given back.
    /// Returns the frames given back.
    pub fn reclaim(&self, target: usize) -> usize {
        let mut freed = 0;
        let mut next = self.head.load(Ordering::Acquire);
        // SAFETY: linked shrinkers are `&'static` and stay linked.
        while let Some(shrinker) = unsafe { next.as_ref() } {
            if freed >= target {
                break;
            }
            if (shrinker.count)() > 0 {
                freed += (shrinker.scan)(target - freed, ReclaimCtx { _e1: () });
            }
            next = shrinker.next.load(Ordering::Acquire);
        }
        freed
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Which CPU is running the shrinkers, if any: the `in_reclaim` flag of 7.2
/// as one word, since one CPU reclaims at a time. It is set and cleared with
/// that CPU's interrupts masked, so it is true only for code the shrinkers
/// run on that CPU.
pub struct Reclaim {
    owner: AtomicUsize,
}

impl Reclaim {
    pub const fn new() -> Self {
        Self { owner: AtomicUsize::new(0) }
    }

    pub fn held_by(&self, cpu: usize) -> bool {
        self.owner.load(Ordering::Acquire) == cpu + 1
    }

    /// Run `f` as the reclaiming CPU; another CPU reclaiming makes it a no-op
    /// that does not run `f`. The caller masks interrupts.
    fn run(&self, cpu: usize, f: impl FnOnce() -> usize) -> Option<usize> {
        self.owner.compare_exchange(0, cpu + 1, Ordering::AcqRel, Ordering::Acquire).ok()?;
        let freed = f();
        self.owner.store(0, Ordering::Release);
        Some(freed)
    }
}

impl Default for Reclaim {
    fn default() -> Self {
        Self::new()
    }
}

fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

/// Counts of the slow path's steps and refusals, read by
/// `/proc/breenix/kmem` and the OOM record.
pub struct Stats {
    /// Requests that left the fast path, per context.
    pub slow: [AtomicU64; 3],
    /// Steps refused by the context, per context: a busy lock not waited for
    /// (`Atomic`), or reclaim not run (`NoReclaim`, `Atomic`).
    pub refused: [AtomicU64; 3],
    /// Failed requests, per context.
    pub failed: [AtomicU64; 3],
    pub reserve_hits: AtomicU64,
    pub reclaim_runs: AtomicU64,
    pub reclaim_frames: AtomicU64,
    /// Reclaims skipped because another CPU was reclaiming.
    pub reclaim_busy: AtomicU64,
    /// The last failure: size, align, and `cause << 8 | ctx`.
    pub last_size: AtomicU64,
    pub last_align: AtomicU64,
    pub last_cause: AtomicU64,
}

impl Stats {
    pub const fn new() -> Self {
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            slow: [ZERO; 3],
            refused: [ZERO; 3],
            failed: [ZERO; 3],
            reserve_hits: ZERO,
            reclaim_runs: ZERO,
            reclaim_frames: ZERO,
            reclaim_busy: ZERO,
            last_size: ZERO,
            last_align: ZERO,
            last_cause: ZERO,
        }
    }

    fn fail(&self, layout: Layout, ctx: Ctx, error: AllocError) -> AllocError {
        add(&self.failed[ctx.index()], 1);
        self.last_size.store(layout.size() as u64, Ordering::Relaxed);
        self.last_align.store(layout.align() as u64, Ordering::Relaxed);
        self.last_cause.store(error.code() << 8 | ctx.index() as u64, Ordering::Relaxed);
        error
    }

    /// Count a step `ctx` may not take, outside the heap's own front: a
    /// kernel window asked to map or flush in `Atomic`.
    pub fn refuse(&self, ctx: Ctx) -> AllocError {
        add(&self.refused[ctx.index()], 1);
        AllocError::Refused
    }

    /// The last failure's cause and context, if a request has failed.
    pub fn last_failure(&self) -> Option<(AllocError, Ctx)> {
        let word = self.last_cause.load(Ordering::Relaxed);
        let cause = AllocError::from_code(word >> 8)?;
        let ctx = [Ctx::Normal, Ctx::NoReclaim, Ctx::Atomic][(word & 3) as usize % 3];
        Some((cause, ctx))
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

/// The logical CPU that a per-CPU base register's value `base` names, for
/// the slab layer's slot lookup (5.4a). The per-CPU array starts at `first`
/// and holds `count` entries `stride` bytes apart. The address of entry `n`
/// names CPU `n`. Other values, such as 0 or a value firmware left in a
/// register whose reset value is UNKNOWN (ARM64's TPIDR_EL1), give an empty
/// `Option` and are not read through. ARM64's `kmem::named_cpu` is its
/// kernel caller (x86-64 names a CPU through `per_cpu`), and the host tests
/// compile it on their host.
#[cfg(any(target_arch = "aarch64", test))]
pub fn cpu_of_base(base: u64, first: u64, stride: u64, count: usize) -> Option<usize> {
    let offset = base.checked_sub(first)?;
    if stride == 0 || offset % stride != 0 {
        return None;
    }
    let index = usize::try_from(offset / stride).ok()?;
    (index < count).then_some(index)
}

/// The emergency reserve's size for a heap of `heap_frames` frames: half of
/// 5.3's `min` for a zone of that size.
pub fn reserve_frames(heap_frames: usize) -> usize {
    (heap_frames / 1024).max(128).min(heap_frames / 16) / 2
}

/// The largest request the slab layer serves (6.1 row 1).
pub const SLAB_MAX: usize = PAGE;
/// The largest request a span arena serves (6.1 row 2).
pub const SPAN_MAX: usize = 512 << 10;
/// The largest alignment the slab layer and the span arenas serve.
pub const SMALL_ALIGN_MAX: usize = PAGE;
/// `kva_window::WindowId::Vmap`'s number, for `NoVa`.
const VMAP_WINDOW: u8 = 1;

/// The layer that serves a layout (6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Row 1: size and alignment at most 4096.
    Slab,
    /// Row 2: size at most 512 KiB, alignment at most 4096.
    Span,
    /// Row 3: a larger size, or an alignment above 4096 at any size.
    Vmap,
}

/// The one layer `layout` routes to (6.1): the first of the three rows it
/// matches. Alignment above 4096 is the VA's, which only the vmap window
/// aligns, up to its own size `vmap_bytes`; a larger alignment, which no
/// window can meet, is `NoVa`.
pub fn route(layout: Layout, vmap_bytes: u64) -> Result<Layer, AllocError> {
    let (size, align) = (layout.size(), layout.align());
    if align <= SMALL_ALIGN_MAX {
        if size <= SLAB_MAX {
            return Ok(Layer::Slab);
        }
        if size <= SPAN_MAX {
            return Ok(Layer::Span);
        }
    }
    if align as u64 <= vmap_bytes && (size as u64) <= vmap_bytes {
        Ok(Layer::Vmap)
    } else {
        Err(AllocError::NoVa { window: VMAP_WINDOW })
    }
}

/// The layer a span or vmap block that cannot resize in place moves to, as
/// a block of `layout` (6.6): `large::reallocate`'s fallback.
pub fn moved_layer(layout: Layout, vmap_bytes: u64) -> Result<Layer, AllocError> {
    route(layout, vmap_bytes)
}

/// The slab caches and per-CPU magazines in front of the heap (6.1 row 1,
/// 6.2, 6.3). Each call masks the CPU's interrupts and reads the CPU's
/// identity itself, so the identity cannot go stale by migration.
pub trait Small {
    /// Whether this layer serves `layout`.
    fn serves(&self, layout: Layout) -> bool;
    /// One object for `layout`. With `wait` false each lock is taken with one
    /// `try_lock`, and a held lock is `Miss::Busy`; `Miss::Exhausted` means
    /// no page could be had.
    fn take(&self, layout: Layout, wait: bool) -> Result<NonNull<u8>, Miss>;
    /// Free an object `take` gave for `layout`.
    ///
    /// # Safety
    ///
    /// `ptr` came from `take` and is not freed twice.
    unsafe fn give(&self, ptr: NonNull<u8>, layout: Layout);
    /// Whether an object `take` gave for `old` also serves `new`.
    fn same_object(&self, old: Layout, new: Layout) -> bool;
    /// 7.2 step 1: this CPU's magazines into the depots, with `try_lock`
    /// only when `wait` is false.
    fn drain_own(&self, wait: bool);
    /// 7.2 step 2: CPU `cpu`'s magazines into the depots, taking its lock
    /// with `try_lock`. Returns false when that lock was busy.
    fn drain(&self, cpu: usize) -> bool;
    /// The CPUs the per-CPU slot table describes.
    fn cpus(&self) -> usize;
    /// 7.2 step 3 again, after the other shrinkers ran: what they freed on
    /// this CPU went to its magazines, so move those to the depots with
    /// `try_lock`, and run the slab shrinker for `target` pages. Returns the
    /// pages given back.
    fn release_freed(&self, target: usize) -> usize;
}

/// The allocator's front (6.1, 6.6, 7.2): requests up to a page go to the
/// small-object layer, larger ones to the fixed heap until E1-9.
pub struct Front<'a, M: Pool, R: Pool, S: Small> {
    pub main: &'a M,
    pub reserve: &'a R,
    pub small: &'a S,
    pub shrinkers: &'a Registry,
    pub reclaim: &'a Reclaim,
    pub stats: &'a Stats,
}

impl<M: Pool, R: Pool, S: Small> Front<'_, M, R, S> {
    /// Allocate `layout` in `requested`, lowered for `cpu`'s state.
    pub fn allocate<C: Cpu>(
        &self,
        layout: Layout,
        requested: Ctx,
        cpu: &C,
    ) -> Result<NonNull<u8>, AllocError> {
        if self.small.serves(layout) {
            self.allocate_small(layout, requested, cpu)
        } else {
            self.allocate_large(layout, requested, cpu)
        }
    }

    /// A request the slab layer serves. The first attempt takes each lock
    /// with `try_lock`; it is the per-CPU pop unless a magazine is empty.
    /// After a miss, in the context the request is lowered to (7.2):
    /// - `Normal` and `NoReclaim` wait for a busy lock and try again;
    /// - `Atomic` drains this CPU's magazines with `try_lock` only, then
    ///   tries the emergency reserve;
    /// - with no page to be had, `Normal` and `NoReclaim` drain this CPU's
    ///   magazines (step 1) and, with `try_lock`, each other CPU's (step 2);
    ///   `Normal` runs the shrinkers, the slab shrinker first (steps 3-4);
    ///   then one retry (step 5).
    fn allocate_small<C: Cpu>(
        &self,
        layout: Layout,
        requested: Ctx,
        cpu: &C,
    ) -> Result<NonNull<u8>, AllocError> {
        let miss = match self.small.take(layout, false) {
            Ok(ptr) => return Ok(ptr),
            Err(miss) => miss,
        };
        let ctx = requested.lowered(cpu.in_interrupt(), self.reclaim.held_by(cpu.id()));
        add(&self.stats.slow[ctx.index()], 1);
        let miss = match miss {
            Miss::Busy if ctx.may_wait() => match self.small.take(layout, true) {
                Ok(ptr) => return Ok(ptr),
                Err(miss) => miss,
            },
            miss => miss,
        };
        if ctx.may_use_reserve() {
            if miss == Miss::Busy {
                add(&self.stats.refused[ctx.index()], 1);
            }
            self.small.drain_own(false);
            return match self.reserve.take(layout, false) {
                Ok(ptr) => {
                    add(&self.stats.reserve_hits, 1);
                    Ok(ptr)
                }
                Err(_) if miss == Miss::Busy => Err(self.stats.fail(layout, ctx, AllocError::Refused)),
                Err(_) => Err(self.stats.fail(layout, ctx, AllocError::NoFrames)),
            };
        }
        self.small.drain_own(true);
        for other in 0..self.small.cpus() {
            self.small.drain(other);
        }
        self.shrinkers.drain_cpus();
        if ctx.may_shrink() {
            self.reclaim(layout, cpu);
        } else {
            add(&self.stats.refused[ctx.index()], 1);
        }
        self.small
            .take(layout, true)
            .map_err(|_| self.stats.fail(layout, ctx, AllocError::NoFrames))
    }

    /// A request above a page, on the fixed heap.
    ///
    /// The first attempt takes the heap lock with `try_lock` in each context,
    /// so an uncontended request reads no per-CPU state. After a miss:
    /// - `Normal` and `NoReclaim` wait for a busy lock and try again;
    /// - `Atomic` takes no other step on the heap and tries the reserve;
    /// - `Normal` on an exhausted heap runs the shrinkers with interrupts
    ///   masked, holding no pool lock, and retries once.
    fn allocate_large<C: Cpu>(
        &self,
        layout: Layout,
        requested: Ctx,
        cpu: &C,
    ) -> Result<NonNull<u8>, AllocError> {
        let miss = match self.main.take(layout, false) {
            Ok(ptr) => return Ok(ptr),
            Err(miss) => miss,
        };
        let ctx = requested.lowered(cpu.in_interrupt(), self.reclaim.held_by(cpu.id()));
        add(&self.stats.slow[ctx.index()], 1);
        let miss = match miss {
            Miss::Busy if ctx.may_wait() => match self.main.take(layout, true) {
                Ok(ptr) => return Ok(ptr),
                Err(miss) => miss,
            },
            miss => miss,
        };
        if ctx.may_use_reserve() {
            if miss == Miss::Busy {
                add(&self.stats.refused[ctx.index()], 1);
            }
            return match self.reserve.take(layout, false) {
                Ok(ptr) => {
                    add(&self.stats.reserve_hits, 1);
                    Ok(ptr)
                }
                Err(_) if miss == Miss::Busy => Err(self.stats.fail(layout, ctx, AllocError::Refused)),
                Err(_) => Err(self.stats.fail(layout, ctx, AllocError::HeapExhausted)),
            };
        }
        if !ctx.may_shrink() {
            add(&self.stats.refused[ctx.index()], 1);
            return Err(self.stats.fail(layout, ctx, AllocError::HeapExhausted));
        }
        self.reclaim(layout, cpu);
        self.main
            .take(layout, true)
            .map_err(|_| self.stats.fail(layout, ctx, AllocError::HeapExhausted))
    }

    /// 7.2 for `frames` frames asked for outside this front: a page fault's
    /// frame, or a page table's. The frame allocator calls it when it has no
    /// frame, and retries once when it returns a nonzero count. Returns the
    /// frames given back.
    ///
    /// In an interrupt handler, or inside a shrinker, the request is
    /// `Atomic`, which skips steps 2-4 (6.6), so it returns 0. Otherwise it
    /// drains this CPU's magazines and then each other CPU's with `try_lock`
    /// (steps 1-2), then runs the shrinkers, the slab shrinker first, for the
    /// frames plus the batch (steps 3-4).
    pub fn reclaim_frames<C: Cpu>(&self, frames: usize, cpu: &C) -> usize {
        if cpu.in_interrupt() || self.reclaim.held_by(cpu.id()) {
            return 0;
        }
        self.small.drain_own(false);
        for other in 0..self.small.cpus() {
            self.small.drain(other);
        }
        let drained = self.shrinkers.drain_cpus();
        drained + self.reclaim_pages(frames.saturating_sub(drained) + RECLAIM_BATCH_FRAMES, cpu)
    }

    /// Run the shrinkers with interrupts masked, holding no pool lock, for
    /// the request's frames plus the batch.
    fn reclaim<C: Cpu>(&self, layout: Layout, cpu: &C) {
        self.reclaim_pages(layout.size().div_ceil(PAGE) + RECLAIM_BATCH_FRAMES, cpu);
    }

    /// Run the shrinkers for `target` frames; returns the frames given back,
    /// 0 when another CPU is reclaiming.
    fn reclaim_pages<C: Cpu>(&self, target: usize, cpu: &C) -> usize {
        // A shrinker that frees through the allocator leaves its blocks in
        // this CPU's magazines, so the slab layer returns them afterwards;
        // the count is the shrinkers' frames plus those pages.
        let freed = cpu.masked(|| {
            self.reclaim
                .run(cpu.id(), || self.shrinkers.reclaim(target) + self.small.release_freed(target))
        });
        match freed {
            Some(frames) => {
                add(&self.stats.reclaim_runs, 1);
                add(&self.stats.reclaim_frames, frames as u64);
                frames
            }
            None => {
                add(&self.stats.reclaim_busy, 1);
                0
            }
        }
    }

    /// Free `ptr`, allocated for `layout`, to the layer that holds it,
    /// chosen by its address (6.1): the reserve, the heap, or else the slab
    /// layer, which checks `layout` against the slab's class.
    ///
    /// # Safety
    ///
    /// `ptr` came from `allocate` and is not freed twice.
    pub unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if self.reserve.contains(ptr) {
            unsafe { self.reserve.give(ptr) }
        } else if self.main.contains(ptr) {
            unsafe { self.main.give(ptr) }
        } else {
            unsafe { self.small.give(ptr, layout) }
        }
    }

    /// Whether `ptr`, allocated for `old`, can keep its place as a block of
    /// `new`: a slab object whose class serves both (6.6).
    pub fn resizes_in_place(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> bool {
        !self.reserve.contains(ptr) && !self.main.contains(ptr) && self.small.same_object(old, new)
    }

    /// `GlobalAlloc::realloc`: `ptr`, allocated for `layout`, as a block of
    /// `new_size` bytes with the same alignment. A block that keeps its place
    /// is returned as is; otherwise a new block is allocated in `Normal`, the
    /// bytes both sizes hold are copied, and the old block is freed.
    ///
    /// # Safety
    ///
    /// `ptr` came from `allocate` for `layout`, is not freed twice, and
    /// `new_size` rounded up to `layout.align()` does not overflow `isize`.
    pub unsafe fn reallocate<C: Cpu>(
        &self,
        ptr: NonNull<u8>,
        layout: Layout,
        new_size: usize,
        cpu: &C,
    ) -> Result<NonNull<u8>, AllocError> {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        if self.resizes_in_place(ptr, layout, new_layout) {
            return Ok(ptr);
        }
        // A heap_index block grows into a free block after it, or gives its
        // tail back, in place (6.4, 6.6). It stays in its pool, whose free
        // takes any layout.
        let resized = if self.main.contains(ptr) {
            unsafe { self.main.resize(ptr, new_size) }
        } else if self.reserve.contains(ptr) {
            unsafe { self.reserve.resize(ptr, new_size) }
        } else {
            false
        };
        if resized {
            return Ok(ptr);
        }
        let new_ptr = self.allocate(new_layout, Ctx::Normal, cpu)?;
        // The copy is private to the caller; it holds no allocator lock.
        unsafe {
            core::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr(), layout.size().min(new_size));
            self.deallocate(ptr, layout);
        }
        Ok(new_ptr)
    }
}
