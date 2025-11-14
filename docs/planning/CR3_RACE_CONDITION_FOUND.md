# CR3 Switching Race Condition - ROOT CAUSE FOUND

**Date**: 2025-11-10
**Status**: Root cause identified, fix in progress

---

## Problem Summary

The ENOSYS test fails because userspace never executes. The kernel sets `next_cr3=0x66f000` in GS:64, but the first IRETQ to userspace doesn't switch CR3. Instead, it returns with kernel CR3 (`0x101000`), causing a page fault when a timer interrupt tries to execute kernel code on the process page table.

---

## Root Cause: Race Condition

### Timeline from Test Output

```
1. Target CR3 stored in GS:64, will be switched before IRETQ  [next_cr3 = 0x66f000]
2. Setting next_cr3=0x66f000 for thread 2 (PID 1)
3. XYZIRET#0: RIP=0x50000000 CS=0x33 ...
4. Z[ INFO] kernel::syscall::handler: R3-IRET #1: ... cr3=0x101000  [Z marker, then trace function]
5. Q$CR3$R3-TIMER #1: ... cr3=0x66f000  [NEW timer interrupt!]
6. PF @ 0x444444449c48 Error: 0x0  [Page fault in kernel code]
```

### The Race Condition

In `timer_entry.asm`, the code path to IRETQ is:

1. **Line 170**: Output 'Z' marker
2. **Line 250**: `swapgs` (switch to kernel GS)
3. **Line 265**: `call trace_iretq_to_ring3` (Rust function - **INTERRUPTS ENABLED**)
4. **Line 278**: `swapgs` (switch back to user GS)
5. **Line 285**: Output 'Q' marker
6. **Line 297**: Read `next_cr3` from GS:64
7. **Line 322**: Switch CR3 if non-zero
8. **Line 339**: `iretq`

**The Problem**: Between steps 3-5, **interrupts are enabled**, so a new timer interrupt can fire before we reach the CR3 switching logic. This is exactly what happened:

- First timer interrupt return: Outputs 'Z', calls trace function, but gets interrupted by NEW timer interrupt before reaching 'Q'
- Second timer interrupt: This one sees 'Q', reads GS:64, finds `next_cr3=0x66f000`, switches CR3, and tries to IRETQ
- **But now we're running kernel interrupt handler code on the process page table**, causing immediate page fault at `0x444444449c48`

---

## Why This Happens

1. Timer interrupts are happening very frequently (multiple per millisecond)
2. The trace function (`trace_iretq_to_ring3`) takes time to execute (logging, CR3 reads, etc.)
3. Interrupts are not disabled during this period
4. A new timer interrupt fires before the first one can complete its IRETQ and switch CR3

---

## Solution Options

### Option 1: Disable Interrupts Before Trace Function (RECOMMENDED)

Move the `CLI` instruction to **before** the trace function call:

```asm
; After swapgs to user GS
mov al, 'Z'
out dx, al

; CRITICAL: Disable interrupts NOW, before any Rust calls
cli

; Swap back to kernel GS for trace function
swapgs

; Call trace function (safe now, interrupts disabled)
call trace_iretq_to_ring3

; Swap back to user GS
swapgs

; Continue with CR3 switching (interrupts still disabled)
mov rax, qword [gs:64]
test rax, rax
jz .no_cr3_switch

; Switch CR3
mov cr3, rax
mov qword [gs:64], 0

.no_cr3_switch:
; IRETQ will re-enable interrupts from saved RFLAGS
iretq
```

**Pros:**
- Simple fix
- Ensures no interrupts between setting next_cr3 and IRETQ
- Trace function still works correctly

**Cons:**
- Interrupts disabled slightly longer (adds ~few microseconds)

### Option 2: Move CR3 Switch Before Trace Function

Check and switch CR3 BEFORE calling the trace function:

```asm
; Check for CR3 switch FIRST
mov rax, qword [gs:64]
test rax, rax
jz .no_early_cr3_switch

; Disable interrupts and switch CR3 early
cli
mov cr3, rax
mov qword [gs:64], 0

.no_early_cr3_switch:
; Now do trace function (on correct CR3)
swapgs
call trace_iretq_to_ring3
swapgs

; IRETQ
iretq
```

**Pros:**
- Trace function sees the correct CR3 value in logs
- Minimizes time with wrong CR3

**Cons:**
- More complex assembly flow
- Still needs CLI before CR3 switch

### Option 3: Remove Trace Function (NOT RECOMMENDED)

Simply remove the trace function call to eliminate the window where interrupts can fire.

**Pros:**
- Simplest change

**Cons:**
- Loses valuable debugging information
- Doesn't solve the fundamental race condition

---

## Recommended Fix

**Implement Option 1**: Add `CLI` instruction immediately after the 'Z' marker and before swapping to kernel GS. This ensures:

1. Interrupts are disabled before any Rust code runs
2. No timer interrupts can fire between setting next_cr3 and IRETQ
3. CR3 switch happens atomically with respect to interrupts
4. IRETQ re-enables interrupts from saved RFLAGS (IF=1)

---

## Files to Modify

1. **kernel/src/interrupts/timer_entry.asm**
   - Move `cli` instruction from line 306 to immediately after line 170 (Z marker)
   - Remove the later `cli` at line 306 (now redundant)

---

## Testing Plan

1. Apply the fix to timer_entry.asm
2. Run `cargo run --package xtask -- ring3-enosys`
3. Verify:
   - 'Q' marker appears after 'Z' marker (no interruption)
   - '$CR3$' marker appears (CR3 switch happens)
   - No page fault at 0x444444449c48
   - "ENOSYS OK" appears in output (userspace executes successfully)

---

## Additional Notes

This same race condition could occur in:
- `syscall/entry.asm` - syscall return path
- Any other code path that sets next_cr3 and expects assembly to switch it before IRETQ

All these paths should be reviewed and fixed similarly.
