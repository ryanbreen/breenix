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

/// Offset of TSS pointer in PerCpuData.
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

/// Guard page size between stacks.
pub const STACK_GUARD_SIZE: usize = PAGE_SIZE;

// ============================================================================
// Per-CPU Stack Region Constants
// ============================================================================

/// Base address for per-CPU kernel stacks region (ARM64).
/// Uses a region within the HHDM (higher-half direct map) that is mapped
/// by the boot page tables. Placed at physical 0x4100_0000 (16MB into RAM
/// after kernel) to stay within typical 512MB QEMU RAM configs.
///
/// QEMU virt RAM layout: physical 0x4000_0000 (1GB mark) for N MB
/// With 512MB RAM: physical 0x4000_0000 to 0x6000_0000
///
/// Stack layout in RAM:
/// - 0x4000_0000 - 0x4100_0000: Kernel image (~16MB)
/// - 0x4100_0000 - 0x4200_0000: Per-CPU stacks (16MB for 8 CPUs)
/// - 0x4200_0000 - 0x6000_0000: Heap and dynamic allocations
///
/// Virtual: 0xFFFF_0000_4100_0000
/// Physical: 0x4100_0000
pub const PERCPU_STACK_REGION_BASE: u64 = HHDM_BASE + 0x4100_0000;

/// Maximum number of CPUs supported on ARM64.
/// Limited to 8 to keep stack region within 512MB RAM constraint.
/// (8 CPUs * 2MB stride = 16MB total)
pub const MAX_CPUS: usize = 8;

/// Total size of per-CPU stack region (ARM64).
/// 8 CPUs * 2MB stride = 16MB
pub const PERCPU_STACK_REGION_SIZE: usize = MAX_CPUS * 2 * 1024 * 1024;
