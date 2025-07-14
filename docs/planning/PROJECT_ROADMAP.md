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

### Currently Working On (January 14 2025)

- 🚧 **Phase 4B**: Building Syscall Infrastructure
  - Syscall dispatch table (4B-1)
  - sys_write for userspace printf (4B-2)
  - sys_exit for clean termination (4B-3)
  - Following narrow PR approach: one syscall, one test, one merge

### Recently Completed (Last Sprint) - January 2025

- ✅ **🎉 PHASE 4A COMPLETE: INT 0x80 Syscall Gate Working!** (Jan 14 2025)
  - **Tagged**: v0.3-syscall-gate-ok
  - **Problem Solved**: Userspace execution hanging at CR3 switch
  - **Root Causes Fixed**:
    - Kernel stack not mapped in process page table (fixed PML4 entry 2)
    - Kernel code not mapped in process page table (fixed PML4 entry 0)
  - **Key Achievements**:
    - INT 0x80 successfully fires from userspace with DPL=3
    - Syscall handler receives control with test value RAX=0x1234
    - Integration test `integ_syscall_gate` passes consistently
    - Guard-rail tests documented to prevent regressions
  - **Technical Details**:
    - IDT[0x80] set to privilege level 3 for user access
    - Both kernel mappings remain non-USER accessible (maintaining isolation)
    - Systematic debugging with INT3 test revealed execution issues
  - **Evidence**: Logs show "SYSCALL_ENTRY: Received syscall from userspace!" and "TEST_MARKER: SYSCALL_OK"

### Recently Completed (Previous Sprints) - July 2025

- ✅ **🎉 ACHIEVED ZERO COMPILER WARNINGS!** (Jul 11 2025)
  - **Starting Point**: 85+ compiler warnings cluttering the build output
  - **Goal**: Completely clean compilation with zero warnings
  - **Approach**: Systematic cleanup without breaking functionality
  - **Key Changes**:
    - Removed ~2,500 lines of dead code (unused functions, modules)
    - Fixed all conditional compilation issues with proper #[cfg] blocks
    - Resolved unreachable code warnings in syscall handlers
    - Added #[allow(dead_code)] only for legitimate future APIs
    - Fixed a critical userspace execution regression during cleanup
  - **Result**:
    - ✅ Zero compiler warnings - completely clean build
    - ✅ All tests pass, including critical multiple_processes test
    - ✅ Improved code clarity and maintainability
    - ✅ Better separation between testing and production code
  - **Evidence**: `cargo build` produces no warnings, all functionality preserved

- ✅ **🎉 SOLVED KERNEL STACK ISOLATION ISSUE!** (Jul 9 2025)
  - **Root Cause**: Kernel stacks were mapped per-process, causing double faults during page table switches
  - **Problem**: When switching page tables in timer interrupt, kernel was still on current thread's stack
  - **Solution**: Implemented production-grade global kernel page table architecture:
    - **Global kernel PDPT**: All PML4 entries 256-511 point to shared kernel_pdpt
    - **O(1) kernel mappings**: New map_kernel_page() API makes mappings instantly visible to all processes
    - **Bitmap stack allocator**: Kernel stacks at 0xffffc900_0000_0000 with 8KB stacks + 4KB guard pages
    - **Per-CPU emergency stacks**: IST[0] uses per-CPU stacks at 0xffffc980_xxxx_xxxx
  - **Result**:
    - ✅ Multiple concurrent processes run without double faults
    - ✅ Both hello_time processes execute successfully
    - ✅ Clean exits with code 0
    - ✅ Context switches work perfectly between processes
  - **Evidence**: Logs show "Hello from userspace!" from both processes, no double faults

- ✅ **🎉 FIXED USERSPACE EXECUTION!** (Jul 7 2025)
  - **Root Cause**: Process page tables missing critical kernel mappings
  - **Issue**: Timer interrupt handler crashed after switching to process page table
  - **Solution**: Copy ALL kernel-only PML4 entries (those without USER_ACCESSIBLE flag)
  - **Result**:
    - ✅ Userspace processes now execute successfully
    - ✅ "Hello from userspace!" messages print with timestamps
    - ✅ System calls work (exit syscall completes successfully)
    - ✅ Multiple concurrent processes run without conflicts
  - **Evidence**: Logs show all 3 hello_time processes executed and exited cleanly

- ✅ **Fixed Scheduler Not Running User Threads** (Jul 7 2025)
  - **Issue**: Timer interrupt waited for full quantum (10 ticks) before scheduling user threads
  - **Solution**: Modified timer interrupt to immediately set need_resched when user threads exist
  - **Result**: Scheduler now properly schedules threads 1, 2, 3, 4 in round-robin fashion
  - **Evidence**: Logs show successful context switches between all user threads

- ✅ **🎉 BREAKTHROUGH: Complete Page Table Isolation Implementation!** (Jul 7 2025)
  - **FIXED: "PageAlreadyMapped" error preventing multiple concurrent processes**
    - Root cause: All processes shared L3 page tables, causing conflicts on second process creation
    - Solution: Implemented selective deep page table copying for proper process isolation
    - Architecture: Each process gets independent L3/L2/L1 tables (OS-standard approach)
    - Performance: Only essential kernel mappings copied (first 16MB), avoids bootloader huge page overhead
    - Result: ✅ Multiple processes can be created without conflicts
  - **ACHIEVED: Proper OS-Standard Page Table Architecture**
    - ✅ Kernel space entries (256+): Shared safely between processes
    - ✅ Essential low memory (entry 0): Deep copied selectively for isolation
    - ✅ Other user space entries: Clean address spaces for each process
    - ✅ No "PageAlreadyMapped" errors in latest kernel runs
    - ✅ Context switching works between multiple processes
    - Evidence: Logs confirm process 1 and 2 created successfully with isolated page tables

- ✅ **🎉 MAJOR MILESTONE: Fork+Exec Pattern FULLY FUNCTIONAL!** (Jul 7 2025)
  - **FIXED: Critical Timer Interrupt #1 Deadlock**
    - Root cause: Logger timestamp calculation calling time functions during interrupt context
    - Solution: Temporarily disabled timestamp logging during interrupt context
    - Result: ✅ Timer interrupts now work perfectly, kernel runs userspace processes
  - **FIXED: ProcessPageTable.translate_page() Bug**
    - Was returning None for ALL userspace addresses due to incorrect offset
    - Fixed by using frame allocator's physical memory offset
    - Result: ✅ Parent process can access memory after fork
  - **ACHIEVED: Complete Fork+Exec Implementation**
    - ✅ Fork system call works correctly (parent gets PID, child gets 0)
    - ✅ Fork test program executes from userspace and calls fork()
    - ✅ Child process successfully calls exec() to load hello_time.elf
    - ✅ "Hello from userspace!" prints from exec'd process
    - ✅ Fixed userspace address validation to accept stack range
    - Evidence: Logs show complete fork→exec→output chain working

- ✅ **🎉 CRITICAL BREAKTHROUGH: Direct Userspace Execution FULLY WORKING!** (Jul 6 2025)
  - **FIXED: Double Fault on int 0x80 from Userspace**
    - Root cause: Kernel stack not mapped in userspace page tables
    - Ring 3 → Ring 0 transitions failed when accessing unmapped kernel stack
    - Solution: Added `copy_kernel_stack_to_process()` to map kernel stack in process page tables
    - Result: ✅ Userspace programs can now successfully call `int 0x80` and make syscalls
    - Evidence: "Hello from userspace!" output with successful syscall completion
  - **MANDATORY REGRESSION TEST ESTABLISHED**
    - Direct execution test (`test_direct_execution()`) MUST pass on every kernel boot
    - Located in `kernel/src/test_exec.rs` - validates core syscall functionality
    - Success criteria: Must see "🎉 USERSPACE SYSCALL" and "Hello from userspace!" output
    - **CRITICAL**: No fork/exec work until this test consistently passes

- ✅ **🎉 MAJOR EXEC PROGRESS: Fixed Multiple Critical Issues!** (Jul 5 2025)
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

- ✅ **🎉 CRITICAL BREAKTHROUGH: Page Table Switching Crash FIXED!** (Jul 4 2025)
  - **ROOT CAUSE IDENTIFIED**: Bootloader maps kernel at 0x10000064360 (PML4 entry 2), not traditional kernel space
  - **PROBLEM**: Previous code only copied PML4 entry 256, missing actual kernel code location
  - **SOLUTION**: Comprehensive fix copies ALL kernel PML4 entries (found 9 vs previous 1)
  - **RESULT**: No more immediate crashes/reboots during page table operations
  - **EVIDENCE**: Exec test now progresses through first ELF segment successfully
  - **FILES MODIFIED**: `/kernel/src/memory/process_memory.rs` - comprehensive kernel mapping strategy

- ✅ **Context Switch Bug Fixes**: Fixed critical userspace execution issues (Jul 4 2025)
  - Added SWAPGS handling to timer interrupt for proper kernel/user transitions
  - Fixed RFLAGS initialization (must have bit 1 set: 0x202 not 0x200)
  - Discovered exec() was hanging due to interrupt deadlock
  - Fixed test code to use with_process_manager() to prevent deadlocks

- ✅ **Exec() Step 1**: Implemented Linux-style ELF loading with physical memory access (Jul 4 2025)
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


### Completed - Page Table Isolation Architecture

- ✅ **SOLVED: Complete Page Table Isolation Implementation**
  - **The OS-Standard Solution Implemented**: Each process gets independent page tables
  - **Architecture**:
    - **Kernel space (PML4 entries 256+)**: Shared safely between processes
    - **Entry 0 (low memory)**: Selectively deep copied (only essential kernel mappings)
    - **Other entries**: Clean address spaces for isolation
  - **Performance Optimized**: Only copies first 16MB containing kernel code, skips bootloader huge pages
  - **Result**: ✅ No more "PageAlreadyMapped" errors, multiple processes can coexist
       - Map out current PML4 entry usage

    2. **Deep Copy Implementation**:
       - When creating new process, allocate fresh L4 table
       - For kernel entries (256-511), must deep copy:
         - Allocate new L3 tables (don't share!)
         - Copy L3 entries, allocating new L2 tables
         - Only share L1 entries (actual physical pages)
       - For user entries (0-255), start empty

    3. **Fix Current Bug**:
       - Current code just copies PML4 entries (shares L3 tables)
       - Must implement recursive copying to create isolated tables
       - This is why second process gets "already mapped" errors

    4. **Why This Is The Only Correct Solution**:
       - **Security**: Process isolation is fundamental to OS security
       - **Stability**: Shared tables mean one process can corrupt another
       - **Standards**: Every production OS uses this approach (Linux, BSD, Windows)
       - **No Shortcuts**: Any "workaround" violates our core principle

  - **Expected Outcome**:
    - Each process has completely independent page tables
    - Multiple processes can load code at the same virtual address
    - True memory isolation between processes
    - Foundation for proper security and stability

### Recently Completed (This Session) - July 7, 2025

- ✅ **Fixed ProcessPageTable.translate_page() Bug**
  - **DISCOVERED**: ProcessPageTable was fundamentally broken
    - translate_page() returned None for ALL userspace addresses
    - Root cause: Incorrect physical offset calculation
    - Fixed by using frame allocator's physical offset
  - **RESULT**: Parent process can now access memory after fork

- ✅ **Fork+Exec Integration Successfully Implemented**
  - **IMPLEMENTED: sys_exec syscall wrapper in userspace**
    - Added SYS_EXEC constant (11) to libbreenix.rs
    - Added syscall2 function for 2-argument syscalls
    - Added sys_exec wrapper function: `sys_exec(path: &str, args: &str) -> u64`

  - **IMPLEMENTED: Fork test integration with exec**
    - Modified fork_test.rs to exec hello_time.elf in child process
    - Child process now calls `sys_exec("/userspace/tests/hello_time.elf", "")`
    - Parent process continues with original iterations
    - Architecture follows standard Unix fork+exec pattern

- ✅ **CRITICAL DISCOVERY: Fork Test Was Already Working!**
  - **EVIDENCE**: Found working execution in focused_run.log:
    - ✅ Userspace execution: "Before fork - PID: 2" printed from Ring 3
    - ✅ Fork syscall: "Calling fork()..." from userspace (CS=0x33, RIP=0x100000ea)
    - ✅ Fork success: Parent PID 2 created child PID 3 successfully
    - ✅ Memory isolation: Proper page table copying and process separation
    - ✅ Process states: Both parent and child processes executing correctly

### Previously Working On - July 7, 2025

- 🚧 **CRITICAL: Page Fault in copy_from_user Function**
  - **Current Status**: Fork works but parent process crashes when accessing userspace memory
  - **Evidence from focused_run.log**:
    - Fork completed successfully (parent PID 2, child PID 3)
    - Parent tried sys_write with buf_ptr=0x10001082, count=33
    - Page fault in copy_from_user at address 0x10001082
    - Error: PAGE FAULT with ErrorCode(0x0) - read access to unmapped page
  - **Root Cause**: copy_from_user page table switching has memory access issue
  - **Impact**: Prevents fork test from completing and printing exec output

- 🚧 **CRITICAL: Scheduler Not Executing Processes in Recent Runs**
  - **Evidence**: Recent manual test created processes but no userspace execution
  - **What Works**: Process creation, scheduling infrastructure, timer interrupts
  - **What Fails**: Processes added to ready queue but never switched to
  - **Difference**: Earlier focused_run.log showed working execution, recent runs don't
  - **Investigation Needed**: Scheduler decision-making or context switching regression

### Immediate Next Steps

1. **Enhanced Userspace Features**
   - Implement wait()/waitpid() system calls for process synchronization
   - Add process exit status collection
   - Parent-child relationship tracking

2. **Shell Development**
   - Create basic shell that can fork+exec commands
   - Command parsing and execution
   - Built-in commands (cd, exit, etc.)

3. **Memory Management Enhancements**
   - Implement mmap/munmap for dynamic memory allocation
   - Add brk/sbrk for heap management
   - Copy-on-write (COW) page optimization for fork()

4. **Performance Optimizations**
   - Profile context switching overhead
   - Optimize TLB flushing strategies
   - Implement lazy FPU state saving


**CURRENT STATUS:**
```
✅ BREAKTHROUGH: Direct userspace execution FULLY WORKING!
✅ ESTABLISHED: Mandatory regression test for direct execution
📋 NEXT PHASE: Fork/exec implementation and validation
```

**CRITICAL REQUIREMENT**: Before proceeding with ANY fork/exec work, we MUST:
1. ✅ Confirm direct execution test passes consistently on every boot
2. 📋 Run additional validation to ensure no regressions in syscall infrastructure
3. 📋 Only then proceed to fork/exec implementation

### Immediate Next Steps - START HERE FOR NEW SESSION

1. **CRITICAL PRIORITY: Implement Proper Page Table Isolation**
   - **Current Bug**: Processes share L3 tables, causing "already mapped" errors
   - **Root Cause**: ProcessPageTable only copies PML4 entries, not deeper tables
   - **Solution**: Implement deep copying of page tables
   - **Steps**:
     a. Modify ProcessPageTable::new() to allocate new L3/L2 tables
     b. For each kernel PML4 entry, recursively copy table hierarchy
     c. Never share L3/L2/L1 structures between processes
     d. Test with multiple concurrent processes
   - **No Workarounds**: This is the only acceptable solution

2. **Document Current Memory Layout**
   - Analyze bootloader mappings to understand kernel placement
   - Document which PML4 entries contain kernel vs user mappings
   - Create memory map showing current layout issues

3. **Future: Higher-Half Kernel Migration**
   - Long-term goal: Move kernel to addresses >= 0xFFFF800000000000
   - Requires bootloader modifications
   - Will completely solve kernel/user address conflicts

**📊 PHASE 8 COMPLETION ASSESSMENT:**
- **Fork System Call**: ✅ 100% - Complete with proper semantics
- **Exec System Call**: ✅ 100% - Fully functional program replacement
- **Fork+Exec Pattern**: ✅ 100% - Successfully tested end-to-end
- **Process Management**: ✅ 95% - Missing only wait/waitpid
- **Memory Isolation**: ✅ 90% - Works but limited by shared L3 tables
- **Overall Phase 8**: ✅ COMPLETE - All critical features implemented

**🎯 ACHIEVEMENT UNLOCKED**: Breenix can now create processes Unix-style with fork() and exec()!


### Threading Infrastructure Status ✅ MAJOR SUCCESS
- **Timer Interrupt Loop**: ✅ FIXED - Eliminated endless terminated thread warnings
- **Idle Transition**: ✅ FIXED - Proper kernel mode setup with idle_loop() function
- **Thread Cleanup**: ✅ FIXED - Terminated threads handled without infinite loops
- **Context Switching**: ✅ WORKING - Clean transitions between userspace and kernel
- **Scheduler Core**: ✅ WORKING - Thread management, ready queue, context saving all functional
- **MCP Integration**: ✅ WORKING - Programmatic testing via HTTP API and real-time logs

### Fork/Exec Implementation Status - ✅ COMPLETE
- **Direct Userspace Execution**: ✅ FULLY WORKING - Ring 3 processes can make syscalls successfully
- **Fork System Call**: ✅ FULLY WORKING - Parent gets child PID, child gets 0
- **Exec System Call**: ✅ FULLY WORKING - Processes can load and execute new programs
- **Fork/Exec Pattern**: ✅ TESTED AND WORKING - Child process successfully execs new program
- **Memory Isolation**: ✅ Each process has separate ProcessPageTable with kernel stack mapping
- **Process Management**: ✅ ProcessManager tracks process relationships
- **Test Infrastructure**: ✅ COMPLETE - All tests passing, fork+exec chain validated
- **Limitation**: One process type at a time due to shared L3 page tables

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
- **Userspace**: ✅ **BREAKTHROUGH** - Direct Ring 3 execution with working int 0x80 syscalls
- **Processes**: Basic process management (fork/exec pattern needs validation)

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

### ✅ Phase 8: Enhanced Process Control (COMPLETE - Jul 7 2025)
- [x] Serial input support for testing
  - [x] UART receive interrupts
  - [x] Serial input stream (async)
  - [x] Command processing via serial
  - [x] Test automation support
- [x] Timer interrupt redesign
  - [x] Minimal timer handler (only timekeeping)
  - [x] Context switching on interrupt return path
  - [x] Proper preemption of userspace processes
- [x] **fork() system call** ✅ FULLY WORKING - July 2025
  - [x] Complete process duplication with memory copying
  - [x] Parent-child process relationships
  - [x] Correct Unix return value semantics
  - [x] Full stack copying between processes
  - [x] ✅ PROVEN: Fork test executes from userspace and creates child process successfully
- [x] **exec() family of system calls** ✅ COMPLETE
  - [x] Process address space replacement
  - [x] ELF loading into new page tables
  - [x] Code and data segment loading
  - [x] Fork+exec integration fully functional
  - [x] Processes can exec new programs successfully
  - Note: Limited to one process type at a time due to shared L3 tables
- [ ] wait()/waitpid() for process synchronization
- [ ] Process priority and scheduling classes
- [ ] Process memory unmapping on exit
- [ ] Process resource limits

### ✅ Phase 8.5: Kernel Stack Isolation (COMPLETE - Jul 9 2025)
- [x] **Global Kernel Page Tables**
  - [x] Implemented shared kernel_pdpt for PML4 entries 256-511
  - [x] All kernel mappings instantly visible to all processes
  - [x] No more per-process kernel stack mapping
- [x] **Kernel Stack Management**
  - [x] Bitmap-based allocator at 0xffffc900_0000_0000
  - [x] 8 KiB stacks with 4 KiB guard pages
  - [x] O(1) allocation and deallocation
- [x] **Per-CPU Emergency Stacks**
  - [x] IST[0] uses per-CPU stacks at 0xffffc980_xxxx_xxxx
  - [x] Prevents stack corruption during double faults
- [x] **Testing and Validation**
  - [x] Multiple concurrent processes run without double faults
  - [x] Context switches work perfectly between processes
  - [x] Both userspace and kernel threads properly isolated

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

### 🚨 MANDATORY PRE-COMMIT TESTING 🚨

**NEVER commit without running the FULL test suite!**

**Before EVERY Commit:**
1. **Run the complete test suite**: `cargo test`
2. **Verify ALL tests pass**:
   - `test_divide_by_zero` - Exception handling
   - `test_invalid_opcode` - Exception handling
   - `test_page_fault` - Exception handling
   - `test_multiple_processes` - 5 concurrent processes
3. **Check test output details**:
   - `test_multiple_processes`: Must see 5 "Hello from userspace!" messages
   - Exception tests: Must see TEST_MARKER output
4. **If ANY test fails**: DO NOT COMMIT - fix the issue first
5. **When adding features**: ADD A TEST to the test harness

### Branch Strategy
- `main` branch for stable code
- Feature branches for all development
- Pull requests for code review
- Comprehensive commit messages
- Co-authorship credits (Ryan Breen + Claude)

### Code Quality Standards
- **Run `cargo test` after EVERY change**
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
4. **Process Creation OS** ✅ - Has fork() and exec() (NEW!)
5. **Storage OS** 🎯 - Can load programs from disk (NEXT)
6. **POSIX-compliant OS** - Pass POSIX conformance tests
7. **Self-Hosting OS** - Can compile itself

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