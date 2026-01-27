# AMD64 sigaltstack failure: what I need

## Context
- Boot stages fail at [72/258] `sigaltstack() syscall verified`.
- Kernel panic reported at `kernel/src/interrupts.rs:1284` (kernel page fault branch).
- Static analysis suggests SA_ONSTACK may restore the wrong stack pointer in the signal frame on AMD64.

## Evidence needed
- The exact fault line from the log that includes:
  - Faulting virtual address
  - Error code
  - RIP and CR3 (if logged)
- Full failure context around the first `sigaltstack` stage (from CI or local log).

## Proposed fix (pending confirmation)
- In AMD64 signal delivery, store the **original** user RSP as `saved_rsp`, even when the handler runs on the alternate stack.
- Keep using the alternate stack top only for **placing the signal frame** and setting the handler stack.

## Approvals needed
- Modify `kernel/src/syscall/handler.rs` (Tier 1 prohibited file) to ensure the syscall-return delivery path uses the correct `saved_rsp`.
- Explain why GDB alone is insufficient if code change is required.

## Next steps once evidence is provided
1. Confirm whether the fault aligns with a bad `saved_rsp` (stack restore to alt stack).
2. If confirmed, implement the fix in:
   - `kernel/src/signal/delivery.rs`
   - `kernel/src/syscall/handler.rs`
3. Validate via GDB session or boot stages, per project policy.
