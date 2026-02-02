# Breenix ARM64 Parity Project

**Tracking Document for ARM64 vs x86-64 Feature Parity**

## Last Updated: 2026-02-02

---

## Executive Summary

**Goal**: Achieve ARM64 test parity with x86-64

| Metric | Baseline | Current | Target |
|--------|----------|---------|--------|
| ARM64 Pass Rate | 41.25% (33/80) | **62.5% (50/80)** | ~90% |
| x86-64 Pass Rate | - | ~90% | - |
| Kernel Feature Parity | - | ~100% | - |

**Progress**: +21.25% improvement from baseline, +17 new tests passing

**Key Fixes Applied**:
1. ~~exec() spinlock deadlock~~ **FIXED** - added explicit lock release before ERET
2. ~~sys_read for RegularFile~~ **FIXED** - implemented ext2 file read support
3. Test infrastructure now uses writable ext2 disk copy

**Remaining Gaps**:
1. Network configuration (ENETUNREACH) - ~6 tests
2. argc/argv setup for initial process - ~4 tests
3. Various syscall issues - ~20 tests

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
**Status**: PARTIALLY FIXED
**Effort**: Medium (individual investigations)

| Syscall | Issue | Test | Status |
|---------|-------|------|--------|
| ~~getitimer~~ | ~~Returns uninitialized memory~~ | `itimer_test` | ✅ FIXED |
| socketpair | Returns EFAULT (14) | `unix_socket_test` | TODO |

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

## Test Results (2026-02-02)

### Summary
| Metric | Baseline (2026-02-01) | Current | Change |
|--------|----------------------|---------|--------|
| PASS | 33 | **50** | **+17** |
| FAIL | 47 | 30 | -17 |
| Pass Rate | 41.25% | **62.5%** | **+21.25%** |

### Passing Tests (50)
```
access_test           cat_test              clock_gettime_test
cow_oom_test          cow_readonly_test     cow_signal_test
cow_stress_test       cp_mv_argv_test       cwd_test
dup_test              echo_argv_test        fcntl_test
file_read_test        fork_memory_test      fork_pending_signal_test
fork_state_test       fork_test             fs_directory_test
getdents_test         head_test             http_test
job_control_test      job_table_test        ls_test
mkdir_argv_test       nonblock_eagain_test  nonblock_test
pause_test            pipe2_test            pipe_concurrent_test
pipe_fork_test        pipeline_test         pty_test
rm_argv_test          session_test          shell_pipe_test
sigchld_job_test      sigchld_test          signal_exec_test
signal_fork_test      signal_handler_test   signal_return_test
sigsuspend_test       tail_test             tty_test
unix_named_socket_test waitpid_test         wc_test
which_test            wnohang_timing_test
```

### Tests Fixed by exec() Fix (+9, 2026-02-02 Session 1)
| Test | Category |
|------|----------|
| `access_test` | Filesystem access checks |
| `cwd_test` | Current working directory |
| `echo_argv_test` | Echo with arguments |
| `fork_test` | **Fork syscall now working** |
| `getdents_test` | Directory listing |
| `itimer_test` | Interval timers |
| `ls_test` | Directory listing |
| `signal_exec_test` | **Signals + exec working together** |
| `which_test` | PATH lookup |

### Tests Fixed by RegularFile Read (+8, 2026-02-02 Session 2)
| Test | Category |
|------|----------|
| `cat_test` | File content display |
| `cp_mv_argv_test` | File copy/move operations |
| `file_read_test` | Explicit file reading |
| `fs_directory_test` | Directory operations |
| `head_test` | File head reading |
| `mkdir_argv_test` | Directory creation |
| `rm_argv_test` | File deletion |
| `tail_test` | File tail reading |
| `wc_test` | Word count (file reading) |

### Failure Breakdown

| Pattern | Count | Root Cause | Priority | Status |
|---------|-------|------------|----------|--------|
| ~~Hangs after exec()~~ | ~~35~~ | ~~ext2 spinlock deadlock~~ | ~~P0~~ | ✅ FIXED |
| ~~sys_read returns EOPNOTSUPP~~ | ~~8~~ | ~~RegularFile not implemented~~ | ~~P1~~ | ✅ FIXED |
| Network ENETUNREACH | ~6 | Network config | P1 | TODO |
| Filesystem write errors | ~6 | ext2 write not implemented | P2 | TODO |
| argc/argv setup | ~4 | Initial process setup | P3 | TODO |
| Signal/process bugs | ~8 | Various | P4 | TODO |
| COW syscall ENOSYS | ~2 | Not implemented | P5 | TODO |
| Other | ~4 | Various | P6 | TODO |

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
| Test Pass Rate | ~90% | **62.5%** | **Improving** |

---

## New Infrastructure: DTrace-Style Tracing Framework

**Added**: 2026-02-02

A comprehensive lock-free tracing framework has been implemented for kernel observability on both x86-64 and ARM64. This infrastructure was originally created during ARM64 debugging (exec spinlock deadlock) and has been generalized into a production-quality subsystem.

### Features

| Component | Description |
|-----------|-------------|
| Per-CPU Ring Buffers | 1024 events × 16 bytes per CPU, lock-free writes |
| Provider/Probe Model | Subsystems define their own trace points |
| Atomic Counters | Per-CPU counters with 64-byte cache alignment |
| GDB Integration | `#[no_mangle]` symbols for direct inspection |
| /proc Integration | `/proc/trace/{enable,events,buffer,counters,providers}` |
| Serial Output | Lock-free panic-safe dump functions |

### Built-in Providers

| Provider | Events |
|----------|--------|
| SYSCALL_PROVIDER | SYSCALL_ENTRY, SYSCALL_EXIT |
| SCHED_PROVIDER | CTX_SWITCH_ENTRY, CTX_SWITCH_EXIT, SCHED_PICK |
| IRQ_PROVIDER | IRQ_ENTRY, IRQ_EXIT, TIMER_TICK |

### Built-in Counters

- `SYSCALL_TOTAL` - Total syscall invocations
- `IRQ_TOTAL` - Total interrupt invocations
- `CTX_SWITCH_TOTAL` - Total context switches
- `TIMER_TICK_TOTAL` - Total timer tick interrupts

### Validation

GDB-based memory dump testing confirms the framework works correctly:
- 278 events captured during kernel boot on x86-64
- TIMER_TICK events with incrementing tick counts
- CTX_SWITCH_ENTRY events showing thread context switches
- Timestamps are monotonically increasing

### Files Added

```
kernel/src/tracing/           # Core framework (~2500 lines)
  mod.rs                      # Public API and re-exports
  core.rs                     # TraceEvent, ring buffers, global state
  buffer.rs                   # Per-CPU TraceCpuBuffer
  timestamp.rs                # RDTSC (x86) / CNTVCT (ARM64) timestamps
  provider.rs                 # TraceProvider, TraceProbe registration
  counter.rs                  # Atomic per-CPU counters
  output.rs                   # Serial dump, GDB helpers
  macros.rs                   # trace_event!, define_trace_counter!
  providers/                  # Built-in providers (syscall, sched, irq)

kernel/src/fs/procfs/         # Virtual filesystem
  mod.rs                      # procfs core infrastructure
  trace.rs                    # /proc/trace/* content generators

scripts/                      # Testing infrastructure
  trace_memory_dump.py        # Parse trace buffer memory dumps
  test_tracing_via_gdb.sh     # Automated GDB-based validation

docs/planning/
  TRACING_FRAMEWORK_DESIGN.md        # Design document
  TRACING_FRAMEWORK_IMPLEMENTATION.md # Implementation guide
```

### Cross-Architecture Support

| Feature | x86-64 | ARM64 |
|---------|--------|-------|
| Timestamp source | RDTSC | CNTVCT_EL0 |
| Serial output | COM1 (0x3F8) | PL011 UART |
| Tracing enabled at boot | Yes | TODO |
| /proc/trace | Yes | TODO |

**Note**: ARM64 tracing init call needs to be added to `main_aarch64.rs`.

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
3. ~~**Run full test suite**~~ ✅ DONE - 62.5% pass rate (50/80)
4. ~~**Implement RegularFile read**~~ ✅ DONE - +8 tests passing
5. **Fix network configuration** - P1 priority, ENETUNREACH errors (~6 tests)
6. **Implement filesystem writes** - P2 priority (~6 tests)
7. **Fix argc/argv setup** - P3 priority, initial process args (~4 tests)
8. **Fix signal/process bugs** - P4 priority (~8 tests)

---

## Session Log

### 2026-02-02 (Session 4) - REGULAR FILE READ FIX
- **Root cause**: ARM64 `sys_read` returned EOPNOTSUPP for `FdKind::RegularFile`
- **Fix**: Implemented proper ext2 file read in `kernel/src/syscall/io.rs`
- **Also fixed**: Test suite now uses writable ext2 disk copy
- **PR #140**: Merged to main
- **Results**: 50/80 passing (62.5%), up from 42/80 (52.5%)
- **+8 new tests passing**:
  - `cat_test`, `cp_mv_argv_test`, `file_read_test`, `fs_directory_test`
  - `head_test`, `mkdir_argv_test`, `rm_argv_test`, `tail_test`, `wc_test`

### 2026-02-02 (Session 3) - TEST SUITE VALIDATION
- **Committed and merged exec() fix** - PR #138 merged to main
- **Ran full ARM64 test suite** - 80 tests
- **Results**: 42/80 passing (52.5%), up from 33/80 (41.25%)
- **+9 new tests passing** after exec() fix:
  - `access_test`, `cwd_test`, `echo_argv_test`, `fork_test`
  - `getdents_test`, `itimer_test`, `ls_test`, `signal_exec_test`, `which_test`
- **Notable**: `itimer_test` now passes (was listed as P3 bug)
- **Remaining failures** categorized:
  - Filesystem write errors (~12 tests) - ext2 mounted read-only?
  - Network errors (~6 tests) - ENETUNREACH
  - argc/argv setup (~4 tests)
  - COW syscalls (~2 tests)
  - Other (~14 tests)

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
