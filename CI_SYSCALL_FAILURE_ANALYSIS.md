# CI Syscall 400/401 Test Failure Analysis

## Summary
The GitHub Actions CI workflow for testing syscalls 400/401 is failing. The test process (`syscall_test`) is created and scheduled but never executes the actual syscalls, causing the test to fail after timeout.

## Test Overview
- **Test File**: `.github/workflows/isolation-syscall.yml`
- **Purpose**: Verify syscalls 400 (share_test_page) and 401 (get_shared_test_page) work end-to-end
- **Test Binary**: `userspace/tests/syscall_test.rs` - Simple program that calls both syscalls and exits with 0 on success

## What We've Tried

### 1. Fixed GitHub Actions Deprecation Issues
**Problem**: CI was failing due to deprecated actions
**Solution**: 
- Updated `actions/checkout@v3` → `v4`
- Updated `actions/upload-artifact@v3` → `v4`
- Replaced deprecated `actions-rs/toolchain` with `dtolnay/rust-toolchain`

### 2. Fixed Missing LLVM Tools
**Problem**: `rust-lld command not found`
**Solution**: Added LLVM tools to PATH in CI workflow:
```yaml
- name: Add LLVM tools to PATH
  run: |
    echo "$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin" >> $GITHUB_PATH
```

### 3. Fixed Missing Test Binaries
**Problem**: `isolation.elf` and `isolation_attacker.elf` not found
**Solution**: Added missing binaries to `userspace/tests/build.sh`:
```bash
cp target/x86_64-breenix/release/isolation isolation.elf
cp target/x86_64-breenix/release/isolation_attacker isolation_attacker.elf
```

### 4. Fixed Double Fault Handler Signature
**Problem**: x86-interrupt ABI doesn't support return types but IDT expects diverging handler
**Multiple Attempts**:
- Removed `-> !` return type (caused type mismatch)
- Added explicit cast (failed due to ABI restrictions)
- Added diverging loop after panic (didn't solve type issue)
- **Final Solution**: Used `transmute` to convert function type:
```rust
let handler: extern "x86-interrupt" fn(InterruptStackFrame, u64) -> ! = 
    core::mem::transmute(double_fault_handler as extern "x86-interrupt" fn(InterruptStackFrame, u64));
```

### 5. Improved Test Execution Timing
**Changes Made**:
- Reduced CI timeout from 30s to 15s (sufficient for test)
- Increased test attempts from 10 to 30 yields
- Added spin loops between yields to allow timer interrupts
- Added periodic logging to track process state

## Current Test Failure Pattern

### CI Log Output
```
[ INFO] kernel: === SYSCALL 400/401 TEST ===
[ INFO] kernel::test_exec: === RUN syscall_test ===
[ INFO] kernel::process::creation: create_user_process: Creating user process 'syscall_test' with new model
[ INFO] kernel::process::manager: Created process syscall_test (PID 3)
[ INFO] kernel::task::scheduler: Added thread 3 'syscall_test' to scheduler (user: true, ready_queue: [1, 2, 3])
[ INFO] kernel::test_exec: Created syscall_test process with PID 3
[ INFO] kernel::test_exec: Yielding to run syscall_test...
[ERROR] kernel::test_exec: ✗ syscall_test did not complete after 10 yields
```

### What's Happening
1. **Process Creation**: ✅ Process is created successfully with PID 3
2. **Scheduling**: ✅ Thread 3 is added to scheduler ready queue
3. **Context Switching**: ✅ Scheduler is switching to thread 3:
   ```
   [ INFO] kernel::task::scheduler: Switching from thread 2 to thread 3
   [ INFO] kernel::task::scheduler: Put thread 3 back in ready queue, state was Running
   ```
4. **Syscall Execution**: ❌ No syscalls are executed:
   - No "SYSCALL entry: rax=400" messages
   - No "TEST: share_page(0xdeadbeef)" messages
   - Process never terminates (no exit code)

### Additional Observations
- Low stack warning detected: `WARNING: Low stack detected! RSP=0x18000010ba0`
- Process is being scheduled and context switched to
- No page faults or double faults from PID 3
- Process appears to be stuck and never reaches the INT 0x80 instructions

## Root Cause Analysis

### Possible Issues
1. **Process Not Reaching Userspace**: The process might be created but never actually jumps to userspace code
2. **ELF Loading Issue**: The syscall_test binary might not be loaded correctly
3. **Page Table Issues**: The process page tables might not be set up correctly
4. **Stack Setup**: The low stack warning suggests potential stack issues
5. **Cooperative Scheduling**: The test might not be giving the process enough chances to run

### Key Missing Evidence
We don't see any of these expected log messages:
- "SYSCALL entry: rax=400" (from syscall handler)
- "Userspace instruction executed" (would indicate reaching userspace)
- Process exit messages
- Any INT 0x80 handling

## Next Steps to Investigate

1. **Add Userspace Execution Logging**: Add logs to confirm when a process actually reaches userspace
2. **Check ELF Entry Point**: Verify the syscall_test ELF is being loaded at the correct address
3. **Page Table Debugging**: Add logging for page table switches during process execution
4. **Stack Setup Verification**: Check if the user stack is set up correctly
5. **Direct Test Execution**: Try running a simpler test first (like hello world) to isolate the issue

## Test Binary Details

### syscall_test.rs
```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let test_value = 0xdead_beef;
        sys_share_test_page(test_value);      // Syscall 400
        let result = sys_get_shared_test_page(); // Syscall 401
        
        if result == test_value {
            sys_exit(0); // Success
        } else {
            sys_exit(1); // Failure
        }
    }
}
```

### Expected Behavior
1. Process starts at _start
2. Executes INT 0x80 with RAX=400
3. Executes INT 0x80 with RAX=401
4. Executes INT 0x80 with RAX=9 (exit)
5. Process terminates with code 0

### Actual Behavior
Process is scheduled but never executes any syscalls or terminates.

## Conclusion
The test is failing because the syscall_test process, while successfully created and scheduled, is not actually executing its code. The process appears to be stuck before reaching the first INT 0x80 instruction. This suggests a fundamental issue with process execution rather than with the syscalls themselves.