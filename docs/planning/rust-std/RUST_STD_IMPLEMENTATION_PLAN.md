# Rust Standard Library Implementation Plan for Breenix

## Executive Summary

This document describes the implementation path for supporting the **real Rust standard library** (`std`) on Breenix OS. The approach follows Redox OS's proven model: implement a libc in Rust that provides C ABI wrappers around existing syscalls, enabling Rust std to work unmodified.

**Goal**: Enable Breenix userspace programs to use standard Rust:
```rust
// No #![no_std] required!
fn main() {
    println!("Hello from real Rust std!");
    let v: Vec<i32> = vec![1, 2, 3];
    std::process::exit(0);
}
```

## Architecture

### Target Architecture
```
┌─────────────────────────────────────────────────────────────┐
│                    Userspace Programs                        │
├─────────────────────────────────────────────────────────────┤
│  Rust Programs (std)  │  C Programs                         │
│  - use std::*         │  - #include <stdio.h>               │
│  - println!           │  - printf()                         │
│  - Vec, String        │  - malloc/free                      │
└───────────┬───────────┴───────────┬─────────────────────────┘
            │                       │
            ▼                       ▼
┌─────────────────────────────────────────────────────────────┐
│                    libbreenix-libc                          │
│              (Rust implementation, C ABI)                   │
│                                                             │
│  #[no_mangle] pub extern "C" fn write(...) -> ssize_t      │
│  #[no_mangle] pub extern "C" fn fork() -> pid_t            │
│  #[no_mangle] pub extern "C" fn __errno_location() -> *i32 │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                      libbreenix                             │
│              (Existing Rust syscall wrappers)               │
│                                                             │
│  pub fn write(fd: i32, buf: &[u8]) -> isize                │
│  pub fn fork() -> isize                                     │
│  pub fn syscall3(nr: u64, a: u64, b: u64, c: u64) -> i64   │
└───────────────────────────┬─────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Breenix Kernel                           │
│                   (int 0x80 syscalls)                       │
└─────────────────────────────────────────────────────────────┘
```

### Why This Approach

1. **Rust std expects libc**: On Unix targets, Rust's std library calls libc functions (write, read, fork, mmap, etc.) via FFI. It does NOT make raw syscalls directly.

2. **Leverage existing work**: libbreenix already implements ~80% of needed syscalls. We just need C ABI wrappers.

3. **Memory safety**: A Rust-based libc prevents buffer overflows and use-after-free bugs in the C library itself.

4. **Proven model**: Redox OS uses this exact approach with their `relibc` library, successfully supporting real-world programs.

5. **Dual benefit**: The same libc enables both Rust std AND C program support.

### What NOT To Do

**Do NOT create a "std-like" library** (e.g., `breenix_std`) that:
- Requires programs to use `#![no_std]`
- Provides custom APIs like `breenix_std::println!`
- Is not the real Rust standard library

This approach is a dead end that doesn't lead to real std support.

## Current State

### libbreenix Syscall Coverage (~80% complete)

| Category | Implemented | Missing |
|----------|-------------|---------|
| **Process** | exit, fork, exec, getpid, gettid, waitpid, yield | clone (threads) |
| **I/O** | read, write, close, pipe, pipe2, dup, dup2, poll, select, fcntl | readv, writev, pread, pwrite |
| **File System** | open, lseek, fstat, getdents64 | stat (path), mkdir, rmdir, chdir, getcwd, rename, unlink |
| **Memory** | brk, sbrk, mmap, munmap, mprotect | mremap |
| **Time** | clock_gettime (REALTIME, MONOTONIC) | nanosleep, gettimeofday |
| **Signals** | sigaction, sigprocmask, kill, sigreturn | sigaltstack, sigsuspend |
| **Terminal** | isatty, tcgetattr, tcsetattr | Full termios |
| **Networking** | socket, bind, sendto, recvfrom | connect, listen, accept |

### Target Specification

Current `x86_64-breenix.json`:
```json
{
  "os": "breenix",  // Changed from "none"
  ...
}
```

**Required changes for std support**:
```json
{
  "llvm-target": "x86_64-unknown-breenix",
  "os": "breenix",
  "target-family": ["unix"],  // ADD THIS - tells std to use sys/unix/
  "env": "",
  ...
}
```

## Implementation Phases

### Phase 1: Minimal std Support (Target: 2-3 weeks)

**Goal**: Get `println!`, `std::process::exit`, basic I/O working

**Deliverables**:
1. `libbreenix-libc` crate with staticlib output
2. Essential C ABI functions
3. Working "Hello World" using real std

**Required C ABI Functions**:

```rust
// libs/libbreenix-libc/src/lib.rs
#![no_std]

// Essential I/O
#[no_mangle]
pub unsafe extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> isize {
    libbreenix::io::write(fd, core::slice::from_raw_parts(buf, count))
}

#[no_mangle]
pub unsafe extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    libbreenix::io::read(fd, core::slice::from_raw_parts_mut(buf, count))
}

#[no_mangle]
pub unsafe extern "C" fn close(fd: i32) -> i32 {
    libbreenix::io::close(fd)
}

// Process control
#[no_mangle]
pub extern "C" fn exit(status: i32) -> ! {
    libbreenix::process::exit(status)
}

#[no_mangle]
pub extern "C" fn _exit(status: i32) -> ! {
    libbreenix::process::exit(status)
}

#[no_mangle]
pub extern "C" fn getpid() -> i32 {
    libbreenix::process::getpid() as i32
}

// Error handling (CRITICAL for std)
#[thread_local]
static mut ERRNO: i32 = 0;

#[no_mangle]
pub extern "C" fn __errno_location() -> *mut i32 {
    unsafe { &raw mut ERRNO }
}

// Memory allocation (for std's allocator)
#[no_mangle]
pub unsafe extern "C" fn mmap(
    addr: *mut u8,
    len: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> *mut u8 {
    libbreenix::memory::mmap(addr, len, prot, flags, fd, offset)
}

#[no_mangle]
pub unsafe extern "C" fn munmap(addr: *mut u8, len: usize) -> i32 {
    libbreenix::memory::munmap(addr, len)
}
```

**Cargo Configuration**:
```toml
# libs/libbreenix-libc/Cargo.toml
[package]
name = "libbreenix-libc"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["staticlib"]
name = "c"  # Produces libc.a

[dependencies]
libbreenix = { path = "../libbreenix" }
```

**Build Command**:
```bash
# Build libc
cargo build --release -p libbreenix-libc

# Build std for Breenix
RUSTFLAGS="-L native=/path/to/libbreenix-libc/target/release" \
cargo build -Z build-std=std,panic_abort --target x86_64-breenix.json
```

**Test Program**:
```rust
// userspace/tests/hello_std_real.rs
// NO #![no_std] - using real std!

fn main() {
    println!("Hello from REAL Rust std!");

    let numbers: Vec<i32> = vec![1, 2, 3, 4, 5];
    let sum: i32 = numbers.iter().sum();
    println!("Sum: {}", sum);

    std::process::exit(0);
}
```

### Phase 2: File System Support (Target: 2-3 weeks)

**Goal**: Get `std::fs` working

**Required Syscalls to Add**:
- `stat(path, buf)` - Get file metadata by path
- `mkdir(path, mode)` - Create directory
- `rmdir(path)` - Remove directory
- `chdir(path)` - Change working directory
- `getcwd(buf, size)` - Get current working directory
- `rename(old, new)` - Rename file
- `unlink(path)` - Delete file

**Required C ABI Functions**:
```rust
#[no_mangle]
pub unsafe extern "C" fn stat(path: *const i8, buf: *mut Stat) -> i32 { ... }

#[no_mangle]
pub unsafe extern "C" fn mkdir(path: *const i8, mode: u32) -> i32 { ... }

#[no_mangle]
pub unsafe extern "C" fn getcwd(buf: *mut i8, size: usize) -> *mut i8 { ... }

// ... etc
```

**Test Program**:
```rust
use std::fs;

fn main() {
    // Read file
    let contents = fs::read_to_string("/hello.txt").unwrap();
    println!("File contents: {}", contents);

    // List directory
    for entry in fs::read_dir("/").unwrap() {
        let entry = entry.unwrap();
        println!("  {}", entry.path().display());
    }
}
```

### Phase 3: Full POSIX libc (Target: 3-4 weeks)

**Goal**: Support C programs with stdio, stdlib, string.h

**Components**:

1. **stdio.h** - Buffered I/O
   - `FILE` struct with buffer management
   - `fopen`, `fclose`, `fread`, `fwrite`, `fflush`
   - `printf`, `fprintf`, `sprintf` (use Rust formatting)
   - `scanf`, `fscanf` (basic implementation)

2. **stdlib.h** - General utilities
   - `malloc`, `free`, `realloc`, `calloc` (wrap mmap or use dlmalloc)
   - `exit`, `abort`, `atexit`
   - `atoi`, `atol`, `strtol`, `strtoul`
   - `qsort`, `bsearch`
   - `getenv`, `setenv`

3. **string.h** - String operations
   - `strlen`, `strcpy`, `strncpy`, `strcat`, `strncat`
   - `strcmp`, `strncmp`, `strchr`, `strrchr`, `strstr`
   - `memcpy`, `memmove`, `memset`, `memcmp`

**Test**: Compile and run a simple C program:
```c
#include <stdio.h>
#include <stdlib.h>

int main() {
    printf("Hello from C!\n");

    int *arr = malloc(10 * sizeof(int));
    for (int i = 0; i < 10; i++) {
        arr[i] = i * i;
    }

    for (int i = 0; i < 10; i++) {
        printf("%d ", arr[i]);
    }
    printf("\n");

    free(arr);
    return 0;
}
```

### Phase 4: Threading Support (Target: 4-6 weeks)

**Goal**: Get `std::thread` working

**Kernel Requirements**:
- `clone()` syscall with `CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND`
- Thread-local storage (TLS) via `%fs` register
- `futex()` syscall for synchronization primitives

**libc Requirements**:
- `pthread_create`, `pthread_join`, `pthread_detach`
- `pthread_mutex_init/lock/unlock/destroy`
- `pthread_cond_init/wait/signal/broadcast/destroy`
- Thread-local errno via TLS

**Test Program**:
```rust
use std::thread;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn main() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];

    for _ in 0..4 {
        let counter = Arc::clone(&counter);
        let handle = thread::spawn(move || {
            for _ in 0..1000 {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("Counter: {}", counter.load(Ordering::SeqCst));
}
```

## Directory Structure

```
libs/
├── libbreenix/                 # Existing - Rust syscall wrappers
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── syscall.rs          # Raw syscall interface
│       ├── process.rs          # Process operations
│       ├── io.rs               # I/O operations
│       ├── memory.rs           # Memory operations
│       ├── time.rs             # Time operations
│       ├── signal.rs           # Signal handling
│       └── ...
│
├── libbreenix-libc/            # NEW - C ABI wrapper
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs              # Crate root, re-exports
│       ├── errno.rs            # __errno_location()
│       ├── unistd.rs           # write, read, fork, exec, etc.
│       ├── fcntl.rs            # open, close, fcntl, etc.
│       ├── sys_stat.rs         # stat, fstat, mkdir, etc.
│       ├── sys_mman.rs         # mmap, munmap, mprotect
│       ├── time.rs             # clock_gettime, nanosleep
│       ├── signal.rs           # sigaction, kill, etc.
│       ├── stdio.rs            # Phase 3: FILE, printf, etc.
│       ├── stdlib.rs           # Phase 3: malloc, free, etc.
│       └── string.rs           # Phase 3: strlen, memcpy, etc.
│
└── [DEPRECATED] breenix_std/   # DELETE - wrong approach
```

## Build System Integration

### Cargo Configuration

```toml
# .cargo/config.toml
[unstable]
build-std = ["std", "core", "alloc", "panic_abort"]
build-std-features = ["compiler-builtins-mem"]

[target.x86_64-breenix]
linker = "rust-lld"
rustflags = [
    "-C", "link-arg=-nostartfiles",
    "-L", "native=libs/libbreenix-libc/target/x86_64-breenix/release",
]
```

### Target Specification

```json
// x86_64-breenix.json
{
  "llvm-target": "x86_64-unknown-breenix",
  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128",
  "arch": "x86_64",
  "target-endian": "little",
  "target-pointer-width": "64",
  "target-c-int-width": "32",
  "os": "breenix",
  "target-family": ["unix"],
  "env": "",
  "vendor": "unknown",
  "linker-flavor": "ld.lld",
  "linker": "rust-lld",
  "executables": true,
  "has-rpath": false,
  "position-independent-executables": true,
  "static-position-independent-executables": true,
  "relro-level": "full",
  "panic-strategy": "abort",
  "disable-redzone": true,
  "features": "-mmx,-sse,+soft-float"
}
```

### Build Script

```bash
#!/bin/bash
# scripts/build_std_userspace.sh

set -e

# Build libc first
echo "Building libbreenix-libc..."
cargo build --release -p libbreenix-libc --target x86_64-breenix.json

# Build userspace programs with real std
echo "Building userspace with std..."
cargo build --release \
    -Z build-std=std,panic_abort \
    --target x86_64-breenix.json \
    -p userspace-std-programs
```

## Testing Strategy

### Unit Tests
Each libc function should have tests verifying correct behavior:
```rust
#[test]
fn test_write_returns_count() {
    let msg = b"hello";
    let result = unsafe { write(1, msg.as_ptr(), msg.len()) };
    assert_eq!(result, 5);
}
```

### Integration Tests
Test complete std functionality:
```rust
// tests/std_integration.rs
#[test]
fn test_vec_allocation() {
    let v: Vec<i32> = (0..1000).collect();
    assert_eq!(v.len(), 1000);
}

#[test]
fn test_string_formatting() {
    let s = format!("Hello, {}!", "Breenix");
    assert_eq!(s, "Hello, Breenix!");
}
```

### Boot Stage Tests
Add markers for kernel test runner:
- `RUST_STD_PRINTLN_WORKS` - println! produces output
- `RUST_STD_VEC_WORKS` - Vec allocation works
- `RUST_STD_FS_WORKS` - File operations work
- `RUST_STD_THREAD_WORKS` - Threading works

## Migration Path

### For Existing Userspace Programs

Current programs use `#![no_std]` + libbreenix:
```rust
#![no_std]
#![no_main]

use libbreenix::io::println;
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Hello");
    exit(0);
}
```

After Phase 1, they can use real std:
```rust
// No attributes needed!

fn main() {
    println!("Hello");
}
```

### Gradual Migration
1. Keep both approaches working during transition
2. Migrate one test program at a time
3. Validate each program works with real std
4. Eventually deprecate libbreenix direct usage in favor of std

## References

- [Redox OS relibc](https://github.com/redox-os/relibc) - Rust-based libc implementation
- [musl libc](https://musl.libc.org/) - Clean C libc for reference
- [Rust std sys/unix](https://github.com/rust-lang/rust/tree/master/library/std/src/sys/pal/unix) - What std expects from libc
- [Custom Rust targets](https://doc.rust-lang.org/rustc/targets/custom.html)
- [build-std documentation](https://doc.rust-lang.org/cargo/reference/unstable.html#build-std)

## Cleanup Required

The following should be removed as it represents the wrong approach:

1. **libs/breenix_std/** - Delete entire directory
2. **userspace/tests/hello_std.rs** - Delete (uses fake std)
3. **kernel/src/test_exec.rs::test_hello_std()** - Remove test function

These were created under the mistaken assumption that a "std-like" library was the goal. The real goal is supporting the actual Rust standard library.

## Success Criteria

Phase 1 is complete when:
- [ ] `libbreenix-libc` crate builds as staticlib
- [ ] Target spec has `"target-family": ["unix"]`
- [ ] `-Z build-std` completes without undefined symbols
- [ ] Simple program with `println!` runs in QEMU
- [ ] `Vec` and `String` allocations work

Phase 2 is complete when:
- [ ] `std::fs::read_to_string()` works
- [ ] `std::fs::read_dir()` works
- [ ] File metadata operations work

Phase 3 is complete when:
- [ ] C program with `printf()` compiles and runs
- [ ] `malloc()`/`free()` work correctly
- [ ] String functions work

Phase 4 is complete when:
- [ ] `std::thread::spawn()` creates threads
- [ ] `Arc<Mutex<T>>` synchronization works
- [ ] Thread-local storage works
