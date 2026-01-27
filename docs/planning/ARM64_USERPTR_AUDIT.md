# ARM64 User Pointer Validation Audit

## Scope
Audit of userspace pointer validation logic and ARM64-specific risks.

Primary source: `kernel/src/syscall/userptr.rs`

## Findings

### 1) User/kernel split was hard-coded for x86_64
- `USER_SPACE_END` previously used the x86_64 canonical split.
- On ARM64 with a high-half kernel, that split is invalid.

### 2) Validation assumes a single global split
- No per-process/VMA-aware validation yet.
- ARM64 should enforce ranges that match TTBR0 user mappings.

### 3) Raw pointer dereference
- `copy_from_user` / `copy_to_user` rely on range checks only.
- Without mapping checks, unmapped user addresses will fault (expected) but kernel-range checks must be correct.

## Status
- **Fixed**: user range bounds are now arch-specific.
- **Remaining**: integrate validation with user page tables/VMA metadata and keep bounds in sync with final TTBR0 layout.

## Suggested Code Touchpoints
- `kernel/src/syscall/userptr.rs`
- `kernel/src/memory/layout.rs`
- `kernel/src/memory/vma/*` (when available for ARM64)
