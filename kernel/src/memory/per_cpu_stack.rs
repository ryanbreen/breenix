//! Per-CPU emergency and IST stacks
//!
//! Range: 0xffffc980_xxxx_xxxx, one 64 KiB slot per logical CPU holding three
//! 8 KiB stacks: double fault, page fault and NMI. Each CPU's TSS points its
//! IST entries at its own slot (`gdt::install_ist_stacks`).

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{PageTableFlags, VirtAddr};
use crate::memory::frame_allocator::allocate_frame;
use crate::task::scheduler::MAX_CPUS;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::PageTableFlags;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

/// Base address for per-CPU emergency stacks
const PER_CPU_STACK_BASE: u64 = 0xffffc980_0000_0000;

/// Size of each emergency stack (8 KiB)
const EMERGENCY_STACK_SIZE: u64 = 8 * 1024;

/// Total size per CPU: emergency, page fault and NMI stacks (24 KiB)
const TOTAL_STACK_SIZE_PER_CPU: u64 = 3 * EMERGENCY_STACK_SIZE;

/// Spacing between consecutive CPUs' slots.
const PER_CPU_SLOT_SPACING: u64 = 0x10000;

const _: () = assert!(TOTAL_STACK_SIZE_PER_CPU <= PER_CPU_SLOT_SPACING);

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
        let stack_base = PER_CPU_STACK_BASE + (cpu_id as u64 * PER_CPU_SLOT_SPACING);
        let stack_bottom = VirtAddr::new(stack_base);
        let stack_top = VirtAddr::new(stack_base + TOTAL_STACK_SIZE_PER_CPU);

        // Map the slot's three stacks (24 KiB = 6 pages)
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

/// Top of logical CPU `cpu`'s double-fault stack.
pub fn emergency_stack(cpu: usize) -> VirtAddr {
    stack_top(cpu, 0)
}

/// Top of logical CPU `cpu`'s page fault IST stack, separate from the
/// emergency stack so the two cannot overrun each other's frames.
pub fn page_fault_stack(cpu: usize) -> VirtAddr {
    stack_top(cpu, 1)
}

/// Top of logical CPU `cpu`'s NMI stack.
pub fn nmi_stack(cpu: usize) -> VirtAddr {
    stack_top(cpu, 2)
}

fn stack_top(cpu: usize, index: u64) -> VirtAddr {
    assert!(cpu < MAX_CPUS, "no IST stacks for CPU {}", cpu);
    let slot = PER_CPU_STACK_BASE + cpu as u64 * PER_CPU_SLOT_SPACING;
    VirtAddr::new(slot + (index + 1) * EMERGENCY_STACK_SIZE)
}
