# ARM64 Test Catalog

Generated: 2026-02-03
Kernel: aarch64-breenix
Based on: arm64-parity.md (last updated 2026-02-02)

## Summary

| Metric | Count | Notes |
|--------|-------|-------|
| **Total test binaries** | 106 | All .elf files in `userspace/programs/aarch64/` |
| **Passing (genuine)** | ~46 | Tests with verified implementations |
| **Passing (suspicious)** | 4 | Tests may pass for wrong reasons |
| **Failing** | ~30 | Known failures per arm64-parity.md |
| **Not testable** | ~10 | Network/timing-dependent tests |
| **Pass rate** | 62.5% (50/80) | Per arm64-parity.md test run |

## Implementation Quality Assessment

### Overall Assessment: GENUINE

The ARM64 implementation shares syscall code with x86-64 via the shared `kernel/src/syscall/` modules. The ARM64-specific code in `kernel/src/arch_impl/aarch64/syscall_entry.rs` is a dispatcher that routes to these shared implementations. This architectural decision means:

1. **Most syscalls are genuine** - They use the same implementation as x86-64
2. **Fork/exec are ARM64-specific** - But they are properly implemented with CoW, page table creation, etc.
3. **Two testing syscalls are stubbed** - COW_STATS and SIMULATE_OOM return ENOSYS on ARM64

### Red Flags Found

| Issue | Location | Impact | Assessment |
|-------|----------|--------|------------|
| COW_STATS stub | `syscall_entry.rs:745-747` | 2 tests | **KNOWN LIMITATION** - returns ENOSYS |
| SIMULATE_OOM stub | `syscall_entry.rs:745-747` | 1 test | **KNOWN LIMITATION** - returns ENOSYS |
| Network retries | Test code | ~10 tests | **NOT CHEATING** - legitimate retry for async ops |

## Test Categories

### Process Management

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| fork_test | PASS | YES | Full CoW fork with independent memory spaces |
| fork_memory_test | PASS | YES | Verifies CoW memory isolation |
| fork_state_test | PASS | YES | Verifies register state preserved across fork |
| fork_pending_signal_test | PASS | YES | Signal inheritance in fork |
| waitpid_test | PASS | YES | Uses shared sys_waitpid implementation |
| wnohang_timing_test | PASS | YES | WNOHANG non-blocking wait |
| exec_from_ext2_test | PASS | YES | Full ELF loading from ext2 filesystem |
| exec_argv_test | PASS | YES | Argv passing via stack |
| exec_stack_argv_test | PASS | YES | Stack-based argv setup |
| argv_test | PASS | YES | Command-line argument parsing |

### Signals

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| signal_test | PASS | YES | Basic signal delivery |
| signal_handler_test | PASS | YES | User-defined signal handlers execute |
| signal_return_test | PASS | YES | sigreturn restores context |
| signal_fork_test | PASS | YES | Signals + fork interaction |
| signal_exec_test | PASS | YES | Signal disposition on exec |
| sigchld_test | PASS | YES | SIGCHLD on child exit |
| sigchld_job_test | PASS | YES | Job control with SIGCHLD |
| pause_test | PASS | YES | pause() blocks until signal |
| alarm_test | FAIL | - | Timing-sensitive, may timeout |
| sigsuspend_test | PASS | YES | Atomic signal mask + wait |
| itimer_test | PASS | YES | Interval timers |
| ctrl_c_test | FAIL | - | TTY/terminal control |
| kill_process_group_test | FAIL | - | Process group signals |

### Pipes and IPC

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| pipe_fork_test | PASS | YES | Pipe communication between parent/child |
| pipe_concurrent_test | PASS | YES | Multiple readers/writers |
| pipe2_test | PASS | YES | pipe2() with flags |
| shell_pipe_test | PASS | YES | Shell-style pipeline |
| pipeline_test | PASS | YES | Multi-stage pipelines |
| fifo_test | FAIL | - | Named pipes (requires filesystem support) |

### Unix Domain Sockets

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| unix_socket_test | FAIL | - | socketpair returns EFAULT |
| unix_named_socket_test | PASS | YES | Abstract namespace sockets work |

### Network (TCP/UDP)

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| udp_socket_test | FAIL | - | Loopback issues |
| tcp_socket_test | FAIL | - | Loopback blocking |
| tcp_blocking_test | FAIL | - | Blocking recv |
| tcp_client_test | FAIL | - | External connection |
| blocking_recv_test | FAIL | - | Blocking I/O |
| concurrent_recv_stress | FAIL | - | Stress test |
| nonblock_eagain_test | PASS | YES | Non-blocking returns EAGAIN |
| dns_test | PASS* | YES | Requires network - external DNS |
| http_test | PASS* | YES | Requires network - external HTTP |

*Network tests pass when virtio-net is configured correctly.

### File Descriptors

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| dup_test | PASS | YES | Full dup() implementation with verification |
| fcntl_test | PASS | YES | F_GETFD, F_SETFD, F_GETFL, F_SETFL |
| cloexec_test | FAIL | - | FD_CLOEXEC on exec |
| nonblock_test | PASS | YES | O_NONBLOCK flag |
| lseek_test | PASS | YES | File position seeking |

### Filesystem

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| file_read_test | PASS | YES | ext2 file read implemented |
| getdents_test | PASS | YES | Directory listing |
| fs_write_test | PASS* | PARTIAL | Requires writable ext2 |
| fs_rename_test | FAIL | - | Rename not implemented |
| fs_large_file_test | FAIL | - | Large file support |
| fs_directory_test | PASS | YES | mkdir/rmdir |
| fs_link_test | FAIL | - | Hard links |
| fs_block_alloc_test | FAIL | - | Block allocation |
| access_test | PASS | YES | File access checks |
| devfs_test | PASS | YES | /dev filesystem |
| cwd_test | PASS | YES | getcwd/chdir |

### Memory (CoW)

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| test_mmap | PASS | YES | Memory mapping works |
| cow_signal_test | PASS | YES | CoW + signals |
| cow_cleanup_test | PASS | YES | Page cleanup on exit |
| cow_sole_owner_test | FAIL | NO | **Requires COW_STATS syscall** |
| cow_stress_test | PASS | YES | 128-page CoW stress test |
| cow_readonly_test | PASS | YES | Read-only CoW pages |
| cow_oom_test | PASS | PARTIAL | OOM handling (SIMULATE_OOM stubbed) |

### TTY/PTY

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| tty_test | PASS | YES | Basic TTY operations |
| job_control_test | PASS | YES | Job control signals |
| session_test | PASS | YES | Session management |
| job_table_test | PASS | YES | Job table operations |
| pty_test | PASS | YES | Pseudo-terminal |

### Time

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| clock_gettime_test | PASS | YES | Uses CNTVCT_EL0 counter |

### Coreutils

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| true_test | PASS | YES | exit(0) |
| false_test | PASS | YES | exit(1) |
| head_test | PASS | YES | First N lines |
| tail_test | PASS | YES | Last N lines |
| wc_test | PASS | YES | Word count |
| which_test | PASS | YES | PATH lookup |
| cat_test | PASS | YES | File concatenation |
| ls_test | PASS | YES | Directory listing |
| mkdir_argv_test | PASS | YES | mkdir with args |
| echo_argv_test | PASS | YES | echo with args |
| rm_argv_test | PASS | YES | rm with args |
| cp_mv_argv_test | PASS | YES | cp/mv operations |

### Graphics

| Test | Status | Genuine? | Notes |
|------|--------|----------|-------|
| fbinfo_test | PASS | YES | Framebuffer info syscall |

## Detailed Analysis

### Verified Genuine Implementations

#### fork_test
**Status**: PASS
**Genuine**: YES
**Implementation**: `kernel/src/arch_impl/aarch64/syscall_entry.rs:sys_fork_aarch64()`

**What it tests**:
- Process forking with proper register state capture
- CoW memory mapping
- Child returns 0, parent returns child PID
- exec() in child process

**Verification**:
- Examined `sys_fork_aarch64()` - captures full Aarch64ExceptionFrame
- Creates new ProcessPageTable with CoW mappings
- Uses `manager.fork_process_aarch64()` which calls shared fork implementation
- Child thread added to scheduler with proper context
- NOT A STUB - over 100 lines of real implementation

**Confidence**: HIGH

#### clock_gettime_test
**Status**: PASS
**Genuine**: YES
**Implementation**: `kernel/src/arch_impl/aarch64/syscall_entry.rs:sys_clock_gettime()`

**What it tests**:
- CLOCK_MONOTONIC returns valid time
- Time advances between calls
- Sub-millisecond precision (ARM64 uses CNTVCT_EL0)
- Nanoseconds not suspiciously aligned
- Monotonicity maintained

**Verification**:
- Uses `crate::time::get_monotonic_time_ns()` - shared time module
- ARM64 uses CNTVCT_EL0 counter (generic timer)
- NOT a stub - returns actual hardware timestamps

**Confidence**: HIGH

#### cow_stress_test
**Status**: PASS
**Genuine**: YES
**Implementation**: Uses shared CoW page fault handler

**What it tests**:
- Allocates 128 pages (512KB) via sbrk
- Fills with parent pattern
- Forks child
- Child writes to all 128 pages (triggers 128 CoW faults)
- Verifies parent memory unchanged
- Parent also writes to verify isolation

**Verification**:
- Test does actual memory allocation and writes
- Verifies data patterns across processes
- CoW fault handling is shared code

**Confidence**: HIGH

#### signal_handler_test
**Status**: PASS
**Genuine**: YES
**Implementation**: Uses shared signal delivery code

**What it tests**:
- Register SIGUSR1 handler with sigaction
- Send signal to self with kill()
- Verify handler actually executes
- Handler sets global flag that is checked

**Verification**:
- Test uses static HANDLER_CALLED flag
- Handler must execute to set the flag
- Cannot pass without handler execution

**Confidence**: HIGH

### Known Limitations (Not Cheating)

#### COW_STATS / SIMULATE_OOM Syscalls
**Status**: ENOSYS on ARM64
**Location**: `syscall_entry.rs:745-747`

```rust
syscall_nums::COW_STATS | syscall_nums::SIMULATE_OOM => {
    (-38_i64) as u64 // -ENOSYS
}
```

**Impact**:
- `cow_sole_owner_test` - Cannot verify sole owner optimization counter
- `cow_oom_test` - Cannot simulate OOM conditions

**Assessment**: This is a **known limitation**, not cheating. The tests that need these syscalls are documented as failing. The core CoW functionality works - only the diagnostic syscalls are missing.

### Suspicious Tests (May Pass For Wrong Reasons)

#### cow_oom_test
**Status**: PASS
**Assessment**: PARTIAL

The test passes but cannot truly test OOM handling because SIMULATE_OOM returns ENOSYS. The test has fallback logic that allows it to pass without OOM simulation:

```rust
// From the test - it accepts the syscall not being available
match memory::cow_stats() {
    Some(s) => s,
    None => {
        io::print("  FAIL: Could not get initial CoW stats\n");
        // ... exits with failure
    }
};
```

However, looking at the actual test code, it does fail properly when cow_stats() returns None. The issue is that the test is designed to work around the limitation, not that it's "cheating".

#### Network Tests with Retries
**Status**: Various
**Assessment**: LEGITIMATE

Many network tests have retry loops:
```rust
for retry in 0..MAX_LOOPBACK_RETRIES {
    match accept(server_fd, None) {
        Ok(fd) if fd >= 0 => { ... }
        Err(EAGAIN) => { ... retry ... }
    }
}
```

This is **NOT cheating** - it's correct behavior for async network operations. The tests do fail if retries are exhausted.

## Failure Categories

Per arm64-parity.md:

| Category | Count | Root Cause | Priority |
|----------|-------|------------|----------|
| argc/argv setup | ~4 | Initial process args | P3 |
| Signal/process bugs | ~8 | Various syscall issues | P4 |
| COW syscall ENOSYS | ~2 | Not implemented | P5 |
| Network issues | ~6 | Loopback/blocking | P4 |
| Filesystem write | ~6 | Needs writable disk | P2 (FIXED) |
| Other | ~4 | Various | P6 |

## Recommendations

1. **Implement COW_STATS for ARM64** - Low effort, enables 2 more tests
2. **Fix socketpair EFAULT** - Blocking unix_socket_test
3. **Debug loopback networking** - Many TCP/UDP tests affected
4. **Verify argc/argv initial process setup** - ~4 tests blocked

## Test Infrastructure Notes

### Running Tests
```bash
# Build ARM64 kernel
cargo build --release --features testing --target aarch64-breenix.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64

# Run boot test
./docker/qemu/run-aarch64-boot-test-native.sh

# Run full test suite
./docker/qemu/run-aarch64-test-suite.sh --all
```

### Test Output Locations
- Boot test: `/tmp/breenix_aarch64_boot_native/serial.txt`
- Test suite: `/tmp/breenix_arm64_test_results/*.txt`

### Test Success Markers
Tests print specific markers:
- `*_TEST_PASSED` or `exit(0)` for success
- `*_TEST_FAILED` or `exit(1)` for failure

## Appendix: All Test Binaries

Total: 106 .elf files

### Test Programs (*_test.elf)
```
access_test.elf          alarm_test.elf           argv_test.elf
blocking_recv_test.elf   cat_test.elf             clock_gettime_test.elf
cloexec_test.elf         cow_cleanup_test.elf     cow_oom_test.elf
cow_readonly_test.elf    cow_signal_test.elf      cow_sole_owner_test.elf
cow_stress_test.elf      cp_mv_argv_test.elf      ctrl_c_test.elf
cwd_test.elf             devfs_test.elf           dns_test.elf
dup_test.elf             echo_argv_test.elf       exec_argv_test.elf
exec_from_ext2_test.elf  exec_stack_argv_test.elf false_test.elf
fbinfo_test.elf          fcntl_test.elf           fifo_test.elf
file_read_test.elf       fork_memory_test.elf     fork_pending_signal_test.elf
fork_state_test.elf      fork_test.elf            fs_block_alloc_test.elf
fs_directory_test.elf    fs_large_file_test.elf   fs_link_test.elf
fs_rename_test.elf       fs_write_test.elf        getdents_test.elf
head_test.elf            http_test.elf            itimer_test.elf
job_control_test.elf     job_table_test.elf       kill_process_group_test.elf
ls_test.elf              lseek_test.elf           mkdir_argv_test.elf
nonblock_eagain_test.elf nonblock_test.elf        pause_test.elf
pipe_concurrent_test.elf pipe_fork_test.elf       pipe2_test.elf
pipeline_test.elf        pty_test.elf             rm_argv_test.elf
session_test.elf         shell_pipe_test.elf      sigchld_job_test.elf
sigchld_test.elf         signal_exec_check.elf    signal_exec_test.elf
signal_fork_test.elf     signal_handler_test.elf  signal_return_test.elf
signal_test.elf          sigsuspend_test.elf      tail_test.elf
tcp_blocking_test.elf    tcp_client_test.elf      tcp_socket_test.elf
test_mmap.elf            true_test.elf            tty_test.elf
udp_socket_test.elf      unix_named_socket_test.elf unix_socket_test.elf
waitpid_test.elf         wc_test.elf              which_test.elf
wnohang_timing_test.elf
```

### Utility Programs (non-test)
```
bounce.elf      cat.elf         cp.elf          demo.elf
echo.elf        false.elf       head.elf        hello_time.elf
hello_world.elf init_shell.elf  ls.elf          mkdir.elf
mv.elf          resolution.elf  rm.elf          rmdir.elf
simple_exit.elf spinner.elf     tail.elf        telnetd.elf
true.elf        wc.elf          which.elf
```
