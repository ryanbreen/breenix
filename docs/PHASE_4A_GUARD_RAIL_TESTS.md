# Phase 4A Guard Rail Tests

**Date**: 2025-01-14
**Purpose**: Document critical tests that should be implemented to prevent regression of Phase 4A fixes

## Overview

After successfully getting INT 0x80 syscalls working from userspace, we identified several critical fixes that must be preserved. These guard-rail tests ensure the fixes remain in place.

## Critical Tests Required

### 1. IDT DPL Test
**File**: `kernel/src/tests/syscall_guardrails.rs::test_idt_dpl_for_syscall`

**Purpose**: Verify IDT entry 0x80 has DPL=3 for user access

**What it tests**:
- IDT[0x80] descriptor has DPL (Descriptor Privilege Level) = 3
- This allows Ring 3 (userspace) to invoke INT 0x80

**Why it matters**: Without DPL=3, userspace gets General Protection Fault when trying INT 0x80

### 2. Kernel Stack Mapping Test
**File**: `kernel/src/tests/syscall_guardrails.rs::test_kernel_stack_mapped_in_process`

**Purpose**: Verify kernel stack region is mapped in process page tables

**What it tests**:
- PML4 entry 2 (idle thread stack region) is present in new process page tables
- The mapping is NOT user accessible (security requirement)

**Why it matters**: Without this mapping, timer interrupts during userspace execution cause triple fault because CPU can't push interrupt frame to kernel stack

### 3. Kernel Code Mapping Test
**File**: `kernel/src/tests/syscall_guardrails.rs::test_kernel_code_mapped_in_process`

**Purpose**: Verify kernel code is accessible after CR3 switch

**What it tests**:
- PML4 entry 0 (kernel code region 0x200000-0x400000) is present in process page tables
- The mapping is NOT user accessible (security requirement)

**Why it matters**: Without this mapping, the CPU hangs immediately after CR3 switch because it can't fetch the next instruction in the interrupt return path

### 4. Low-Half Isolation Test
**File**: `kernel/src/tests/syscall_guardrails.rs::test_low_half_isolation`

**Purpose**: Verify process isolation is maintained

**What it tests**:
- PML4 entries 1-7 are unused (for process isolation)
- High-half entries (256-511) contain kernel mappings
- No high-half entries are user accessible

**Why it matters**: Prevents one process from accessing another process's memory

## Implementation Status

The test code has been written in `kernel/src/tests/syscall_guardrails.rs` but needs to be integrated into the kernel test framework when the build system supports it.

## Manual Verification

Until automated tests are integrated, these checks can be performed manually:

1. **IDT DPL Check**: The kernel already logs this at boot:
   ```
   IDT[0x80] DPL=3 (should be 3 for user access)
   ```

2. **Page Table Mappings**: The kernel logs these during process creation:
   ```
   KSTACK_FIX: Mapped idle thread stack PML4 entry 2 for context switch support
   KERNEL_CODE_FIX: Mapped PML4 entry 0 for kernel code access (0x200000-0x400000)
   ```

3. **Syscall Success**: The integration test verifies end-to-end functionality:
   ```bash
   cargo test --package breenix --test integ_syscall_gate
   ```

## Critical Code Locations

These are the key fixes that must be preserved:

1. **IDT Setup**: `kernel/src/interrupts/mod.rs`
   - Look for: `set_privilege_level(3)` on the syscall handler

2. **Kernel Stack Mapping**: `kernel/src/memory/process_memory.rs:305-317`
   - Maps PML4 entry 2 (idle thread stack region)

3. **Kernel Code Mapping**: `kernel/src/memory/process_memory.rs:242-251`
   - Maps PML4 entry 0 (kernel code region)

## Regression Prevention

To prevent regression:

1. Never remove the PML4 entry mappings in `ProcessPageTable::new()`
2. Always keep IDT[0x80] at DPL=3
3. Run the syscall_gate integration test before merging any memory/interrupt changes
4. Look for these log messages during testing:
   - `SYSCALL_ENTRY: Received syscall from userspace!`
   - `TEST_MARKER: SYSCALL_OK`

## Conclusion

Phase 4A is complete with INT 0x80 syscalls working from userspace. The critical fixes are:
- ✅ IDT entry 0x80 set to DPL=3
- ✅ Kernel stack (PML4 entry 2) mapped in process page tables
- ✅ Kernel code (PML4 entry 0) mapped in process page tables
- ✅ Process isolation maintained (PML4 entries 1-7 unused)

These guard-rail tests ensure these fixes remain in place as the kernel evolves.