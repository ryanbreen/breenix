# Breenix Next Steps

## Current Status
We have successfully achieved userspace execution with working syscalls! 

### What's Working:
- ✅ Ring 3 (userspace) execution via IRETQ
- ✅ INT 0x80 syscall mechanism
- ✅ Basic syscalls: GetTime, Write
- ✅ Memory protection (user vs kernel pages)
- ✅ TSS with kernel stack (RSP0) for handling interrupts from userspace
- ✅ ELF loading and execution

### What Needs Work:
- ❌ SWAPGS support (currently disabled)
- ❌ Proper program termination (sys_exit)
- ❌ Scheduler integration
- ❌ Multiple processes/threads

## Immediate Next Steps

### 1. Implement sys_exit (In Progress)
- Add proper program termination instead of HLT
- Return control to kernel
- Clean up resources

### 2. Fix SWAPGS Support
- Set up MSR_KERNEL_GS_BASE for kernel TLS
- Set up MSR_GS_BASE for userspace (can be 0 initially)
- Re-enable SWAPGS in syscall entry/exit

### 3. Basic Process Management
- Create a Process struct to track userspace programs
- Implement process creation from ELF
- Track process state (running, terminated)
- Basic process cleanup on exit

### 4. Scheduler Integration
- Connect userspace execution to the task scheduler
- Allow multiple userspace processes
- Implement preemptive multitasking for userspace
- Handle timer interrupts from userspace

### 5. Expand Syscall Interface
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