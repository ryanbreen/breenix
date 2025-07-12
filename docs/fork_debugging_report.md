# Fork System Call Debugging Report

## Executive Summary

The Breenix kernel's fork() implementation appears to create child processes correctly but the child process never executes any userspace instructions after returning from the fork syscall. The child gets stuck at the same instruction pointer (RIP) indefinitely, despite being scheduled and appearing to return to userspace.

## Core Issue

**Problem**: After fork(), the child process:
1. Is created with correct context (RIP=0x10000018, RAX=0)
2. Is scheduled by the scheduler
3. Appears to return to userspace (CS=0x33, from_userspace=true)
4. But NEVER executes a single instruction - RIP remains at 0x10000018 forever

**Expected Behavior**: 
- Parent should get child PID in RAX and continue execution
- Child should get 0 in RAX and take the child branch

**Actual Behavior**:
- Parent works correctly (gets PID 5, continues execution)
- Child is stuck at RIP=0x10000018 and never progresses

## Test Program Analysis

The test program (`simple_wait_test`) does:
```rust
let pid = sys_fork() as i64;
if pid == 0 {
    // Child process
    print("Child: Hello from child!\n");
    print("Child: Exiting with status 42\n");
    sys_exit(42);
} else if pid > 0 {
    // Parent process  
    print("Parent: Forked child, waiting...\n");
    // ... wait for child
}
```

Disassembly around fork:
```asm
10000013: 6a 05        pushq   $0x5        # syscall number for fork
10000015: 58           popq    %rax
10000016: cd 80        int     $0x80       # fork syscall
10000018: 48 85 c0     testq   %rax, %rax  # test if RAX is 0 (child)
1000001b: 75 25        jne     0x10000042  # jump if not zero (parent)
1000001d: ...          # child code continues here
```

## Key Findings

### 1. Fork Implementation
The fork implementation correctly:
- Creates new page table for child
- Clones memory (16 pages copied, 2 shared read-only)
- Shares code pages as read-only (0x10000000 mapped to physical 0x61e000)
- Copies syscall frame context to child
- Sets child RAX to 0, parent RAX to child PID

### 2. Context Switching Evidence
From logs showing the child (thread 5) being scheduled:
```
[ INFO] Restored userspace context for thread 5: RIP=0x10000018, RSP=0x555555593ff8, RAX=0x0, CS=0x33, SS=0x2b, RFLAGS=0x10202
[ INFO] Scheduled page table switch for process 5 on return: frame=0x63e000
[DEBUG] TSS RSP0 updated: 0xffffc9000001b000 -> 0xffffc9000002d000
[ INFO] get_next_page_table: Returning page table frame 0x63e000 for switch
```

Then immediately:
```
[DEBUG] Context switch on interrupt return: 5 -> 4
[DEBUG] Context switch: from_userspace=true, CS=0x33
[TRACE] Saved userspace context for thread 5: RIP=0x10000018, RSP=0x555555593ff8, RAX=0x0
```

### 3. Critical Observation
The child's RIP NEVER changes from 0x10000018 across multiple timer interrupts:
- Timer runs at 10Hz (100ms intervals)
- Child is scheduled multiple times
- Each time saved context shows same RIP=0x10000018
- Parent progresses from 0x10000018 to 0x10000059 (works correctly)

### 4. No Errors Detected
- No double faults
- No page faults  
- No exceptions
- Page table switch appears to work (frame 0x63e000)
- Code page is properly shared between parent/child

## Code Paths

### Fork Syscall Handler
```rust
// kernel/src/syscall/handlers.rs
pub fn sys_fork_with_frame(frame: &super::handler::SyscallFrame) -> SyscallResult {
    sys_fork_with_full_frame(frame)
}

fn sys_fork_with_full_frame(frame: &super::handler::SyscallFrame) -> SyscallResult {
    // ... create child page table ...
    match manager.fork_process_with_frame(parent_pid, frame, child_page_table) {
        Ok(child_pid) => {
            // ... spawn child thread to scheduler ...
            SyscallResult::Ok(child_pid.as_u64())
        }
    }
}
```

### Process Manager Fork
```rust
// kernel/src/process/manager.rs
pub fn fork_process_with_frame(&mut self, parent_pid: ProcessId, 
                               frame: &crate::syscall::handler::SyscallFrame,
                               mut child_page_table: Box<ProcessPageTable>) -> Result<ProcessId, &'static str> {
    // ... clone memory ...
    self.complete_fork_with_frame(parent_pid, child_pid, frame, child_process)
}

fn complete_fork_with_frame(&mut self, parent_pid: ProcessId, child_pid: ProcessId, 
                           frame: &crate::syscall::handler::SyscallFrame,
                           mut child_process: Process) -> Result<ProcessId, &'static str> {
    // Create child thread with syscall frame context
    let mut child_thread = Thread::new(...);
    
    // Copy all registers from syscall frame
    child_thread.context.rax = 0;  // Fork returns 0 in child
    child_thread.context.rbx = frame.rbx;
    child_thread.context.rcx = frame.rcx;
    // ... copy all other registers ...
    child_thread.context.rip = frame.rip;  // 0x10000018
    child_thread.context.rsp = frame.rsp;
    child_thread.context.cs = frame.cs;    // 0x33 (user code)
    child_thread.context.ss = frame.ss;    // 0x2b (user data)
    child_thread.context.rflags = frame.rflags; // 0x10202 (interrupts enabled)
}
```

### Syscall Return Path
```asm
; kernel/src/syscall/entry.asm
syscall_entry:
    ; Save registers, call handler, etc...
    
    ; Check if we need to switch page tables before returning
    call get_next_page_table
    test rax, rax
    jz .no_page_table_switch
    
    ; Switch to the process page table
    mov cr3, rax
    ; TLB flush code...
    
.no_page_table_switch:
    ; Restore registers
    iretq  ; Return to userspace
```

## Hypotheses

### 1. Page Table Switch Issue
Although logs show page table switch to frame 0x63e000, the switch might not be effective:
- TLB might have stale entries
- Switch happens too late in return path
- Child's page table might be missing critical mappings

### 2. CPU State Issue  
The child thread might not have proper CPU state for userspace execution:
- Missing segment descriptor setup
- Incorrect privilege level transition
- Some CPU flag preventing execution

### 3. Instruction Fetch Problem
The CPU might be unable to fetch the instruction at 0x10000018:
- Despite shared mapping, child might not have execute permission
- Cache coherency issue after page table switch
- Hardware-specific issue with instruction prefetch

### 4. Interrupt Timing Issue
Child might be getting interrupted before executing any instruction:
- Timer interrupt pending during IRETQ
- Some other interrupt source preventing execution
- Interrupt flag handling issue

### 5. Context Save/Restore Issue
The way we save/restore context for newly created threads might differ:
- First time scheduling a forked thread might have special case bug
- Register state might be corrupted during save/restore
- Stack pointer or other critical register might be wrong

## Questions for External Expert

1. **Page Table Switching**: Is switching CR3 in the syscall return path (after restoring registers but before IRETQ) the correct approach? Could this cause instruction fetch issues?

2. **TLB Flushing**: The code does elaborate TLB flushing (clearing/setting CR4 PGE bit). Could this cause issues with instruction fetch immediately after IRETQ?

3. **First Instruction After Fork**: What are the x86-64 architectural requirements for the first instruction executed by a newly forked process? Are there any special considerations?

4. **Interrupt Timing**: If a timer interrupt is pending when IRETQ executes, would it prevent even a single instruction from executing in the child?

5. **Debug Suggestions**: What debug techniques would you recommend to determine if the child is actually executing the instruction at 0x10000018 but appearing stuck due to measurement?

## Additional Context

- Kernel uses INT 0x80 for syscalls (not SYSCALL/SYSRET)
- Timer runs at 10Hz (100ms intervals)  
- No hardware debugging available (QEMU environment)
- Parent process works correctly, only child is affected
- Other process creation (not fork) works correctly
- Memory is properly mapped (confirmed by parent executing same code)

## Next Steps

Need expert guidance on:
1. How to verify if instruction at 0x10000018 is actually executing
2. Whether page table switch timing could cause this issue
3. Any x86-64 specific quirks with fork and IRETQ
4. Debugging techniques to isolate the root cause