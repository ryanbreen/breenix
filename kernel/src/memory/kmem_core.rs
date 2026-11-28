//! Allocation contexts, allocation errors, the reclaim registry and the slow
//! path (E1 plan, 6.6 and 7.1 to 7.3), with no kernel dependencies. The
//! kernel supplies the pools, the CPU and the interrupt masking
//! (`memory/kmem.rs`); `tools/kmem-host-tests` supplies simulated ones.
//!
//! Until E1-9 the pools are the fixed kernel heap and its emergency reserve.
//! The layers E1-5 to E1-8 add (per-CPU magazines and page caches, remote
//! drains, span and vmap growth, the pending-unmap flush) bring their own
//! steps of the context table, and each checks the context the same way.
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
    /// request found the heap lock busy and the reserve could not serve it.
    Refused,
}

impl AllocError {
    pub fn name(self) -> &'static str {
        match self {
            AllocError::HeapExhausted => "heap_exhausted",
            AllocError::Refused => "refused",
        }
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

/// The shrinkers in cost order. Registration links a shrinker in with a
/// compare-exchange, and a linked shrinker stays linked, so a walk needs no lock.
pub struct Registry {
    head: AtomicPtr<Shrinker>,
}

impl Registry {
    pub const fn new() -> Self {
        Self { head: AtomicPtr::new(core::ptr::null_mut()) }
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
        let cause = match error {
            AllocError::HeapExhausted => 1,
            AllocError::Refused => 2,
        };
        self.last_cause.store(cause << 8 | ctx.index() as u64, Ordering::Relaxed);
        error
    }

    /// The last failure's cause and context, if a request has failed.
    pub fn last_failure(&self) -> Option<(AllocError, Ctx)> {
        let word = self.last_cause.load(Ordering::Relaxed);
        let cause = match word >> 8 {
            1 => AllocError::HeapExhausted,
            2 => AllocError::Refused,
            _ => return None,
        };
        let ctx = [Ctx::Normal, Ctx::NoReclaim, Ctx::Atomic][(word & 3) as usize % 3];
        Some((cause, ctx))
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

/// The emergency reserve's size for a heap of `heap_frames` frames: half of
/// 5.3's `min` for a zone of that size.
pub fn reserve_frames(heap_frames: usize) -> usize {
    (heap_frames / 1024).max(128).min(heap_frames / 16) / 2
}

/// The allocator's front over the fixed heap (6.6, 7.2).
pub struct Front<'a, M: Pool, R: Pool> {
    pub main: &'a M,
    pub reserve: &'a R,
    pub shrinkers: &'a Registry,
    pub reclaim: &'a Reclaim,
    pub stats: &'a Stats,
}

impl<M: Pool, R: Pool> Front<'_, M, R> {
    /// Allocate `layout` in `requested`, lowered for `cpu`'s state.
    ///
    /// The first attempt takes the heap lock with `try_lock` in each context,
    /// so an uncontended request reads no per-CPU state. After a miss:
    /// - `Normal` and `NoReclaim` wait for a busy lock and try again;
    /// - `Atomic` takes no other step on the heap and tries the reserve;
    /// - `Normal` on an exhausted heap runs the shrinkers with interrupts
    ///   masked, holding no pool lock, and retries once.
    pub fn allocate<C: Cpu>(
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
        let target = layout.size().div_ceil(PAGE) + RECLAIM_BATCH_FRAMES;
        let freed = cpu.masked(|| self.reclaim.run(cpu.id(), || self.shrinkers.reclaim(target)));
        match freed {
            Some(frames) => {
                add(&self.stats.reclaim_runs, 1);
                add(&self.stats.reclaim_frames, frames as u64);
            }
            None => add(&self.stats.reclaim_busy, 1),
        }
        self.main
            .take(layout, true)
            .map_err(|_| self.stats.fail(layout, ctx, AllocError::HeapExhausted))
    }
}
