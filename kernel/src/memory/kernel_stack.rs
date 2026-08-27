//! Kernel stack allocator with bitmap management
//!
//! Reserves VA range 0xffffc900_0000_0000 – 0xffffc900_07ff_ffff (128 MiB) for kernel stacks.
//! Each stack gets 512 KiB usable space + 4 KiB guard page (total 516 KiB per slot).

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;
#[cfg(target_arch = "x86_64")]
use crate::memory::frame_allocator::{allocate_frame, deallocate_frame};
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "boot_tests")]
use core::sync::atomic::AtomicU8;
#[cfg(target_arch = "x86_64")]
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::{structures::paging::PageTableFlags, VirtAddr};

static KSTACK_SLOTS_ALLOCATED: AtomicU64 = AtomicU64::new(0);
static KSTACK_SLOTS_FREED: AtomicU64 = AtomicU64::new(0);
static KSTACK_FRAMES_MAPPED: AtomicU64 = AtomicU64::new(0);
static KSTACK_FRAMES_RELEASED: AtomicU64 = AtomicU64::new(0);
static KSTACK_LIVE_SLOT_CHECKS: AtomicU64 = AtomicU64::new(0);
static KSTACK_LIVE_SLOT_REFUSALS: AtomicU64 = AtomicU64::new(0);
static KSTACK_LIVE_SLOT_REFUSALS_INJECTED: AtomicU64 = AtomicU64::new(0);
static KSTACK_DROP_REFUSED_LIVE: AtomicU64 = AtomicU64::new(0);
static KSTACK_PTE_OVERWRITE_REFUSALS: AtomicU64 = AtomicU64::new(0);
static KSTACK_PUBLICATIONS: AtomicU64 = AtomicU64::new(0);
static KSTACK_PUBLICATIONS_POOLED: AtomicU64 = AtomicU64::new(0);
static KSTACK_PUBLICATIONS_SCHEDULER_OWNED: AtomicU64 = AtomicU64::new(0);
static KSTACK_PUBLICATIONS_ROW_RESIDUAL: AtomicU64 = AtomicU64::new(0);
static KSTACK_PUBLICATIONS_UNOWNED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Slot-identity measurement cohort (#646)
//
// The kernel-stack ownership oracle used to measure its own workload with the
// process-wide `KSTACK_SLOTS_ALLOCATED` / `KSTACK_SLOTS_FREED` deltas taken
// across its stress loop. Those counters move for every kernel stack in the
// system, so a thread reaped on another CPU inside the oracle's window landed
// in the oracle's "free" delta with no matching allocation in the same window:
// the equality the oracle asserted was a property of the whole system being
// quiescent, which nothing enforces (`slot_free_delta` one high, `slot_balance`
// -1, ~25% of `-smp 4` boots).
//
// The cohort below attributes slot motion to an *identity* rather than to a
// window. The oracle enrols the exact slot indices it allocates; `KernelStack::
// drop` matches each return against that enrolment before the bitmap bit is
// released. Every free is therefore classified as either a cohort return or a
// foreign return, and the two partition the global free delta exactly, whatever
// the rest of the system is doing on the other CPUs.
//
// A slot returned twice with no intervening allocation is still sitting in
// `COHORT_STATE_RETURNED` and is counted as a double return — the real
// unpaired-free reading the old global delta could never distinguish from
// ordinary concurrency. The allocator clears the state when it hands an index
// back out, which is what stops an *ordinary* return of a reallocated slot from
// being misreported as that double return.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const COHORT_TABLE_SLOTS: usize = MAX_KERNEL_STACKS;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const COHORT_TABLE_SLOTS: usize = aarch64::ARM64_MAX_KERNEL_STACKS;

/// Slot index is not a member of the measurement cohort.
#[cfg(feature = "boot_tests")]
const COHORT_STATE_ABSENT: u8 = 0;
/// Slot index was enrolled by the oracle and has not been returned yet.
#[cfg(feature = "boot_tests")]
const COHORT_STATE_ENROLLED: u8 = 1;
/// Slot index was enrolled and has been returned exactly once; it stays in this
/// state until the allocator hands the index out again.
#[cfg(feature = "boot_tests")]
const COHORT_STATE_RETURNED: u8 = 2;

#[cfg(feature = "boot_tests")]
static KSTACK_COHORT_STATE: [AtomicU8; COHORT_TABLE_SLOTS] =
    [const { AtomicU8::new(COHORT_STATE_ABSENT) }; COHORT_TABLE_SLOTS];
#[cfg(feature = "boot_tests")]
static KSTACK_COHORT_ENROLLED: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static KSTACK_COHORT_RETURNED: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static KSTACK_COHORT_DOUBLE_RETURN: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static KSTACK_FOREIGN_RETURNED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
static KSTACK_QUIESCE_BASELINE_OUTSTANDING: AtomicU64 = AtomicU64::new(u64::MAX);
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
static KSTACK_QUIESCE_WATCH_BITMAP: [AtomicU64; BITMAP_SIZE] =
    [const { AtomicU64::new(0) }; BITMAP_SIZE];
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
static KSTACK_QUIESCE_WATCH_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Drop the cohort state for a slot index the allocator has just claimed.
///
/// Called with the index owned by the caller and before the new `KernelStack`
/// escapes, so no return for this index can be in flight. This guards the
/// double-return counter against a false positive: once a cohort slot has been
/// returned and the pool hands the index to some other owner, that owner's
/// eventual return is an ordinary one and must not be read as a second return
/// of the cohort's slot.
#[cfg(feature = "boot_tests")]
#[inline]
fn cohort_note_allocation(index: usize) {
    if let Some(state) = KSTACK_COHORT_STATE.get(index) {
        state.store(COHORT_STATE_ABSENT, Ordering::Release);
    }
    #[cfg(target_arch = "x86_64")]
    quiesce_note_allocation(index);
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
#[inline]
fn quiesce_note_allocation(index: usize) {
    if !KSTACK_QUIESCE_WATCH_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let word_index = index / 64;
    let bit_index = index % 64;
    if let Some(word) = KSTACK_QUIESCE_WATCH_BITMAP.get(word_index) {
        word.fetch_or(1u64 << bit_index, Ordering::AcqRel);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
#[inline]
fn quiesce_note_owned_or_returned(index: usize) {
    let word_index = index / 64;
    let bit_index = index % 64;
    if let Some(word) = KSTACK_QUIESCE_WATCH_BITMAP.get(word_index) {
        word.fetch_and(!(1u64 << bit_index), Ordering::AcqRel);
    }
}

/// Classify a slot return by identity. Runs inside `KernelStack::drop`, before
/// the bitmap bit is released, so the index cannot be reallocated underneath
/// the classification.
#[cfg(feature = "boot_tests")]
#[inline]
fn cohort_note_return(index: usize) {
    let Some(state) = KSTACK_COHORT_STATE.get(index) else {
        KSTACK_FOREIGN_RETURNED.fetch_add(1, Ordering::Relaxed);
        #[cfg(target_arch = "x86_64")]
        quiesce_note_owned_or_returned(index);
        return;
    };
    match state.compare_exchange(
        COHORT_STATE_ENROLLED,
        COHORT_STATE_RETURNED,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => {
            KSTACK_COHORT_RETURNED.fetch_add(1, Ordering::Relaxed);
        }
        Err(COHORT_STATE_RETURNED) => {
            KSTACK_COHORT_DOUBLE_RETURN.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            KSTACK_FOREIGN_RETURNED.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[cfg(target_arch = "x86_64")]
    quiesce_note_owned_or_returned(index);
}

/// Identity-scoped slot accounting for the ownership oracle.
#[cfg(feature = "boot_tests")]
#[derive(Clone, Copy, Debug, Default)]
pub struct KernelStackCohortCounters {
    pub enrolled: u64,
    pub returned: u64,
    pub double_returned: u64,
    pub foreign_returned: u64,
}

#[cfg(feature = "boot_tests")]
pub fn kernel_stack_cohort_counters() -> KernelStackCohortCounters {
    KernelStackCohortCounters {
        enrolled: KSTACK_COHORT_ENROLLED.load(Ordering::Relaxed),
        returned: KSTACK_COHORT_RETURNED.load(Ordering::Relaxed),
        double_returned: KSTACK_COHORT_DOUBLE_RETURN.load(Ordering::Relaxed),
        foreign_returned: KSTACK_FOREIGN_RETURNED.load(Ordering::Relaxed),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelStackOwnership {
    /// Exactly one owner and it is the scheduler's publication copy.
    SchedulerOwned,
    /// The row still holds an allocation after publication — two owners.
    RowResidual,
    /// The thread names a pool kernel stack but no copy owns it — the #579 leak shape.
    Unowned,
    /// The thread's kernel stack does not come from the bitmap pool (kernel threads,
    /// boot threads, inherited tops) — nothing to own.
    NotPooled,
}

pub fn classify_kernel_stack_ownership(
    kernel_stack_top: Option<u64>,
    published_owns: bool,
    row_still_owns: bool,
) -> KernelStackOwnership {
    if row_still_owns {
        KernelStackOwnership::RowResidual
    } else if kernel_stack_top.is_some_and(is_kernel_stack_va) {
        if published_owns {
            KernelStackOwnership::SchedulerOwned
        } else {
            KernelStackOwnership::Unowned
        }
    } else {
        KernelStackOwnership::NotPooled
    }
}

pub(crate) fn note_publication(ownership: KernelStackOwnership) {
    KSTACK_PUBLICATIONS.fetch_add(1, Ordering::Relaxed);
    match ownership {
        KernelStackOwnership::SchedulerOwned => {
            KSTACK_PUBLICATIONS_POOLED.fetch_add(1, Ordering::Relaxed);
            KSTACK_PUBLICATIONS_SCHEDULER_OWNED.fetch_add(1, Ordering::Relaxed);
        }
        // Publication currently uses `Option::take`, so a residual row owner is
        // structurally impossible. Keep the legitimately-zero counter as a tripwire
        // against any future publication path that copies rather than moves.
        KernelStackOwnership::RowResidual => {
            KSTACK_PUBLICATIONS_ROW_RESIDUAL.fetch_add(1, Ordering::Relaxed);
        }
        KernelStackOwnership::Unowned => {
            KSTACK_PUBLICATIONS_POOLED.fetch_add(1, Ordering::Relaxed);
            KSTACK_PUBLICATIONS_UNOWNED.fetch_add(1, Ordering::Relaxed);
        }
        KernelStackOwnership::NotPooled => {}
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KernelStackPoolCounters {
    pub slots_allocated: u64,
    pub slots_freed: u64,
    pub frames_mapped: u64,
    pub frames_released: u64,
    pub live_slot_checks: u64,
    pub live_slot_refusals: u64,
    pub live_slot_refusals_injected: u64,
    pub drop_refused_live: u64,
    pub pte_overwrite_refusals: u64,
    pub publications: u64,
    pub publications_pooled: u64,
    pub publications_scheduler_owned: u64,
    pub publications_row_residual: u64,
    pub publications_unowned: u64,
}

pub fn kernel_stack_pool_counters() -> KernelStackPoolCounters {
    KernelStackPoolCounters {
        slots_allocated: KSTACK_SLOTS_ALLOCATED.load(Ordering::Relaxed),
        slots_freed: KSTACK_SLOTS_FREED.load(Ordering::Relaxed),
        frames_mapped: KSTACK_FRAMES_MAPPED.load(Ordering::Relaxed),
        frames_released: KSTACK_FRAMES_RELEASED.load(Ordering::Relaxed),
        live_slot_checks: KSTACK_LIVE_SLOT_CHECKS.load(Ordering::Relaxed),
        live_slot_refusals: KSTACK_LIVE_SLOT_REFUSALS.load(Ordering::Relaxed),
        live_slot_refusals_injected: KSTACK_LIVE_SLOT_REFUSALS_INJECTED
            .load(Ordering::Relaxed),
        drop_refused_live: KSTACK_DROP_REFUSED_LIVE.load(Ordering::Relaxed),
        pte_overwrite_refusals: KSTACK_PTE_OVERWRITE_REFUSALS.load(Ordering::Relaxed),
        publications: KSTACK_PUBLICATIONS.load(Ordering::Relaxed),
        publications_pooled: KSTACK_PUBLICATIONS_POOLED.load(Ordering::Relaxed),
        publications_scheduler_owned: KSTACK_PUBLICATIONS_SCHEDULER_OWNED.load(Ordering::Relaxed),
        publications_row_residual: KSTACK_PUBLICATIONS_ROW_RESIDUAL.load(Ordering::Relaxed),
        publications_unowned: KSTACK_PUBLICATIONS_UNOWNED.load(Ordering::Relaxed),
    }
}

/// Outstanding pool slots at the first call to this function.
///
/// First-caller-wins via `compare_exchange`, so every later caller reuses the
/// same baseline. `test_framework::executor::run_all_tests()` calls this once,
/// early, after the permanent boot-test oracle threads are started and before
/// the registered test workload creates kernel-stack churn; at that point the
/// only outstanding slots are the boot-time permanent ones, whatever that count
/// is. This is measurement only: it arms nothing, so the census number and the
/// watched window are set independently and neither can silently move the other.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn kernel_stack_quiesce_baseline_outstanding() -> u64 {
    let current = kernel_stack_pool_counters();
    let outstanding = current
        .slots_allocated
        .saturating_sub(current.slots_freed);
    match KSTACK_QUIESCE_BASELINE_OUTSTANDING.compare_exchange(
        u64::MAX,
        outstanding,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => outstanding,
        Err(existing) => existing,
    }
}

/// Open the watched window for the x86 quiesce leak census.
///
/// The watch is opened by the x86 kernel-stack ownership gate immediately before
/// its stress window and closed immediately after, so a watched slot is exactly a
/// slot allocated inside that window — the window #661 F2 names. Threads created
/// before the window (the permanent boot-test oracles) and after it (the
/// userspace phase) legitimately outlive quiescence and are deliberately not
/// watched, so the `leaked=0` pin cannot be reddened by a live thread that was
/// never in scope.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn kernel_stack_quiesce_start_watch() {
    for word in KSTACK_QUIESCE_WATCH_BITMAP.iter() {
        word.store(0, Ordering::Release);
    }
    KSTACK_QUIESCE_WATCH_ACTIVE.store(true, Ordering::Release);
}

/// Stop watching new foreign allocations for the x86 quiesce leak census.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn kernel_stack_quiesce_stop_watch() {
    KSTACK_QUIESCE_WATCH_ACTIVE.store(false, Ordering::Release);
}

/// Count watched foreign slots still live at quiescence.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn kernel_stack_quiesce_leaked_slots() -> u64 {
    let Some(bitmap) = STACK_BITMAP.try_lock() else {
        return u64::MAX;
    };
    bitmap
        .iter()
        .zip(KSTACK_QUIESCE_WATCH_BITMAP.iter())
        .map(|(current, watched)| {
            (current & watched.load(Ordering::Acquire)).count_ones() as u64
        })
        .sum()
}

pub(crate) fn note_kernel_stack_pte_overwrite_refused() {
    KSTACK_PTE_OVERWRITE_REFUSALS.fetch_add(1, Ordering::Relaxed);
}

/// Base address for kernel stack allocation
#[cfg(target_arch = "x86_64")]
const KERNEL_STACK_BASE: u64 = 0xffffc900_0000_0000;

/// End address for kernel stack allocation (128 MiB total space)
/// Increased to 128 MiB to support 512KB stacks and fork-heavy workloads.
#[cfg(target_arch = "x86_64")]
const KERNEL_STACK_END: u64 = 0xffffc900_0800_0000;

/// Size of each kernel stack (512 KiB)
/// Increased to 512KB to handle interactive mode's deep call stacks.
/// The keyboard interrupt handler path triggers framebuffer echo rendering:
/// keyboard_interrupt → push_char_nonblock → input_char_nonblock → output_char_nonblock
/// → write_char_to_framebuffer → split_screen → font rendering
/// This path can use 300KB+ of stack when combined with interrupt frame overhead
/// and nested help command processing with terminal output formatting.
#[cfg(target_arch = "x86_64")]
const KERNEL_STACK_SIZE: u64 = 512 * 1024;

/// Size of guard page (4 KiB)
#[cfg(target_arch = "x86_64")]
const GUARD_PAGE_SIZE: u64 = 4 * 1024;

/// Total size per stack slot (stack + guard)
#[cfg(target_arch = "x86_64")]
const STACK_SLOT_SIZE: u64 = KERNEL_STACK_SIZE + GUARD_PAGE_SIZE;

/// Maximum number of kernel stacks
#[cfg(target_arch = "x86_64")]
const MAX_KERNEL_STACKS: usize =
    ((KERNEL_STACK_END - KERNEL_STACK_BASE) / STACK_SLOT_SIZE) as usize;

/// Bitmap to track allocated stacks (1 bit per stack)
/// Using u64 array for efficient bit operations
#[cfg(target_arch = "x86_64")]
const BITMAP_SIZE: usize = (MAX_KERNEL_STACKS + 63) / 64;
#[cfg(target_arch = "x86_64")]
static STACK_BITMAP: Mutex<[u64; BITMAP_SIZE]> = Mutex::new([0; BITMAP_SIZE]);

/// True when the single online x86 CPU still names or executes on this slot.
///
/// x86 is single-CPU today. This covers both the per-CPU/TSS RSP0 mirror and
/// the currently executing stack pointer. The predicate must be extended to
/// inspect every online CPU when x86 SMP lands.
#[cfg(target_arch = "x86_64")]
pub(crate) fn is_kernel_stack_slot_live(stack_top: u64) -> bool {
    let bottom = stack_top.saturating_sub(KERNEL_STACK_SIZE);
    (crate::per_cpu::is_initialized() && crate::per_cpu::kernel_stack_top() == stack_top)
        || {
            let rsp: u64;
            unsafe {
                core::arch::asm!(
                    "mov {}, rsp",
                    out(reg) rsp,
                    options(nomem, nostack, preserves_flags)
                );
            }
            rsp > bottom && rsp <= stack_top
        }
}

#[cfg(target_arch = "x86_64")]
fn free_stack_slot(index: usize) {
    let mut bitmap = STACK_BITMAP.lock();
    let word_index = index / 64;
    let bit_index = index % 64;
    bitmap[word_index] &= !(1u64 << bit_index);
}

/// A kernel stack allocation
#[derive(Debug)]
pub struct KernelStack {
    /// Index in the bitmap
    index: usize,
    /// Bottom of the stack (lowest address, above guard page)
    bottom: VirtAddr,
    /// Top of the stack (highest address)
    top: VirtAddr,
    /// Process whose death should return this slot, when process-owned.
    owner_pid: Option<u64>,
}

impl KernelStack {
    /// Get the top of the stack (for RSP initialization)
    pub fn top(&self) -> VirtAddr {
        self.top
    }

    /// Get the bottom of the stack
    pub fn bottom(&self) -> VirtAddr {
        self.bottom
    }

    pub fn set_owner_pid(&mut self, owner_pid: Option<u64>) {
        self.owner_pid = owner_pid;
    }

    /// Enrol this allocation's slot identity in the measurement cohort (#646).
    ///
    /// Enrolment travels with the slot index, not with this `KernelStack`
    /// value, so publication moving the allocation to the scheduler-owned copy
    /// keeps the enrolment intact; the return is matched wherever the final
    /// owner is dropped.
    #[cfg(feature = "boot_tests")]
    pub fn enroll_in_measurement_cohort(&self) {
        if let Some(state) = KSTACK_COHORT_STATE.get(self.index) {
            state.store(COHORT_STATE_ENROLLED, Ordering::Release);
            KSTACK_COHORT_ENROLLED.fetch_add(1, Ordering::Relaxed);
            #[cfg(target_arch = "x86_64")]
            quiesce_note_owned_or_returned(self.index);
        }
    }

    /// Get the guard page address
    #[cfg(target_arch = "x86_64")]
    #[allow(dead_code)]
    pub fn guard_page(&self) -> VirtAddr {
        VirtAddr::new(self.bottom.as_u64() - GUARD_PAGE_SIZE)
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        #[cfg(target_arch = "aarch64")]
        {
            if is_kernel_stack_slot_live(self.top.as_u64()) {
                KSTACK_DROP_REFUSED_LIVE.fetch_add(1, Ordering::Relaxed);
                return;
            }
            #[cfg(feature = "boot_tests")]
            cohort_note_return(self.index);
            aarch64::free_kernel_stack(self.index);
            KSTACK_SLOTS_FREED.fetch_add(1, Ordering::Relaxed);
            if let Some(pid) = self.owner_pid {
                // This recorder runs inside Drop: it must remain allocation-free
                // and lock-light so slot return cannot introduce a teardown hazard.
                crate::tracing::providers::teardown::record_kernel_stack_slot_return(pid);
            }
            return;
        }

        #[cfg(target_arch = "x86_64")]
        {
            if is_kernel_stack_slot_live(self.top.as_u64()) {
                KSTACK_DROP_REFUSED_LIVE.fetch_add(1, Ordering::Relaxed);
                return;
            }

            let num_pages = (KERNEL_STACK_SIZE / 4096) as usize;
            let mut unmap_failed = false;
            for i in 0..num_pages {
                let virt_addr = self.bottom + (i as u64 * 4096);
                match unsafe {
                    crate::memory::kernel_page_table::unmap_kernel_page(virt_addr)
                } {
                    Ok(Some(frame)) => {
                        deallocate_frame(frame);
                        KSTACK_FRAMES_RELEASED.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => {}
                    Err(_) => unmap_failed = true,
                }
            }

            if unmap_failed {
                log::trace!("Refusing to free kernel stack slot after unmap failure");
                return;
            }

            #[cfg(feature = "boot_tests")]
            cohort_note_return(self.index);
            free_stack_slot(self.index);
            KSTACK_SLOTS_FREED.fetch_add(1, Ordering::Relaxed);
            if let Some(pid) = self.owner_pid {
                // This recorder runs inside Drop: it must remain allocation-free
                // and lock-light so slot return cannot introduce a teardown hazard.
                crate::tracing::providers::teardown::record_kernel_stack_slot_return(pid);
            }
            log::trace!("Freed kernel stack slot {}", self.index);
        }
    }
}

/// Allocate a new kernel stack
///
/// This allocates 8 KiB for the stack + 4 KiB guard page.
/// The stack is immediately mapped in the global kernel page tables.
#[cfg(target_arch = "x86_64")]
pub fn allocate_kernel_stack() -> Result<KernelStack, &'static str> {
    // Find a free slot in the bitmap
    let mut bitmap = STACK_BITMAP.lock();

    let mut slot_index = None;
    for (word_idx, word) in bitmap.iter_mut().enumerate() {
        if *word != u64::MAX {
            // This word has at least one free bit
            for bit_idx in 0..64 {
                let global_idx = word_idx * 64 + bit_idx;
                if global_idx >= MAX_KERNEL_STACKS {
                    break;
                }

                if (*word & (1u64 << bit_idx)) == 0 {
                    // Found a free slot
                    *word |= 1u64 << bit_idx;
                    slot_index = Some(global_idx);
                    break;
                }
            }

            if slot_index.is_some() {
                break;
            }
        }
    }

    let index = slot_index.ok_or("No free kernel stack slots")?;
    drop(bitmap); // Release the lock early
    // The index is now owned by this caller, so no return for it can be in
    // flight: clear the cohort state here so a later ordinary return of this
    // reallocated slot is not misread as a second return of the slot's
    // previous occupant (#646).
    #[cfg(feature = "boot_tests")]
    cohort_note_allocation(index);

    // Calculate addresses
    let slot_base = KERNEL_STACK_BASE + (index as u64 * STACK_SLOT_SIZE);
    let guard_page = VirtAddr::new(slot_base);
    let stack_bottom = VirtAddr::new(slot_base + GUARD_PAGE_SIZE);
    let stack_top = VirtAddr::new(slot_base + STACK_SLOT_SIZE);

    KSTACK_LIVE_SLOT_CHECKS.fetch_add(1, Ordering::Relaxed);
    if is_kernel_stack_slot_live(stack_top.as_u64()) {
        KSTACK_LIVE_SLOT_REFUSALS.fetch_add(1, Ordering::Relaxed);
        free_stack_slot(index);
        return Err("kernel-stack allocator selected a live slot");
    }

    // Map the stack pages (but not the guard page)
    // CRITICAL: Do NOT use GLOBAL flag for stack pages (per Cursor guidance)
    // Stack pages are per-thread and GLOBAL would keep stale TLB entries
    // Also set NO_EXECUTE since stacks should not contain executable code
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

    let num_pages = (KERNEL_STACK_SIZE / 4096) as usize;
    log::debug!("Mapping {} pages for kernel stack {}", num_pages, index);
    for i in 0..num_pages {
        let virt_addr = stack_bottom + (i as u64 * 4096);

        // Allocate a physical frame
        let frame = allocate_frame().ok_or("Out of memory for kernel stack")?;

        log::trace!(
            "  Mapping stack page {}: {:#x} -> {:#x}",
            i,
            virt_addr,
            frame.start_address()
        );

        // Map it in the global kernel page tables
        unsafe {
            log::trace!(
                "Mapping kernel stack page {:#x} -> {:#x}",
                virt_addr,
                frame.start_address()
            );
            crate::memory::kernel_page_table::map_kernel_page(
                virt_addr,
                frame.start_address(),
                flags,
            )?;
            KSTACK_FRAMES_MAPPED.fetch_add(1, Ordering::Relaxed);
            log::trace!("Kernel stack page {:#x} mapped successfully", virt_addr);
        }
    }

    log::debug!(
        "Allocated kernel stack {} at {:#x}-{:#x} (guard at {:#x})",
        index,
        stack_bottom,
        stack_top,
        guard_page
    );

    KSTACK_SLOTS_ALLOCATED.fetch_add(1, Ordering::Relaxed);
    Ok(KernelStack {
        index,
        bottom: stack_bottom,
        top: stack_top,
        owner_pid: None,
    })
}

/// Initialize the kernel stack allocator
///
/// This should be called during memory system initialization.
#[cfg(target_arch = "x86_64")]
pub fn init() {
    // The bitmap is already statically initialized to all zeros (all free)
    log::info!(
        "Kernel stack allocator initialized: {} slots available",
        MAX_KERNEL_STACKS
    );
    log::info!(
        "  Stack range: {:#x} - {:#x}",
        KERNEL_STACK_BASE,
        KERNEL_STACK_END
    );
    log::info!(
        "  Stack size: {} KiB + {} KiB guard",
        KERNEL_STACK_SIZE / 1024,
        GUARD_PAGE_SIZE / 1024
    );
}

// =============================================================================
// ARM64-specific kernel stack allocator (HHDM)
// =============================================================================

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use super::{
        VirtAddr, KSTACK_LIVE_SLOT_CHECKS, KSTACK_LIVE_SLOT_REFUSALS,
        KSTACK_SLOTS_ALLOCATED,
    };
    use crate::irq_safe_mutex::IrqSafeMutex;
    use core::sync::atomic::Ordering;

    /// ARM64 kernel stack base (in high-half direct map)
    /// Physical range: 0x5420_0000 .. 0x5620_0000 (32MB for kernel stacks)
    ///
    /// IMPORTANT: This must be AFTER the heap region to avoid collision!
    /// - Heap: 0x5020_0000 to 0x541F_FFFF (64 MB)
    /// - Kernel stacks: 0x5420_0000 to 0x561F_FFFF (32 MB)
    const ARM64_KERNEL_STACK_PHYS_BASE: u64 = 0x5420_0000;
    const ARM64_KERNEL_STACK_PHYS_END: u64 = 0x5620_0000;
    pub(crate) const ARM64_KERNEL_STACK_BASE: u64 =
        crate::arch_impl::aarch64::constants::HHDM_BASE + ARM64_KERNEL_STACK_PHYS_BASE;
    pub(crate) const ARM64_KERNEL_STACK_END: u64 =
        crate::arch_impl::aarch64::constants::HHDM_BASE + ARM64_KERNEL_STACK_PHYS_END;

    /// Stack size for ARM64 (64KB per stack)
    const ARM64_KERNEL_STACK_SIZE: u64 = 64 * 1024;

    /// Guard page size (4KB)
    const ARM64_GUARD_PAGE_SIZE: u64 = 4 * 1024;

    /// Total slot size (stack + guard)
    pub(crate) const ARM64_STACK_SLOT_SIZE: u64 = ARM64_KERNEL_STACK_SIZE + ARM64_GUARD_PAGE_SIZE;

    /// Bitmap to track allocated ARM64 stacks.
    pub(crate) const ARM64_MAX_KERNEL_STACKS: usize =
        ((ARM64_KERNEL_STACK_END - ARM64_KERNEL_STACK_BASE) / ARM64_STACK_SLOT_SIZE) as usize;
    const ARM64_BITMAP_SIZE: usize = (ARM64_MAX_KERNEL_STACKS + 63) / 64;

    static ARM64_STACK_BITMAP: IrqSafeMutex<[u64; ARM64_BITMAP_SIZE]> =
        IrqSafeMutex::new([0; ARM64_BITMAP_SIZE]);

    /// A kernel stack allocation for ARM64
    #[derive(Debug)]
    pub struct Aarch64KernelStack {
        /// Index in the bitmap
        pub index: usize,
        /// Bottom of the stack (lowest address, above guard page)
        pub bottom: VirtAddr,
        /// Top of the stack (highest address)
        pub top: VirtAddr,
    }

    impl Aarch64KernelStack {
        /// Get the top of the stack (for SP initialization)
        pub fn top(&self) -> VirtAddr {
            self.top
        }
    }

    /// True when an online CPU still names or has a resume SP inside this slot.
    ///
    /// A userspace return normally stores the slot top in `user_rsp_scratch`,
    /// while a suspended EL1 continuation may store an interior SP. Treat both
    /// forms as live so neither reclamation nor allocation can race the final
    /// architectural handoff off the old stack.
    pub(crate) fn is_kernel_stack_slot_live(stack_top: u64) -> bool {
        let stack_bottom = stack_top.saturating_sub(ARM64_KERNEL_STACK_SIZE);
        (0..crate::arch_impl::aarch64::constants::MAX_CPUS).any(|cpu_id| {
            if !crate::arch_impl::aarch64::smp::is_cpu_online(cpu_id) {
                return false;
            }
            let Some((live_top, live_resume_sp)) =
                crate::per_cpu_aarch64::live_stack_snapshot(cpu_id)
            else {
                return false;
            };
            live_top == stack_top
                || (live_resume_sp >= stack_bottom && live_resume_sp <= stack_top)
        })
    }

    /// True when `sp` names a byte inside the 64 KiB stack whose top is `stack_top`.
    pub(crate) fn sp_within_kernel_stack(sp: u64, stack_top: u64) -> bool {
        let stack_bottom = stack_top.saturating_sub(ARM64_KERNEL_STACK_SIZE);
        sp > stack_bottom && sp <= stack_top
    }

    /// Top of the reusable pool slot with this index, i.e. the value the per-CPU
    /// custody words hold for a thread running on that slot.
    pub(crate) fn kernel_stack_slot_top(index: usize) -> u64 {
        ARM64_KERNEL_STACK_BASE + (index as u64 + 1) * ARM64_STACK_SLOT_SIZE
    }

    /// Allocate a kernel stack for ARM64
    ///
    /// Uses a bitmap over a reserved high-half direct map region so fork-heavy
    /// userspace workloads can reuse stacks after waitpid reaps children.
    pub fn allocate_kernel_stack() -> Result<Aarch64KernelStack, &'static str> {
        let mut bitmap = ARM64_STACK_BITMAP.lock();

        let mut slot_index = None;
        for (word_idx, word) in bitmap.iter_mut().enumerate() {
            if *word == u64::MAX {
                continue;
            }

            for bit_idx in 0..64 {
                let global_idx = word_idx * 64 + bit_idx;
                if global_idx >= ARM64_MAX_KERNEL_STACKS {
                    break;
                }

                if (*word & (1u64 << bit_idx)) == 0 {
                    *word |= 1u64 << bit_idx;
                    slot_index = Some(global_idx);
                    break;
                }
            }

            if slot_index.is_some() {
                break;
            }
        }

        let index = slot_index.ok_or("ARM64 kernel stack pool exhausted")?;
        drop(bitmap);
        // See the x86 arm: clearing on allocation keeps an ordinary return of a
        // reallocated slot from being misread as a double return (#646).
        #[cfg(feature = "boot_tests")]
        super::cohort_note_allocation(index);

        let slot_base = ARM64_KERNEL_STACK_BASE + (index as u64 * ARM64_STACK_SLOT_SIZE);
        let stack_bottom = VirtAddr::new(slot_base + ARM64_GUARD_PAGE_SIZE);
        let stack_top = VirtAddr::new(slot_base + ARM64_STACK_SLOT_SIZE);

        KSTACK_LIVE_SLOT_CHECKS.fetch_add(1, Ordering::Relaxed);
        if is_kernel_stack_slot_live(stack_top.as_u64()) {
            KSTACK_LIVE_SLOT_REFUSALS.fetch_add(1, Ordering::Relaxed);
            free_kernel_stack(index);
            return Err("kernel-stack allocator selected a live slot");
        }

        // ROOT FIX (launcher-spawn EC=0x0/EC=0xe crash,
        // docs/planning/aarch64-launcher-spawn-crash/ROOT_CAUSE.md): scrub the
        // ENTIRE slot on every allocation, not just on first use. A bitmap-
        // reused slot (commit 04c9655a) can still hold a stale exception or
        // scheduler frame physically resident from its previous occupant
        // (e.g. an idle/schedule_from_kernel frame near the top of the
        // stack). If a later ret-based kernel resume
        // (aarch64_ret_to_kernel_context / schedule_from_kernel) is ever
        // dispatched onto this slot before that region has been overwritten
        // by the new owner's own execution, it can read that stale data
        // instead of state rebuilt from the fresh Thread.context, landing on
        // idle's leftover register file (elr = &WAKE_SITE_SCHEDULE, i.e.
        // __bss_start) and either UDF-faulting at 0x0 (EC=0x0) or ERETing
        // with an illegal SPSR (EC=0xe). Zeroing here removes the stale data
        // unconditionally, for every consumer, regardless of dispatch path.
        //
        // SAFETY: `stack_bottom..stack_top` is HHDM-mapped, present, writable
        // physical memory for the entire lifetime of the bitmap pool (see
        // ARM64_KERNEL_STACK_BASE/END above) -- true whether this slot is
        // brand new or reused, so writing zeros here is always valid.
        unsafe {
            core::ptr::write_bytes(
                stack_bottom.as_mut_ptr::<u8>(),
                0,
                ARM64_KERNEL_STACK_SIZE as usize,
            );
        }

        log::debug!(
            "ARM64 kernel stack allocated: {:#x}-{:#x}",
            stack_bottom.as_u64(),
            stack_top.as_u64()
        );

        KSTACK_SLOTS_ALLOCATED.fetch_add(1, Ordering::Relaxed);
        Ok(Aarch64KernelStack {
            index,
            bottom: stack_bottom,
            top: stack_top,
        })
    }

    pub fn free_kernel_stack(index: usize) {
        if index >= ARM64_MAX_KERNEL_STACKS {
            return;
        }

        let mut bitmap = ARM64_STACK_BITMAP.lock();
        let word_index = index / 64;
        let bit_index = index % 64;
        bitmap[word_index] &= !(1u64 << bit_index);
    }

    /// True if `addr` lies within the bitmap-backed reusable kernel-stack region.
    ///
    /// Lock-free range check (no bitmap access). Used by the SAVE_SKEW crash
    /// diagnostic to flag whether a saved exception frame sits on a reused fork
    /// kernel stack (commit 04c9655a), one of the two candidate upstream writers
    /// of the launcher-spawn context-corruption crash.
    pub fn is_in_reused_kstack_region(addr: u64) -> bool {
        addr >= ARM64_KERNEL_STACK_BASE && addr < ARM64_KERNEL_STACK_END
    }

    /// Initialize the ARM64 kernel stack allocator
    pub fn init() {
        let total_slots =
            (ARM64_KERNEL_STACK_END - ARM64_KERNEL_STACK_BASE) / ARM64_STACK_SLOT_SIZE;
        log::info!(
            "ARM64 kernel stack allocator initialized: {} slots available",
            total_slots
        );
        log::info!(
            "  Stack range (virt): {:#x} - {:#x}",
            ARM64_KERNEL_STACK_BASE,
            ARM64_KERNEL_STACK_END
        );
        log::info!(
            "  Stack range (phys): {:#x} - {:#x}",
            ARM64_KERNEL_STACK_PHYS_BASE,
            ARM64_KERNEL_STACK_PHYS_END
        );
        log::info!(
            "  Stack size: {} KiB + {} KiB guard",
            ARM64_KERNEL_STACK_SIZE / 1024,
            ARM64_GUARD_PAGE_SIZE / 1024
        );
    }
}

#[cfg(target_arch = "aarch64")]
pub use aarch64::{
    allocate_kernel_stack as allocate_kernel_stack_aarch64, init as init_aarch64,
    is_in_reused_kstack_region as is_in_reused_kstack_region_aarch64, Aarch64KernelStack,
};

#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::{
    is_kernel_stack_slot_live, kernel_stack_slot_top, sp_within_kernel_stack,
    ARM64_KERNEL_STACK_BASE, ARM64_KERNEL_STACK_END, ARM64_MAX_KERNEL_STACKS,
    ARM64_STACK_SLOT_SIZE,
};

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn addr_in_kernel_stack_pool(addr: u64) -> bool {
    addr >= ARM64_KERNEL_STACK_BASE && addr <= ARM64_KERNEL_STACK_END
}

pub(crate) fn is_kernel_stack_va(addr: u64) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        addr >= KERNEL_STACK_BASE && addr < KERNEL_STACK_END
    }
    #[cfg(target_arch = "aarch64")]
    {
        addr >= ARM64_KERNEL_STACK_BASE && addr < ARM64_KERNEL_STACK_END
    }
}

/// Run the production live-slot predicate against a caller-supplied stack top and
/// account a refusal exactly as the allocator would, tagging it as injected so the
/// oracle can separate deliberate injections from production refusals.
#[cfg(feature = "boot_tests")]
pub fn probe_live_slot_guard_injection(stack_top: u64) -> bool {
    KSTACK_LIVE_SLOT_CHECKS.fetch_add(1, Ordering::Relaxed);
    let is_live = is_kernel_stack_slot_live(stack_top);
    if is_live {
        KSTACK_LIVE_SLOT_REFUSALS.fetch_add(1, Ordering::Relaxed);
        KSTACK_LIVE_SLOT_REFUSALS_INJECTED.fetch_add(1, Ordering::Relaxed);
    }
    is_live
}

/// The stack top the current CPU is architecturally living on, if any — the
/// negative arm's known-live input.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn current_live_stack_top_for_test() -> Option<u64> {
    if !crate::per_cpu::is_initialized() {
        return None;
    }
    let stack_top = crate::per_cpu::kernel_stack_top();
    (stack_top != 0).then_some(stack_top)
}

/// The stack top the current CPU is architecturally living on, if any — the
/// negative arm's known-live input.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn current_live_stack_top_for_test() -> Option<u64> {
    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    crate::per_cpu_aarch64::live_stack_snapshot(cpu_id).map(|snapshot| snapshot.0)
}

/// ARM64: Use the aarch64-specific allocator
#[cfg(target_arch = "aarch64")]
pub fn allocate_kernel_stack() -> Result<KernelStack, &'static str> {
    let aarch64_stack = allocate_kernel_stack_aarch64()?;
    // Convert to KernelStack format for API compatibility
    Ok(KernelStack {
        index: aarch64_stack.index,
        bottom: aarch64_stack.bottom,
        top: aarch64_stack.top,
        owner_pid: None,
    })
}

/// ARM64: Initialize the kernel stack allocator
#[cfg(target_arch = "aarch64")]
pub fn init() {
    init_aarch64();
}
