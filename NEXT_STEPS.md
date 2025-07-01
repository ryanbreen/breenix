# Breenix Next Steps

## Current Status
We have successfully achieved userspace execution with working syscalls, SWAPGS support, and basic process management!

### What's Working:
- ✅ Ring 3 (userspace) execution via IRETQ
- ✅ INT 0x80 syscall mechanism
- ✅ Basic syscalls: GetTime, Write, Exit, Yield
- ✅ Memory protection (user vs kernel pages)
- ✅ TSS with kernel stack (RSP0) for handling interrupts from userspace
- ✅ ELF loading and execution
- ✅ SWAPGS support for secure kernel/user GS separation
- ✅ Process struct with full lifecycle management
- ✅ Multiple process creation and management
- ✅ sys_exit implementation with process cleanup via scheduler
- ✅ Integration between ProcessManager and kernel's preemptive scheduler
- ✅ Processes scheduled as tasks in unified scheduler

### What Needs Work:
- ❌ Timer interrupt context switching for userspace processes
- ❌ Process context saving/restoration during preemption
- ❌ Resource cleanup (memory unmapping, file descriptors, etc.)
- ❌ Proper console synchronization (framebuffer conflicts)

## Immediate Next Steps

### 1. Timer Interrupt Context Switching for Userspace (PRIORITY)
- Implement proper context saving when timer interrupt fires in userspace
- Handle privilege level transitions during timer interrupts
- Save/restore full userspace context (all registers, flags, segments)
- Test preemptive multitasking with multiple userspace processes
- Ensure SWAPGS is properly handled during timer interrupts

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