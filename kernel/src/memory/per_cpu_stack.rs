//! Per-CPU emergency and IST stacks
//!
//! Range: 0xffffc980_xxxx_xxxx (one per CPU)
//! These stacks are used for NMI and double-fault handling

use crate::memory::frame_allocator::allocate_frame;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

/// Base address for per-CPU emergency stacks
const PER_CPU_STACK_BASE: u64 = 0xffffc980_0000_0000;

/// Size of each emergency stack (8 KiB)
const EMERGENCY_STACK_SIZE: u64 = 8 * 1024;

/// Total size per CPU: emergency stack + page fault stack (16 KiB)
const TOTAL_STACK_SIZE_PER_CPU: u64 = 2 * EMERGENCY_STACK_SIZE;

/// Maximum number of CPUs supported
const MAX_CPUS: usize = 256;

/// Per-CPU emergency stack info
#[allow(dead_code)]
#[derive(Debug)]
pub struct PerCpuStack {
    pub cpu_id: usize,
    pub stack_top: VirtAddr,
    pub stack_bottom: VirtAddr,
}

/// Initialize per-CPU emergency stacks
///
/// This allocates and maps emergency stacks for each CPU.
/// Should be called during early boot before SMP initialization.
/// Returns number of stacks initialized.
pub fn init_per_cpu_stacks(num_cpus: usize) -> Result<usize, &'static str> {
    if num_cpus > MAX_CPUS {
        return Err("Too many CPUs");
    }

    log::info!(
        "Initializing per-CPU emergency stacks for {} CPUs",
        num_cpus
    );

    // Don't use Vec here to avoid heap allocation during early boot
    // (heap allocator lock can deadlock with other locks)

    for cpu_id in 0..num_cpus {
        // Calculate stack address for this CPU
        let stack_base = PER_CPU_STACK_BASE + (cpu_id as u64 * 0x10000); // 64KB spacing
        let stack_bottom = VirtAddr::new(stack_base);
        let stack_top = VirtAddr::new(stack_base + TOTAL_STACK_SIZE_PER_CPU);

        // Map both emergency stack and page fault stack (16KB total = 4 pages)
        let num_pages = (TOTAL_STACK_SIZE_PER_CPU / 4096) as usize;
        for i in 0..num_pages {
            let virt_addr = stack_bottom + (i as u64 * 4096);

            // Allocate a physical frame
            let frame = allocate_frame().ok_or("Out of memory for emergency stack")?;

            // Map it in the global kernel page tables
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
            unsafe {
                crate::memory::kernel_page_table::map_kernel_page(
                    virt_addr,
                    frame.start_address(),
                    flags,
                )?;
            }
        }

        log::debug!(
            "CPU {} emergency stack: {:#x} - {:#x}",
            cpu_id,
            stack_bottom,
            stack_top
        );
    }
    log::info!("Initialized {} per-CPU emergency stacks", num_cpus);
    Ok(num_cpus)
}

/// Get the emergency stack for the current CPU (used for double fault)
///
/// Note: This assumes CPU ID can be obtained from APIC or similar
pub fn current_cpu_emergency_stack() -> VirtAddr {
    // TODO: Get actual CPU ID from APIC
    let cpu_id = 0; // For now, assume CPU 0

    let stack_base = PER_CPU_STACK_BASE + (cpu_id as u64 * 0x10000);
    VirtAddr::new(stack_base + EMERGENCY_STACK_SIZE)
}

/// Get the page fault IST stack for the current CPU
///
/// This is a separate stack from the emergency stack to avoid conflicts
pub fn current_cpu_page_fault_stack() -> VirtAddr {
    // TODO: Get actual CPU ID from APIC
    let cpu_id = 0; // For now, assume CPU 0

    let stack_base = PER_CPU_STACK_BASE + (cpu_id as u64 * 0x10000) + EMERGENCY_STACK_SIZE;
    VirtAddr::new(stack_base + EMERGENCY_STACK_SIZE)
}
