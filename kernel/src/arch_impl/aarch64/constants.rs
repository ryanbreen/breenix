//! ARM64 architecture constants.
//!
//! This module centralizes all AArch64-specific magic numbers and constants
//! used by the kernel. Values are chosen to mirror x86_64 layout where
//! possible while respecting ARM64 address space conventions.

#![allow(dead_code)] // HAL constants - complete API for AArch64 architecture

// ============================================================================
// Memory Layout Constants
// ============================================================================

/// Base address for the higher-half kernel mapping.
/// The kernel is mapped starting at this address.
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Base of the higher-half direct map (HHDM).
/// Physical memory is mapped at this virtual base.
pub const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Base address for per-CPU data regions.
pub const PERCPU_BASE: u64 = 0xFFFF_FE00_0000_0000;

/// Base address for fixed mappings (fixmap).
pub const FIXMAP_BASE: u64 = 0xFFFF_FD00_0000_0000;

/// Base address for MMIO mappings.
pub const MMIO_BASE: u64 = 0xFFFF_E000_0000_0000;

/// Start of userspace stack region.
pub const USER_STACK_REGION_START: u64 = 0x0000_FFFF_FF00_0000;

/// End of userspace stack region (canonical boundary).
pub const USER_STACK_REGION_END: u64 = 0x0001_0000_0000_0000;

/// Userspace memory starts at 1GB to avoid low-address conflicts.
pub const USERSPACE_BASE: u64 = 0x0000_0000_4000_0000;

/// Maximum userspace address (below canonical boundary).
pub const USERSPACE_MAX: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Start of mmap allocation region for ARM64 userspace.
/// Placed between code/data end (2GB) and stack region start (~1TB).
/// Region: 0x0000_0001_0000_0000 to 0x0000_00FF_FE00_0000 (~1020 GB)
pub const MMAP_REGION_START: u64 = 0x0000_0001_0000_0000; // 4GB
/// End of mmap allocation region (gap before stack).
pub const MMAP_REGION_END: u64 = 0x0000_00FF_FE00_0000; // ~1TB, well below stack

// ============================================================================
// Page Table Constants
// ============================================================================

/// Number of page table levels in AArch64 (L0 -> L1 -> L2 -> L3).
pub const PAGE_LEVELS: usize = 4;

/// Standard page size (4 KiB).
pub const PAGE_SIZE: usize = 4096;

/// Large page size (2 MiB).
pub const LARGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Huge page size (1 GiB).
pub const HUGE_PAGE_SIZE: usize = 1024 * 1024 * 1024;

/// Number of entries per page table (512 for 4KB pages with 8-byte entries).
pub const ENTRIES_PER_TABLE: usize = 512;

/// Bit shifts for extracting page table indices from virtual addresses.
pub const L0_SHIFT: usize = 39;
pub const L1_SHIFT: usize = 30;
pub const L2_SHIFT: usize = 21;
pub const L3_SHIFT: usize = 12;

/// Mask for 9-bit page table index.
pub const PAGE_TABLE_INDEX_MASK: usize = 0x1FF;

// ============================================================================
// Interrupt Constants
// ============================================================================

/// ARM generic timer PPI interrupt number.
pub const TIMER_IRQ: u32 = 30;

/// Software generated interrupt for rescheduling IPIs.
pub const SGI_RESCHEDULE: u32 = 0;

/// Software generated interrupt for timer re-arm.
/// Sent to a CPU whose timer has stopped firing (Parallels HVF vtimer death).
/// The receiving CPU re-arms its virtual timer in the SGI handler.
pub const SGI_TIMER_REARM: u32 = 1;

// ============================================================================
// GIC Constants
// ============================================================================

/// GIC distributor base address (QEMU virt).
pub const GICD_BASE: u64 = 0x0800_0000;

/// GIC CPU interface base address (QEMU virt).
pub const GICC_BASE: u64 = 0x0801_0000;

/// Shared Peripheral Interrupts start at this ID.
pub const SPI_BASE: u32 = 32;

// ============================================================================
// Per-CPU Data Offsets
// ============================================================================

/// Offset of cpu_id in PerCpuData.
pub const PERCPU_CPU_ID_OFFSET: usize = 0;

/// Offset of current_thread pointer in PerCpuData.
pub const PERCPU_CURRENT_THREAD_OFFSET: usize = 8;

/// Offset of kernel_stack_top in PerCpuData.
pub const PERCPU_KERNEL_STACK_TOP_OFFSET: usize = 16;

/// Offset of idle_thread pointer in PerCpuData.
pub const PERCPU_IDLE_THREAD_OFFSET: usize = 24;

/// Offset of preempt_count in PerCpuData.
pub const PERCPU_PREEMPT_COUNT_OFFSET: usize = 32;

/// Offset of need_resched flag in PerCpuData.
pub const PERCPU_NEED_RESCHED_OFFSET: usize = 36;

/// Offset of user_rsp_scratch in PerCpuData.
pub const PERCPU_USER_RSP_SCRATCH_OFFSET: usize = 40;

/// Offset of the shared TSS / ARM64 nested-resume scratch slot in PerCpuData.
pub const PERCPU_TSS_OFFSET: usize = 48;

/// Offset of softirq_pending in PerCpuData.
pub const PERCPU_SOFTIRQ_PENDING_OFFSET: usize = 56;

/// Offset of next_cr3 in PerCpuData.
pub const PERCPU_NEXT_CR3_OFFSET: usize = 64;

/// Offset of kernel_cr3 in PerCpuData.
pub const PERCPU_KERNEL_CR3_OFFSET: usize = 72;

/// Offset of saved_process_cr3 in PerCpuData.
pub const PERCPU_SAVED_PROCESS_CR3_OFFSET: usize = 80;

/// Offset of exception_cleanup_context flag in PerCpuData.
pub const PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET: usize = 88;

/// Offset of scratch register save area in PerCpuData.
/// Used by assembly ERET paths to save/restore one register across SP switches.
pub const PERCPU_ERET_SCRATCH_OFFSET: usize = 96;

/// Offset of the second ERET scratch register save area in PerCpuData.
/// Holds `frame.x17` for the dispatch ERET path in `aarch64_enter_exception_frame`,
/// which needs a second scratch register after its SP switch has made the
/// exception frame unaddressable.
pub const PERCPU_ERET_SCRATCH2_OFFSET: usize = 144;

/// Offset of dispatch ELR in PerCpuData.
/// Written by context switch Rust code, read by assembly ERET path.
/// Immune to cross-CPU frame overwrite race (per-CPU, not on shared stack).
pub const PERCPU_DISPATCH_ELR_OFFSET: usize = 104;

/// Offset of dispatch SPSR in PerCpuData.
/// Written by context switch Rust code, read by assembly ERET path.
/// Immune to cross-CPU frame overwrite race (per-CPU, not on shared stack).
pub const PERCPU_DISPATCH_SPSR_OFFSET: usize = 112;

/// Offset of the ELR captured by an assembly ERET invariant redirect.
pub const PERCPU_ERET_GUARD_ELR_OFFSET: usize = 120;

/// Offset of the SPSR captured by an assembly ERET invariant redirect.
pub const PERCPU_ERET_GUARD_SPSR_OFFSET: usize = 128;

/// Offset of the ERET guard source tag, published after ELR/SPSR.
pub const PERCPU_ERET_GUARD_SOURCE_OFFSET: usize = 136;

/// Offset of X29 captured by an assembly ERET invariant redirect.
pub const PERCPU_ERET_GUARD_X29_OFFSET: usize = 152;

/// Offset of X30 captured by an assembly ERET invariant redirect.
pub const PERCPU_ERET_GUARD_X30_OFFSET: usize = 160;

/// Offset of SP captured by an assembly ERET invariant redirect.
pub const PERCPU_ERET_GUARD_SP_OFFSET: usize = 168;

/// Offset of the assembly ERET invariant redirect count.
pub const PERCPU_ERET_GUARD_COUNT_OFFSET: usize = 176;

/// Offset of the fixed per-CPU idle/exception stack top.
pub const PERCPU_IDLE_STACK_TOP_OFFSET: usize = 184;

// ============================================================================
// Preempt Count Bit Layout (Linux-compatible)
// ============================================================================

/// Shift for PREEMPT field (bits 0-7).
pub const PREEMPT_SHIFT: u32 = 0;

/// Shift for SOFTIRQ field (bits 8-15).
pub const SOFTIRQ_SHIFT: u32 = 8;

/// Shift for HARDIRQ field (bits 16-25).
pub const HARDIRQ_SHIFT: u32 = 16;

/// Shift for NMI field (bit 26 only - Linux uses 1 bit for NMI).
pub const NMI_SHIFT: u32 = 26;

/// Bit position for PREEMPT_ACTIVE flag.
pub const PREEMPT_ACTIVE_BIT: u32 = 28;

/// Mask for PREEMPT field.
pub const PREEMPT_MASK: u32 = 0xFF;

/// Mask for SOFTIRQ field.
pub const SOFTIRQ_MASK: u32 = 0xFF << SOFTIRQ_SHIFT;

/// One softirq-execution nesting level. The low bit distinguishes execution
/// from bottom-half disable nesting.
pub const SOFTIRQ_OFFSET: u32 = 1 << SOFTIRQ_SHIFT;

/// One bottom-half disable nesting level.
pub const SOFTIRQ_DISABLE_OFFSET: u32 = 2 * SOFTIRQ_OFFSET;

/// Mask for HARDIRQ field.
pub const HARDIRQ_MASK: u32 = 0x3FF << HARDIRQ_SHIFT;

/// Mask for NMI field (1 bit only, matching Linux kernel).
pub const NMI_MASK: u32 = 0x1 << NMI_SHIFT;

/// PREEMPT_ACTIVE flag value.
pub const PREEMPT_ACTIVE: u32 = 1 << PREEMPT_ACTIVE_BIT;

// ============================================================================
// Stack Sizes
// ============================================================================

/// Default kernel stack size (512 KiB).
/// Increased to 512KB to handle deep call stacks.
pub const KERNEL_STACK_SIZE: usize = 512 * 1024;

/// Physical/virtual spacing between adjacent per-CPU stack slots.
///
/// This value is part of the boot.S stack layout contract and must not change.
pub const PERCPU_STACK_STRIDE: u64 = 0x20_0000;

/// `log2(PERCPU_STACK_STRIDE)`, so slot attribution is a shift rather than a
/// division on the dispatch path.
pub const PERCPU_STACK_STRIDE_SHIFT: u32 = 21;

const _: () = assert!(PERCPU_STACK_STRIDE == 1 << PERCPU_STACK_STRIDE_SHIFT);

/// Size of the lower, scheduler-owned half of each per-CPU stack slot.
pub const PERCPU_SCHED_STACK_SIZE: u64 = 0x10_0000;

const _: () = assert!(KERNEL_STACK_SIZE as u64 <= PERCPU_SCHED_STACK_SIZE);
// The lower scheduler half plus the upper idle/exception stack must fit in one
// stride. This also proves that one slot's upper stack cannot extend past
// percpu_kernel_stack_top(cpu) into the next slot.
const _: () = assert!(PERCPU_SCHED_STACK_SIZE + KERNEL_STACK_SIZE as u64 <= PERCPU_STACK_STRIDE);

/// Debug-only sentinel at the scheduler/idle stack-half boundary.
///
/// This is an overrun detector, not an unmapped guard page: it is checked only
/// while recording a fatal postmortem.
pub const PERCPU_STACK_BOUNDARY_CANARY: u64 = 0x4252_4545_4E49_5853;

/// Guard page size between stacks.
pub const STACK_GUARD_SIZE: usize = PAGE_SIZE;

// ============================================================================
// Per-CPU Stack Region Constants
// ============================================================================

/// Base address for per-CPU kernel stacks region (ARM64).
/// Uses a region within the HHDM (higher-half direct map) that is mapped
/// by the boot page tables. Placed at ram_base + 0x0300_0000 (48MB into RAM,
/// after the kernel image + full BSS including large framebuffer statics).
///
/// RAM layout (relative to ram_base):
/// - +0x0000_0000 - +0x0300_0000: Kernel image + BSS (~48MB, incl. 7.5MB PCI_3D_FRAMEBUFFER)
/// - +0x0300_0000 - +0x0400_0000: Per-CPU stacks (16MB for 8 CPUs × 2MB each)
/// - +0x0400_0000 - end:          Heap and dynamic allocations
///
/// IMPORTANT: Must be kept in sync with:
///   - FRAME_ALLOC_START in platform_config.rs (starts at ram_base + 0x0400_0000)
///   - stack_base_phys in main_aarch64.rs (set_stack_base_phys call)
///
/// Platform-dependent physical base:
/// - QEMU/Parallels (ram at 0x4000_0000): physical 0x4300_0000
/// - VMware (ram at 0x8000_0000): physical 0x8300_0000
#[inline]
pub fn percpu_stack_region_base() -> u64 {
    HHDM_BASE + 0x4300_0000 + crate::platform_config::ram_base_offset()
}

/// Top of the upper, idle/exception-owned half of a per-CPU stack slot.
///
/// This is intentionally the same address boot.S has always installed as the
/// per-CPU kernel stack top.
#[inline]
pub fn percpu_kernel_stack_top(cpu: usize) -> u64 {
    percpu_stack_region_base() + (cpu as u64 + 1) * PERCPU_STACK_STRIDE
}

/// Bottom of the upper, idle/exception-owned half of a per-CPU stack slot.
#[inline]
pub fn percpu_kernel_stack_bottom(cpu: usize) -> u64 {
    percpu_stack_region_base() + cpu as u64 * PERCPU_STACK_STRIDE + PERCPU_SCHED_STACK_SIZE
}

/// Top of the lower, scheduler-owned half of a per-CPU stack slot.
#[inline]
pub fn percpu_sched_stack_top(cpu: usize) -> u64 {
    percpu_kernel_stack_bottom(cpu)
}

/// Write each stack-half boundary sentinel once, before scheduler/SMP startup.
///
/// AArch64 stacks grow downward. The word at the half boundary can therefore
/// detect the upper idle/exception stack reaching down into the boundary. It
/// cannot detect a scheduler-stack overrun toward higher addresses: normal
/// stack growth never approaches the boundary from the lower half.
pub fn initialize_percpu_stack_boundary_canaries() {
    for cpu in 0..MAX_CPUS {
        unsafe {
            core::ptr::write_volatile(
                percpu_sched_stack_top(cpu) as *mut u64,
                PERCPU_STACK_BOUNDARY_CANARY,
            );
        }
    }
}

/// Read the upper idle/exception-half downward-overrun sentinel for fatal
/// postmortem diagnostics only; this is not a bidirectional stack guard.
pub fn percpu_stack_boundary_canary_is_intact(cpu: usize) -> bool {
    unsafe {
        core::ptr::read_volatile(percpu_sched_stack_top(cpu) as *const u64)
            == PERCPU_STACK_BOUNDARY_CANARY
    }
}

// ============================================================================
// Per-CPU Stack Ownership Record
// ============================================================================

/// Distinctive constant in the ownership record's first word (ASCII "STKOWNER").
///
/// Stored XORed with the owning CPU index so that a record whose two words were
/// written at different times, or by something that is not
/// `publish_percpu_stack_owner`, does not read back as a valid owner.
pub const PERCPU_STACK_OWNER_MAGIC: u64 = 0x5354_4B4F_574E_4552;

/// Address of a CPU's two-`u64` stack-ownership record.
///
/// The record sits in the 16 bytes directly above the existing half-boundary
/// canary word, at the very bottom of the idle/exception half. It is NOT at the
/// top of the slot because `boot.S` computes each secondary CPU's initial SP
/// itself (`sp = SMP_STACK_BASE_PHYS + (cpu_id + 1) * 0x20_0000`): the slot top
/// is part of the boot contract and is not ours to move in this change. The
/// bottom is the only 16 bytes of the half that ordinary downward stack growth
/// cannot reach without first destroying the boundary canary, which is already
/// checked in the fatal postmortem.
#[inline]
pub fn percpu_stack_owner_sentinel(cpu: usize) -> u64 {
    percpu_kernel_stack_bottom(cpu) + 8
}

/// Stamp `cpu`'s ownership of its own idle/exception stack half.
///
/// Called from `per_cpu_aarch64::init_cpu` on the CPU itself, before that CPU's
/// first guarded stack-top install.
pub fn publish_percpu_stack_owner(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let record = percpu_stack_owner_sentinel(cpu) as *mut u64;
    unsafe {
        core::ptr::write_volatile(record, PERCPU_STACK_OWNER_MAGIC ^ cpu as u64);
        core::ptr::write_volatile(record.add(1), cpu as u64);
    }
}

/// The CPU that published ownership of `slot`, or `None` when the slot carries
/// no well-formed record.
///
/// Both words must agree — `word0 ^ MAGIC == word1` — and name a CPU in range.
/// Anything else (all zeroes before publication, a half-written record, stack
/// data that overran the boundary) reads as unpublished rather than as some
/// arbitrary CPU.
pub fn percpu_stack_published_owner(slot: usize) -> Option<usize> {
    if slot >= MAX_CPUS {
        return None;
    }
    let record = percpu_stack_owner_sentinel(slot) as *const u64;
    let word0 = unsafe { core::ptr::read_volatile(record) };
    let word1 = unsafe { core::ptr::read_volatile(record.add(1)) };
    let owner = word0 ^ PERCPU_STACK_OWNER_MAGIC;
    if owner != word1 || owner >= MAX_CPUS as u64 {
        return None;
    }
    Some(owner as usize)
}

/// The per-CPU stack slot an address belongs to, or `None` outside the region.
///
/// Stack tops are exclusive upper bounds — `percpu_kernel_stack_top(cpu)` is
/// `base + (cpu + 1) * PERCPU_STACK_STRIDE`, one past the last byte of slot
/// `cpu` — so the region is treated as `(base, base + size]` and attribution
/// uses the last addressable byte below the value. A half-open `[base, ...)`
/// test with a plain `(value - base) / stride` would put every legitimate
/// own-slot top in the NEXT slot and push the last slot's top out of the region
/// entirely. Two comparisons and a shift: this runs on the dispatch path.
#[inline]
pub fn percpu_stack_slot_of(addr: u64) -> Option<usize> {
    let base = percpu_stack_region_base();
    if addr <= base || addr > base + PERCPU_STACK_REGION_SIZE as u64 {
        return None;
    }
    Some(((addr - 1 - base) >> PERCPU_STACK_STRIDE_SHIFT) as usize)
}

/// Legacy constant for compile-time contexts (diagnostics). Uses the default
/// QEMU/Parallels base. Runtime code should use percpu_stack_region_base().
pub const PERCPU_STACK_REGION_BASE_DEFAULT: u64 = HHDM_BASE + 0x4300_0000;

/// Maximum number of CPUs supported on ARM64.
/// Limited to 8 to keep stack region within 512MB RAM constraint.
/// (8 CPUs * 2MB stride = 16MB total)
pub const MAX_CPUS: usize = 8;

/// Total size of per-CPU stack region (ARM64).
/// 8 CPUs * 2MB stride = 16MB
pub const PERCPU_STACK_REGION_SIZE: usize = MAX_CPUS * PERCPU_STACK_STRIDE as usize;
