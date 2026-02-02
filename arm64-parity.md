# Breenix ARM64 Parity Project

**Tracking Document for ARM64 vs x86-64 Feature Parity**

## Last Updated: 2026-02-01

---

## Executive Summary

**Goal**: Achieve ARM64 test parity with x86-64

| Metric | Current | Target |
|--------|---------|--------|
| ARM64 Pass Rate | 41.25% (33/80) | ~90% |
| x86-64 Pass Rate | ~90% | - |
| Kernel Feature Parity | ~100% | - |

**Critical Blocker**: ~~35 tests hang after `exec()`~~ **FIXED 2026-02-01**

**Key Insight**: The kernel itself has excellent parity. The gap was primarily:
1. ~~One critical bug (exec return path) blocking 35 tests~~ **FIXED** - spinlock deadlock
2. Test infrastructure gaps (not kernel functionality)

**Post-Fix Status**: exec() syscall now works. Need to re-run full test suite to measure new pass rate.

---

## Key Design Decisions

### KD-1: Skip Parallel Boot Testing for ARM64

**Decision**: Do NOT create `run-aarch64-boot-parallel.sh`

**Rationale**:
- ARM64 boots natively on Apple Silicon - extremely fast compared to x86-64 in Docker/QEMU
- Primary benefit of parallel boot testing on x86-64 is CI throughput - not needed for ARM64
- Race condition stress testing is better addressed by kthread parallel tests
- Engineering effort better spent on actual bug fixes (exec return path)

**Date**: 2026-02-01

---

### KD-2: Kthread Parallel Testing is Valuable

**Decision**: Create `run-aarch64-kthread-parallel.sh`

**Rationale**:
- Tests threading subsystem under load (scheduler, context switching, locks)
- Architecture-specific behavior that needs validation
- Stress testing kthreads has caught real bugs on x86-64

**Date**: 2026-02-01

---

## Project Plan

### Priority -1: Project Planning [COMPLETE]
- [x] Analyze ARM64 vs x86-64 feature delta
- [x] Document key design decisions
- [x] Create prioritized action plan
- [x] Establish tracking in arm64-parity.md

---

### Priority 0: Fix exec() Spinlock Deadlock [CRITICAL PATH] ✅ COMPLETE
**Status**: FIXED (2026-02-01)

**Problem**: 35 tests hung after calling `exec()`. The child process was created but never executed userspace code.

**Root Cause Identified**: **Spinlock deadlock in ext2 filesystem**

The bug was NOT in the return-to-userspace path. The actual problem:
1. `run_userspace_from_ext2()` acquires `ext2::root_fs()` spinlock at boot
2. This function **never returns** - it jumps directly to userspace via ERET
3. The MutexGuard was never dropped, so the spinlock stayed locked forever
4. When userspace fork+exec tried to load ELF from ext2, it deadlocked at `root_fs()`

**Debugging Approach**:
- Implemented lock-free trace buffer (`kernel/src/arch_impl/aarch64/trace.rs`)
- Added byte markers throughout exec() path (no locks, no serial, no formatting)
- Examined trace buffer via GDB after hang
- Trace showed: `D D D E N O 1` then hang - identified `load_elf_from_ext2()` as hang location
- Further tracing narrowed it to `ext2::root_fs()` lock acquisition

**Fix Applied** (main_aarch64.rs line 75):
```rust
let elf_data = fs.read_file_content(&inode)?;
// CRITICAL: Release ext2 lock BEFORE creating process and jumping to userspace.
// return_to_userspace() never returns, so fs_guard would never be dropped.
// If we hold the lock, fork/exec in userspace will deadlock trying to acquire it.
drop(fs_guard);
```

**Validation**:
- `fork_test`: PASS - child exit(42), parent exit(0)
- `exec_from_ext2_test`: PASS - exec's /bin/hello_world successfully
- ARM64 boot test: PASS

**Files Modified**:
- `kernel/src/main_aarch64.rs` - Added explicit `drop(fs_guard)` after ELF read
- `kernel/src/arch_impl/aarch64/trace.rs` - Created trace buffer module (can be removed)
- `kernel/src/arch_impl/aarch64/syscall_entry.rs` - Added trace points (can be removed)

---

### Priority 1: Implement COW Syscalls on ARM64
**Status**: NOT STARTED
**Effort**: Low (stub exists, just needs implementation)

**Problem**: COW_STATS and SIMULATE_OOM return ENOSYS on ARM64, blocking 6 tests.

**Files**:
- `kernel/src/arch_impl/aarch64/syscall_entry.rs` lines 736-738

**Tests blocked**:
- COW stress/validation tests that use these diagnostic syscalls

---

### Priority 2: Create Kthread Parallel Test Script
**Status**: NOT STARTED
**Effort**: Medium

Create `docker/qemu/run-aarch64-kthread-parallel.sh` modeled on x86-64 version.

**Purpose**: Stress test threading subsystem (scheduler, context switch, locks)

---

### Priority 3: Fix Specific Syscall Bugs
**Status**: NOT STARTED
**Effort**: Medium (individual investigations)

| Syscall | Issue | Test |
|---------|-------|------|
| getitimer | Returns uninitialized memory (0xCCCCCCCC) | `itimer_test` |
| socketpair | Returns EFAULT (14) | `unix_socket_test` |

---

### Priority 4: Network Configuration
**Status**: NOT STARTED
**Effort**: Medium

**Problem**: UDP/TCP tests fail with ENETUNREACH (101)

**Tests affected**: `udp_socket_test`, network-related tests

---

### Priority 5: Port Integration Tests
**Status**: NOT STARTED
**Effort**: High (ongoing)

Port x86-64 Rust integration tests to ARM64:

| Test File | Priority | Status |
|-----------|----------|--------|
| `syscall_tests.rs` | High | Not started |
| `memory_tests.rs` | High | Not started |
| `system_tests.rs` | Medium | Not started |
| `exception_tests.rs` | Medium | Not started |
| `guard_page_tests.rs` | Low | Not started |
| `stack_bounds_tests.rs` | Low | Not started |

---

## Test Results (Current Baseline)

### Summary
| Metric | Value |
|--------|-------|
| PASS | 33 |
| FAIL | 47 |
| Pass Rate | **41.25%** |

### Passing Tests (33)
```
clock_gettime_test    cow_oom_test          cow_readonly_test
cow_signal_test       cow_stress_test       dup_test
fcntl_test            fork_memory_test      fork_pending_signal_test
fork_state_test       http_test             job_control_test
job_table_test        nonblock_eagain_test  nonblock_test
pause_test            pipe2_test            pipe_concurrent_test
pipe_fork_test        pipeline_test         pty_test
session_test          shell_pipe_test       sigchld_job_test
sigchld_test          signal_fork_test      signal_handler_test
signal_return_test    sigsuspend_test       tty_test
unix_named_socket_test waitpid_test         wnohang_timing_test
```

### Failure Breakdown

| Pattern | Count | Root Cause | Priority | Status |
|---------|-------|------------|----------|--------|
| Hangs after exec() | ~35 | ext2 spinlock deadlock | **P0** | ✅ FIXED |
| COW syscall ENOSYS | 6 | Not implemented | P1 | TODO |
| Specific syscall bugs | 3 | Individual fixes | P3 | TODO |
| Network ENETUNREACH | 3 | Network config | P4 | TODO |

---

## Architecture Parity Matrix

| Dimension | x86-64 | ARM64 | Parity |
|-----------|--------|-------|--------|
| HAL/Kernel Core | Full | Full | 100% |
| Syscalls | ~70 | ~68 | ~97% |
| Drivers | 3 types | 4 types | 100% |
| Userspace Binaries | 120 | 120 | 100% |
| Shell Test Scripts | 8 | 4 | 50% |
| Rust Integration Tests | 16 | 2 | 12.5% |
| Test Pass Rate | ~90% | 41% | **Gap** |

---

## Completed Work (Previous Session)

### Bugs Fixed

#### 1. IRQ Handler Register Restoration (boot.S)
- **Root Cause**: x0/x1 not restored from exception frame after context switch
- **Fix**: Properly restore all registers x0-x30 before ERET
- **Commit**: `35a569b`
- **Impact**: +5 tests passing (fork return value was corrupted)

#### 2. exit() Syscall (syscall_entry.rs)
- **Root Cause**: Just halted system instead of terminating process
- **Fix**: Call `ProcessScheduler::handle_thread_exit()`, trigger reschedule
- **Commit**: `35a569b`
- **Impact**: Child processes now exit properly, parents wake from waitpid()

#### 3. Test Suite Missing Feature Flag
- **Root Cause**: Built without `--features testing`
- **Fix**: Added flag to build command
- **Impact**: exec() no longer returns ENOSYS

### Tests Fixed by Previous Session
- `waitpid_test` - exit() handler fix
- `wnohang_timing_test` - exit() handler fix
- `sigchld_job_test` - exit() handler fix
- `pipe_fork_test` - fork return value fix
- `job_control_test` - combined fixes

---

## Files Reference

### ARM64-Specific Kernel Code
```
kernel/src/arch_impl/aarch64/           # 16 modules
kernel/src/arch_impl/aarch64/syscall_entry.rs  # Syscall dispatcher
kernel/src/arch_impl/aarch64/boot.S     # Exception handling, context switch
kernel/src/arch_impl/aarch64/process.rs # Process setup
```

### Test Infrastructure
```
docker/qemu/run-aarch64-*.sh            # ARM64 test scripts
tests/arm64_boot_post_test.rs           # ARM64 integration tests
tests/shared_qemu_aarch64.rs            # Shared QEMU harness
```

### x86-64 Reference (for comparison/porting)
```
kernel/src/syscall/handler.rs           # x86-64 syscall handler
kernel/src/process/manager.rs           # Process setup (shared, arch-conditional)
docker/qemu/run-kthread-parallel.sh     # Reference for parallel testing
tests/syscall_tests.rs                  # Tests to port
```

---

## Next Actions

1. ~~**Reproduce exec() hang**~~ ✅ DONE
2. ~~**Debug and fix exec() bug**~~ ✅ DONE - spinlock deadlock fixed
3. **Run full test suite** - Re-test all 80 tests to measure new pass rate
4. **Implement COW syscalls** - P1 priority, should unblock 6 more tests
5. **Create kthread parallel test** - P2 priority

---

## Session Log

### 2026-02-01 (Session 2) - MAJOR BREAKTHROUGH
- **FIXED P0 exec() bug** - Root cause was ext2 spinlock deadlock, not return-to-userspace
- Implemented lock-free trace buffer debugging system
- Trace markers identified exact hang location: `ext2::root_fs()` in `load_elf_from_ext2()`
- Fix: Added `drop(fs_guard)` in `run_userspace_from_ext2()` before jumping to userspace
- Validated: fork_test PASS, exec_from_ext2_test PASS, boot test PASS
- Remaining issue: argc/argv not set up for initial process (separate from exec)

### 2026-02-01 (Session 1)
- Renamed NEXT.md to arm64-parity.md
- Documented KD-1 (skip parallel boot) and KD-2 (kthread parallel valuable)
- Established prioritized project plan with P0 = exec() bug
- Baseline: 33/80 tests passing (41.25%)
