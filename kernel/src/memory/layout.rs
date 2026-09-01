//! Canonical kernel memory layout constants
//!
//! Defines the standard memory layout for kernel space, including
//! per-CPU stacks and other kernel regions. This establishes a
//! production-grade memory layout that all page tables will share.

#[cfg(target_arch = "aarch64")]
use crate::arch_impl::aarch64::constants as aarch64_const;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

// Virtual address layout constants
#[cfg(target_arch = "x86_64")]
pub const KERNEL_LOW_BASE: u64 = 0x100000; // Current low-half kernel base (1MB)
#[cfg(target_arch = "aarch64")]
pub const KERNEL_LOW_BASE: u64 = 0x40080000; // Physical load base

#[cfg(target_arch = "x86_64")]
pub const KERNEL_BASE: u64 = 0xffffffff80000000; // Upper half kernel base
#[cfg(target_arch = "aarch64")]
pub const KERNEL_BASE: u64 = aarch64_const::KERNEL_HIGHER_HALF_BASE + KERNEL_LOW_BASE;

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const HHDM_BASE: u64 = 0xffff800000000000; // Higher-half direct map
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const HHDM_BASE: u64 = aarch64_const::HHDM_BASE; // Higher-half direct map

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const PERCPU_BASE: u64 = 0xfffffe0000000000; // Per-CPU area
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const PERCPU_BASE: u64 = aarch64_const::PERCPU_BASE;

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const FIXMAP_BASE: u64 = 0xfffffd0000000000; // Fixed mappings (GDT/IDT/TSS)
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const FIXMAP_BASE: u64 = aarch64_const::FIXMAP_BASE;

#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const MMIO_BASE: u64 = 0xffffe00000000000; // MMIO regions
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const MMIO_BASE: u64 = aarch64_const::MMIO_BASE; // MMIO regions

// === User Space Memory Layout ===

/// Base of user space (1GB mark)
/// Userspace base moved to 1GB to avoid PML4[0] conflict with kernel
/// This places userspace in PDPT[1] while kernel stays in PDPT[0]
#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const USERSPACE_BASE: u64 = 0x40000000; // 1GB - avoids kernel conflict
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
///
/// Arch-generic and NOT `#[cfg]`-split: this is the single, canonical
/// definition, and `kernel::memory::vma::{MMAP_REGION_START, MMAP_REGION_END}`
/// (the constants `sys_mmap` and `Process::new`'s `mmap_hint` seeding
/// actually consume -- `kernel/src/syscall/mmap.rs`,
/// `kernel/src/process/{process.rs,manager.rs}`) re-export these same
/// values rather than redeclaring their own copy.
///
/// Before this, `vma.rs` hardcoded these exact numbers with no `#[cfg]` at
/// all -- so on aarch64 the REAL mmap allocator was already handing out
/// addresses in this x86-shaped window (confirmed live: a leftover boot
/// serial log recorded a real aarch64 userspace mapping at
/// `virt=0x7ffffdf86000`, just below `MMAP_REGION_END`), while this file
/// separately declared a second, disjoint aarch64 window
/// (`aarch64_const::MMAP_REGION_START/END`,
/// `[0x1_0000_0000, 0xFF_FE00_0000)`) that no allocator ever actually used
/// -- only `is_valid_user_range`'s mmap arm consulted it. That divergence
/// is what made every aarch64 process's first malloc'd-buffer syscall
/// (`sys_write` on `print!`'s heap-allocated `LineWriter`, since this libc's
/// `malloc` is implemented as `mmap`) refuse with EFAULT (#729 B4-a).
///
/// **What this collapse actually derives, precisely (#729 review M-1):**
/// only `MMAP_REGION_END`, the *upper* bound. `mmap_hint` -- the field both
/// producers descend from -- is seeded from `vma::MMAP_REGION_END` at five
/// call sites (`kernel/src/process/process.rs:377`,
/// `kernel/src/process/manager.rs:3375/3710/3996/4309`), so the allocators'
/// starting point and this constant are now provably the same value on
/// both arches.
///
/// `MMAP_REGION_START`, the *lower* bound, was a different story at #729
/// round-3 time: **no producer consulted it.** Both `sys_mmap`
/// (`kernel/src/syscall/mmap.rs`) and its five siblings in
/// `kernel/src/syscall/graphics.rs` (`handle_create_window_buffer`,
/// `handle_resize_window_buffer`, `handle_map_window_buffer`,
/// `handle_map_compositor_texture`, `sys_fbmmap`) each enforced their own
/// hardcoded floor, `0x1000_0000` (256 MiB) -- not this constant -- when
/// deciding whether the downward-descending `mmap_hint` had run out of
/// room. The real floor an aarch64/x86_64 process's `mmap_hint` could
/// descend to was therefore `0x1000_0000`, not `MMAP_REGION_START`
/// (`0x7000_0000_0000`) -- the same "validator anchored to a bound the
/// allocator does not use" shape that caused B4-a in the first place
/// (#742).
///
/// **Fixed by #742:** every one of those six call sites now floors against
/// this constant directly (`crate::memory::vma::MMAP_REGION_START`, the
/// same re-export `is_valid_user_range`'s mmap arm reads), so the
/// allocators and the validator agree by construction, not coincidence.
/// `sys_mmap`'s `MAP_FIXED` arm additionally gained its own
/// `[MMAP_REGION_START, MMAP_REGION_END)` region check (#742 M-2) -- before
/// that fix it accepted any page-aligned address with no region check at
/// all, so a `MAP_FIXED` mapping outside the three windows this file's
/// closed allow-list recognizes would return memory userspace could touch
/// directly but no syscall could ever accept via a user pointer.
/// `MMAP_REGION_START` is also still read by `VmaList::find_free_region`
/// (`#[allow(dead_code)]`, zero live callers) -- unchanged by this fix.
pub const MMAP_REGION_START: u64 = 0x7000_0000_0000;

/// End of mmap allocation region (gap before stack)
///
/// This bound genuinely is the allocators' own value -- see the derivation
/// note on [`MMAP_REGION_START`] above. Since #742, the same is true of
/// `MMAP_REGION_START`: every known producer seeds or floors `mmap_hint`
/// against one of these two named constants, not an independently
/// hardcoded literal.
///
/// That is a source-level fact backed by a structural ratchet
/// (`tests/mmap_floor_structure.rs`), not a mathematical proof that no
/// future producer could ever drift again (PR #744 review B2 -- the prior
/// wording here claimed "there is nowhere left for either bound ... to
/// drift apart again", which nothing enforced). The ratchet census-pins
/// every currently known producer/seed site by (file, enclosing function,
/// occurrence count) and fails the build the moment one of them stops
/// naming these constants -- but a brand-new producer written tomorrow in
/// some other shape entirely (never mentioning `MMAP_REGION_START`/`_END`
/// by name) would not be walked into that census automatically; extending
/// the pinned anchor list is how a legitimately new producer joins the law.
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

/// Maximum user stack size (2 MiB)
/// Demand-paged stack growth will not exceed this limit
pub const MAX_USER_STACK_SIZE: u64 = 2 * 1024 * 1024;

// PML4 indices for different regions
#[allow(dead_code)]
pub const BOOTSTRAP_PML4_INDEX: u64 = 3; // Bootstrap stack at 0x180000000000

// === STEP 1: Canonical per-CPU stack layout constants ===

/// Base address for the kernel higher half
#[allow(dead_code)]
#[cfg(target_arch = "x86_64")]
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;
#[allow(dead_code)]
#[cfg(target_arch = "aarch64")]
pub const KERNEL_HIGHER_HALF_BASE: u64 = aarch64_const::KERNEL_HIGHER_HALF_BASE;

/// Base address for per-CPU kernel stacks region
/// x86_64: PML4[402] = 0xffffc90000000000 - matching existing kernel stack region
/// ARM64: Uses the same virtual address (mapped appropriately)
#[cfg(target_arch = "x86_64")]
pub const PERCPU_STACK_REGION_BASE: u64 = 0xffffc90000000000;
#[cfg(target_arch = "aarch64")]
pub const PERCPU_STACK_REGION_BASE: u64 = aarch64_const::PERCPU_STACK_REGION_BASE_DEFAULT;

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
#[cfg(target_arch = "x86_64")]
pub const MAX_CPUS: usize = 256;
#[cfg(target_arch = "aarch64")]
pub const MAX_CPUS: usize = aarch64_const::MAX_CPUS;

/// Total size of virtual address space reserved for all CPU stacks
#[cfg(target_arch = "x86_64")]
pub const PERCPU_STACK_REGION_SIZE: usize = MAX_CPUS * PERCPU_STACK_STRIDE;
#[cfg(target_arch = "aarch64")]
pub const PERCPU_STACK_REGION_SIZE: usize = aarch64_const::PERCPU_STACK_REGION_SIZE;

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
    log::info!(
        "LAYOUT: percpu stack base={:#x}, size={} KiB, stride={} MiB, guard={} KiB",
        PERCPU_STACK_REGION_BASE,
        PERCPU_STACK_SIZE / 1024,
        PERCPU_STACK_STRIDE / (1024 * 1024),
        PERCPU_STACK_GUARD_SIZE / 1024
    );
    log::info!("LAYOUT: Max CPUs supported: {}", MAX_CPUS);
    log::info!(
        "LAYOUT: Total stack region size: {} MiB",
        PERCPU_STACK_REGION_SIZE / (1024 * 1024)
    );

    // Log first few CPU stack addresses as examples
    for cpu_id in 0..4.min(MAX_CPUS) {
        log::info!(
            "LAYOUT: CPU {} stack: base={:#x}, top={:#x}",
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
    log::info!(
        "KLAYOUT: Per-CPU base={:#x} size={:#x}",
        percpu_info.0,
        percpu_info.1
    );
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
/// sections reside. `USERSPACE_BASE`/`USERSPACE_CODE_DATA_END` are already
/// per-arch (via `#[cfg]` at their own declarations above), so this body
/// does not need to be duplicated per arch too -- an identical-by-coincidence
/// `#[cfg]`-split pair with no shared logic is exactly the shape that let
/// the mmap and stack arms silently diverge (#729 B4-a).
#[inline]
pub fn is_user_code_data_address(addr: u64) -> bool {
    addr >= USERSPACE_BASE && addr < USERSPACE_CODE_DATA_END
}

/// Check if an address is in userspace stack region
///
/// The stack region is in high canonical space, from USER_STACK_REGION_START to
/// USER_STACK_REGION_END. This region is separate from code/data to allow for
/// better compatibility and to avoid conflicts.
///
/// Delegates to [`is_valid_user_stack_range`] (a single-address range of
/// length 1) rather than repeating the bound logic here, so this and the
/// range form used by `copy_from_user` cannot independently drift the way
/// they did before #729 B4-a (this function was never wrong, but its
/// sibling range form was, and a hand-duplicated second copy of the same
/// window is exactly the shape that let that happen unnoticed).
#[inline]
pub fn is_user_stack_address(addr: u64) -> bool {
    is_valid_user_stack_range(addr, addr)
}

/// Check if an address is in userspace mmap region
///
/// The mmap region is where anonymous memory mappings (used by Vec, Box, etc.)
/// are placed. It spans from MMAP_REGION_START to MMAP_REGION_END, which are
/// now arch-generic (see that constant's doc comment for why this used to be
/// arch-split with no `#[cfg]` on the underlying constants at all -- the
/// actual #729 B4-a defect).
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
#[inline]
pub fn is_valid_user_address(addr: u64) -> bool {
    is_user_code_data_address(addr) || is_user_mmap_address(addr) || is_user_stack_address(addr)
}

/// Check if an address RANGE `[addr, addr+len)` lies entirely within a
/// single valid userspace region (code/data, mmap, or stack).
///
/// This is the range form of [`is_valid_user_address`]. It is what
/// `copy_from_user`/`copy_string_from_user`
/// (`kernel/src/syscall/handlers.rs`) use instead of
/// `syscall::userptr::validate_user_buffer`'s broad canonical-half bound
/// check: on x86_64 that bound (`[0, 0x0000_8000_0000_0000)`) also contains
/// the kernel's own mapped PIE image and heap.
/// `ProcessPageTable::new` (`kernel/src/memory/process_memory.rs`) copies
/// those PML4 entries into every process's page table -- without
/// `USER_ACCESSIBLE`, but the kernel itself runs at CPL 0 and does not
/// consult that bit, so a supervisor-mode read of a kernel-mapped address
/// still succeeds. A userspace-supplied pointer into either region must
/// therefore be refused by address CLASS, not merely bounds-checked against
/// "somewhere in the low canonical half" (#729 review finding B4).
///
/// This function is a closed allow-list of the three real user regions, so
/// it structurally cannot admit a kernel address regardless of where the
/// per-boot kernel PIE relocation lands (x86_64 has been observed to pick
/// more than one base across otherwise-identical boots -- see #739 /
/// `breenix-gdb-chat/scripts/gdb_chat.py`'s `KERNEL_BASE_X86` comment).
///
/// A range that spans two regions (e.g. straddling the code/data-to-mmap
/// gap) is rejected, not merged -- deliberately conservative: no legitimate
/// single userspace buffer crosses a region boundary.
///
/// `len == 0` is always accepted (nothing to validate). The compile-time
/// assertions below prove this holds for both observed x86_64 kernel PIE
/// bases, the kernel heap (both arches), and a representative user address,
/// on every build -- not just at review time.
#[inline]
pub const fn is_valid_user_range(addr: u64, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let last = match addr.checked_add(len as u64 - 1) {
        Some(l) => l,
        None => return false,
    };

    let in_code_data = addr >= USERSPACE_BASE
        && addr < USERSPACE_CODE_DATA_END
        && last >= USERSPACE_BASE
        && last < USERSPACE_CODE_DATA_END;
    let in_mmap = addr >= MMAP_REGION_START
        && addr < MMAP_REGION_END
        && last >= MMAP_REGION_START
        && last < MMAP_REGION_END;

    in_code_data || in_mmap || is_valid_user_stack_range(addr, last)
}

// x86_64: `kernel/src/memory/stack.rs`'s `find_free_virtual_space` hands out
// successive per-thread stack TOPS ascending from USER_STACK_REGION_START
// toward USER_STACK_REGION_END (so the real occupied range's upper bound is
// genuinely USER_STACK_REGION_END -- unchanged from before). Each stack's
// demand-paged growth is governed by `kernel/src/interrupts.rs`'s
// `handle_stack_growth`, which will map pages down to, and refuses to grow
// past, `USER_STACK_REGION_START.saturating_sub(MAX_USER_STACK_SIZE)` --
// the exact same constant used here, so this predicate can never be
// tighter than what the growth handler will actually map. It is
// conservative in the accepting direction rather than exact: the bound
// here is the theoretical worst case (a stack top pinned at
// USER_STACK_REGION_START itself), while the shipped `exec` paths actually
// pin `USER_STACK_TOP` at `USER_STACK_REGION_START + 0x10000` (64 KiB;
// `process/manager.rs:3300,3632`, `syscall/handlers.rs:2153`) and
// `stack.rs`'s allocator only ascends from USER_STACK_REGION_START. Every
// real `stack_top` is therefore >= USER_STACK_REGION_START, so every
// process's actual growth cap is >= this predicate's floor -- the
// predicate is never tighter than reality, just not pinned to the exact
// placement `exec` uses.
#[cfg(target_arch = "x86_64")]
#[inline]
const fn is_valid_user_stack_range(addr: u64, last: u64) -> bool {
    let region_bottom = USER_STACK_REGION_START.saturating_sub(MAX_USER_STACK_SIZE);
    addr >= region_bottom
        && addr < USER_STACK_REGION_END
        && last >= region_bottom
        && last < USER_STACK_REGION_END
}

// aarch64: there is no demand-paged stack growth on this arch (no
// `handle_stack_growth` equivalent exists in `arch_impl::aarch64`) --
// `kernel/src/process/manager.rs`'s ARM64 process-creation paths
// allocate and map a single, fully-backed `USER_STACK_SIZE` (64 KiB) stack
// per process, with its top *always* pinned at the fixed address
// USER_STACK_REGION_START (never ascending the way x86_64's does). This
// pin does NOT rest on "no two stacks are ever co-resident in one address
// space" -- a CLONE_VM thread genuinely does share its parent's address
// space and page table. It rests on `sys_clone` taking a caller-supplied
// `child_stack` (`syscall/clone.rs:59,155,164`), which comes from the
// mmap region and is covered by that arm instead -- every *non-CLONE_VM*
// process still gets its own page table with its own single 64 KiB
// window at this fixed address. USER_STACK_SIZE is
// therefore the true, complete, exact extent for that window -- not MAX_USER_STACK_SIZE
// (that constant is x86_64's growth cap; it does not describe anything
// aarch64's allocator ever actually maps). Before this, the aarch64 arm
// used a hardcoded, unexplained "1 MiB for multiple stacks" literal that
// matched neither number and was too narrow for a real process's stack
// once this predicate became load-bearing on the copy_from_user hot path
// (#729 B4-a).
#[cfg(target_arch = "aarch64")]
#[inline]
const fn is_valid_user_stack_range(addr: u64, last: u64) -> bool {
    let region_bottom = USER_STACK_REGION_START.saturating_sub(USER_STACK_SIZE as u64);
    addr >= region_bottom
        && addr < USER_STACK_REGION_START
        && last >= region_bottom
        && last < USER_STACK_REGION_START
}

// === B4 (#729 review) compile-time proof ===
//
// Proves -- on every build, both architectures, not just at review time --
// that `is_valid_user_range` refuses kernel-mapped addresses and accepts a
// real user address. This is the exact function `copy_from_user` and
// `copy_string_from_user` call, so this is a proof about the shipped path,
// not a standalone unit test of the predicate in isolation.
//
// x86_64: kernel PIE image. `ProcessPageTable::new` copies PML4[1]
// (0x0000_0080_0000_0000) and PML4[2] (0x0000_0100_0000_0000) -- the two
// bases the kernel PIE relocation has been observed to pick across
// otherwise-identical boots (#739) -- into every process page table.
#[cfg(target_arch = "x86_64")]
const _: () = assert!(
    !is_valid_user_range(0x0000_0080_0000_0000, 1),
    "x86_64 kernel PIE candidate base 0x8000000000 (PML4[1]) must be refused"
);
#[cfg(target_arch = "x86_64")]
const _: () = assert!(
    !is_valid_user_range(0x0000_0100_0000_0000, 1),
    "x86_64 kernel PIE candidate base 0x10000000000 (PML4[2]) must be refused"
);

// Both arches: kernel heap start must never validate as a user address.
// (On aarch64 the heap lives in the TTBR1-only region and a userspace
// pointer can never reach it through TTBR0 at all; this assertion still
// documents and locks the invariant at the shared-predicate level.)
const _: () = assert!(
    !is_valid_user_range(crate::memory::heap::HEAP_START, 1),
    "kernel heap start must be refused"
);

// A representative address inside the userspace code/data region -- where
// a process's brk-extended heap actually lives (sys_brk caps growth at
// USERSPACE_CODE_DATA_END, see kernel/src/syscall/memory.rs) -- must be
// accepted. This is the address class #729 was actually concerned about;
// M4's review finding confirmed it was already covered by the pre-existing
// per-byte `is_valid_user_address`, so no new heap-specific arm was needed
// here, only restoring a closed allow-list instead of a broad bound.
const _: () = assert!(
    is_valid_user_range(USERSPACE_BASE + 0x10_0000, 4096),
    "an address inside the userspace code/data (brk-heap) region must be accepted"
);

// === Anti-vacuity: ACCEPTANCE, not just refusal (#729 B4-a follow-up) ===
//
// The refusal asserts above are necessary but were not sufficient: they
// proved kernel addresses are rejected and ONE interior code/data address
// is accepted, but never asserted acceptance for the mmap or stack arms at
// all -- so both arms shipped completely unexercised by this "proof", and
// the aarch64 stack arm's too-narrow hardcoded window (and, the actual
// cause of the boot failure, the mmap arm's divergent region -- see
// `MMAP_REGION_START`'s doc comment above) sailed through every build
// undetected (#729 review finding M-c). Each assert below is anchored on
// the address the real allocator/mapper for that region actually produces
// -- with one exception, disclosed at its own assert below, not a second
// hand-picked number, so a future regression of this shape fails the build
// instead of shipping a dead init again.
//
// Honest scope note on "fails the build" (#729 review m-3, updated by
// #742/PR #744 review F6 -- the mmap arm below has since been demonstrated
// too, so this note no longer overclaims it as unverified): the aarch64
// stack-bottom assert below has been demonstrated, by mutation, to actually
// catch a regression -- halving `USER_STACK_SIZE` in its `region_bottom`
// computation reddens exactly this assert
// (`docs/planning/green-program/nic-bus/serials/sweep3-prove3/
// falsify-direction-B-narrowed-stack-build-error.txt`). The two mmap
// asserts added by #742 have likewise been demonstrated by mutation, each
// in isolation: loosening the mmap arm's lower-bound comparison by one
// (`addr >= MMAP_REGION_START - 1`) reddens only the tight-refusal assert
// below and nothing else; setting `MMAP_REGION_START = 0x7FFF_FEE0_0000`
// and `MMAP_REGION_END = 0x5000_0000` reddens only the non-empty assert
// below and nothing else (PR #744 review F3 -- the mutation on record
// before this note, `MMAP_REGION_START = MMAP_REGION_END`, also reddened
// the two acceptance asserts above it, so it did not isolate the
// non-empty assert on its own; this pair of bounds does). The x86 stack
// and code/data acceptance asserts remain sound by the same const-eval
// mechanism and the same reasoning, but as of this commit no mutation has
// been run against them specifically -- they are unverified by direct
// falsification, not yet proven the way the stack-bottom and mmap ones
// now are.
//
// mmap: anchored on `vma::MMAP_REGION_END`/`_START` by name (not
// `layout::MMAP_REGION_*`, even though they are the same definition after
// the collapse above) so this keeps asserting against the actual runtime
// consumer's own path -- if a future edit ever gives `vma` its own
// diverging copy again, THIS assert (not just a human reading the doc
// comment) fails the build.
//
// Since #742, the START assert is no longer just a re-export-integrity
// check: `sys_mmap` and all five `graphics.rs` mmap-hint producers now
// floor their downward-descending `mmap_hint` against
// `vma::MMAP_REGION_START` directly (not a second, independently
// hardcoded `0x1000_0000`). That link -- producers floor here, this
// assert accepts here -- is a source-level fact enforced by the
// structural ratchet in `tests/mmap_floor_structure.rs`, not by this
// const-eval assert itself: this assert alone still only proves a
// property of `is_valid_user_range`, exactly as it did before #742 (PR
// #744 review B2 -- the prior wording here overclaimed this assert as
// itself "a proof about the lowest address a real allocator can hand
// out", which nothing here enforces on its own). Together with that
// ratchet, the practical claim holds for every producer known today.
const _: () = assert!(
    is_valid_user_range(crate::memory::vma::MMAP_REGION_END - 1, 1),
    "the highest address the mmap allocator hands out (MMAP_REGION_END - 1) must be accepted"
);
const _: () = assert!(
    is_valid_user_range(crate::memory::vma::MMAP_REGION_START, 1),
    "the mmap region's lower bound (MMAP_REGION_START, now the allocators' real floor -- #742) must be accepted"
);
// One byte below the real producer floor must be refused -- proves the
// boundary asserted above is tight, not merely "somewhere inside is
// accepted". At `MMAP_REGION_START - 1` (0x6FFF_FFFF_FFFF) on both arches
// this falls *above* `USERSPACE_CODE_DATA_END` (0x8000_0000, 2GiB) -- not
// below it, contrary to what this comment used to say (PR #744 review
// F4) -- so it cannot be admitted by the code/data arm's own upper bound
// either. It is also below `USER_STACK_REGION_START`
// (0x7FFF_FF00_0000) by a wide margin on both arches, so the stack arm's
// lower bound can't absorb it either. With all three windows' own bounds
// ruling it out, the only way this assert can pass is if the mmap arm's
// lower bound is exactly `MMAP_REGION_START`, which is exactly what #742
// made the allocators' own floor.
const _: () = assert!(
    !is_valid_user_range(crate::memory::vma::MMAP_REGION_START - 1, 1),
    "one byte below the mmap region's real floor (MMAP_REGION_START, #742) must be refused"
);
// The mmap region must be non-empty for the floor to mean anything: if a
// future edit ever let `MMAP_REGION_START` reach or pass
// `MMAP_REGION_END`, every producer floor check above would refuse space
// unconditionally (`sys_mmap`/`graphics.rs` would ENOMEM on their very
// first allocation) rather than merely narrowing the region -- this is
// now load-bearing in a way it was not before #742, when nothing
// downstream of `MMAP_REGION_START` actually consumed it at runtime.
const _: () = assert!(
    MMAP_REGION_START < MMAP_REGION_END,
    "the mmap region (now the allocators' own [MMAP_REGION_START, MMAP_REGION_END) floor/ceiling, #742) must be non-empty"
);

// code/data: both ends of the real ELF load region, not just one interior
// point.
const _: () = assert!(
    is_valid_user_range(USERSPACE_BASE, 1),
    "the bottom of the userspace code/data region must be accepted"
);
const _: () = assert!(
    is_valid_user_range(USERSPACE_CODE_DATA_END - 1, 1),
    "the top of the userspace code/data region -- where sys_brk caps heap growth -- must be accepted"
);

// user stack: both ends of the real per-arch extent. x86_64's demand-paged
// growth handler (`kernel/src/interrupts.rs::handle_stack_growth`) will
// map down to exactly `USER_STACK_REGION_START - MAX_USER_STACK_SIZE`, and
// the ascending allocator (`kernel/src/memory/stack.rs`) can hand out a
// stack top up to `USER_STACK_REGION_END`. aarch64 has no growth handler --
// every process's stack is the fixed, fully-mapped `USER_STACK_SIZE` window
// pinned at `USER_STACK_REGION_START` (`kernel/src/process/manager.rs`).
// This is the exact address class whose bottom end was too narrow and
// refused every aarch64 process's first stack-adjacent access once this
// predicate became load-bearing on the copy_from_user hot path.
#[cfg(target_arch = "x86_64")]
const _: () = assert!(
    is_valid_user_range(USER_STACK_REGION_END - 1, 1),
    "the top of the real x86_64 user stack extent must be accepted"
);
#[cfg(target_arch = "x86_64")]
const _: () = assert!(
    is_valid_user_range(
        USER_STACK_REGION_START.saturating_sub(MAX_USER_STACK_SIZE),
        1
    ),
    "the deepest address handle_stack_growth will ever map must be accepted"
);
#[cfg(target_arch = "aarch64")]
const _: () = assert!(
    is_valid_user_range(USER_STACK_REGION_START - 1, 1),
    "the top of the real aarch64 user stack extent must be accepted"
);
#[cfg(target_arch = "aarch64")]
const _: () = assert!(
    is_valid_user_range(
        USER_STACK_REGION_START - USER_STACK_SIZE as u64,
        1
    ),
    "the bottom of the real aarch64 user stack extent (USER_STACK_SIZE) must be accepted"
);

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
