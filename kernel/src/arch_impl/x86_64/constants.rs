//! x86_64 architecture constants.
//!
//! This module centralizes all x86_64-specific magic numbers and constants
//! that were previously scattered throughout the kernel.

// ============================================================================
// Memory Layout Constants
// ============================================================================

/// Base address for the higher-half kernel mapping.
/// The kernel is mapped starting at this address.
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;

/// Base of the higher-half direct map (HHDM).
/// Physical memory is identity-mapped starting here.
pub const HHDM_BASE: u64 = 0xFFFF_8000_0000_0000;

/// Base address for per-CPU data regions.
pub const PERCPU_BASE: u64 = 0xFFFF_FE00_0000_0000;

/// Base address for fixed mappings (fixmap).
pub const FIXMAP_BASE: u64 = 0xFFFF_FD00_0000_0000;

/// Base address for MMIO mappings.
pub const MMIO_BASE: u64 = 0xFFFF_E000_0000_0000;

/// Start of the per-CPU kernel stack region (PML4 index 402).
pub const PERCPU_STACK_REGION_BASE: u64 = 0xFFFF_C900_0000_0000;

/// Start of the IST stack region (PML4 index 403).
pub const IST_STACK_REGION_BASE: u64 = 0xFFFF_C980_0000_0000;

/// Start of userspace stack region.
pub const USER_STACK_REGION_START: u64 = 0x7FFF_FF00_0000;

/// End of userspace stack region (canonical boundary).
pub const USER_STACK_REGION_END: u64 = 0x8000_0000_0000;

/// Userspace memory starts at 1GB to avoid PML4[0] conflicts.
pub const USERSPACE_BASE: u64 = 0x4000_0000;

/// Maximum userspace address (below canonical hole).
pub const USERSPACE_MAX: u64 = 0x7FFF_FFFF_FFFF;

// ============================================================================
// Page Table Constants
// ============================================================================

/// Number of page table levels in x86_64 (PML4 -> PDPT -> PD -> PT).
pub const PAGE_LEVELS: usize = 4;

/// Standard page size (4 KiB).
pub const PAGE_SIZE: usize = 4096;

/// Large page size (2 MiB).
pub const LARGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Huge page size (1 GiB).
pub const HUGE_PAGE_SIZE: usize = 1024 * 1024 * 1024;

/// Number of entries per page table (512 for 4KB pages with 8-byte entries).
pub const ENTRIES_PER_TABLE: usize = 512;

/// PML4 index where kernel space begins (upper half).
pub const KERNEL_PML4_START: usize = 256;

/// PML4 index for per-CPU kernel stacks.
pub const PERCPU_STACK_PML4_INDEX: usize = 402;

/// PML4 index for IST (Interrupt Stack Table) stacks.
pub const IST_STACK_PML4_INDEX: usize = 403;

/// Bit shifts for extracting page table indices from virtual addresses.
pub const PML4_SHIFT: usize = 39;
pub const PDPT_SHIFT: usize = 30;
pub const PD_SHIFT: usize = 21;
pub const PT_SHIFT: usize = 12;

/// Mask for 9-bit page table index.
pub const PAGE_TABLE_INDEX_MASK: usize = 0x1FF;

// ============================================================================
// Interrupt Constants
// ============================================================================

/// Syscall interrupt vector (INT 0x80).
pub const SYSCALL_VECTOR: u8 = 0x80;

/// Base vector for PIC IRQs (remapped from 0-15 to 32-47).
pub const PIC_IRQ_OFFSET: u8 = 32;

/// Timer interrupt vector (IRQ0 + offset).
pub const TIMER_VECTOR: u8 = PIC_IRQ_OFFSET;

/// Keyboard interrupt vector (IRQ1 + offset).
pub const KEYBOARD_VECTOR: u8 = PIC_IRQ_OFFSET + 1;

/// Serial port COM1 interrupt vector (IRQ4 + offset).
pub const SERIAL_COM1_VECTOR: u8 = PIC_IRQ_OFFSET + 4;

/// Double fault exception vector.
pub const DOUBLE_FAULT_VECTOR: u8 = 8;

/// Page fault exception vector.
pub const PAGE_FAULT_VECTOR: u8 = 14;

/// General protection fault vector.
pub const GP_FAULT_VECTOR: u8 = 13;

// ============================================================================
// GDT Segment Selectors
// ============================================================================

/// Kernel code segment selector (Ring 0).
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;

/// Kernel data segment selector (Ring 0).
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;

/// User code segment selector (Ring 3, with RPL=3).
pub const USER_CODE_SELECTOR: u16 = 0x18 | 3;

/// User data segment selector (Ring 3, with RPL=3).
pub const USER_DATA_SELECTOR: u16 = 0x20 | 3;

/// TSS segment selector.
pub const TSS_SELECTOR: u16 = 0x28;

// ============================================================================
// Per-CPU Data Offsets
// ============================================================================

/// Offset of cpu_id in PerCpuData (GS-relative).
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
// MSR (Model Specific Register) Numbers
// ============================================================================

/// FS base MSR (for user TLS).
pub const MSR_FS_BASE: u32 = 0xC000_0100;

/// GS base MSR (current GS segment base).
pub const MSR_GS_BASE: u32 = 0xC000_0101;

/// Kernel GS base MSR (swapped with GS_BASE by SWAPGS).
pub const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

// ============================================================================
// Stack Sizes
// ============================================================================

/// Default kernel stack size (64 KiB).
pub const KERNEL_STACK_SIZE: usize = 64 * 1024;

/// IST stack size for double fault handler.
pub const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

/// IST stack size for page fault handler.
pub const PAGE_FAULT_STACK_SIZE: usize = 16 * 1024;

/// Guard page size between stacks.
pub const STACK_GUARD_SIZE: usize = PAGE_SIZE;
