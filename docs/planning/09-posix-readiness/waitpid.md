# wait() and waitpid() Implementation

Date: 2025-01-11

## Overview

This document describes the implementation of POSIX-compliant wait() and waitpid() system calls in Breenix. These system calls allow parent processes to wait for their children to exit and retrieve exit status information.

## Implementation Details

### Data Structure Changes

1. **Process Structure** (`kernel/src/process/process.rs`):
   - Added `exit_status: Option<u8>` - stores the 8-bit exit status for wait/waitpid
   - Already had `parent: Option<ProcessId>` - tracks parent process
   - Already had `children: Vec<ProcessId>` - tracks child processes

2. **Thread States** (`kernel/src/task/thread.rs`):
   - Added `Blocked(BlockedReason)` state
   - Added `BlockedReason::Wait` for threads blocked on wait/waitpid

3. **Scheduler** (`kernel/src/task/scheduler.rs`):
   - Added `waiters: Vec<Waiter>` to track blocked parents
   - Added `WaitMode` enum (AnyChild, SpecificChild)
   - Added methods to add/remove/wake waiters

### System Call Interface

**Syscall Numbers** (Linux-compatible):
- `SYS_WAIT = 7`
- `SYS_WAITPID = 61`

**Function Signatures**:
```rust
pub fn sys_wait(status_ptr: u64) -> SyscallResult
pub fn sys_waitpid(pid: i64, status_ptr: u64, options: u32) -> SyscallResult
```

**Return Values**:
- Success: PID of reaped child
- WNOHANG + no ready child: 0
- Error: negative errno

**Error Codes**:
- `ECHILD (-10)`: No children exist
- `EINTR (-4)`: Interrupted by signal (future)
- `EINVAL (-22)`: Invalid arguments
- `EFAULT (-14)`: Bad pointer

### Algorithm

#### _exit() Path
1. Set `process.exit_status = Some(code & 0xFF)`
2. Set process state to `Terminated`
3. Wake any parent waiters matching this child
4. Mark thread as terminated
5. Force reschedule

#### wait()/waitpid() Path
1. Validate caller has children (else ECHILD)
2. Check if any matching child has `exit_status != None`:
   - If yes: copy status to user, return child PID
   - If WNOHANG: return 0
   - Otherwise: block thread with `Wait` reason
3. When woken, loop back to check again
4. Handle interrupts (future: EINTR)

### Concurrency Considerations

- All Process mutations protected by PROCESS_MANAGER lock
- Scheduler operations use interrupt-disabled critical sections
- Never hold locks during:
  - User memory access (copy_to_user)
  - Blocking operations (scheduler::add_waiter)
- Wake operations are deferred to avoid lock ordering issues

### Future Work

1. **Zombie Processes**: Currently processes are cleaned up immediately. Future implementation should:
   - Keep process structure until wait() collects status
   - Implement zombie state for proper POSIX semantics

2. **Process Groups**: Support for negative pid values in waitpid():
   - `pid < -1`: Wait for any child in process group |pid|
   - `pid == 0`: Wait for any child in same process group
   - `pid == -1`: Wait for any child (implemented)

3. **Signal Support**: 
   - SIGCHLD delivery on child exit
   - EINTR when wait is interrupted by signal

4. **Extended Status**:
   - Core dump flag (high bit)
   - Stopped/continued status
   - Signal termination info

## Testing

Four integration tests verify correct behavior:

1. **wait_many**: Parent forks 5 children, each exits with different status. Verifies all are collected and statuses sum correctly.

2. **waitpid_specific**: Parent forks 2 children, waits for each specifically. Verifies correct PID and status returned.

3. **wait_nohang_polling**: Child sleeps before exiting. Parent polls with WNOHANG. Verifies 0 returned while waiting, then child PID.

4. **echld_error**: Process with no children calls wait(). Verifies ECHILD error returned.

## Usage Example

```rust
// Fork a child
let pid = unsafe { sys_fork() };
if pid == 0 {
    // Child process
    unsafe { sys_exit(42) };
} else {
    // Parent process
    let mut status: u32 = 0;
    let child_pid = unsafe { sys_wait(&mut status) };
    let exit_code = status & 0xFF;  // Extract exit code
    println!("Child {} exited with status {}", child_pid, exit_code);
}
```

## Standards Compliance

This implementation follows POSIX.1-2017 semantics for wait() and waitpid() with the following limitations:
- No process group support yet
- No signal support yet  
- No zombie processes (immediate cleanup)
- Status is simple 8-bit value (no signal/core dump info)

These limitations are acceptable for the current development phase and will be addressed as the OS matures.