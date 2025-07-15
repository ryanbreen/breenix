# Phase 4B/4C Roadmap: Building Out Syscall Infrastructure

**Date**: 2025-01-14
**Baseline**: v0.3-syscall-gate-ok

## Overview

With INT 0x80 working, we can now build a proper syscall infrastructure. This roadmap follows the principle of narrow, focused PRs - one syscall, one test, one merge.

## Phase 4B: Basic Syscalls

### 4B-1: Syscall Dispatch Table

**Goal**: Replace hardcoded syscall handling with a proper dispatch table

**Deliverables**:
- `kernel/src/syscall/mod.rs` with:
  - `const SYS_MAX: usize`
  - `dispatch(nr: usize, args...) -> Result<usize, i32>`
  - Function pointer table for registered syscalls

**Test**: `integ_sys_unknown`
- Call undefined syscall number
- Expect -ENOSYS (38) return

**Implementation Notes**:
```rust
// Example dispatch table structure
static SYSCALL_TABLE: [Option<SyscallHandler>; SYS_MAX] = [
    Some(sys_read),    // 0
    Some(sys_write),   // 1
    Some(sys_exit),    // 2
    None,             // 3 - unimplemented
    // ...
];
```

### 4B-2: sys_write(fd, buf, len)

**Goal**: Minimal userspace printf via serial output

**Deliverables**:
- `sys_write` implementation that:
  - Validates fd (only 1=stdout, 2=stderr for now)
  - Safely reads from user buffer
  - Outputs to serial port

**Test**: `integ_hello_world`
- Userspace prints "Hello, Breenix!"
- Verify exact string appears once in output

**Safety Requirements**:
- Validate user buffer is readable
- Check length limits
- Handle page faults gracefully

### 4B-3: sys_exit(status)

**Goal**: Clean process termination

**Deliverables**:
- `sys_exit` implementation that:
  - Saves exit status
  - Marks thread as terminated
  - Triggers scheduler to pick next thread
  - No zombie processes

**Test**: `integ_exit_zero`
- Process calls exit(0)
- Verify process is reaped
- Exit code properly propagated
- No scheduler panics

## Phase 4C: Process Management

### 4C-1: fork() - Copy-on-Write Stub

**Goal**: Basic fork implementation (can start without full COW)

**Deliverables**:
- `sys_fork` that:
  - Creates new process with copied address space
  - Parent gets child PID > 0
  - Child gets 0
  - Both continue execution after fork

**Test**: `integ_fork_twice`
- Original process forks twice
- Should have 4 processes total
- No crashes or panics
- Each prints unique message

**Implementation Path**:
1. Start with full memory copy (no COW)
2. Add COW in separate PR
3. Keep fork() interface stable

### 4C-2: execve() - Minimal Implementation

**Goal**: Replace process image with new ELF

**Deliverables**:
- `sys_execve` that:
  - Loads new ELF binary
  - Replaces address space
  - Starts execution at entry point
  - Preserves PID

**Test**: `integ_exec_echo`
- Process execs into "echo" binary
- Echo prints "exec ok"
- Verify old image fully replaced

## Implementation Guidelines

### Hard-Won Tips

1. **Never widen PML4 0-7**: Copy individual entries as needed, always clear USER bit

2. **Add validation on every CR3 switch**:
```rust
debug_assert!(Cr3::read() != PhysFrame::from_start_address(PHYS_BOOT_PT));
```

3. **Use test harness for error paths**: Test GP/SS/#PF without crashing QEMU

4. **Benchmark context switches**: Use serial breadcrumbs to measure cycle counts

### When Things Go Wrong

1. **Run guard-rail tests first** - They catch 90% of regressions

2. **Trust the breadcrumbs** - Missing output means you haven't reached that point

3. **Tag frequently**:
   - v0.3.1-dispatch-table
   - v0.3.2-sys-write
   - v0.3.3-sys-exit
   - etc.

## Success Metrics

Each phase component is complete when:
- Integration test passes consistently
- No compiler warnings
- Guard-rail tests still pass
- Performance doesn't regress
- Clean PR with focused changes

## PR Structure

Each PR should contain:
1. Syscall implementation
2. Integration test
3. Unit test (if applicable)
4. Documentation updates
5. No unrelated changes

Keep momentum with small, verified increments!