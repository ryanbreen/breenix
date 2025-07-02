# Breenix OS Project Roadmap

This is the master project roadmap for Breenix OS. It consolidates all existing documentation and provides a unified view of completed work and future goals. This document is actively maintained and referenced during development.

## Current Development Status

### Recently Completed (Last Sprint)
- ✅ **MAJOR**: Fixed timer interrupt doom loop with terminated threads
- ✅ **MAJOR**: Fixed idle thread context switching and kernel mode transitions  
- ✅ **MAJOR**: Implemented proper thread cleanup without infinite loops
- ✅ Added fork() system call skeleton with thread ID debugging
- ✅ Enhanced timer interrupt handler to prevent userspace execution loops
- ✅ Added idle thread transitions with proper assembly context setup
- ✅ Implemented MCP server connectivity and HTTP API for programmatic testing
- ✅ Built comprehensive fork testing infrastructure (Ctrl+F, forktest command)
- ✅ Added scheduler improvements to handle terminated thread states properly

### Currently Working On (Phase 8: Enhanced Process Control)
- 🚧 **Timer scheduling aggressiveness** - Userspace threads get preempted too quickly
- 🚧 Fork() implementation ready but needs timer scheduling fix to test properly
- 🚧 Thread 3 starts userspace execution but immediately switches to idle thread 0
- 🚧 Need to adjust timer frequency or scheduling policy for userspace execution

### Immediate Next Steps
1. **Fix aggressive timer scheduling** - Allow userspace threads to run before preemption
2. **Test fork() system call** - Verify thread ID tracking and basic functionality  
3. **Implement actual fork() logic** - Process duplication with copy-on-write memory
4. **Add wait()/waitpid()** - Process synchronization and zombie prevention
5. **Implement execve()** - Program replacement within existing process

### Threading Infrastructure Status ✅
- **Timer Interrupt Loop**: FIXED - No more endless terminated thread warnings
- **Idle Transition**: FIXED - Proper kernel mode setup with idle_loop() function
- **Thread Cleanup**: FIXED - Terminated threads handled without infinite loops
- **Context Switching**: WORKING - Between userspace and kernel modes
- **Scheduler**: WORKING - But too aggressive with preemption timing

### Fork Implementation Status
- **System Call**: `sys_fork()` implemented in `kernel/src/syscall/handlers.rs:184-235`
- **Test Infrastructure**: Complete with Ctrl+F keyboard trigger and MCP commands
- **Current Behavior**: Returns fake PID 42, logs thread context for debugging
- **Threading Issue**: Userspace never runs long enough to call fork() due to aggressive scheduling
- **Next**: Fix timer scheduling to allow userspace execution, then test fork()

### Next Major Milestone
**Phase 11: Disk I/O** - Enable dynamic program loading from disk instead of embedding in kernel

### High-Interest Features
- **Serial Input** - Would enable remote control and automated testing
- **Network Stack** - The day we send our first ping packet will be epic! 🎉

## Project Vision

Breenix is an experimental x86_64 operating system written in Rust, focusing on:
- Modern OS architecture with safety-first design
- Preemptive multitasking with process isolation
- **POSIX compliance** as a primary goal
- Clean separation between kernel and userspace
- Comprehensive testing at every level

### POSIX Compliance Strategy
We aim for IEEE Std 1003.1-2017 (POSIX.1-2017) compliance, focusing on:
- **Base Definitions** - Headers, types, constants
- **System Interfaces** - Core system calls
- **Shell & Utilities** - Command line tools
- **Realtime** - Optional realtime extensions (later phases)

## Status Overview

### Current Capabilities
- **Boot**: UEFI and BIOS boot support via bootloader crate
- **Display**: Framebuffer graphics with text rendering
- **Memory**: Full virtual memory with paging, heap allocation, and guard pages
- **Interrupts**: Complete interrupt handling with timer and keyboard
- **I/O**: Serial console with input/output, keyboard input with async processing
- **Scheduling**: Preemptive round-robin scheduler with context switching
- **Userspace**: Ring 3 execution with syscalls and ELF loading
- **Processes**: Basic process management with scheduler integration

### Key Statistics
- Memory: 94 MiB usable physical memory
- Heap: 1024 KiB with bump allocator
- Timer: 1000Hz (1ms ticks)
- Tests: 25+ integration tests

## Phase Tracking

### ✅ Phase 0: Foundation (COMPLETE)
- [x] Project setup with custom x86_64 target
- [x] Basic boot with bootloader crate
- [x] Framebuffer initialization
- [x] Serial output for debugging
- [x] Logger with dual output (framebuffer + serial)
- [x] Early boot message buffering

### ✅ Phase 1: Core Kernel Infrastructure (COMPLETE)
- [x] GDT with kernel/user segments and TSS
- [x] IDT with exception handlers
- [x] PIC configuration for hardware interrupts
- [x] Physical memory management (frame allocator)
- [x] Virtual memory with paging
- [x] Heap allocation with `#[global_allocator]`
- [x] Stack allocation with guard pages
- [x] Double fault handling with dedicated stack

### ✅ Phase 2: Device Drivers & I/O (COMPLETE)
- [x] PIT timer (1000Hz)
- [x] RTC for wall clock time
- [x] PS/2 keyboard with full scancode processing
- [x] Async keyboard stream with interrupt-driven queue
- [x] Serial UART 16550 driver
- [x] Time tracking (monotonic + real time)

### ✅ Phase 3: Async Infrastructure (COMPLETE)
- [x] Async executor with Future support
- [x] Task spawning and management
- [x] Waker implementation with ArrayQueue
- [x] Async keyboard input stream
- [x] Cooperative multitasking foundation

### ✅ Phase 4: Thread Management (COMPLETE)
- [x] Thread Local Storage (TLS) using GS segment
- [x] Thread control blocks (TCB)
- [x] Context switching assembly routines
- [x] Preemptive scheduler (round-robin)
- [x] Timer-driven preemption
- [x] Idle thread for CPU idle states
- [x] Kernel thread spawning

### ✅ Phase 5: System Calls (COMPLETE)
- [x] INT 0x80 syscall mechanism
- [x] Syscall dispatcher and handler
- [x] Register preservation/restoration
- [x] Basic syscalls implemented:
  - [x] sys_exit (0) - Process termination
  - [x] sys_write (1) - Console output
  - [x] sys_read (2) - Input (returns 0)
  - [x] sys_yield (3) - Yield to scheduler
  - [x] sys_get_time (4) - Get system ticks
- [x] SWAPGS support for secure kernel entry

### ✅ Phase 6: Userspace Execution (COMPLETE)
- [x] Ring 3 privilege level support
- [x] User segment setup in GDT
- [x] IRETQ-based ring transitions
- [x] ELF64 loader for executables
- [x] Memory protection (user/kernel split)
- [x] TSS RSP0 for kernel stack on interrupts
- [x] Userspace test programs (hello_time, counter, spinner)

### ✅ Phase 7: Process Management (COMPLETE)
- [x] Process structure with lifecycle states
- [x] Process ID (PID) allocation
- [x] ProcessManager for tracking processes
- [x] Integration with thread scheduler
- [x] Process termination and cleanup
- [x] Multiple concurrent process execution
- [x] Process context saving/restoration
- [x] Timer interrupt handling for userspace
- [x] Keyboard responsiveness after process exit

### 🚧 Phase 8: Enhanced Process Control (IN PROGRESS)
- [ ] Serial input support for testing
  - [ ] UART receive interrupts
  - [ ] Serial input stream (async)
  - [ ] Command processing via serial
  - [ ] Test automation support
- [ ] fork() system call
- [ ] exec() family of system calls
- [ ] wait()/waitpid() for process synchronization
- [ ] Process priority and scheduling classes
- [ ] Process memory unmapping on exit
- [ ] Process resource limits

### 📋 Phase 9: Memory Management Syscalls (PLANNED)
- [ ] mmap/munmap for memory allocation
- [ ] brk/sbrk for heap management
- [ ] mprotect for changing page permissions
- [ ] Demand paging
- [ ] Copy-on-write (COW) pages
- [ ] Shared memory regions

### 📋 Phase 10: Basic Filesystem (PLANNED)
- [ ] Virtual File System (VFS) layer
- [ ] File descriptor table per process
- [ ] Device files (/dev/console, /dev/null)
- [ ] Simple RAM filesystem
- [ ] open/close/read/write syscalls
- [ ] Basic directory operations

### 📋 Phase 11: Disk I/O & Persistent Storage (PLANNED)
- [ ] ATA PIO driver for disk access
- [ ] Block device abstraction
- [ ] Buffer cache for disk blocks
- [ ] FAT32 filesystem (read-only initially)
- [ ] exec() loading programs from disk
- [ ] Persistent filesystem mounting

### 📋 Phase 12: Inter-Process Communication (PLANNED)
- [ ] Pipes (anonymous)
- [ ] Named pipes (FIFOs)
- [ ] Shared memory segments
- [ ] Basic signal infrastructure
- [ ] Message queues

### 📋 Phase 13: Userspace Runtime (PLANNED)
- [ ] Minimal libc implementation
- [ ] Dynamic memory allocator (malloc/free)
- [ ] String and memory functions
- [ ] printf family of functions
- [ ] System call wrappers
- [ ] Static linking support

### 📋 Phase 14: Shell & Utilities (PLANNED)
- [ ] Basic command shell
- [ ] Process launching from shell
- [ ] Built-in commands (cd, exit)
- [ ] Command line parsing
- [ ] Basic utilities:
  - [ ] echo
  - [ ] cat
  - [ ] ls
  - [ ] ps
  - [ ] kill

### 📋 Phase 15: Advanced Features (FUTURE)
- [ ] SMP (multicore) support
- [ ] POSIX threads (pthreads)
- [ ] Advanced scheduling (CFS-like)
- [ ] Memory swapping to disk
- [ ] Dynamic linking
- [ ] Security features (ASLR, NX, etc.)

### 🎯 High-Priority Features (Not Yet Phased)

#### Serial Input
- [ ] UART receive interrupt handler
- [ ] Input buffer management
- [ ] Serial console with line editing
- [ ] Remote command execution via serial
- [ ] Integration with shell when available
- **Benefits**: Remote debugging, automated testing, headless operation

#### Network Stack
- [ ] Ethernet driver (e1000 or RTL8139)
- [ ] Basic packet send/receive
- [ ] ARP protocol
- [ ] IP layer (IPv4 initially)
- [ ] ICMP (ping!)
- [ ] UDP sockets
- [ ] TCP/IP stack
- [ ] Socket syscalls
- **First Milestone**: Send a ping packet! 🎉

## Current Development Focus

### Immediate Priority: Disk I/O Foundation
Based on our discussion, the next major milestone is implementing disk I/O to enable dynamic program loading:

1. **Simple RAM Disk** - Store programs in memory to simulate disk
2. **Read-only FAT32** - Well-documented, good tooling
3. **ATA PIO Driver** - Simpler than AHCI, works in QEMU
4. **exec() from disk** - Load programs dynamically
5. **Remove static embedding** - No more include_bytes!

This will transform Breenix from a kernel with built-in programs to a true OS that can load and execute programs from storage.

## Testing Strategy

### Current Test Infrastructure
- Shared QEMU runner for efficiency (~45 seconds for all tests)
- Integration tests with real kernel execution
- POST completion detection for test synchronization
- Visual testing mode with QEMU display
- Categories: Boot, Memory, Interrupts, Logging, Timer, System

### Testing Philosophy
- Write tests early in development cycle
- Test both success and failure cases
- Integration tests over unit tests for OS code
- Use feature flags for test-specific code paths
- Maintain fast test execution times

## Development Workflow

### Branch Strategy
- `main` branch for stable code
- Feature branches for all development
- Pull requests for code review
- Comprehensive commit messages
- Co-authorship credits (Ryan Breen + Claude)

### Code Quality Standards
- Zero compiler warnings policy
- Clean up dead code before merging
- Follow existing code patterns
- Document complex algorithms
- Safety comments for unsafe blocks

### Build System
- Custom x86_64-breenix target
- Separate targets for kernel and userspace
- Automated userspace program building
- UEFI and BIOS disk image generation

## Technical Decisions

### Memory Layout
```
Virtual Memory Map:
0x0000_0000_0000_0000 - 0x0000_7FFF_FFFF_FFFF : User space (128 TiB)
0xFFFF_8000_0000_0000 - 0xFFFF_FFFF_FFFF_FFFF : Kernel space (128 TiB)

Physical Memory Offset: 0x28000000000 (provided by bootloader)

Process Memory Layout:
0x1000_0000 : Default program load address (256 MB)
0x5555_5555_4000 : User stack top (grows down)
```

### Interrupt/Syscall Mechanism
- INT 0x80 for system calls (Linux-compatible)
- Hardware interrupts via PIC (no APIC yet)
- Software interrupts for exceptions
- SWAPGS for kernel/user GS separation

### Scheduler Design
- Preemptive round-robin scheduling
- Timer-driven (1000Hz tick rate)
- Separate idle thread (ID 0)
- Integration with async executor
- Userspace thread priority over kernel tasks

## POSIX Compliance Tracking

### System Calls Progress
Track implementation of POSIX.1-2017 required system interfaces:

#### Process Management
- [ ] fork() - Create new process
- [ ] exec*() - Execute program (execve, execl, etc.)
- [ ] _exit() - Terminate process
- [x] exit() - Terminate with cleanup (partial)
- [ ] wait() - Wait for child
- [ ] waitpid() - Wait for specific child
- [ ] getpid() - Get process ID
- [ ] getppid() - Get parent process ID
- [ ] kill() - Send signal

#### File Operations
- [ ] open() - Open file
- [ ] close() - Close file descriptor
- [x] read() - Read from file (stub)
- [x] write() - Write to file (console only)
- [ ] lseek() - Set file position
- [ ] dup() - Duplicate file descriptor
- [ ] dup2() - Duplicate to specific FD
- [ ] pipe() - Create pipe
- [ ] stat() - Get file status
- [ ] fstat() - Get file status by FD

#### Memory Management
- [ ] brk() - Change data segment size
- [ ] sbrk() - Change data segment size
- [ ] mmap() - Map memory
- [ ] munmap() - Unmap memory
- [ ] mprotect() - Change memory protection

#### Time
- [x] time() - Get time (via get_time)
- [ ] gettimeofday() - Get time of day
- [ ] clock_gettime() - Get clock time
- [ ] nanosleep() - High resolution sleep

#### Signals (Basic Set)
- [ ] signal() - Set signal handler
- [ ] sigaction() - Examine/change signal action
- [ ] sigprocmask() - Change signal mask
- [ ] raise() - Send signal to self
- [ ] pause() - Wait for signal

### POSIX Headers Required
- [ ] <unistd.h> - Standard symbolic constants and types
- [ ] <sys/types.h> - Data types
- [ ] <sys/stat.h> - File status
- [ ] <fcntl.h> - File control options
- [ ] <signal.h> - Signals
- [ ] <time.h> - Time types
- [ ] <errno.h> - Error codes
- [ ] <stdio.h> - Standard I/O
- [ ] <stdlib.h> - Standard library
- [ ] <string.h> - String operations

### Utilities Progress
Minimum POSIX.1-2017 utilities we should implement:
- [ ] sh - Shell
- [ ] echo - Display text
- [ ] cat - Concatenate files
- [ ] ls - List directory
- [ ] cp - Copy files
- [ ] mv - Move files
- [ ] rm - Remove files
- [ ] mkdir - Make directories
- [ ] rmdir - Remove directories
- [ ] pwd - Print working directory
- [ ] cd - Change directory (shell builtin)
- [ ] ps - Process status
- [ ] kill - Terminate processes
- [ ] true/false - Return success/failure
- [ ] test/[ - Evaluate expressions

## Success Metrics

### Per-Phase Validation
Each phase is considered complete when:
- All features are implemented and tested
- Integration tests pass reliably
- No regressions in existing functionality
- Documentation is updated
- Code review is complete

### Overall Project Milestones
1. **"Hello World" OS** ✅ - Can print to screen
2. **Interactive OS** ✅ - Can respond to keyboard
3. **Multitasking OS** ✅ - Can run multiple programs
4. **Storage OS** 🎯 - Can load programs from disk (NEXT)
5. **POSIX-compliant OS** - Pass POSIX conformance tests
6. **Self-Hosting OS** - Can compile itself

## Resources and References

### External Dependencies
- `bootloader` crate for boot process
- `x86_64` crate for CPU abstractions
- `uart_16550` for serial port
- `pic8259` for interrupt controller
- `embedded-graphics` for framebuffer

### Project Documentation
Detailed planning and design documents are organized in `docs/planning/`:
- Each roadmap phase has its own numbered directory (00-15)
- Historical decisions and implementation notes preserved
- POSIX compliance tracking and strategy
- Legacy migration guides

Key documents:
- `docs/planning/NEXT_STEPS.md` - Immediate priorities
- `docs/planning/posix-compliance/` - POSIX compliance strategy
- `docs/planning/legacy-migration/` - Feature comparison with legacy code
- `docs/planning/06-userspace-execution/` - Userspace implementation details
- `docs/planning/11-disk-io-persistent-storage/` - Disk I/O planning (next major milestone)

### External Documentation
- Intel SDM for x86_64 architecture
- OSDev Wiki for general concepts
- Rust Book for language features
- Blog OS by Philipp Oppermann for inspiration
- IEEE Std 1003.1-2017 for POSIX specifications

---

Last Updated: 2025-02-01
Next Review: After Phase 8 completion
Current Development Status should be updated after each PR merge