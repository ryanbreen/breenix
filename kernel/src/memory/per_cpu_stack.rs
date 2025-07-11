//! Per-CPU emergency and IST stacks
//! 
//! Range: 0xffffc980_xxxx_xxxx (one per CPU)
//! These stacks are used for NMI and double-fault handling

use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;
use crate::memory::frame_allocator::allocate_frame;

/// Base address for per-CPU emergency stacks
const PER_CPU_STACK_BASE: u64 = 0xffffc980_0000_0000;

/// Size of each emergency stack (8 KiB)
const EMERGENCY_STACK_SIZE: u64 = 8 * 1024;

/// Maximum number of CPUs supported
const MAX_CPUS: usize = 256;

/// Per-CPU emergency stack info
#[derive(Debug)]
pub struct PerCpuStack {
    // Stack information stored for potential future use
}

/// Initialize per-CPU emergency stacks
/// 
/// This allocates and maps emergency stacks for each CPU.
/// Should be called during early boot before SMP initialization.
pub fn init_per_cpu_stacks(num_cpus: usize) -> Result<alloc::vec::Vec<PerCpuStack>, &'static str> {
    if num_cpus > MAX_CPUS {
        return Err("Too many CPUs");
    }
    
    log::info!("Initializing per-CPU emergency stacks for {} CPUs", num_cpus);
    
    use alloc::vec::Vec;
    let mut stacks = Vec::new();
    
    for cpu_id in 0..num_cpus {
        // Calculate stack address for this CPU
        let stack_base = PER_CPU_STACK_BASE + (cpu_id as u64 * 0x10000); // 64KB spacing
        let stack_bottom = VirtAddr::new(stack_base);
        let stack_top = VirtAddr::new(stack_base + EMERGENCY_STACK_SIZE);
        
        // Map the stack pages
        let num_pages = (EMERGENCY_STACK_SIZE / 4096) as usize;
        for i in 0..num_pages {
            let virt_addr = stack_bottom + (i as u64 * 4096);
            
            // Allocate a physical frame
            let frame = allocate_frame()
                .ok_or("Out of memory for emergency stack")?;
            
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
        
        log::debug!("CPU {} emergency stack: {:#x} - {:#x}", 
                   cpu_id, stack_bottom, stack_top);
        
        stacks.push(PerCpuStack {
            // Stack information stored for potential future use
        });
    }
    
    log::info!("Initialized {} per-CPU emergency stacks", stacks.len());
    Ok(stacks)
}

/// Get the emergency stack for the current CPU
/// 
/// Note: This assumes CPU ID can be obtained from APIC or similar
pub fn current_cpu_emergency_stack() -> VirtAddr {
    // TODO: Get actual CPU ID from APIC
    let cpu_id = 0; // For now, assume CPU 0
    
    let stack_base = PER_CPU_STACK_BASE + (cpu_id as u64 * 0x10000);
    VirtAddr::new(stack_base + EMERGENCY_STACK_SIZE)
}