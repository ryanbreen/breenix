
//! Canonical kernel memory layout constants
//! 
//! Defines the standard memory layout for kernel space, including
//! per-CPU stacks and other kernel regions. This establishes a
//! production-grade memory layout that all page tables will share.

use x86_64::VirtAddr;

// Virtual address layout constants
pub const KERNEL_LOW_BASE: u64 = 0x100000;           // Current low-half kernel base (1MB)
pub const KERNEL_BASE: u64 = 0xffffffff80000000;     // Upper half kernel base
pub const HHDM_BASE: u64 = 0xffff800000000000;       // Higher-half direct map
pub const PERCPU_BASE: u64 = 0xfffffe0000000000;     // Per-CPU area
pub const FIXMAP_BASE: u64 = 0xfffffd0000000000;     // Fixed mappings (GDT/IDT/TSS)
pub const MMIO_BASE: u64 = 0xffffe00000000000;       // MMIO regions

// PML4 indices for different regions
pub const KERNEL_PML4_INDEX: u64 = 402;              // Kernel stacks at 0xffffc90000000000
pub const BOOTSTRAP_PML4_INDEX: u64 = 3;             // Bootstrap stack at 0x180000000000

// === STEP 1: Canonical per-CPU stack layout constants ===

/// Base address for the kernel higher half
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;

/// Base address for per-CPU kernel stacks region
/// This is at PML4[402] = 0xffffc90000000000 - matching existing kernel stack region
pub const PERCPU_STACK_REGION_BASE: u64 = 0xffffc90000000000;

/// Size of each per-CPU kernel stack (32 KiB)
/// This is sufficient for kernel operations including interrupt handling
pub const PERCPU_STACK_SIZE: usize = 32 * 1024; // 32 KiB

/// Size of guard page between stacks (4 KiB)
/// Guard pages prevent stack overflow from corrupting adjacent stacks
pub const PERCPU_STACK_GUARD_SIZE: usize = 4 * 1024; // 4 KiB

/// Stride between per-CPU stack regions (2 MiB aligned)
/// Aligning to 2 MiB allows potential huge page optimizations
/// Each CPU gets: stack + guard + padding to reach 2 MiB
pub const PERCPU_STACK_STRIDE: usize = 2 * 1024 * 1024; // 2 MiB

/// Maximum number of CPUs supported
/// This determines how much virtual address space to reserve for stacks
pub const MAX_CPUS: usize = 256;

/// Total size of virtual address space reserved for all CPU stacks
pub const PERCPU_STACK_REGION_SIZE: usize = MAX_CPUS * PERCPU_STACK_STRIDE;

/// Calculate the virtual address for a specific CPU's stack region
/// 
/// Returns the base address of the stack region for the given CPU.
/// The actual stack grows downward from (base + PERCPU_STACK_SIZE).
pub fn percpu_stack_base(cpu_id: usize) -> VirtAddr {
    assert!(cpu_id < MAX_CPUS, "CPU ID {} exceeds MAX_CPUS", cpu_id);
    let offset = cpu_id * PERCPU_STACK_STRIDE;
    VirtAddr::new(PERCPU_STACK_REGION_BASE + offset as u64)
}

/// Calculate the top of the stack for a specific CPU (where RSP starts)
/// 
/// The stack grows downward, so the top is at base + size
pub fn percpu_stack_top(cpu_id: usize) -> VirtAddr {
    let base = percpu_stack_base(cpu_id);
    base + PERCPU_STACK_SIZE as u64
}

/// Get the guard page address for a specific CPU's stack
/// 
/// The guard page is placed immediately after the stack (at lower addresses)
/// to catch stack overflows
pub fn percpu_stack_guard(cpu_id: usize) -> VirtAddr {
    let base = percpu_stack_base(cpu_id);
    base - PERCPU_STACK_GUARD_SIZE as u64
}

/// Log the memory layout during initialization (STEP 1 validation)
pub fn log_layout() {
    log::info!("LAYOUT: Kernel memory layout initialized:");
    log::info!("LAYOUT: percpu stack base={:#x}, size={} KiB, stride={} MiB, guard={} KiB",
        PERCPU_STACK_REGION_BASE,
        PERCPU_STACK_SIZE / 1024,
        PERCPU_STACK_STRIDE / (1024 * 1024),
        PERCPU_STACK_GUARD_SIZE / 1024
    );
    log::info!("LAYOUT: Max CPUs supported: {}", MAX_CPUS);
    log::info!("LAYOUT: Total stack region size: {} MiB", PERCPU_STACK_REGION_SIZE / (1024 * 1024));
    
    // Log first few CPU stack addresses as examples
    for cpu_id in 0..4.min(MAX_CPUS) {
        log::info!("LAYOUT: CPU {} stack: base={:#x}, top={:#x}",
            cpu_id,
            percpu_stack_base(cpu_id).as_u64(),
            percpu_stack_top(cpu_id).as_u64()
        );
    }
}

/// Check if an address is in the kernel's upper-half region
#[inline]
pub fn is_kernel_address(addr: x86_64::VirtAddr) -> bool {
    let pml4_index = (addr.as_u64() >> 39) & 0x1FF;
    pml4_index == KERNEL_PML4_INDEX
}

/// Check if an address is in the bootstrap stack region
#[inline]
pub fn is_bootstrap_address(addr: x86_64::VirtAddr) -> bool {
    let pml4_index = (addr.as_u64() >> 39) & 0x1FF;
    pml4_index == BOOTSTRAP_PML4_INDEX
}

/// Convert a low-half kernel address to its high-half alias
#[inline]
pub fn high_alias_from_low(low: u64) -> u64 {
    // Kernel is currently at 0x100000, will be aliased at 0xffffffff80000000
    low - KERNEL_LOW_BASE + KERNEL_BASE
}

// Get kernel section addresses
// TODO: Phase 3 will provide real symbols via linker script
// For now, we use approximate values based on typical kernel layout
pub fn get_kernel_image_range() -> (usize, usize) {
    // Kernel is currently loaded at 0x100000 (1MB)
    // Typical kernel size is under 2MB
    (0x100000, 0x300000)
}

pub fn get_kernel_text_range() -> (usize, usize) {
    // Text section starts at kernel base
    (0x100000, 0x200000)
}

pub fn get_kernel_rodata_range() -> (usize, usize) {
    // Read-only data follows text
    (0x200000, 0x250000)
}

pub fn get_kernel_data_range() -> (usize, usize) {
    // Data section
    (0x250000, 0x280000)
}

pub fn get_kernel_bss_range() -> (usize, usize) {
    // BSS section at end
    (0x280000, 0x300000)
}

/// Log kernel layout information (Phase 0)
pub fn log_kernel_layout() {
    let (image_start, image_end) = get_kernel_image_range();
    let (text_start, text_end) = get_kernel_text_range();
    let (rodata_start, rodata_end) = get_kernel_rodata_range();
    let (data_start, data_end) = get_kernel_data_range();
    let (bss_start, bss_end) = get_kernel_bss_range();
    
    log::info!(
        "KLAYOUT: image={:#x}..{:#x} text={:#x}..{:#x} rodata={:#x}..{:#x} data={:#x}..{:#x} bss={:#x}..{:#x}",
        image_start, image_end,
        text_start, text_end,
        rodata_start, rodata_end,
        data_start, data_end,
        bss_start, bss_end
    );
    
    // Log other critical structures
    log_control_structures();
}

/// Log GDT, IDT, TSS, and per-CPU information
fn log_control_structures() {
    use crate::gdt;
    use crate::interrupts;
    use crate::per_cpu;
    
    // Get GDT info
    let gdt_info = gdt::get_gdt_info();
    log::info!("KLAYOUT: GDT base={:#x} limit={}", gdt_info.0, gdt_info.1);
    
    // Get IDT info  
    let idt_info = interrupts::get_idt_info();
    log::info!("KLAYOUT: IDT base={:#x} limit={}", idt_info.0, idt_info.1);
    
    // Get TSS info
    let tss_info = gdt::get_tss_info();
    log::info!("KLAYOUT: TSS base={:#x} RSP0={:#x}", tss_info.0, tss_info.1);
    
    // Get per-CPU info
    let percpu_info = per_cpu::get_percpu_info();
    log::info!("KLAYOUT: Per-CPU base={:#x} size={:#x}", percpu_info.0, percpu_info.1);
}