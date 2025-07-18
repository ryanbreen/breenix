# Milestone A Completion Report: Syscalls 400/401 Work End-to-End

**Date**: 2025-01-18  
**Test Run**: logs/breenix_20250718_051743.log

## Executive Summary

All required components for Milestone A have been implemented and proven to work end-to-end. The syscall test binary successfully executes both test syscalls and exits with code 0.

## Required Log Evidence

From the QEMU serial log at `/Users/wrb/fun/code/breenix/logs/breenix_20250718_051743.log`:

### 1. Required Log Lines (In Order)

```
[ INFO] kernel::syscall::handler: SYSCALL entry: rax=400
[ INFO] kernel::syscall::handlers::test_syscalls: TEST: share_page(0xdeadbeef)
[ INFO] kernel::syscall::handler: SYSCALL entry: rax=401
[ INFO] kernel::syscall::handlers::test_syscalls: TEST: get_page -> 0xdeadbeef
[ INFO] kernel::task::process_task: Process 3 (thread 3) exited with code 0
```

### 2. Execution Trace

Process 3 (syscall_test) execution sequence:

1. **Line 3648**: First syscall
   ```
   [ INFO] kernel::syscall::handler: SYSCALL entry: rax=400
   [ INFO] kernel::syscall::handlers::test_syscalls: TEST: share_page(0xdeadbeef)
   ```

2. **Line 3732**: Second syscall
   ```
   [ INFO] kernel::syscall::handler: SYSCALL entry: rax=401
   [ INFO] kernel::syscall::handlers::test_syscalls: TEST: get_page -> 0xdeadbeef
   ```

3. **Line 3747**: Successful exit
   ```
   [ INFO] kernel::task::process_task: Process 3 (thread 3) exited with code 0
   ```

### 3. No Unknown Syscall Messages

Grep proof showing zero "Unknown syscall" messages for 400/401:

```bash
$ grep "Unknown syscall.*400\|Unknown syscall.*401" logs/breenix_20250718_051743.log
# No output - confirms no unknown syscall messages for 400/401
```

## Test Binary Verification

The syscall_test binary (50 bytes) was built from pure assembly with no dependencies:

```assembly
_start:
    mov     eax, 400          ; syscall 400
    mov     edi, 0xdeadbeef   ; test value
    int     0x80              ; make syscall
    
    mov     eax, 401          ; syscall 401  
    int     0x80              ; make syscall
    
    mov     rdx, 0xdeadbeef   ; expected value
    cmp     rax, rdx          ; compare result
    jne     fail              ; jump if not equal
    
success:
    mov     eax, 9            ; sys_exit
    xor     edi, edi          ; exit code 0
    int     0x80
```

## Success Criteria Met

✅ **Syscall 400 executed**: Log shows `SYSCALL entry: rax=400`  
✅ **Handler logged correctly**: `TEST: share_page(0xdeadbeef)`  
✅ **Syscall 401 executed**: Log shows `SYSCALL entry: rax=401`  
✅ **Handler returned value**: `TEST: get_page -> 0xdeadbeef`  
✅ **Process exited successfully**: `Process 3 (thread 3) exited with code 0`  
✅ **No unknown syscall errors**: Grep confirms no "Unknown syscall" for 400/401  
✅ **Round-trip worked**: Process exited with 0, meaning comparison passed

## Conclusion

Milestone A is complete. The test demonstrates that:
1. Userspace processes can execute
2. Syscalls 400 and 401 are properly dispatched
3. Values are correctly stored and retrieved
4. The syscall frame layout is correct
5. Return values work properly

The only remaining task is to wire the CI workflow.