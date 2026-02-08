# Phase 1 Validation Findings

## Summary

This document captures the results of attempting to build a real Rust std program for Breenix.

**Date**: 2025-01-04
**Updated**: 2026-01-04
**Status**: libc crate patched; blocked on Rust std library support

## Progress Update (2026-01-04)

### Completed: libc Crate Fork with Breenix Support

Successfully vendored and patched the libc crate to add Breenix support:

**Location**: `libs/libc/`

**Files Created/Modified**:
1. `libs/libc/src/unix/breenix/mod.rs` - Complete Breenix type definitions (~800 lines)
2. `libs/libc/src/unix/mod.rs` - Added Breenix to OS selection cfg_if
3. `libs/libc/src/new/mod.rs` - Excluded Breenix from unsupported modules
4. `libs/libc/Cargo.toml` - Version set to 0.2.174 for compatibility
5. `userspace/tests/Cargo.toml` - Added patch.crates-io for libc
6. `userspace/tests/.cargo/config.toml` - Added patch.crates-io for build-std

**Breenix libc module includes**:
- All core C types (c_char, c_int, c_long, etc.)
- All POSIX types (time_t, mode_t, off_t, pid_t, etc.)
- Complete struct definitions (stat, dirent, sockaddr, termios, etc.)
- pthread types (opaque, sized for Linux glibc x86_64 compatibility)
- All required constants (O_*, S_*, CLOCK_*, AF_*, errno values, etc.)
- fd_set macros and helper functions

### Current Blocker: Rust Standard Library

The libc crate now compiles successfully for Breenix. However, the build now fails in Rust's standard library itself:

```
error[E0432]: unresolved import `super::platform::fs`
  --> .../std/src/os/unix/fs.rs:10:22
   |
10 | use super::platform::fs::MetadataExt as _;
   |                      ^^ could not find `fs` in `platform`

error[E0425]: cannot find function `current_exe` in module `os_imp`
   --> .../std/src/env.rs:759:13
    |
759 |     os_imp::current_exe()
    |             ^^^^^^^^^^^ not found in `os_imp`

error[E0425]: cannot find function `bind` in crate `libc`
   --> .../std/src/os/unix/net/datagram.rs:105:23
```

**Root Cause**: Rust std has platform abstraction layers (`sys/pal/unix/`) that need OS-specific implementations. Every function like `current_exe()`, `bind()`, `stat()`, etc. has OS-specific implementations guarded by `#[cfg(target_os = "...")]`. Breenix isn't recognized.

## Two-Layer Problem

Building Rust std for a new OS requires changes at TWO levels:

### Layer 1: libc Crate (DONE)
- Type definitions for C types and structs
- Constants (file flags, errno values, etc.)
- Location: `libs/libc/src/unix/breenix/mod.rs`
- Status: Complete

### Layer 2: Rust std Library (NOT DONE)
- OS-specific implementations of std functions
- Platform abstraction layer (`sys/pal/unix/`)
- Location: Rust source code in rustup toolchain
- Status: Would require forking Rust std

## Options for Rust std Support

### Option A: Fork Rust std (Complex)

Fork the Rust standard library and add Breenix support:

1. Clone rust-lang/rust repository
2. Add `library/std/src/sys/pal/unix/breenix/` module
3. Implement all required functions (current_exe, bind, stat, etc.)
4. Use custom sysroot with `-Z build-std`

**Pros**:
- Full native Breenix support
- Clean architecture

**Cons**:
- Very large undertaking (hundreds of functions)
- Must maintain fork against upstream changes
- Significant ongoing maintenance burden

### Option B: Piggyback on Linux (Recommended Short-term)

Change Breenix target to appear as Linux to Rust std:

1. Modify `x86_64-breenix.json` to set `"os": "linux"`
2. Keep libc fork with Breenix-specific adjustments
3. Breenix syscalls are already Linux-compatible

**Pros**:
- Works immediately
- Minimal maintenance
- Leverages existing Linux support

**Cons**:
- Not a "pure" Breenix target
- Some Linux-specific behavior may not match Breenix
- May need to patch specific functions that differ

### Option C: Stub Implementation (Minimal)

Create minimal stubs for required functions:

1. Fork std or create wrapper crate
2. Implement only what's needed for basic programs
3. Return errors for unsupported functionality

**Pros**:
- Fastest path to working binaries
- Can expand incrementally

**Cons**:
- Limited functionality
- Not production-ready

## Recommended Path Forward

**For immediate unblocking**: Use Option B (Linux piggyback)

1. Change target OS from "breenix" to "linux" in target spec
2. Keep the patched libc with Breenix-specific types
3. This leverages Breenix's Linux-compatible syscall ABI

**For long-term**: Consider Option A after core functionality stabilizes

Once Breenix has a stable syscall interface and core functionality, properly adding it to Rust upstream would be the clean solution.

## Changes Made Previously

### 1. Fixed sbrk Signedness Bug

**File**: `libs/libbreenix-libc/src/lib.rs`

**Problem**: The `sbrk` function was casting `isize` to `usize`, losing sign information.

**Fix**: Added explicit handling for negative increments (documented as unsupported).

### 2. Created Test Program

**Location**: `userspace/tests/`

Created a new crate with:
- `Cargo.toml` - Configured for `-Z build-std`
- `.cargo/config.toml` - Build configuration with proper linker flags
- `src/hello_std_real.rs` - Test program using real Rust std

## Files Created/Modified

### New Files
1. `libs/libc/` - Vendored libc crate (cloned from github.com/rust-lang/libc)
2. `libs/libc/src/unix/breenix/mod.rs` - Breenix type definitions

### Modified Files
1. `libs/libc/src/unix/mod.rs` - Added Breenix to OS selection
2. `libs/libc/src/new/mod.rs` - Excluded Breenix from unsupported new-style modules
3. `libs/libc/Cargo.toml` - Set version to 0.2.174
4. `userspace/tests/Cargo.toml` - Added libc patch
5. `userspace/tests/.cargo/config.toml` - Added libc patch for build-std

## Conclusion

The libc crate work is complete. The remaining blocker is Rust's standard library itself, which requires OS-specific implementations for many functions. The recommended path forward is to temporarily use Linux as the target OS (since Breenix has a Linux-compatible syscall ABI) while long-term considering proper Rust std integration.
