//! Per-process Linux resource limits. All updates are serialized by PROCESS_MANAGER.
use super::{Process, ProcessId, ProcessManager};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const CPU: usize = 0;
pub const FSIZE: usize = 1;
pub const DATA: usize = 2;
pub const STACK: usize = 3;
pub const CORE: usize = 4;
pub const NPROC: usize = 6;
pub const NOFILE: usize = 7;
pub const MEMLOCK: usize = 8;
pub const AS: usize = 9;
pub const SIGPENDING: usize = 11;
pub const COUNT: usize = 16;
pub const INFINITY: u64 = u64::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rlimit {
    pub soft: u64,
    pub hard: u64,
}

fn defaults() -> [Rlimit; COUNT] {
    let mut limits = [Rlimit {
        soft: INFINITY,
        hard: INFINITY,
    }; COUNT];
    limits[STACK].soft = crate::memory::layout::MAX_USER_STACK_SIZE;
    limits[NOFILE] = Rlimit {
        soft: crate::ipc::fd::INITIAL_FDS as u64,
        hard: crate::ipc::MAX_FDS as u64,
    };
    limits[CORE].soft = 0;
    limits[SIGPENDING] = Rlimit {
        soft: DEFAULT_SIGPENDING,
        hard: DEFAULT_SIGPENDING,
    };
    limits
}

/// RLIMIT_SIGPENDING's default: how many realtime signal instances one real
/// user may have queued, which bounds the kernel memory they hold.
const DEFAULT_SIGPENDING: u64 = 4096;

/// Count the user's unreaped tasks under the same lock that publishes a fork.
/// Zombies retain their charge until wait reaps them; the iterator excludes tombstones.
pub fn fork_allowed(manager: &ProcessManager, pid: ProcessId) -> bool {
    let Some(parent) = manager.get_process(pid) else {
        return false;
    };
    parent.cred.uid == 0
        || (manager
            .iter_processes()
            .filter(|(_, p)| p.cred.uid == parent.cred.uid)
            .count() as u64)
            < parent.limits.get(NPROC).soft
}

impl Process {
    /// Grow the user stack down to the page holding `addr`, as an access
    /// there grows it on Linux: every page from the current bottom down is
    /// mapped, within MAX_USER_STACK_SIZE, RLIMIT_STACK and RLIMIT_AS, without
    /// crossing another live VMA. The bottom moves with each page, so growth
    /// that stops part way leaves it
    /// at the lowest page mapped. Stack pages are never executable. An
    /// address at or above the bottom needs no growth; another thread may
    /// have grown the stack past it first. Returns whether `addr`'s page is
    /// mapped now. PROCESS_MANAGER held.
    pub fn grow_user_stack(&mut self, addr: u64) -> bool {
        #[cfg(target_arch = "aarch64")]
        use crate::memory::arch_stub::{Page, PageTableFlags, Size4KiB, VirtAddr};
        use crate::memory::frame_allocator::{allocate_frame, deallocate_leaf_frame};
        use crate::memory::layout::MAX_USER_STACK_SIZE;
        #[cfg(target_arch = "x86_64")]
        use x86_64::{
            structures::paging::{Page, PageTableFlags, Size4KiB},
            VirtAddr,
        };

        let stack_top = self.user_stack_top;
        let stack_bottom = self.user_stack_bottom;
        let page_aligned = addr & !0xFFF;
        if stack_top == 0
            || page_aligned >= stack_top
            || stack_top - page_aligned > MAX_USER_STACK_SIZE
            || stack_top - page_aligned > self.limits.get(STACK).soft
            || (page_aligned < stack_bottom
                && self
                    .mapped_bytes()
                    .saturating_add(stack_bottom - page_aligned)
                    > self.limits.get(AS).soft)
        {
            return false;
        }
        // Growth must not overwrite another live mapping on the way down.
        if page_aligned < stack_bottom
            && self
                .vmas
                .iter()
                .any(|v| v.start.as_u64() < stack_bottom && v.end.as_u64() > page_aligned)
        {
            return false;
        }
        if page_aligned < stack_bottom
            && self.memory_locks.future
            && (crate::syscall::memory_advice::check_limit(
                self,
                self.memory_locks.additional(page_aligned, stack_bottom),
            )
            .is_err()
                || !self.memory_locks.can_insert(page_aligned, stack_bottom))
        {
            return false;
        }
        let Some(page_table) = self.page_table.as_mut() else {
            return false;
        };
        if addr >= stack_bottom {
            return page_table.translate(VirtAddr::new(page_aligned)).is_some();
        }

        #[cfg(target_arch = "aarch64")]
        let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
        #[cfg(target_arch = "x86_64")]
        let hhdm_base = crate::memory::physical_memory_offset().as_u64();
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;
        let mut page_addr = stack_bottom;
        let mut grown = true;
        while page_addr > page_aligned {
            page_addr -= 4096;
            let Some(frame) = allocate_frame() else {
                grown = false;
                break;
            };
            // SAFETY: the frame was just allocated and is reached through the
            // direct map; nothing else refers to it yet.
            unsafe {
                core::ptr::write_bytes(
                    (hhdm_base + frame.start_address().as_u64()) as *mut u8,
                    0,
                    4096,
                );
            }
            let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(page_addr));
            if page_table.map_page(page, frame, flags).is_err() {
                let _ = deallocate_leaf_frame(frame);
                grown = false;
                break;
            }
            // The page was not present, and x86 caches no translation for a
            // non-present page, so no other CPU has one to drop.
            #[cfg(target_arch = "x86_64")]
            x86_64::instructions::tlb::flush(VirtAddr::new(page_addr));
            self.user_stack_bottom = page_addr;
        }

        // ARM64: make the new descriptors visible to the table walker before
        // the access is made. They replace invalid entries, which the TLB
        // does not hold, so no invalidation is needed.
        // SAFETY: barriers only.
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags));
        }
        crate::syscall::memory_advice::record_future(self, self.user_stack_bottom, stack_bottom);
        grown
    }

    pub fn mapped_bytes(&self) -> u64 {
        self.image_size
            .saturating_add(self.heap_end.saturating_sub(self.heap_start))
            .saturating_add(self.user_stack_top.saturating_sub(self.user_stack_bottom))
            .saturating_add(self.vmas.iter().map(|v| v.size()).sum::<u64>())
    }

    pub fn data_bytes(&self) -> u64 {
        self.image_data_size
            .saturating_add(self.heap_end.saturating_sub(self.heap_start))
            .saturating_add(
                self.vmas
                    .iter()
                    .filter(|v| {
                        v.flags.contains(crate::memory::vma::MmapFlags::PRIVATE)
                            && v.prot.contains(crate::memory::vma::Protection::WRITE)
                    })
                    .map(|v| v.size())
                    .sum::<u64>(),
            )
    }

    /// Consume scheduler-produced CPU signals without taking its lock.
    pub fn check_cpu_limit(&mut self) {
        if self.limits.pending.load(Ordering::Acquire) == 0 {
            return;
        }
        let pending = self.limits.pending.fetch_and(!1, Ordering::AcqRel);
        if pending & 1 != 0 {
            self.signals.set_pending(crate::signal::constants::SIGXCPU);
        }
        if pending & 2 != 0 {
            self.signals.set_pending(crate::signal::constants::SIGKILL);
        }
    }
}

/// Shared by CLONE_VM siblings, independently copied by fork and spawn.
/// PM serializes limit reads and updates; the scheduler reads CPU thresholds
/// atomically and charges time without acquiring PM or walking thread rows.
pub struct Limits {
    values: [(AtomicU64, AtomicU64); COUNT],
    ticks: AtomicU64,
    next: AtomicU64,
    pending: AtomicU32,
}

impl Limits {
    pub fn has_pending_cpu_signals(&self) -> bool {
        self.pending.load(Ordering::Acquire) != 0
    }

    pub fn new() -> Arc<Self> {
        Self::from_values(defaults())
    }

    fn from_values(values: [Rlimit; COUNT]) -> Arc<Self> {
        Arc::new(Self {
            values: values.map(|v| (AtomicU64::new(v.soft), AtomicU64::new(v.hard))),
            ticks: AtomicU64::new(0),
            next: AtomicU64::new(0),
            pending: AtomicU32::new(0),
        })
    }

    pub fn inherit(&self) -> Arc<Self> {
        Self::from_values(core::array::from_fn(|i| self.get(i)))
    }

    pub fn get(&self, resource: usize) -> Rlimit {
        Rlimit {
            soft: self.values[resource].0.load(Ordering::Acquire),
            hard: self.values[resource].1.load(Ordering::Acquire),
        }
    }

    pub fn set(&self, resource: usize, value: Rlimit) {
        self.values[resource].1.store(value.hard, Ordering::Release);
        self.values[resource].0.store(value.soft, Ordering::Release);
        if resource == CPU {
            self.next.store(0, Ordering::Release);
            self.charge_cpu(0);
        }
    }

    /// Charge one execution interval. Exited threads' time remains in the group.
    pub fn charge_cpu(&self, ticks: u64) {
        let total = self
            .ticks
            .fetch_add(ticks, Ordering::Relaxed)
            .saturating_add(ticks);
        let limit = self.get(CPU);
        if limit.soft == INFINITY && limit.hard == INFINITY {
            return;
        }
        let seconds = total.saturating_mul(crate::time::timer::MS_PER_TICK) / 1000;
        if seconds >= limit.hard.max(1) {
            self.pending.fetch_or(2, Ordering::Release);
        } else if seconds >= limit.soft.max(1) {
            let next = self.next.load(Ordering::Relaxed);
            if seconds >= next
                && self
                    .next
                    .compare_exchange(
                        next,
                        seconds.saturating_add(1),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                self.pending.fetch_or(1, Ordering::Release);
            }
        }
    }
}
