# Phase 4B-1: Syscall Dispatch Table Implementation Plan

**Status**: Ready to implement
**Target Tag**: v0.3.1-dispatch-table

## Validated Adjustments from Review

### 1. Bounded SYS_MAX (Memory Efficiency)
```rust
// Instead of 256-entry array wasting 2KB
pub const SYS_MAX: usize = 32;  // Only ~10 syscalls for months

// In dispatcher
if nr >= SYS_MAX {
    return -ENOSYS;  // -38
}
```

### 2. Compile-Time USER Bit Check
```rust
// In ProcessPageTable::new(), add after copying each kernel entry:
debug_assert!(
    !entry.flags().contains(PageTableFlags::USER_ACCESSIBLE),
    "Kernel PML4[{}] has USER bit set - SECURITY VIOLATION!", idx
);
```

### 3. Return Type Fix (POSIX ABI)
```rust
// Match Linux/POSIX exactly
type SyscallHandler = fn(&mut InterruptStackFrame) -> isize;

// Negative values are -errno
pub fn dispatch(nr: usize, frame: &mut InterruptStackFrame) -> isize {
    if nr >= SYS_MAX {
        return -38;  // -ENOSYS
    }
    
    match SYSCALL_TABLE[nr] {
        Some(handler) => handler(frame),
        None => -38,  // -ENOSYS
    }
}
```

### 4. Wire sys_write to Serial Immediately
```rust
fn sys_write(frame: &mut InterruptStackFrame) -> isize {
    let fd = frame.rdi as i32;
    let buf = frame.rsi as usize;
    let len = frame.rdx as usize;
    
    // For now, only stdout(1) and stderr(2) to serial
    if fd != 1 && fd != 2 {
        return -9;  // -EBADF
    }
    
    // TODO: Add framebuffer path later
    write_serial_from_user(buf, len)
}
```

### 5. Stack Guard Pages (Future)
```
Future stack layout:
[4KB guard page - unmapped]
[User stack - mapped]
[4KB guard page - unmapped]
```

## Implementation Checklist

### Step 1: Core Dispatch Infrastructure
- [ ] Create `kernel/src/syscall/table.rs` with bounded array
- [ ] Add errno constants (-ENOSYS=-38, -EBADF=-9, etc.)
- [ ] Implement dispatch function returning isize
- [ ] Add compile-time USER bit assertion

### Step 2: Refactor Existing Handler
- [ ] Modify `rust_syscall_handler` to use dispatch
- [ ] Keep test syscall (0x1234) working temporarily
- [ ] Update return type to isize throughout

### Step 3: Create Unknown Syscall Test
- [ ] Write `userspace/tests/syscall_unknown_test.rs`
- [ ] Call syscall 999, expect -38 in RAX
- [ ] Create `tests/integ_sys_unknown.rs`

### Step 4: Verification
- [ ] Existing syscall_gate test passes
- [ ] New unknown syscall test passes  
- [ ] No compiler warnings
- [ ] Guard-rail tests still pass

## Code Structure

```rust
// kernel/src/syscall/table.rs
pub const SYS_READ: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_EXIT: usize = 2;
pub const SYS_MAX: usize = 32;

// Standard errno values (negative)
pub const ENOSYS: isize = -38;
pub const EBADF: isize = -9;
pub const EFAULT: isize = -14;

type SyscallHandler = fn(&mut InterruptStackFrame) -> isize;

static SYSCALL_TABLE: [Option<SyscallHandler>; SYS_MAX] = {
    let mut table = [None; SYS_MAX];
    // Populated in later PRs
    table
};

pub fn dispatch(nr: usize, frame: &mut InterruptStackFrame) -> isize {
    if nr >= SYS_MAX {
        return ENOSYS;
    }
    
    match SYSCALL_TABLE[nr] {
        Some(handler) => handler(frame),
        None => ENOSYS,
    }
}
```

## Future Considerations (Not This PR)

1. **COW Fork**: Keep full copy for now, add COW marking later
2. **Page Fault Handling**: Let existing #PF handler kill process
3. **FD Table**: Needed when sys_exit meets waitpid
4. **Dynamic Table**: Switch to Vec when >64 syscalls

## Success Metrics

- Dispatch overhead < 50 cycles
- Unknown syscall returns -38 correctly
- No regressions in existing tests
- Clean separation of concerns

## Next PR Preview

After v0.3.1-dispatch-table is tagged:
- 4B-2: sys_write with serial output
- 4B-3: sys_exit with process termination
- Each gets its own PR and test