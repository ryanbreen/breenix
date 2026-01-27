
//! Canonical kernel memory layout constants
//! 
//! Defines the standard memory layout for kernel space, including
//! per-CPU stacks and other kernel regions. This establishes a
//! production-grade memory layout that all page tables will share.

#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;
#[cfg(target_arch = "aarch64")]
use crate::arch_impl::aarch64::constants as aarch64_const;

// Virtual address layout constants
#[cfg(target_arch = "x86_64")]
pub const KERNEL_LOW_BASE: u64 = 0x100000;           // Current low-half kernel base (1MB)
#[cfg(target_arch = "aarch64")]
pub const KERNEL_LOW_BASE: u64 = 0x40080000;         // Physical load base

#[cfg(target_arch = "x86_64")]
pub const KERNEL_BASE: u64 = 0xffffffff80000000;     // Upper half kernel base
#[cfg(target_arch = "aarch64")]
pub const KERNEL_BASE: u64 = aarch64_const::KERNEL_HIGHER_HALF_BASE + KERNEL_LOW_BASE;

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const HHDM_BASE: u64 = 0xffff800000000000;       // Higher-half direct map
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const HHDM_BASE: u64 = aarch64_const::HHDM_BASE;  // Higher-half direct map

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const PERCPU_BASE: u64 = 0xfffffe0000000000;     // Per-CPU area
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const PERCPU_BASE: u64 = aarch64_const::PERCPU_BASE;

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const FIXMAP_BASE: u64 = 0xfffffd0000000000;     // Fixed mappings (GDT/IDT/TSS)
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const FIXMAP_BASE: u64 = aarch64_const::FIXMAP_BASE;

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const MMIO_BASE: u64 = 0xffffe00000000000;       // MMIO regions
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const MMIO_BASE: u64 = aarch64_const::MMIO_BASE;  // MMIO regions

// === User Space Memory Layout ===

/// Base of user space (1GB mark)
/// Userspace base moved to 1GB to avoid PML4[0] conflict with kernel
/// This places userspace in PDPT[1] while kernel stays in PDPT[0]
#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const USERSPACE_BASE: u64 = 0x40000000;          // 1GB - avoids kernel conflict
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const USERSPACE_BASE: u64 = aarch64_const::USERSPACE_BASE;

/// End of user code/data region (2GB)
/// This defines the upper boundary of the region where user programs' code and data
/// can be loaded. The stack lives in a separate, higher region.
#[cfg(target_arch = "x86_64")]
pub const USERSPACE_CODE_DATA_END: u64 = 0x80000000;
#[cfg(target_arch = "aarch64")]
pub const USERSPACE_CODE_DATA_END: u64 = 0x0000_0000_8000_0000;

/// Start of mmap allocation region (below stack)
/// This is where anonymous mmap allocations (used by Rust's Vec/Box) are placed.
/// The region grows downward from MMAP_REGION_END toward MMAP_REGION_START.
pub const MMAP_REGION_START: u64 = 0x7000_0000_0000;

/// End of mmap allocation region (gap before stack)
pub const MMAP_REGION_END: u64 = 0x7FFF_FE00_0000;

/// User stack allocation region start (high canonical space)
/// User stacks are allocated in this high canonical range for better compatibility
/// with different QEMU configurations and to avoid conflicts with code/data region
#[cfg(target_arch = "x86_64")]
pub const USER_STACK_REGION_START: u64 = 0x7FFF_FF00_0000;
#[cfg(target_arch = "aarch64")]
pub const USER_STACK_REGION_START: u64 = aarch64_const::USER_STACK_REGION_START;

/// User stack allocation region end (canonical boundary)
/// This is the top of the lower-half canonical address space, just before
/// the non-canonical hole that separates user and kernel space
#[cfg(target_arch = "x86_64")]
pub const USER_STACK_REGION_END: u64 = 0x8000_0000_0000;
#[cfg(target_arch = "aarch64")]
pub const USER_STACK_REGION_END: u64 = aarch64_const::USER_STACK_REGION_END;

/// Default user stack size (64 KiB)
/// This is the standard size allocated for user process stacks
#[allow(dead_code)]
pub const USER_STACK_SIZE: usize = 64 * 1024;

// PML4 indices for different regions
#[allow(dead_code)]
pub const BOOTSTRAP_PML4_INDEX: u64 = 3;             // Bootstrap stack at 0x180000000000

// === STEP 1: Canonical per-CPU stack layout constants ===

/// Base address for the kernel higher half
#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const KERNEL_HIGHER_HALF_BASE: u64 = aarch64_const::KERNEL_HIGHER_HALF_BASE;

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

/// Base address for kernel TLS (Thread-Local Storage) allocation
/// This is placed within the same PML4 entry AND same PDPT entry as per-CPU stacks.
/// Using offset 768 MiB keeps us within PDPT[0] (0-1GiB range) where page tables
/// already exist from stack allocation.
/// Layout within PML4[402], PDPT[0]:
/// - 0x00000000..0x20000000 (512 MiB): Per-CPU stacks (256 CPUs * 2 MiB)
/// - 0x20000000..0x30000000 (256 MiB): Dynamic kernel stacks
/// - 0x30000000..0x40000000 (256 MiB): TLS blocks
pub const KERNEL_TLS_REGION_BASE: u64 = PERCPU_STACK_REGION_BASE + 0x3000_0000; // +768 MiB

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
#[allow(dead_code)]
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

/// Check if an address is in the bootstrap stack region
#[allow(dead_code)]
#[inline]
pub fn is_bootstrap_address(addr: VirtAddr) -> bool {
    let pml4_index = (addr.as_u64() >> 39) & 0x1FF;
    pml4_index == BOOTSTRAP_PML4_INDEX
}

/// Convert a low-half kernel address to its high-half alias
#[allow(dead_code)]
#[inline]
pub fn high_alias_from_low(low: u64) -> u64 {
    // Kernel is currently at 0x100000, will be aliased at 0xffffffff80000000
    low - KERNEL_LOW_BASE + KERNEL_BASE
}

// Get kernel section addresses
// TODO: Phase 3 will provide real symbols via linker script
// For now, we use approximate values based on typical kernel layout
#[allow(dead_code)]
pub fn get_kernel_image_range() -> (usize, usize) {
    // Kernel is currently loaded at 0x100000 (1MB)
    // Typical kernel size is under 2MB
    (0x100000, 0x300000)
}

#[allow(dead_code)]
pub fn get_kernel_text_range() -> (usize, usize) {
    // Text section starts at kernel base
    (0x100000, 0x200000)
}

#[allow(dead_code)]
pub fn get_kernel_rodata_range() -> (usize, usize) {
    // Read-only data follows text
    (0x200000, 0x250000)
}

#[allow(dead_code)]
pub fn get_kernel_data_range() -> (usize, usize) {
    // Data section
    (0x250000, 0x280000)
}

#[allow(dead_code)]
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

/// Log GDT, IDT, TSS, and per-CPU information (x86_64 only)
#[cfg(target_arch = "x86_64")]
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

/// Log control structures (ARM64 - minimal implementation)
#[cfg(target_arch = "aarch64")]
fn log_control_structures() {
    log::info!("KLAYOUT: ARM64 - using exception vectors and TPIDR_EL1 for per-CPU");
}

// === User Space Address Validation Functions ===

/// Check if an address is in userspace code/data region
///
/// The code/data region spans from USERSPACE_BASE (1GB) to USERSPACE_CODE_DATA_END (2GB).
/// This is where ELF programs are loaded and where their .text, .data, .rodata, and .bss
/// sections reside.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn is_user_code_data_address(addr: u64) -> bool {
    addr >= USERSPACE_BASE && addr < USERSPACE_CODE_DATA_END
}

#[cfg(target_arch = "aarch64")]
#[inline]
pub fn is_user_code_data_address(addr: u64) -> bool {
    addr >= USERSPACE_BASE && addr < USERSPACE_CODE_DATA_END
}

/// Check if an address is in userspace stack region
///
/// The stack region is in high canonical space, from USER_STACK_REGION_START to
/// USER_STACK_REGION_END. This region is separate from code/data to allow for
/// better compatibility and to avoid conflicts.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn is_user_stack_address(addr: u64) -> bool {
    addr >= USER_STACK_REGION_START && addr < USER_STACK_REGION_END
}

// ARM64: stack is in high user range (lower half canonical space).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn is_user_stack_address(addr: u64) -> bool {
    addr >= USER_STACK_REGION_START && addr < USER_STACK_REGION_END
}

/// Check if an address is in userspace mmap region
///
/// The mmap region is where anonymous memory mappings (used by Vec, Box, etc.)
/// are placed. It spans from MMAP_REGION_START to MMAP_REGION_END.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn is_user_mmap_address(addr: u64) -> bool {
    addr >= MMAP_REGION_START && addr < MMAP_REGION_END
}

// ARM64: mmap region is in the lower half, below the stack.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn is_user_mmap_address(addr: u64) -> bool {
    addr >= MMAP_REGION_START && addr < MMAP_REGION_END
}

/// Check if an address is in ANY valid userspace region
///
/// This validates that an address falls within either the code/data region,
/// the mmap region, or the stack region. Any other address is considered
/// invalid for userspace access.
///
/// Note: This only checks that the address is in a valid region - it does NOT
/// verify that the specific page is mapped. Accessing an unmapped address in
/// a valid region will cause a page fault, which is the correct behavior.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn is_valid_user_address(addr: u64) -> bool {
    is_user_code_data_address(addr) || is_user_mmap_address(addr) || is_user_stack_address(addr)
}

#[cfg(target_arch = "aarch64")]
#[inline]
pub fn is_valid_user_address(addr: u64) -> bool {
    is_user_code_data_address(addr) || is_user_mmap_address(addr) || is_user_stack_address(addr)
}

// === Compile-time Layout Assertions ===

/// Verify that user regions don't overlap
/// This compile-time check ensures our memory layout is consistent
const _: () = assert!(
    USERSPACE_CODE_DATA_END <= MMAP_REGION_START,
    "User code/data region overlaps with mmap region!"
);

const _: () = assert!(
    MMAP_REGION_END <= USER_STACK_REGION_START,
    "Mmap region overlaps with stack region!"
);
