# Kthread Debugging Handoff Document

**Date**: 2026-01-19
**Branch**: `feature/kthreads2`

## Summary

The kthread join test was experiencing intermittent hangs (~50% failure rate). Root cause was identified and fixed: a `log::debug!` call in `master_kernel_pml4()` caused deadlocks when called from interrupt context.

## Root Cause (FIXED)

**File**: `kernel/src/memory/kernel_page_table.rs:619`

**Problem**: The `master_kernel_pml4()` function had a `log::debug!` statement:
```rust
pub fn master_kernel_pml4() -> Option<PhysFrame> {
    let result = MASTER_KERNEL_PML4.lock().clone();
    log::debug!("master_kernel_pml4() returning {:?}", result);  // PROBLEM
    result
}
```

**Call chain when deadlock occurred**:
```
timer_interrupt_entry
  → check_need_resched_and_switch
    → switch_to_thread
      → setup_kernel_thread_return
        → switch_to_kernel_page_table
          → master_kernel_pml4()
            → log::debug!()  ← DEADLOCK
```

When a timer interrupt fired while another thread held the logger spinlock, the context switch code tried to acquire the same lock, causing a deadlock.

**Fix applied**: Removed the `log::debug!` call and added a comment explaining why logging must not be added:
```rust
/// IMPORTANT: This function is called from the context switch path (via
/// switch_to_kernel_page_table), so it must not do any logging. Logging
/// from interrupt context can cause deadlocks if the logger lock is held.
pub fn master_kernel_pml4() -> Option<PhysFrame> {
    MASTER_KERNEL_PML4.lock().clone()
}
```

## Verification Status

The kthread test now passes when run in isolation:
```bash
cargo run -p xtask -- kthread-test
# Output: === KTHREAD JOIN TEST: PASS ===
```

**UNRESOLVED**: We need 5 consecutive passing runs to confirm the fix is stable. The earlier test failures may have been caused by:
1. The now-fixed deadlock bug
2. Parallel test tasks competing for QEMU resources
3. Stale QEMU processes interfering with new runs

## Docker QEMU Isolation (In Progress)

### Goal
Run QEMU inside Docker containers for complete isolation, enabling:
- Parallel test execution without resource conflicts
- Reproducible test environments
- CI/CD integration

### Implementation Status

**Files created**:
- `docker/qemu/Dockerfile` - Ubuntu 24.04 with QEMU
- `docker/qemu/run-kthread-test.sh` - Script to run test in container

**Current issue**: Docker on macOS uses Linux VM underneath, and inside that VM, QEMU uses TCG (software CPU emulation) instead of hardware virtualization. This makes tests ~10-100x slower than native QEMU with Hypervisor.framework.

**Native macOS QEMU**: ~30 seconds per kthread test
**Docker QEMU (TCG)**: ~10+ minutes per kthread test (boot doesn't complete in 60s timeout)

### Docker Script Details

The `run-kthread-test.sh` script:
1. Copies OVMF firmware files to a temp directory (pflash needs write access)
2. Launches QEMU inside Docker with isolated filesystem
3. Monitors serial output for kthread markers
4. Cleans up container when done

Key Docker mount points:
```bash
-v "$UEFI_IMG:/breenix/breenix-uefi.img:ro"
-v "$BREENIX_ROOT/target/test_binaries.img:/breenix/test_binaries.img:ro"
-v "$BREENIX_ROOT/target/ext2.img:/breenix/ext2.img:ro"
-v "$OUTPUT_DIR:/output"  # For OVMF files and serial logs
```

### Next Steps for Docker

1. **Option A**: Accept slow Docker tests for CI, use native for development
2. **Option B**: Investigate running QEMU with KVM inside Docker on Linux hosts (CI only)
3. **Option C**: Use macOS-specific isolation (separate user sessions, process namespaces)

## Atomic Trace System

For debugging without log contention, we implemented an atomic memory-based trace system:

**File**: `kernel/src/atomic_trace.rs`

Key functions:
- `trace_ctx_switch_start(old, new)` - Record context switch
- `trace_kthread_context_save(tid, rip, rsp)` - Record saved context
- `trace_kthread_context_restore(tid, rip, rsp)` - Record restored context

Inspect via GDB:
```gdb
print kernel::atomic_trace::TRACE
```

## Files Modified This Session

1. `kernel/src/memory/kernel_page_table.rs` - Removed log::debug from master_kernel_pml4()
2. `kernel/src/atomic_trace.rs` - Added kthread context save/restore trace fields
3. `kernel/src/interrupts/context_switch.rs` - Added atomic trace calls
4. `docker/qemu/Dockerfile` - New file
5. `docker/qemu/run-kthread-test.sh` - New file

## Testing Commands

```bash
# Kill any stale QEMU
killall -9 qemu-system-x86_64 2>/dev/null

# Run kthread test
cargo run -p xtask -- kthread-test

# Run full boot stages test
cargo run -p xtask -- boot-stages

# GDB debugging
./breenix-gdb-chat/scripts/gdb_session.sh start
./breenix-gdb-chat/scripts/gdb_session.sh cmd "break kernel::interrupts::context_switch::setup_kernel_thread_return"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"
```

## Critical Reminders

1. **NEVER add logging to interrupt/context switch paths** - causes deadlocks
2. **ALWAYS kill QEMU before running tests** - `killall -9 qemu-system-x86_64`
3. **Check for prohibited files** before modifying (see CLAUDE.md)
4. **Use GDB for debugging**, not serial output in hot paths
