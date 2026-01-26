# ARM64 Memory Layout Diff (High-Half Kernel Transition)

## Current Source of Truth (after changes)
- Boot path now enables MMU in `kernel/src/arch_impl/aarch64/boot.S` with TTBR0/TTBR1 split.
- High-half direct map (HHDM) base: `0xFFFF_0000_0000_0000`.
- Kernel is linked into the HHDM (VMA = HHDM_BASE + physical).
- Identity-map MMU setup in `kernel/src/arch_impl/aarch64/mmu.rs` is now **legacy** and skipped if MMU is already on.

## User/Kernel VA Model
- **Userspace (TTBR0)**: lower-half canonical range
  - Code/data: `0x0000_0000_4000_0000 .. 0x0000_0000_8000_0000`
  - Mmap: `0x0000_7000_0000_0000 .. 0x0000_7FFF_FE00_0000`
  - Stack: `0x0000_FFFF_FF00_0000 .. 0x0001_0000_0000_0000`
- **Kernel (TTBR1)**: high-half direct map (HHDM)
  - `virt = HHDM_BASE + phys`

## Mismatches (now reduced)
1) Legacy identity-map assumptions still exist in drivers and allocators.
2) Some ARM64 subsystems still use low physical addresses directly instead of HHDM conversions.

## Status
- **Fixed**: `memory/layout.rs` and `syscall/userptr.rs` now use ARM64-specific layout bounds.
- **Fixed**: `init_physical_memory_offset_aarch64()` now uses HHDM base.
- **Fixed**: VirtIO MMIO drivers now convert queue/buffer addresses to physical via HHDM offset.
- **Remaining**:
  - Remove remaining identity-map assumptions (kernel stack allocator, device drivers, misc helpers).
  - Ensure TTBR0 is updated to real user page tables once userspace runs.

## Next Actions
1) Audit remaining identity-map assumptions and convert to HHDM.
2) Ensure all MMIO accesses use `phys -> virt` conversion via HHDM.
3) Validate that user page tables are populated and TTBR0 is switched on context switch.
