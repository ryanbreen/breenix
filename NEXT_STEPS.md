# Breenix Next Steps

## Current Status
We have successfully achieved userspace execution with working syscalls, SWAPGS support, and basic process management!

### What's Working:
- ✅ Ring 3 (userspace) execution via IRETQ
- ✅ INT 0x80 syscall mechanism
- ✅ Basic syscalls: GetTime, Write, Exit
- ✅ Memory protection (user vs kernel pages)
- ✅ TSS with kernel stack (RSP0) for handling interrupts from userspace
- ✅ ELF loading and execution
- ✅ SWAPGS support for secure kernel/user GS separation
- ✅ Process struct with full lifecycle management
- ✅ Multiple process creation and management
- ✅ sys_exit implementation with process cleanup
- ✅ Basic round-robin scheduling in ProcessManager

### What Needs Work:
- ❌ Integration between ProcessManager and kernel's preemptive scheduler
- ❌ Automatic context switching between processes
- ❌ Process context saving/restoration for preemption
- ❌ Resource cleanup (memory unmapping, file descriptors, etc.)

## Immediate Next Steps

### 1. Integrate ProcessManager with Kernel Scheduler (PRIORITY)
- Bridge the gap between process::ProcessManager and task::scheduler
- Add processes to the kernel's preemptive scheduler as tasks
- Implement process context saving/restoration
- Enable automatic context switching via timer interrupts
- Remove direct execution model in favor of scheduler-based execution

#### Integration Architecture:
**Current State:** Two parallel systems that don't communicate:
1. **Task Scheduler** (`kernel/src/task/scheduler.rs`)
   - Manages kernel threads/tasks
   - Preemptive scheduling with timer interrupts
   - Full context switching support
   - Async executor integration

2. **Process Manager** (`kernel/src/process/`)
   - Manages userspace processes
   - Round-robin scheduling (manual)
   - Direct execution model
   - NOT connected to task scheduler

**Goal:** Unify these systems so processes become special tasks in the kernel scheduler

### 2. Complete Process Resource Management
- Implement memory unmapping on process exit
- Add file descriptor table and cleanup
- Implement child process reparenting to init
- Add signal handling infrastructure

### 3. Enhanced Process Control
- Implement fork() system call
- Add exec() family of system calls
- Implement wait()/waitpid() for process synchronization
- Add process priority and nice values

### 4. Expand Syscall Interface
- mmap/munmap for memory allocation
- fork/exec for process creation
- open/close/read/write for file I/O (initially just console)
- waitpid for process synchronization

## Longer Term Goals

### 6. Userspace Standard Library
- Create a minimal libc
- System call wrappers
- Basic memory allocation (malloc/free using mmap)
- String functions
- printf implementation

### 7. Shell and Utilities
- Simple command-line shell
- Basic utilities (ls, cat, echo, etc.)
- Process launching from shell

### 8. File System
- Virtual file system layer
- Simple file system implementation
- Device files (/dev/console, etc.)

### 9. Signals
- Signal delivery mechanism
- Basic signal handlers
- SIGKILL, SIGTERM, SIGSEGV, etc.

### 10. Security
- Proper privilege checking in syscalls
- Resource limits
- Basic access control

## Architecture Decisions

### Scheduler Integration Plan
1. **Create ProcessTask wrapper** - A Task type that wraps a Process
2. **Modify scheduler to handle ProcessTask** - Extend scheduler to understand userspace tasks
3. **Connect timer interrupts** - Ensure timer interrupts work from userspace
4. **Implement proper context switching** - Save/restore full CPU state for processes
5. **Remove direct execution** - Replace perform_process_exit_switch with scheduler

### Memory Layout
```
0x0000_0000_0000_0000 - 0x0000_7FFF_FFFF_FFFF : User space (128 TiB)
0xFFFF_8000_0000_0000 - 0xFFFF_FFFF_FFFF_FFFF : Kernel space (128 TiB)
```

### Process Memory Map
```
0x0000_0000_0040_0000 : Default program load address (4 MiB)
0x0000_5555_5555_0000 : User stack region
0x0000_7FFF_FFFF_F000 : Stack top (grows down)
```

### Syscall Conventions
- INT 0x80 mechanism (Linux-compatible)
- System V ABI for parameters
- RAX: syscall number and return value
- RDI, RSI, RDX, R10, R8, R9: arguments