# Kernel Page Table Architecture Fix - Operational Plan

## Problem Statement
- Kernel stacks are mapped early in boot to current PML4
- Master kernel PML4 is created LATER by copying entries  
- Process page tables inherit from master but don't have actual kernel stack page mappings
- CR3 switch causes hang because kernel stack isn't accessible

## Root Cause
The kernel stacks are mapped only in the early/boot PML4. The "master" kernel PML4 is built later by copying entries but omits the actual kernel stack mappings. Process PML4s inherit from master and therefore don't see kernel stacks.

## Phase 1 - Minimal Production-Grade Fix to Reach Ring 3

### Step 1: Establish Canonical Kernel Layout ✅
**Files:** `kernel/src/memory/layout.rs`

**Implement:**
- [x] Define constants for higher-half layout:
  - `KERNEL_HIGHER_HALF_BASE` = 0xFFFF_8000_0000_0000
  - `PERCPU_STACK_REGION_BASE` = 0xffffc90000000000
  - `PERCPU_STACK_SIZE` = 32 KiB
  - `PERCPU_STACK_GUARD_SIZE` = 4 KiB
  - `PERCPU_STACK_STRIDE` = 2 MiB
- [x] Reserve contiguous higher-half region for 256 CPU_MAX kernel stacks with guard pages

**Validation:**
- [x] Boot and confirm layout log appears
- [x] Check logs: `LAYOUT: percpu stack base=0xffffc90000000000, size=32 KiB, stride=2 MiB, guard=4 KiB`

**Status:** COMPLETE - Layout constants established and logging verified

### Step 2: Build Real Master Kernel PML4 with Stacks Mapped ✅
**Files:** `kernel/src/memory/kernel_page_table.rs`

**Implement:**
- [x] Create `build_master_kernel_pml4()` that:
  - Allocates fresh PML4
  - Copies existing kernel mappings
  - Verifies kernel stack region
- [x] **FIXED**: Explicitly allocate and map per-CPU kernel stacks in master PML4
  - Allocates frames and creates full page table hierarchy
  - Maps CPU 0's stack pages with GLOBAL flag
- [x] Ensure GLOBAL flag is set on kernel stack mappings

**Validation:**
- [x] Confirm master PT creation logs
- [x] Check stack mapping logs show stacks actually mapped:
  - "STEP 2: Allocated PD for kernel stacks at frame PhysFrame[4KiB](0x54c000)"
  - "STEP 2: Allocated PT for kernel stacks at frame PhysFrame[4KiB](0x54d000)"
  - "STEP 2: Mapping 8 pages for CPU 0 kernel stack"
  - "STEP 2: Successfully mapped CPU 0 kernel stack pages"

**Status:** COMPLETE - Master PML4 now explicitly maps kernel stacks

### Step 3: Switch CR3 to Master Kernel PML4 🚧
**Files:** `kernel_page_table.rs`, `main.rs`

**Implement:**
- [x] After building master PML4, switch CR3 to it
- [x] Verify kernel stack mapping with safe probe
- [x] Add logs for CR3 switch and verification

**Issue Found:** Kernel hangs after CR3 switch when verifying stack
- Current stack at 0x180000125c0 is bootstrap stack (PML4[3])
- We preserved PML4[3] but it may not have actual stack pages mapped
- Need to ensure bootstrap stack is fully mapped before switching

**Validation:**
- [ ] Boot continues without page faults
- [x] Logs show CR3 switch (but then hangs)

### Step 4: Set TSS.rsp0 to Shared Higher-Half Stack ⬜
**Files:** `gdt.rs`, `per_cpu.rs`

**Implement:**
- [ ] Compute percpu_stack_top(cpu_id) from layout
- [ ] Set tss.rsp0 accordingly
- [ ] Ensure this happens after master PML4 active

**Validation:**
- [ ] Check logs: `GDT: TSS.rsp0 set cpu=N rsp0=0x...`

### Step 5: Process PML4 Inherits Kernel Higher-Half ⬜
**Files:** `process_memory.rs`, `process/creation.rs`

**Implement:**
- [ ] In create_process_address_space():
  - Allocate fresh PML4 for process
  - For kernel higher-half: link to SAME tables as master (not copies)
  - Do not re-map kernel stacks (already in shared half)

**Validation:**
- [ ] Process creation logs show inherited kernel mappings
- [ ] Kernel stack region confirmed present

### Step 6: Add Pre-Switch Assertions ⬜
**Files:** `scheduler.rs`, `thread.rs`

**Implement:**
- [ ] Before CR3 switch, assert kernel stack is mapped in target
- [ ] Page-table walk for percpu_stack_top
- [ ] Panic if missing with clear error

**Validation:**
- [ ] Logs show assertion checks passing
- [ ] No abort messages

### Step 7: Instrument Syscall/IRQ Entry ⬜
**Files:** `syscall/entry.asm`, `interrupts.rs`

**Implement:**
- [ ] Log on first entry from ring 3
- [ ] Confirm we're on higher-half kernel stack
- [ ] Log user RSP, kernel RSP, TSS.rsp0

**Validation:**
- [ ] See syscall entry/exit logs
- [ ] Kernel RSP matches expected TSS.rsp0

### Step 8: Page Fault Logging ⬜
**Files:** `interrupts.rs`, `memory/paging.rs`

**Implement:**
- [ ] On page fault, log CR2, error code
- [ ] Add PT walk dump for faulting VA

**Validation:**
- [ ] If page faults occur, detailed logs available

### Step 9: Ring 3 Smoke Test ⬜
**Files:** `process/creation.rs`, `test_exec.rs`

**Implement:**
- [ ] Launch tiny userspace program
- [ ] Execute instructions, trigger syscall
- [ ] Print "Hello from userspace!"

**Validation:**
- [ ] ✅ See "Hello from userspace!" in logs
- [ ] No crashes or hangs

## Progress Tracking

### Current Status: Step 2 - COMPLETE ✅
### Next Action: Step 3 - Switch CR3 to master kernel PML4

## Validation Checkpoints
After each step, we will:
1. Run the kernel
2. Check specific log outputs
3. Consult Cursor if issues arise
4. Only proceed to next step after validation passes