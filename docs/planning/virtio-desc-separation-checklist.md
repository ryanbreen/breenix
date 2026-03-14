# VirtIO Descriptor Separation — Bisection Checklist

## Goal
Move 3-desc commands from desc[0..2] to desc[4..6] to prevent
descriptor corruption when timed-out 3-desc commands complete late.

## Proven Facts
- Parallels VirtIO GPU supports non-zero head descriptor indices
  (Linux uses head=39, head=75 routinely — bpftrace verified)
- The previous attempt crashed between VirGL Step 9 and Step 10
- The crash was a Breenix bug, NOT a Parallels limitation

## Method
One atomic change per step. Build, deploy, verify boot completes
through VirGL Step 10 + BWM startup. If a step fails, THAT step
contains the bug. Do not proceed to the next step.

Pass criteria: Serial log shows "VirGL 3D pipeline initialized
successfully" AND BWM starts AND windows are discovered.

---

## Step 1: Baseline verification
- [ ] Build and deploy current code (desc[0..2], head=0)
- [ ] Verify full boot succeeds (Steps 1-10, BWM, windows)
- [ ] Record last_used_idx value at Step 10 entry (add temporary serial_println)
- **Change: NONE. Establishes that current code works.**

## Step 2: Change ONLY the avail ring head from 0 to 4
- [ ] In send_command_3desc: keep desc[0..2] unchanged
- [ ] Change ONLY: `avail.ring[(idx % 16) as usize] = 4;` (was 0)
- [ ] Build, deploy, check boot
- **Isolates: Does Parallels process a chain starting at desc[0]
  when the avail ring says head=4? (It shouldn't — this should FAIL
  because desc[4] is uninitialized. But if it somehow passes, we
  learn that Parallels ignores the head index.)**
- **Expected result: FAIL (device reads uninitialized desc[4])**

## Step 3: Move descriptors to [4..6], head=4, keep desc chain correct
- [ ] Write descriptors to desc[4], desc[5], desc[6]
- [ ] Set desc[4].next=5, desc[5].next=6
- [ ] Set avail.ring head=4
- [ ] Keep everything else identical (same resp buffer, same cache flush)
- [ ] Build, deploy, check boot
- **Isolates: Do desc[4..6] with head=4 work at all?**
- **If FAIL: The bug is in descriptor setup or cache flush coverage**

## Step 4: If Step 3 fails — check cache flush coverage
- [ ] The current cache flush does `dma_cache_clean(q_addr, 512)`
- [ ] desc[4] is at offset 64, desc[6] ends at offset 112 — within 512
- [ ] Add explicit per-descriptor cache flush:
      `dma_cache_clean(&desc[4], 16); dma_cache_clean(&desc[5], 16); dma_cache_clean(&desc[6], 16);`
- [ ] Build, deploy, check boot
- **Isolates: Is the bulk cache flush not covering desc[4..6]?**

## Step 5: If Step 3 passes — verify 2-desc still works after 3-desc
- [ ] Step 3 passed: 3-desc with head=4 works
- [ ] Check that the NEXT command (2-desc, head=0) also works
- [ ] This is the Step 9→10 transition that crashed before
- [ ] Add serial_println before and after Step 10's send_command
- [ ] Build, deploy, check boot
- **Isolates: Does the 2-desc path work after a 3-desc used head=4?**

## Step 6: If Step 5 fails — check used ring tracking
- [ ] After 3-desc completes with head=4, the used ring entry has id=4
- [ ] After 2-desc completes with head=0, the used ring entry has id=0
- [ ] last_used_idx tracks the used.idx counter, NOT the entry IDs
- [ ] Add serial_println in send_command showing:
      used.idx, last_used_idx, used.ring[last_used_idx % 16].id
- [ ] Build, deploy, check boot
- **Isolates: Is last_used_idx getting confused by mixed head values?**

## Step 7: If Step 5 fails — check for descriptor clobbering
- [ ] After 3-desc writes to desc[4..6], does anything overwrite desc[0..1]?
- [ ] The 3-desc cache flush covers 512 bytes — this includes desc[0..1]
- [ ] But desc[0..1] aren't being WRITTEN, only flushed
- [ ] Check if desc[0..1] have stale data from a PREVIOUS 2-desc command
- [ ] The stale desc[0].addr might point to an old cmd buffer
- [ ] Add serial_println showing desc[0].addr and desc[1].addr before
      the 2-desc send_command at Step 10
- **Isolates: Are stale descriptors causing the device to read wrong memory?**

---

## Diagnostic serial_println template
```rust
// Add temporarily at Step 10 entry point:
crate::serial_println!("[diag] pre-Step10: last_used_idx={} avail.idx={}",
    state.last_used_idx,
    unsafe { read_volatile(&(*(&raw const PCI_CTRL_QUEUE)).avail.idx) }
);
```

## Rules
1. ONE change per step
2. Build + deploy + verify after EACH step
3. If a step fails, do NOT proceed — debug THAT step
4. Record exact serial output for each step
5. No guessing. No "it must be X." Only evidence.
