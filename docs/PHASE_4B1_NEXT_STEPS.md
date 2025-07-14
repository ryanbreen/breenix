# Phase 4B-1: Syscall Dispatch Table - Next Steps

**Current Status**: Phase 4A complete (v0.3-syscall-gate-ok)
**Next Goal**: Replace hardcoded syscall handling with proper dispatch table

## Immediate Action Items

### 1. Review Current Syscall Handler
- Location: `kernel/src/syscall/handler.rs`
- Currently handles test syscall (RAX=0x1234) directly
- Need to refactor into table-driven dispatch

### 2. Design Dispatch Table Structure
```rust
// kernel/src/syscall/mod.rs
pub const SYS_READ: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_EXIT: usize = 2;
pub const SYS_MAX: usize = 256;

type SyscallHandler = fn(&mut InterruptStackFrame) -> Result<usize, i32>;

static SYSCALL_TABLE: [Option<SyscallHandler>; SYS_MAX] = {
    let mut table = [None; SYS_MAX];
    table[SYS_READ] = Some(sys_read);
    table[SYS_WRITE] = Some(sys_write);
    table[SYS_EXIT] = Some(sys_exit);
    table
};
```

### 3. Create Unknown Syscall Test
- Create `integ_sys_unknown` test
- Call syscall with invalid number (e.g., 999)
- Expect -ENOSYS (38) in RAX

### 4. Implementation Steps
1. Create syscall number constants
2. Build dispatch table infrastructure
3. Modify `rust_syscall_handler` to use dispatch
4. Return -ENOSYS for unimplemented syscalls
5. Ensure existing syscall_gate test still passes

### 5. Testing Strategy
- Keep existing syscall_gate test working
- Add new test for unknown syscall
- Verify dispatch overhead is minimal

## Files to Modify
1. `kernel/src/syscall/mod.rs` - Add dispatch table
2. `kernel/src/syscall/handler.rs` - Use dispatch table
3. `tests/integ_sys_unknown.rs` - New test
4. `userspace/tests/syscall_unknown_test.rs` - Test binary

## Success Criteria
- ✅ Dispatch table replaces hardcoded handling
- ✅ Unknown syscalls return -ENOSYS
- ✅ Existing syscall_gate test still passes
- ✅ Clean, table-driven architecture
- ✅ No performance regression

## Tag Target
When complete: `v0.3.1-dispatch-table`