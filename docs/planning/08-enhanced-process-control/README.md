# Phase 8: Enhanced Process Control

This directory contains planning documents for enhanced process control features in Breenix.

## Current Status
- Phase: 8
- Status: 🚧 IN PROGRESS

## Goals
Implement the core POSIX process control system calls that enable:
- Process creation without exec (fork)
- Program replacement (exec family)
- Process synchronization (wait/waitpid)
- Process hierarchy and resource management

## Planned Features

### Serial Input (Testing Infrastructure)
- [ ] UART receive interrupt handler
- [ ] Async serial input stream
- [ ] Command processing from serial
- [ ] Test automation support
- **Purpose**: Enable remote testing of process control features

### Process Control Syscalls
- [ ] fork() system call - Create child process
- [ ] execve() system call - Execute new program
- [ ] exec family wrappers (execl, execlp, etc.)
- [ ] wait() system call - Wait for any child
- [ ] waitpid() system call - Wait for specific child
- [ ] Process priority and nice values
- [ ] Process memory unmapping on exit
- [ ] Process resource limits (rlimit)

## Key Design Decisions
- **fork()**: Implement with copy-on-write (COW) for efficiency
- **exec()**: Replace process image while keeping PID
- **wait()**: Handle zombie processes correctly
- **Resources**: Track and cleanup all process resources

## Implementation Order
1. **Serial input** - Testing infrastructure for everything else
2. **fork()** - Most critical, enables everything else
3. **wait()/waitpid()** - Needed to prevent zombies
4. **execve()** - Core exec implementation
5. **exec wrappers** - Convenience functions
6. **Resource management** - Cleanup and limits

## Testing Strategy
- Unit tests for each syscall
- Process tree tests (parent/child relationships)
- Resource leak tests
- Signal delivery tests (SIGCHLD)
- Stress tests with many processes

## Success Criteria
- Can create process trees with fork()
- Can replace process image with exec()
- Parent can wait for child termination
- No resource leaks on process exit
- Proper zombie process handling