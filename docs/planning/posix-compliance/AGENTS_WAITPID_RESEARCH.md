# AGENTS.md: waitpid Implementation Research and Handoff

## Mission

Research and implement the `waitpid()` system call for Breenix, including a test case. This implements Phase 2 of the POSIX roadmap: "wait/waitpid - Enable shell process management".

## Status: RESEARCH COMPLETE - READY FOR IMPLEMENTATION

## Current State Assessment

### getpid (Syscall 39) - COMPLETE
- Implementation: `kernel/src/syscall/handlers.rs:543-582`
- Works correctly: Returns ProcessId for current thread via process manager
- Test coverage: Used in `userspace/tests/fork_test.rs`

### waitpid - NOT IMPLEMENTED
- Linux syscall number: 7 (wait4 is 61)
- Breenix syscall number: **7** (use wait4 semantics)
- Current state: Missing entirely

---

## OS Research: waitpid Implementation

### 1. POSIX waitpid Semantics

```c
pid_t waitpid(pid_t pid, int *status, int options);
```

**Arguments:**
- `pid`: Which processes to wait for
  - `> 0`: Wait for specific child with that PID
  - `0`: Wait for any child in same process group (not needed initially)
  - `-1`: Wait for ANY child process
  - `< -1`: Wait for any child in process group |pid| (not needed initially)
- `status`: Pointer to store exit status (can be NULL)
- `options`: Flags controlling behavior
  - `WNOHANG` (0x1): Return immediately if no child has exited
  - `WUNTRACED` (0x2): Also return if child is stopped (not needed initially)
  - `WCONTINUED` (0x8): Also return if stopped child has continued (not needed initially)

**Return Values:**
- Success: PID of child that changed state
- `WNOHANG` with no state change: 0
- Error: -1 with errno set
  - `ECHILD` (10): No children to wait for

### 2. Status Word Encoding (Linux Standard)

The status word is a 32-bit integer with encoded information:
```
Bits 0-6:  Signal number that caused exit (0 if normal exit)
Bit 7:     Core dump flag
Bits 8-15: Exit code (if normal exit) or signal number (if signaled)
```

**Macros for decoding (we'll implement in userspace):**
```c
#define WIFEXITED(status)   (((status) & 0x7f) == 0)
#define WEXITSTATUS(status) (((status) >> 8) & 0xff)
#define WIFSIGNALED(status) (((status) & 0x7f) != 0 && ((status) & 0x7f) != 0x7f)
#define WTERMSIG(status)    ((status) & 0x7f)
```

For Breenix initial implementation:
- Normal exit: `status = (exit_code << 8)`
- No signal support initially

### 3. Process States for wait/waitpid

Current Breenix process states (`kernel/src/process/process.rs:26-38`):
```rust
pub enum ProcessState {
    Creating,
    Ready,
    Running,
    Blocked,
    Terminated(i32), // Already stores exit code!
}
```

**Critical insight:** Breenix already has `Terminated(i32)` which stores the exit code.

**Missing state: Zombie**

A zombie process is:
- A process that has terminated
- But parent hasn't called wait() on it yet
- Must keep minimal info (PID, exit status) until parent reaps

**Implementation choice:** Use `Terminated(i32)` as the zombie state. The process stays in the process table with this state until the parent calls waitpid().

### 4. Reference Implementations

**xv6 (simple, educational):**
```c
int wait(uint64 addr) {
    struct proc *pp;
    int havekids, pid;
    struct proc *p = myproc();

    acquire(&wait_lock);

    for(;;) {
        havekids = 0;
        for(pp = proc; pp < &proc[NPROC]; pp++) {
            if(pp->parent == p) {
                havekids = 1;
                if(pp->state == ZOMBIE) {
                    pid = pp->pid;
                    // Copy exit status to user address
                    if(addr != 0 && copyout(...) < 0) {
                        release(&wait_lock);
                        return -1;
                    }
                    freeproc(pp);
                    release(&wait_lock);
                    return pid;
                }
            }
        }

        if(!havekids || killed(p)) {
            release(&wait_lock);
            return -1;
        }

        sleep(p, &wait_lock);  // Wait for child to exit
    }
}
```

**Key patterns from xv6:**
1. Loop until a zombie child is found
2. Copy exit status to userspace if address provided
3. Clean up zombie process
4. Return ECHILD if no children exist
5. Sleep if no zombie children but children exist (blocking wait)

### 5. Blocking vs Non-Blocking

**Non-blocking (WNOHANG):**
- Scan children once
- Return 0 if no zombie children found
- Return PID if zombie found

**Blocking (default):**
- Scan children
- If zombie found, return immediately
- If no zombie but children exist, BLOCK the parent
- Wake parent when any child exits
- Re-scan and return

**For initial implementation:** Start with WNOHANG only (simpler), then add blocking.

### 6. Data Structures Needed

Current process structure already has:
```rust
pub struct Process {
    pub id: ProcessId,
    pub parent: Option<ProcessId>,    // Can find children
    pub children: Vec<ProcessId>,     // Already tracking children!
    pub state: ProcessState,          // Includes Terminated(i32)
    pub exit_code: Option<i32>,       // Redundant with Terminated
    // ...
}
```

**Good news:** The existing structure is sufficient for basic waitpid!

### 7. Orphan Process Handling

When a parent exits before its children:
- Children become "orphans"
- Should be reparented to init (PID 1)
- For now: children can just have parent = None

This is already partially handled in `exit_process()`:
```rust
// TODO: Reparent children to init
```

---

## Implementation Plan

### Phase 1: sys_waitpid (WNOHANG only)

**File:** `kernel/src/syscall/handlers.rs`

```rust
/// sys_waitpid - Wait for process state change
///
/// Arguments (from registers):
///   rdi (arg1) = pid: Which process to wait for (-1 = any child)
///   rsi (arg2) = wstatus: Pointer to status variable (can be 0/null)
///   rdx (arg3) = options: Wait options (WNOHANG = 1)
///
/// Returns:
///   >0 = PID of child that changed state
///   0  = WNOHANG specified and no child changed state
///   -ECHILD = No children to wait for
pub fn sys_waitpid(pid: i64, wstatus: u64, options: u64) -> SyscallResult {
    const WNOHANG: u64 = 1;
    const ECHILD: u64 = 10;

    x86_64::instructions::interrupts::without_interrupts(|| {
        // Get current process
        let current_thread = crate::task::scheduler::current_thread_id()?;
        let manager = crate::process::manager().as_ref()?;
        let (current_pid, current_process) = manager.find_process_by_thread(current_thread)?;

        // Check if we have any children
        if current_process.children.is_empty() {
            return SyscallResult::Err(ECHILD);
        }

        // Find a terminated (zombie) child
        for &child_pid in &current_process.children {
            if let Some(child) = manager.get_process(child_pid) {
                let matches_requested = pid == -1 || pid as u64 == child_pid.as_u64();

                if matches_requested {
                    if let ProcessState::Terminated(exit_code) = child.state {
                        // Found a zombie child!

                        // Write status to userspace if pointer provided
                        if wstatus != 0 {
                            let status_word = (exit_code as u32) << 8; // Normal exit encoding
                            // TODO: Write status_word to userspace address wstatus
                        }

                        // Reap the zombie (remove from process table)
                        // TODO: manager.reap_process(child_pid);

                        return SyscallResult::Ok(child_pid.as_u64());
                    }
                }
            }
        }

        // No zombie children found
        if options & WNOHANG != 0 {
            return SyscallResult::Ok(0); // WNOHANG: return immediately
        }

        // TODO: Block until child exits (Phase 2)
        SyscallResult::Err(ECHILD) // For now, just return error
    })
}
```

### Phase 2: Blocking wait (future)

Add a wait queue and wake mechanism:
1. Parent adds itself to a wait queue
2. Child's exit() wakes waiting parent
3. Parent rescans for zombie children

### Phase 3: Reaping zombies

Add `reap_process()` to ProcessManager:
1. Remove process from parent's children list
2. Remove process from process table
3. Free any remaining resources

---

## Syscall Registration

**File:** `kernel/src/syscall/mod.rs`

Add to `SyscallNumber` enum:
```rust
WaitPid = 7,  // Linux syscall number
```

Add to `TryFrom<u64>`:
```rust
7 => Some(Self::WaitPid),
```

**File:** `kernel/src/syscall/dispatcher.rs`

Add dispatch case:
```rust
SyscallNumber::WaitPid => {
    handlers::sys_waitpid(arg1 as i64, arg2, arg3)
}
```

---

## Userspace Test

**File:** `userspace/tests/waitpid_test.rs`

```rust
//! waitpid test program - tests getpid and waitpid syscalls

#![no_std]
#![no_main]

mod libbreenix;
use libbreenix::{sys_write, sys_exit, sys_getpid, sys_fork, sys_waitpid};

// Test markers for kernel detection
const WAITPID_START: &[u8] = b"=== WAITPID TEST START ===\n";
const WAITPID_PASS: &[u8] = b"=== WAITPID TEST PASS ===\n";
const WAITPID_FAIL: &[u8] = b"=== WAITPID TEST FAIL ===\n";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let _ = sys_write(1, WAITPID_START);

        // Test 1: getpid returns non-zero for user process
        let _ = sys_write(1, b"Test 1: getpid returns non-zero\n");
        let my_pid = sys_getpid();
        if my_pid == 0 {
            let _ = sys_write(1, b"FAIL: getpid returned 0\n");
            let _ = sys_write(1, WAITPID_FAIL);
            sys_exit(1);
        }
        let _ = sys_write(1, b"PASS: getpid returned non-zero PID\n");

        // Test 2: Fork and waitpid
        let _ = sys_write(1, b"Test 2: fork and waitpid\n");
        let fork_result = sys_fork();

        if fork_result == 0 {
            // Child process
            let _ = sys_write(1, b"CHILD: exiting with code 42\n");
            sys_exit(42);
        } else {
            // Parent process
            let _ = sys_write(1, b"PARENT: waiting for child\n");

            // Wait for the child with WNOHANG in a loop
            let mut status: i32 = 0;
            let mut attempts = 0;
            loop {
                let result = sys_waitpid(-1, &mut status as *mut i32 as u64, 1); // WNOHANG
                if result > 0 {
                    // Child reaped
                    let exit_code = (status >> 8) & 0xff;
                    if exit_code == 42 {
                        let _ = sys_write(1, b"PASS: Child exited with code 42\n");
                        let _ = sys_write(1, WAITPID_PASS);
                        sys_exit(0);
                    } else {
                        let _ = sys_write(1, b"FAIL: Wrong exit code\n");
                        let _ = sys_write(1, WAITPID_FAIL);
                        sys_exit(1);
                    }
                }

                attempts += 1;
                if attempts > 1000000 {
                    let _ = sys_write(1, b"FAIL: Timeout waiting for child\n");
                    let _ = sys_write(1, WAITPID_FAIL);
                    sys_exit(1);
                }

                // Small delay
                for _ in 0..1000 {
                    core::ptr::read_volatile(&0u8);
                }
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        let _ = sys_write(1, b"PANIC in waitpid test!\n");
        let _ = sys_write(1, WAITPID_FAIL);
        sys_exit(255);
    }
}
```

Add to `libbreenix.rs`:
```rust
pub const SYS_WAITPID: u64 = 7;

pub unsafe fn sys_waitpid(pid: i64, wstatus: u64, options: u64) -> i64 {
    syscall3(SYS_WAITPID, pid as u64, wstatus, options) as i64
}
```

---

## Agent Handoff Instructions

This research document should be used to spawn implementation agents.

### Agent 1: Kernel Implementation

**Task:** Implement sys_waitpid in the kernel

**Files to modify:**
1. `kernel/src/syscall/mod.rs` - Add WaitPid syscall number
2. `kernel/src/syscall/dispatcher.rs` - Add dispatch case
3. `kernel/src/syscall/handlers.rs` - Implement sys_waitpid
4. `kernel/src/process/manager.rs` - Add reap_process() method

**Key requirements:**
- Follow the implementation plan in Phase 1 above
- Handle WNOHANG option
- Write status to userspace memory using existing page table translation
- Remove child from parent's children list when reaped
- Remove process from process table when reaped
- Return ECHILD (-10) when no children exist

**Test command:**
```bash
cargo run -p xtask -- boot-stages
```

### Agent 2: Userspace Test Implementation

**Task:** Create the waitpid test in userspace

**Files to create/modify:**
1. `userspace/tests/waitpid_test.rs` - New test file
2. `userspace/tests/libbreenix.rs` - Add sys_waitpid wrapper
3. `userspace/build.sh` - Add waitpid_test to build

**Key requirements:**
- Follow the test structure in existing tests (fork_test.rs)
- Use the test markers pattern for kernel detection
- Test getpid returns non-zero
- Test fork + waitpid with WNOHANG
- Verify correct exit code is received

**Build command:**
```bash
./userspace/build.sh
```

### Agent 3: Integration Test

**Task:** Add integration test to test suite

**Files to modify:**
1. `tests/shared_qemu.rs` or create new test file
2. `kernel/src/userspace_test.rs` - Add waitpid_test binary

**Key requirements:**
- Test should detect "WAITPID TEST PASS" in output
- Follow existing test patterns (see fork_test integration)

---

## Memory Layout Reference

For writing status to userspace:
- Use `ProcessPageTable::translate_page()` to get physical address
- Write the status word directly to physical memory
- Or use the existing write mechanism from write syscall

---

## Error Codes Reference

```rust
const ECHILD: i64 = 10;   // No child processes
const EINVAL: i64 = 22;   // Invalid argument
const ESRCH: i64 = 3;     // No such process
```

---

## Completion Criteria

1. `sys_getpid()` continues to work (already passing)
2. `sys_waitpid()` returns child PID when child has exited
3. `sys_waitpid()` returns 0 with WNOHANG when no zombie children
4. `sys_waitpid()` returns -ECHILD when process has no children
5. Status word contains correct exit code
6. Zombie processes are cleaned up after waitpid
7. Test case passes: "WAITPID TEST PASS" appears in output

---

## Next Steps After Implementation

1. Update memory's syscall compliance matrix:
   - Change waitpid from "NOT IMPLEMENTED" to "COMPLETE"

2. Update POSIX roadmap:
   - Mark Phase 2 (wait/waitpid) as complete
   - Note any limitations (e.g., WNOHANG only)

3. Consider future enhancements:
   - Blocking wait (requires wait queue)
   - Process groups
   - Signal-based termination status
