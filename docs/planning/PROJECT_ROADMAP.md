# Breenix OS Project Roadmap

This is the master project roadmap for Breenix OS. It consolidates all existing documentation and provides a unified view of completed work and future goals. This document is actively maintained and referenced during development.

## 🚨 CRITICAL DESIGN PRINCIPLE 🚨

**BREENIX IS NOT A TOY - WE BUILD FOR THE LONG HAUL**

Under **NO CIRCUMSTANCES** are we allowed to choose "easy" workarounds that deviate from standard OS development practices. When faced with a choice between:
- **The "hard" way**: Following proper OS design patterns used in Linux, FreeBSD, etc.
- **The "easy" way**: Quick hacks or workarounds that avoid complexity

**We ALWAYS choose the hard way.** This is a real operating system project, not a prototype. Every design decision must be made with production-quality standards in mind. This includes:
- Page table switching during exec() (OS-standard practice)
- Proper copy-on-write fork() implementation
- Standard syscall interfaces and semantics
- Real virtual memory management
- Proper interrupt and exception handling

**If it's good enough for Linux, it's the standard we follow.**

## Current Development Status

### Recently Completed (Last Sprint) - January 2025

- ✅ **🎉 MAJOR EXEC PROGRESS: Fixed Multiple Critical Issues!** (Jan 5 PM)
  - **FIXED: Interrupt Preemption Issue**
    - Process creation was being interrupted, leaving system in inconsistent state
    - Added `without_interrupts` to `create_user_process` for atomic operation
    - Result: Process creation now completes successfully
  - **FIXED: User Mapping Inheritance Bug**
    - New page tables were copying user process mappings from kernel page table
    - Modified `ProcessPageTable::new()` to skip entries with USER_ACCESSIBLE flag
    - Result: No more "PageAlreadyMapped" errors during exec
  - **FIXED: TLB Flush Hang**
    - TLB flush now completes successfully (was hanging at line 159)
    - Process successfully loads ELF and attempts to run
  - **DEBUGGING APPROACH**:
    - Identified issue wasn't TLB flush itself but process being interrupted
    - Deleted obsolete TLB_FLUSH_HANG_ANALYSIS.md as issue was resolved
  - **FILES MODIFIED**:
    - `/kernel/src/process/creation.rs` - interrupt protection
    - `/kernel/src/memory/process_memory.rs` - skip user mappings

- ✅ **🎉 CRITICAL BREAKTHROUGH: Page Table Switching Crash FIXED!** (Jan 5 AM)
  - **ROOT CAUSE IDENTIFIED**: Bootloader maps kernel at 0x10000064360 (PML4 entry 2), not traditional kernel space
  - **PROBLEM**: Previous code only copied PML4 entry 256, missing actual kernel code location
  - **SOLUTION**: Comprehensive fix copies ALL kernel PML4 entries (found 9 vs previous 1)
  - **RESULT**: No more immediate crashes/reboots during page table operations
  - **EVIDENCE**: Exec test now progresses through first ELF segment successfully
  - **FILES MODIFIED**: `/kernel/src/memory/process_memory.rs` - comprehensive kernel mapping strategy

- ✅ **Context Switch Bug Fixes**: Fixed critical userspace execution issues (Jan 4)
  - Added SWAPGS handling to timer interrupt for proper kernel/user transitions
  - Fixed RFLAGS initialization (must have bit 1 set: 0x202 not 0x200)
  - Discovered exec() was hanging due to interrupt deadlock
  - Fixed test code to use with_process_manager() to prevent deadlocks

- ✅ **Exec() Step 1**: Implemented Linux-style ELF loading with physical memory access (Jan 4)
  - No more page table switching during ELF loading
  - Fixed post-exec scheduling hang
  - Discovered and partially fixed stack mapping issue

- ✅ **🎉 MAJOR MILESTONE: Fork() System Call FULLY WORKING!**
  - Implemented complete Unix-style fork() with proper process duplication
  - Full memory copying between parent and child processes (65KB stacks)
  - Process isolation via separate ProcessPageTables for each process
  - Correct fork semantics: parent gets child PID, child gets 0
  - Fixed critical interrupt handling deadlock with try_manager() in interrupt contexts
  - Fixed ProcessPageTable.map_page hang by switching to GlobalFrameAllocator

- ✅ **MAJOR**: Fixed per-process virtual address space isolation (previous sprint)
  - Created ProcessPageTable for complete memory isolation between processes
  - Implemented load_elf_into_page_table() for process-specific ELF loading
  - Added automatic page table switching during context switches

### Recently Completed (This Session) - January 6, 2025

- ✅ **🎉 MONUMENTAL ACHIEVEMENT: HELLO WORLD FROM USERSPACE!**
  - **FIXED: Syscall Register Alignment Bug**
    - Root cause: SyscallFrame struct field order didn't match assembly push order
    - Assembly pushed RAX last (lowest address), but struct expected r15 at lowest
    - Result: All registers misaligned, causing wrong syscall numbers and arguments
    - Solution: Reordered SyscallFrame fields to match actual stack layout
  - **RESULT: First successful userspace hello world!**
    - Process executes from Ring 3 (CS=0x33)
    - Makes proper write syscall with correct parameters
    - Prints "Hello from userspace!" to console
    - Exits cleanly with code 0
  - **SUPPORTING FIXES**:
    - Reverted problematic stack mapping code to restore userspace execution
    - Bypassed serial input issue with auto-test on boot
    - Created proper hello world ELF with write syscall

### Currently Working On

- 🚧 **Fork/Exec/Spawn Integration** - Making userspace work with proper process model
  - Current approach: Direct ELF creation and exec testing
  - Need: Integration with standard fork/exec/spawn patterns

### Immediate Next Steps - START HERE FOR NEW SESSION

**🎯 PRIMARY OBJECTIVE**: Integrate userspace execution with proper fork/exec/spawn model

**CURRENT STATUS:**
```
✅ VICTORY: Userspace execution FULLY WORKING with hello world!
Last achievement: "Hello from userspace!" printed via write syscall
Current invocation: Direct test using test_exec_real_userspace() on boot
Need: Proper fork/exec/spawn integration for real process management
```

1. **STEP 1: Fork/Exec/Spawn Integration** (IMMEDIATE PRIORITY)
   - Current: test_exec creates process directly and runs hello world ELF
   - Goal: Make exectest command work through serial interface
   - Integrate with existing fork() implementation for proper process model
   - Ensure exec() replaces process image correctly in forked children

2. **STEP 2: Verify Correct Binary Loading**
   - Confirm hello_world.elf contains "Hello from second process!" strings
   - Verify it calls sys_write for output and sys_exit(42) not sys_exit(6)
   - Test that 4159-byte binary loads instead of mystery 42-byte one

3. **STEP 3: Complete Success Validation**
   - Should see: sys_write calls with "Hello from second process!" output
   - Should see: sys_exit(42) instead of sys_exit(6)
   - Verify full userspace program execution with expected output

4. **STEP 4: Test Additional Userspace Programs**
   - Test hello_time.elf and other userspace programs
   - Verify multiple programs work with corrected embedding
   - Complete comprehensive userspace execution testing

**📊 PROGRESS ASSESSMENT:**
- **Userspace Execution Infrastructure**: ✅ 100% - PROVEN WORKING!
- **Page Table Management**: ✅ 100% - All switching/mapping functional
- **Syscall Interface**: ✅ 100% - sys_exit called from userspace successfully
- **ELF Loading Process**: ✅ 95% - Loads and executes, just wrong binary
- **Binary Embedding**: ❌ 5% - include_bytes! not picking up correct files
- **Overall Exec()**: 🚧 95% complete (infrastructure proven, just need correct binary)

**📖 REFERENCES**:
- Success evidence: "Context switch on interrupt return: 0 -> 1" + "Syscall 0 from userspace"
- File paths: `/kernel/src/userspace_test.rs` - check include_bytes! paths
- Expected file: `/userspace/tests/hello_world.elf` (4159 bytes)
- Test command: `exectest` via MCP
- Log search: "sys_exit called with code" to see current vs expected exit code


### Threading Infrastructure Status ✅ MAJOR SUCCESS
- **Timer Interrupt Loop**: ✅ FIXED - Eliminated endless terminated thread warnings
- **Idle Transition**: ✅ FIXED - Proper kernel mode setup with idle_loop() function
- **Thread Cleanup**: ✅ FIXED - Terminated threads handled without infinite loops
- **Context Switching**: ✅ WORKING - Clean transitions between userspace and kernel
- **Scheduler Core**: ✅ WORKING - Thread management, ready queue, context saving all functional
- **MCP Integration**: ✅ WORKING - Programmatic testing via HTTP API and real-time logs

### Fork/Exec Implementation Status ✅ MAJOR SUCCESS
- **Fork System Call**: ✅ FULLY WORKING - Complete process duplication with memory copying
- **Memory Isolation**: ✅ Each process has separate ProcessPageTable
- **Stack Copying**: ✅ Full 65KB stack contents copied from parent to child
- **Return Values**: ✅ Correct Unix semantics (parent gets child PID, child gets 0)
- **Process Management**: ✅ ProcessManager tracks parent-child relationships
- **Exec System Call**: 🚧 95% complete - only BSS segment mapping needs fix
- **Test Infrastructure**: ✅ Complete with keyboard triggers and MCP commands

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

### 🚧 Phase 8: Enhanced Process Control (IN PROGRESS - 80% COMPLETE)
- [x] Serial input support for testing
  - [x] UART receive interrupts
  - [x] Serial input stream (async)
  - [x] Command processing via serial
  - [x] Test automation support
- [x] Timer interrupt redesign
  - [x] Minimal timer handler (only timekeeping)
  - [x] Context switching on interrupt return path
  - [x] Proper preemption of userspace processes
- [x] **fork() system call** ✅ FULLY WORKING - January 2025
  - [x] Complete process duplication with memory copying
  - [x] Parent-child process relationships
  - [x] Correct Unix return value semantics
  - [x] Full stack copying between processes
- [🚧] **exec() family of system calls** (95% complete - BSS segment issue)
  - [x] Process address space replacement
  - [x] ELF loading into new page tables
  - [x] Code and data segment loading
  - [🚧] BSS segment mapping hang needs fix
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