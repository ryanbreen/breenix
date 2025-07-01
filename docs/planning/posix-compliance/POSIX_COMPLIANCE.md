# POSIX Compliance Strategy for Breenix

## Overview

This document outlines our strategy for achieving POSIX.1-2017 compliance in Breenix OS. POSIX compliance is a primary goal that will enable running existing UNIX software without modification.

## Why POSIX?

1. **Proven Design** - POSIX represents decades of UNIX evolution
2. **Software Ecosystem** - Thousands of programs just work
3. **Clear Specifications** - IEEE standards define exact behavior
4. **Educational Value** - Learn real-world OS interfaces
5. **Test Suites** - Validation tools ensure correctness

## Compliance Levels

### Phase 1: Core POSIX (Current Focus)
Essential system calls for basic POSIX programs:
- Process management (fork, exec, wait)
- Basic file I/O (open, read, write, close)
- Memory management (mmap, munmap)
- Time functions
- Basic signals

### Phase 2: POSIX.1-2017 Base
Full base specification compliance:
- Complete file system interface
- Process groups and sessions
- Terminal control
- User/group management
- Extended signals

### Phase 3: POSIX Options
Optional features we'll implement:
- Threads (POSIX.1c)
- Realtime (POSIX.1b)
- Advanced IPC
- File locking

## Implementation Strategy

### 1. System Call Interface
```rust
// POSIX-compliant system call numbers (Linux compatible)
pub enum SyscallNumber {
    Exit = 1,      // _exit()
    Fork = 2,      // fork()
    Read = 3,      // read()
    Write = 4,     // write()
    Open = 5,      // open()
    Close = 6,     // close()
    Waitpid = 7,   // waitpid()
    // ... following Linux x86_64 syscall numbers
}
```

### 2. Error Handling
POSIX defines specific errno values:
```rust
#[repr(i32)]
pub enum Errno {
    EPERM = 1,    // Operation not permitted
    ENOENT = 2,   // No such file or directory
    ESRCH = 3,    // No such process
    EINTR = 4,    // Interrupted system call
    EIO = 5,      // I/O error
    // ... full POSIX errno set
}
```

### 3. Type Definitions
Match POSIX type requirements:
```rust
pub type pid_t = i32;      // Process ID
pub type uid_t = u32;      // User ID
pub type gid_t = u32;      // Group ID
pub type mode_t = u32;     // File mode
pub type off_t = i64;      // File offset
pub type ssize_t = isize;  // Signed size
```

### 4. File Descriptors
POSIX mandates specific FD behavior:
- 0 = stdin
- 1 = stdout  
- 2 = stderr
- Lowest available FD on open()
- Inheritance across fork()
- Proper dup/dup2 semantics

## Testing Strategy

### 1. Unit Tests
Test each syscall in isolation:
```rust
#[test]
fn test_fork_basic() {
    let pid = unsafe { syscall!(FORK) };
    if pid == 0 {
        // Child process
        exit(0);
    } else {
        // Parent process
        assert!(pid > 0);
        waitpid(pid, null_mut(), 0);
    }
}
```

### 2. POSIX Test Suite
Use Open POSIX Test Suite:
- Automated conformance testing
- Covers all POSIX interfaces
- Results tracking

### 3. Real Program Testing
Test with standard UNIX utilities:
- coreutils (ls, cat, echo, etc.)
- bash shell
- make
- gcc (eventual goal)

## Current Status

### Implemented (Partial)
- [x] write() - Console only
- [x] read() - Returns 0 (no input)
- [x] exit() - Basic termination
- [x] time functions - Via get_time

### High Priority
- [ ] fork() - Critical for POSIX
- [ ] exec() - Load programs
- [ ] wait()/waitpid() - Process sync
- [ ] open()/close() - File descriptors
- [ ] mmap()/munmap() - Memory management

### Medium Priority  
- [ ] getpid()/getppid()
- [ ] dup()/dup2()
- [ ] pipe()
- [ ] basic signals

## Challenges & Solutions

### 1. Fork Implementation
Challenge: Copying entire address space
Solution: Copy-on-write (COW) pages

### 2. Signal Delivery
Challenge: Interrupting system calls
Solution: Restartable system calls with EINTR

### 3. File Descriptors
Challenge: Sharing across processes
Solution: Reference-counted FD table

### 4. Process Groups
Challenge: Job control semantics
Solution: Process group table with session leaders

## Milestones

### Milestone 1: "Minimal POSIX"
Can run simple C programs:
```c
#include <unistd.h>
int main() {
    write(1, "Hello POSIX!\n", 13);
    return 0;
}
```

### Milestone 2: "Shell Ready"
Can run a POSIX shell:
- fork/exec working
- pipes functional
- job control basics

### Milestone 3: "Self-Hosting"
Can compile programs:
- Full file system
- mmap for dynamic memory
- Sufficient syscalls for GCC/Clang

## Resources

### Specifications
- IEEE Std 1003.1-2017 (POSIX.1-2017)
- Single UNIX Specification v4
- Linux man pages (for practical reference)

### Test Suites
- Open POSIX Test Suite
- Linux Test Project (LTP)
- POSIX compliance test from IEEE

### Reference Implementations
- FreeBSD (clean POSIX implementation)
- Linux (practical compatibility)
- illumos (SunOS heritage)

## FAQ

**Q: Why Linux syscall numbers?**
A: Compatibility with existing toolchains and binaries. We can compile programs with standard tools.

**Q: Full compliance or subset?**
A: Start with core subset, expand based on needs. Full compliance is the goal but not required initially.

**Q: What about Linux-specific syscalls?**
A: Implement common ones (like clone) for compatibility, but focus on POSIX first.

**Q: How long will this take?**
A: Core POSIX: ~6 months. Full compliance: ~2 years. But usable much sooner!

---

Remember: POSIX is a journey, not a destination. Each implemented syscall brings us closer to running real software!