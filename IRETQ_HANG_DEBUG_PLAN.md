# IRETQ Hang Debug Plan

## Current Status
✅ Kernel stack mapping issue SOLVED - CR3 switch works
🔴 Kernel hangs when attempting IRETQ to userspace

## Symptoms
- CR3 successfully switches to process page table (0x66b000)
- Kernel continues executing on kernel stack (0xffffc9000000f1a0)
- Execution reaches end of `restore_userspace_thread_context()`
- System hangs - no IRETQ log message observed
- No double fault or triple fault

## Cursor's Initial Assessment
The IRETQ hang is likely related to:
1. User IRET frame setup issues
2. Segment selectors (CS/SS) configuration
3. RFLAGS settings
4. Missing user code/stack mappings

## Debug Plan (To Validate with Cursor)

### Phase 1: Verify IRET Frame Setup
- [ ] Log the complete IRET frame before attempting IRETQ
  - RIP (should be 0x10000000 for hello_world)
  - CS (should be 0x33 for ring 3 code)
  - RFLAGS (check IF bit, IOPL, etc.)
  - RSP (should be user stack ~0x7fffff011008)
  - SS (should be 0x2b for ring 3 data)
- [ ] Verify frame is at correct stack location
- [ ] Check stack alignment (16-byte aligned?)

### Phase 2: Verify User Mappings
- [ ] Confirm user code is mapped at 0x10000000
  - Check PML4[0] exists in process page table
  - Verify page is USER_ACCESSIBLE
  - Verify page is not NO_EXECUTE
- [ ] Confirm user stack is mapped at 0x7fffff000000
  - Check proper USER_ACCESSIBLE flags
  - Verify WRITABLE flag set

### Phase 3: Segment Descriptor Verification
- [ ] Verify GDT entries for user segments
  - CS selector 0x33 → valid ring 3 code segment
  - SS selector 0x2b → valid ring 3 data segment
- [ ] Check segment limits and base addresses
- [ ] Verify DPL = 3 for user segments

### Phase 4: Assembly-Level Debug
- [ ] Add logging immediately before IRETQ instruction
- [ ] Check RSP points to valid IRET frame
- [ ] Verify interrupts state (should be disabled)
- [ ] Consider using QEMU monitor to inspect CPU state

### Phase 5: Common IRETQ Issues to Check
- [ ] Stack pointer (RSP) must point to valid IRET frame
- [ ] All 5 values must be on stack: RIP, CS, RFLAGS, RSP, SS
- [ ] CS and SS must be valid ring 3 selectors
- [ ] Target RIP must be in executable, user-accessible page
- [ ] Target RSP must be in writable, user-accessible page
- [ ] RFLAGS must not have reserved bits set incorrectly

## Questions for Cursor
1. What's the most common cause of IRETQ hangs in your experience?
2. Should we add explicit checks before IRETQ to validate the frame?
3. Is there a way to detect if IRETQ executed but faulted immediately?
4. Could this be a CPL/privilege level mismatch issue?
5. Should we try with interrupts enabled (IF=1) in RFLAGS?

## Next Steps
1. Implement comprehensive IRET frame logging
2. Consult with Cursor on the debug plan
3. Add diagnostics based on Cursor's guidance
4. Systematically verify each component
5. Fix the root cause preventing IRETQ completion

## Success Criteria
- IRETQ completes without hanging
- Userspace code begins execution at 0x10000000
- "Hello from userspace!" message appears in logs
- System call 0x80 executed from userspace
- Clean return to kernel via syscall