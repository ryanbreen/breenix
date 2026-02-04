# ARM64 Parity Session Handoff Document

**Date**: 2026-02-02
**Session Focus**: ARM64 test parity improvements
**Result**: Pass rate improved from 41.25% (33/80) to 62.5% (50/80)

---

## Executive Summary

This session addressed two critical bugs blocking ARM64 userspace test execution:

1. **exec() spinlock deadlock** - Prevented all fork+exec operations
2. **sys_read EOPNOTSUPP for regular files** - Prevented all file read operations

Both fixes were validated, committed, and merged. The ARM64 test pass rate improved by 21.25 percentage points.

---

## Change 1: exec() Spinlock Deadlock Fix

### Problem Statement

35 ARM64 tests were hanging after calling `exec()`. The child process was created but never executed userspace code. Initial hypothesis was a return-to-userspace bug, but investigation revealed a spinlock deadlock.

### Root Cause Analysis

**Location**: `kernel/src/main_aarch64.rs` in `run_userspace_from_ext2()`

**The Bug**:
```rust
fn run_userspace_from_ext2(path: &str) -> Result<Infallible, &'static str> {
    let fs_guard = ext2::root_fs();  // Acquires spinlock
    let fs = fs_guard.as_ref().ok_or("ext2 not mounted")?;

    // ... load ELF binary ...
    let elf_data = fs.read_file_content(&inode)?;

    // ... create process ...

    return_to_userspace(...);  // NEVER RETURNS - uses ERET
    // fs_guard is NEVER dropped!
}
```

**Deadlock Sequence**:
1. `run_userspace_from_ext2()` acquires `ext2::root_fs()` spinlock via MutexGuard
2. Loads init_shell ELF binary from ext2 filesystem
3. Calls `return_to_userspace()` which executes `ERET` and never returns
4. MutexGuard destructor never runs → spinlock held forever
5. When userspace calls `fork()` → child calls `exec()` → tries to load ELF
6. `load_elf_from_ext2()` calls `ext2::root_fs()` → **DEADLOCK**

**Evidence**:
- Lock type confirmed: `spin::Mutex` at `kernel/src/fs/ext2/mod.rs:1279`
- `return_to_userspace()` uses `options(noreturn)` and ends with `eret`
- Trace buffer debugging showed hang at `ext2::root_fs()` acquisition

### The Fix

**File**: `kernel/src/main_aarch64.rs` (line 75-78)

```rust
let elf_data = fs.read_file_content(&inode)?;

// CRITICAL: Release ext2 lock BEFORE creating process and jumping to userspace.
// return_to_userspace() never returns, so fs_guard would never be dropped.
// If we hold the lock, fork/exec in userspace will deadlock trying to acquire it.
drop(fs_guard);

// ... rest of function continues with lock released ...
```

**Why This Works**:
- `elf_data` is a `Vec<u8>` containing an owned copy of the file contents
- After reading, we no longer need filesystem access
- Explicit `drop()` releases the lock before the point of no return
- Subsequent `fork()`/`exec()` calls can now acquire the lock

### Validation

| Test | Result |
|------|--------|
| `fork_test` | PASS (child exit 42, parent exit 0) |
| `exec_from_ext2_test` | PASS (exec'd /bin/hello_world) |
| ARM64 boot test | PASS |

### PR Reference

- **PR #138**: `arm64: fix exec() deadlock by releasing ext2 lock before userspace jump`
- **Commit**: `c482bc1`

---

## Change 2: RegularFile Read Implementation

### Problem Statement

After fixing exec(), file read operations were failing with "Failed to read file" errors. Tests like `file_read_test`, `cat_test`, `head_test` all failed despite being able to open files successfully.

### Root Cause Analysis

**Location**: `kernel/src/syscall/io.rs` (lines 337-339)

**The Bug**:
```rust
match &fd_entry.kind {
    // ... other FdKind variants handled ...

    FdKind::RegularFile(_) | FdKind::Device(_) => {
        SyscallResult::Err(super::errno::EOPNOTSUPP as u64)  // Returns error 95!
    }
}
```

The ARM64 `sys_read` syscall handler had a stub implementation for regular files that simply returned `EOPNOTSUPP` (Operation not supported, errno 95).

**Evidence**:
- `file_read_test` output: "fstat: size = 17" (open works) then "Failed to read file"
- `devfs_test` output: "write to /dev/null failed with error: -95"
- Error 95 = EOPNOTSUPP, matching the stub implementation

### The Fix

**File**: `kernel/src/syscall/io.rs`

Replaced the stub with a proper implementation:

```rust
FdKind::RegularFile(inode_num) => {
    // Get current file position
    let current_pos = fd_entry.file_offset;
    let inode = *inode_num;

    // Release process manager lock before filesystem I/O
    drop(manager_guard);

    // Read from ext2 filesystem
    let fs_guard = crate::fs::ext2::root_fs();
    let fs = match fs_guard.as_ref() {
        Some(fs) => fs,
        None => return SyscallResult::Err(super::errno::EIO as u64),
    };

    // Get file size from inode
    let inode_data = match fs.read_inode(inode) {
        Ok(i) => i,
        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
    };
    let file_size = inode_data.size() as u64;

    // Check if we're at or past EOF
    if current_pos >= file_size {
        return SyscallResult::Ok(0);
    }

    // Calculate how much to read
    let remaining = file_size - current_pos;
    let to_read = core::cmp::min(count, remaining) as usize;

    // Read from filesystem
    let data = match fs.read_file_range(inode, current_pos as usize, to_read) {
        Ok(d) => d,
        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
    };

    let bytes_read = data.len();

    // Copy to userspace
    if copy_to_user_bytes(buf_ptr, &data).is_err() {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }

    // Update file position (re-acquire lock)
    let mut manager_guard = crate::process::manager();
    if let Some(manager) = &mut *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread_mut(thread_id) {
            if let Some(fd_entry) = process.fd_table.get_mut(fd as i32) {
                fd_entry.file_offset = current_pos + bytes_read as u64;
            }
        }
    }

    SyscallResult::Ok(bytes_read as u64)
}
```

Also implemented `FdKind::Device` reads:

```rust
FdKind::Device(device_type) => {
    let dt = *device_type;
    drop(manager_guard);

    let mut buf = alloc::vec![0u8; count as usize];
    match crate::fs::devfs::device_read(dt, &mut buf) {
        Ok(n) => {
            if n > 0 {
                if copy_to_user_bytes(buf_ptr, &buf[..n]).is_err() {
                    return SyscallResult::Err(super::errno::EFAULT as u64);
                }
            }
            SyscallResult::Ok(n as u64)
        }
        Err(e) => SyscallResult::Err(e as u64),
    }
}
```

### Key Implementation Details

1. **Lock Management**: Process manager lock is dropped before filesystem I/O to avoid holding locks during slow operations
2. **EOF Handling**: Returns 0 when file position >= file size
3. **Position Update**: File offset is updated after successful read
4. **Error Mapping**: All errors map to appropriate errno values (EIO, EFAULT)
5. **Uses Existing APIs**: Leverages `ext2::read_file_range()` and `devfs::device_read()`

### Validation

| Test | Before | After |
|------|--------|-------|
| `file_read_test` | FAIL | PASS |
| `cat_test` | FAIL | PASS |
| `head_test` | FAIL | PASS |
| `tail_test` | FAIL | PASS |
| `wc_test` | FAIL | PASS |

### PR Reference

- **PR #140**: `arm64: implement RegularFile and Device read in sys_read`
- **Commit**: `039da30`

---

## Change 3: Test Suite Writable Disk Fix

### Problem Statement

The ARM64 test suite was using `readonly=on` for the ext2 disk in QEMU, causing all filesystem write tests to fail with I/O errors.

### Root Cause

**Location**: `docker/qemu/run-aarch64-test-suite.sh` (line 118)

```bash
-drive if=none,id=ext2,format=raw,readonly=on,file="$EXT2_DISK"
```

The `readonly=on` flag tells QEMU to mount the disk as read-only at the hypervisor level. Any write attempts from the guest kernel fail silently or with I/O errors.

**Note**: This is intentional for parallel testing to prevent corruption of the master ext2 image. The fix creates a copy for each test run.

### The Fix

**File**: `docker/qemu/run-aarch64-test-suite.sh`

1. Create a writable copy at startup:
```bash
# Create a writable copy of the ext2 disk for tests that need write access
EXT2_DISK_WRITABLE="$RESULTS_DIR/ext2-writable.img"
echo "Creating writable copy of ext2 disk..."
cp "$EXT2_DISK" "$EXT2_DISK_WRITABLE"
```

2. Reset the copy before each test:
```bash
run_test() {
    # Reset writable ext2 disk to clean state for each test
    cp "$EXT2_DISK" "$EXT2_DISK_WRITABLE"
    # ...
}
```

3. Use writable copy without readonly flag:
```bash
-drive if=none,id=ext2,format=raw,file="$EXT2_DISK_WRITABLE"
```

### Impact

This fix enables filesystem write tests to run, though many still fail due to unimplemented write syscalls in the kernel. The infrastructure is now correct.

---

## Test Results Summary

### Before This Session
| Metric | Value |
|--------|-------|
| Passed | 33 |
| Failed | 47 |
| Pass Rate | 41.25% |

### After This Session
| Metric | Value |
|--------|-------|
| Passed | 50 |
| Failed | 30 |
| Pass Rate | 62.5% |

### Tests Fixed by exec() Fix (+9)
```
access_test, cwd_test, echo_argv_test, fork_test, getdents_test,
itimer_test, ls_test, signal_exec_test, which_test
```

### Tests Fixed by RegularFile Read (+8)
```
cat_test, cp_mv_argv_test, file_read_test, fs_directory_test,
head_test, mkdir_argv_test, rm_argv_test, tail_test, wc_test
```

---

## Files Modified

| File | Change |
|------|--------|
| `kernel/src/main_aarch64.rs` | Added `drop(fs_guard)` before userspace jump |
| `kernel/src/syscall/io.rs` | Implemented RegularFile and Device read |
| `docker/qemu/run-aarch64-test-suite.sh` | Writable ext2 disk copy for tests |
| `kernel/src/arch_impl/aarch64/trace.rs` | Added trace buffer for debugging (can be removed) |
| `kernel/src/arch_impl/aarch64/syscall_entry.rs` | Added trace points (can be removed) |
| `arm64-parity.md` | Updated tracking document |

---

## Remaining Work

### Still Failing (30 tests)

| Category | Count | Root Cause |
|----------|-------|------------|
| Network ENETUNREACH | ~6 | QEMU network not configured |
| Filesystem writes | ~6 | sys_write for RegularFile not implemented |
| argc/argv setup | ~4 | Initial process doesn't receive arguments |
| Signal/process bugs | ~8 | Various syscall issues |
| COW syscalls | ~2 | COW_STATS/SIMULATE_OOM return ENOSYS |
| Other | ~4 | Various |

### Recommended Next Steps

1. **P1**: Configure QEMU networking for ARM64 (affects 6 tests)
2. **P2**: Implement `sys_write` for `FdKind::RegularFile` (mirrors read implementation)
3. **P3**: Fix argc/argv setup in `run_userspace_from_ext2()`
4. **P4**: Investigate individual signal/process test failures

---

## Verification Commands

To reproduce the test results:

```bash
# Build ARM64 kernel with testing features
cargo build --release --features testing --target aarch64-breenix.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64

# Run full test suite
./docker/qemu/run-aarch64-test-suite.sh --all

# Run specific test
./docker/qemu/run-aarch64-test-suite.sh file_read_test
```

---

## Review Checklist

For reviewers validating this work:

- [ ] Verify exec() fix: `drop(fs_guard)` is placed after `read_file_content()` but before `return_to_userspace()`
- [ ] Verify RegularFile read: Implementation reads from ext2, copies to userspace, updates file offset
- [ ] Verify Device read: Implementation calls `devfs::device_read()`
- [ ] Verify test suite: Creates writable copy, resets between tests
- [ ] Run `file_read_test` and verify PASS
- [ ] Run `fork_test` and verify PASS
- [ ] Confirm no regressions in previously passing tests

---

*Document generated: 2026-02-02*
*Author: Claude Code (Opus 4.5)*
