//! Boot test catalog: ID constants and metadata for BTRT entries.
//!
//! Each test milestone has a unique ID, name, and category.
//! IDs are stable across releases -- never reuse or renumber.
//!
//! # ID Ranges
//!
//! - 0-99: Kernel initialization milestones
//! - 100-199: Subsystem initialization
//! - 200-299: Userspace binary load/exec
//! - 300-499: Userspace test results
//! - 500-511: Reserved

/// Category of a boot test milestone (wire protocol -- all variants required for host-side parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootTestCategory {
    /// Core kernel init (GDT, IDT, memory, serial, etc.)
    KernelInit = 0,
    /// Driver/hardware init (PCI, NIC, disk, etc.)
    DriverInit = 1,
    /// Subsystem init (scheduler, FS, network stack, etc.)
    Subsystem = 2,
    /// Userspace binary load and exec
    UserspaceExec = 3,
    /// Userspace test result (exit code check)
    UserspaceResult = 4,
}

/// A boot test definition in the catalog.
pub struct BootTestDef {
    pub id: u16,
    pub name: &'static str,
    pub category: BootTestCategory,
}

// =============================================================================
// Kernel Initialization (0-99)
// =============================================================================

pub const KERNEL_ENTRY: u16 = 0;
pub const SERIAL_INIT: u16 = 1;
pub const GDT_IDT_INIT: u16 = 2;
pub const PER_CPU_INIT: u16 = 3;
pub const MEMORY_INIT: u16 = 4;
pub const HEAP_INIT: u16 = 5;
pub const FRAME_ALLOC_INIT: u16 = 6;
pub const INTERRUPTS_ENABLED: u16 = 7;
pub const TRACING_INIT: u16 = 8;
pub const TIMER_INIT: u16 = 9;

// =============================================================================
// Driver Initialization (10-29, within kernel init range)
// =============================================================================

pub const PCI_ENUMERATION: u16 = 10;
pub const NIC_INIT: u16 = 11;
pub const VIRTIO_BLK_INIT: u16 = 12;

// =============================================================================
// Subsystem Initialization (100-199)
// =============================================================================

pub const SCHEDULER_INIT: u16 = 100;
pub const KTHREAD_SUBSYSTEM: u16 = 101;
pub const WORKQUEUE_INIT: u16 = 102;
pub const FILESYSTEM_INIT: u16 = 103;
pub const EXT2_MOUNT: u16 = 104;
pub const NETWORK_STACK_INIT: u16 = 105;
pub const PROCFS_INIT: u16 = 106;
pub const PIPE_SUBSYSTEM: u16 = 107;

// =============================================================================
// Userspace Execution (200-299)
// =============================================================================

pub const USERSPACE_PROCESS_CREATE: u16 = 200;
pub const USERSPACE_ELF_LOAD: u16 = 201;
pub const USERSPACE_FIRST_INSTRUCTION: u16 = 202;
pub const USERSPACE_FIRST_SYSCALL: u16 = 203;

// =============================================================================
// Boot Test Framework (250-259) -- parallel test framework results
// =============================================================================

pub const BOOT_TESTS_START: u16 = 250;
pub const BOOT_TESTS_COMPLETE: u16 = 251;

// =============================================================================
// ARM64-Specific (30-49, within kernel init range)
// =============================================================================

pub const AARCH64_MMU_INIT: u16 = 30;
pub const AARCH64_EXCEPTION_VECTORS: u16 = 31;
pub const AARCH64_GIC_INIT: u16 = 32;
pub const AARCH64_TIMER_INIT: u16 = 33;
pub const AARCH64_UART_INIT: u16 = 34;
pub const AARCH64_FRAMEBUFFER_INIT: u16 = 35;

// =============================================================================
// Full Catalog
// =============================================================================

/// Complete catalog of boot test milestones.
///
/// This is the single source of truth for test ID → name mappings.
/// The xtask host-side catalog (`xtask/src/btrt_catalog.rs`) must be
/// kept in sync with this list.
pub static CATALOG: &[BootTestDef] = &[
    // Kernel init
    BootTestDef { id: KERNEL_ENTRY, name: "kernel_entry", category: BootTestCategory::KernelInit },
    BootTestDef { id: SERIAL_INIT, name: "serial_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: GDT_IDT_INIT, name: "gdt_idt_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: PER_CPU_INIT, name: "per_cpu_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: MEMORY_INIT, name: "memory_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: HEAP_INIT, name: "heap_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: FRAME_ALLOC_INIT, name: "frame_alloc_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: INTERRUPTS_ENABLED, name: "interrupts_enabled", category: BootTestCategory::KernelInit },
    BootTestDef { id: TRACING_INIT, name: "tracing_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: TIMER_INIT, name: "timer_init", category: BootTestCategory::KernelInit },
    // Drivers
    BootTestDef { id: PCI_ENUMERATION, name: "pci_enumeration", category: BootTestCategory::DriverInit },
    BootTestDef { id: NIC_INIT, name: "nic_init", category: BootTestCategory::DriverInit },
    BootTestDef { id: VIRTIO_BLK_INIT, name: "virtio_blk_init", category: BootTestCategory::DriverInit },
    // Subsystems
    BootTestDef { id: SCHEDULER_INIT, name: "scheduler_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: KTHREAD_SUBSYSTEM, name: "kthread_subsystem", category: BootTestCategory::Subsystem },
    BootTestDef { id: WORKQUEUE_INIT, name: "workqueue_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: FILESYSTEM_INIT, name: "filesystem_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: EXT2_MOUNT, name: "ext2_mount", category: BootTestCategory::Subsystem },
    BootTestDef { id: NETWORK_STACK_INIT, name: "network_stack_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: PROCFS_INIT, name: "procfs_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: PIPE_SUBSYSTEM, name: "pipe_subsystem", category: BootTestCategory::Subsystem },
    // Userspace exec
    BootTestDef { id: USERSPACE_PROCESS_CREATE, name: "userspace_process_create", category: BootTestCategory::UserspaceExec },
    BootTestDef { id: USERSPACE_ELF_LOAD, name: "userspace_elf_load", category: BootTestCategory::UserspaceExec },
    BootTestDef { id: USERSPACE_FIRST_INSTRUCTION, name: "userspace_first_instruction", category: BootTestCategory::UserspaceExec },
    BootTestDef { id: USERSPACE_FIRST_SYSCALL, name: "userspace_first_syscall", category: BootTestCategory::UserspaceExec },
    // Boot tests framework
    BootTestDef { id: BOOT_TESTS_START, name: "boot_tests_start", category: BootTestCategory::Subsystem },
    BootTestDef { id: BOOT_TESTS_COMPLETE, name: "boot_tests_complete", category: BootTestCategory::Subsystem },
    // ARM64
    BootTestDef { id: AARCH64_MMU_INIT, name: "aarch64_mmu_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_EXCEPTION_VECTORS, name: "aarch64_exception_vectors", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_GIC_INIT, name: "aarch64_gic_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_TIMER_INIT, name: "aarch64_timer_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_UART_INIT, name: "aarch64_uart_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_FRAMEBUFFER_INIT, name: "aarch64_framebuffer_init", category: BootTestCategory::KernelInit },
];

/// Look up a test name by ID.
pub fn test_name(id: u16) -> &'static str {
    for def in CATALOG {
        if def.id == id {
            return def.name;
        }
    }
    "unknown"
}
