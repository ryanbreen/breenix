# AMD64 sigaltstack() Failure Analysis (Boot Stage 72/258)

## Context
- Boot stages report failure at stage 72/258: sigaltstack() syscall verified.
- Meaning: sigaltstack() or SA_ONSTACK signal delivery is failing in AMD64.

## What I Could Not Retrieve
- The referenced GitHub Actions job log requires authentication, so I could not pull the exact failure output from that URL in this environment.

## Likely Root Cause (Code Analysis)
The signal delivery paths currently save the *handler* stack pointer as the return stack pointer when SA_ONSTACK is used. That means sigreturn restores to the alternate stack instead of the original main stack.

This is incorrect POSIX behavior and can cause:
- The process to continue executing on the alternate stack after the handler returns.
- Subsequent sigaltstack() calls to behave unexpectedly.
- Failures in the sigaltstack_test that expects normal execution to continue on the main stack.

### Evidence in Code
- `kernel/src/signal/delivery.rs` (x86_64 and aarch64):
  - `SignalFrame.saved_rsp` / `saved_sp` is set to `user_rsp` / `user_sp`.
  - When SA_ONSTACK is used, `user_rsp` / `user_sp` is the *alternate* stack top.
  - This is the value restored by sigreturn.
- `kernel/src/syscall/handler.rs` (syscall-return delivery path):
  - `SignalFrame.saved_rsp` is set to `user_rsp` in `deliver_to_user_handler_syscall()`.
  - For SIGUSR1 delivered on syscall return (the sigaltstack test path), this is the hot path.

## Fix Required
- Save the *original* user stack pointer into `SignalFrame.saved_rsp` / `saved_sp`.
- Continue to use the alternate stack for the handler frame placement only.

### Status
- Updated `kernel/src/signal/delivery.rs` to save the original stack pointer for both x86_64 and ARM64 signal delivery.
- The syscall-return path fix **still needs to be applied** in `kernel/src/syscall/handler.rs` (Tier-1 prohibited file; requires explicit approval).

## Next Steps
1. Apply the same saved_rsp fix in `kernel/src/syscall/handler.rs` (needs approval).
2. Run boot-stages or targeted signal tests to confirm `SIGALTSTACK_TEST_PASSED`.
3. If failure persists, inspect for:
   - SA_ONSTACK flag propagation in sigaction
   - alt stack address validation vs user space bounds
   - any failure to clear `alt_stack.on_stack` in sigreturn paths

