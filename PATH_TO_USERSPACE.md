# Path to Userspace Roadmap

This document outlines the roadmap for transforming Breenix from a kernel-only system to a full operating system with multithreading, system calls, and userspace support.

## Current State (as of 2025-01-30)

### ✅ Implemented
- **Memory Management**: Physical frame allocator, virtual paging, heap allocation (1MB), stack allocation with guard pages
- **Interrupt Infrastructure**: GDT with kernel segments and TSS, IDT with exception handlers, PIC for timer/keyboard
- **Async Foundation**: Cooperative async executor (single-threaded), task futures system, waker implementation
- **Thread Local Storage**: TLS using GS segment, per-thread data structures
- **Basic I/O**: Serial output, framebuffer graphics, keyboard input, PIT timer (1000Hz)

### ❌ Missing
- System call infrastructure
- True multithreading with preemptive scheduling
- Process/task management beyond async tasks
- Userspace support and privilege separation
- ELF loading and execution

## Phase 1: System Call Infrastructure

### Goals
Establish the foundation for system calls that will enable communication between userspace (future) and kernel.

### Tasks
1. **Syscall Interrupt Handler**
   - Add INT 0x80 handler to IDT (traditional Linux approach)
   - Alternative: Consider SYSCALL/SYSRET for better performance (Phase 1.5)
   - Handle register preservation and restoration
   - Switch stacks if needed

2. **Syscall Dispatcher**
   - Create syscall number registry
   - Dispatch based on RAX register (syscall number)
   - Pass parameters via RDI, RSI, RDX, R10, R8, R9 (System V ABI)
   - Return value in RAX, error in RDX

3. **Basic Syscalls**
   - `sys_exit` (0): Terminate current task
   - `sys_write` (1): Write to serial/framebuffer
   - `sys_read` (2): Read from keyboard buffer
   - `sys_yield` (3): Yield to scheduler (initially just returns)
   - `sys_get_time` (4): Get current timer ticks

4. **Testing**
   - Kernel-mode syscall tests
   - Parameter validation tests
   - Error handling tests

## Phase 2: Enhanced Task Management

### Goals
Transform the async executor into a full preemptive multitasking system.

### Tasks
1. **Task Control Block (TCB)**
   - Extend beyond current TLS TCB
   - Saved register state (RAX-R15, RIP, RSP, RFLAGS)
   - Kernel/user stack pointers
   - Task state (Running, Ready, Blocked)
   - Priority and scheduling info

2. **Context Switching**
   - Assembly routines for register save/restore
   - Stack switching
   - TLS switching (update GS base)
   - FPU/SSE state handling

3. **Preemptive Scheduler**
   - Timer interrupt drives scheduling
   - Round-robin initially
   - Ready queue management
   - Block/unblock mechanisms

4. **Thread Syscalls**
   - `sys_thread_create`: Spawn new kernel thread
   - `sys_thread_exit`: Terminate thread
   - `sys_thread_yield`: Cooperative yield
   - `sys_thread_join`: Wait for thread completion

## Phase 3: Userspace Support

### Goals
Enable running untrusted code in ring 3 with proper isolation.

### Tasks
1. **GDT User Segments**
   - User code segment (DPL=3)
   - User data segment (DPL=3)
   - Update segment selectors

2. **Privilege Switching**
   - Syscall entry: ring 3 → ring 0
   - Syscall exit: ring 0 → ring 3
   - Update TSS RSP0 on context switch
   - SWAPGS for TLS access

3. **Memory Protection**
   - User/kernel memory split (e.g., >0x8000000000000000 is kernel)
   - User page tables per process
   - Prevent user access to kernel pages
   - Copy-on-write for efficiency

4. **ELF Loader**
   - Parse ELF64 headers
   - Load PT_LOAD segments
   - Set up initial stack
   - Jump to entry point

## Phase 4: Process Management

### Goals
Add process abstraction on top of threads.

### Tasks
1. **Process Structure**
   - Process ID (PID) allocation
   - Parent/child relationships
   - Process memory map
   - Open file table (future)
   - Thread list

2. **Process Syscalls**
   - `sys_fork`: Create child process
   - `sys_exec`: Replace process image
   - `sys_wait`: Wait for child termination
   - `sys_getpid`: Get process ID
   - `sys_getppid`: Get parent PID

3. **Process Lifecycle**
   - Creation and initialization
   - Zombie state handling
   - Resource cleanup
   - Signal delivery (basic)

## Phase 5: Advanced Features

### Goals
Build production-ready OS features.

### Tasks
1. **Synchronization**
   - Kernel spinlocks
   - Sleeping mutexes
   - Condition variables
   - Futexes for userspace

2. **Inter-Process Communication**
   - Pipes
   - Shared memory
   - Message queues
   - Signals

3. **File System**
   - VFS layer
   - Simple RAM filesystem
   - File descriptors
   - Basic I/O syscalls

4. **Device Drivers**
   - Driver framework
   - Block device abstraction
   - Character devices
   - Network stack (future)

## Implementation Guidelines

### Incremental Development
- Each phase builds on the previous
- Test thoroughly before moving on
- Keep legacy code as reference until feature parity

### Safety First
- Validate all syscall parameters
- Bounds check everything
- Use Rust's type system for safety
- Minimize unsafe code

### Performance Considerations
- Start simple, optimize later
- Profile before optimizing
- Consider cache effects
- Balance safety and speed

### Testing Strategy
- Unit tests for components
- Integration tests for syscalls
- Stress tests for scheduler
- Security tests for isolation

## Success Metrics

### Phase 1 Complete When:
- Can make syscalls from kernel mode
- Basic syscalls work correctly
- Tests pass reliably

### Phase 2 Complete When:
- Multiple threads run concurrently
- Preemptive scheduling works
- Context switching is stable

### Phase 3 Complete When:
- Can load and run userspace programs
- Ring 3 code cannot access kernel memory
- Syscalls work from userspace

### Phase 4 Complete When:
- Can fork and exec processes
- Parent/child relationships work
- Process cleanup is correct

### Phase 5 Complete When:
- Have working synchronization primitives
- Basic IPC mechanisms function
- Simple filesystem operational