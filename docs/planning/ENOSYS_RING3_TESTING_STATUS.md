# ENOSYS Ring 3 Testing Status

**Date**: November 20, 2025
**Status**: BROKEN - Kernel page fault prevents userspace execution

## Current Problem

The kernel crashes with a page fault when attempting to run userspace processes. This affects both the Ring-3 Smoke Test and the Ring-3 ENOSYS Test.

### Evidence

When running locally:
```
PF @ 0xffffffff8a16252d Error: 0x0 (P=0, W=0, U=0, I=0)
FEXCEPTION: PAGE FAULT - Now using IST stack for reliable diagnostics
```

The faulting address `0xffffffff8a16252d` is in the kernel high-half but outside normal kernel code range - likely a bad function pointer or corrupted jump target.

### Why the Smoke Test "Passes"

The smoke test passes in CI for the wrong reason:
1. Kernel creates userspace processes
2. Kernel prints `KERNEL_POST_TESTS_COMPLETE` marker
3. xtask sees the marker and considers the test passed
4. THEN the kernel enables interrupts and crashes

The completion markers are printed BEFORE userspace actually runs. The test is passing without testing userspace execution.

### Why the ENOSYS Test Fails

The ENOSYS test correctly requires actual userspace output:
1. Userspace must run
2. Userspace must call syscall 999
3. Kernel must return -38 (ENOSYS)
4. Userspace must print "ENOSYS OK"

Since userspace never runs due to the page fault, this test fails honestly.

## What ENOSYS Should Test

The syscall_enosys userspace program (`userspace/tests/syscall_enosys.rs`):

```rust
let rv = unsafe { syscall0(SYS_UNKNOWN) };  // SYS_UNKNOWN = 999
if rv == ENOSYS_U64 {  // -38 wrapped to u64
    write_str("ENOSYS OK\n");
} else {
    write_str("ENOSYS FAIL\n");
}
```

This validates:
1. Userspace can execute in Ring 3
2. INT 0x80 syscall mechanism works
3. Kernel correctly returns ENOSYS for undefined syscalls
4. Return value is properly passed back to userspace
5. sys_write syscall works for output

## How to Test Locally

### Run ENOSYS Test
```bash
cargo run -p xtask -- ring3-enosys
```

This should:
1. Build kernel with `testing,external_test_bins` features
2. Build userspace ELF files in `userspace/tests/`
3. Boot kernel in QEMU
4. Wait for "ENOSYS OK" in serial output

### Run Smoke Test
```bash
cargo run -p xtask -- ring3-smoke
```

### Check Serial Output
```bash
cat target/xtask_ring3_enosys_output.txt
# or
cat target/xtask_ring3_smoke_output.txt
```

### Visual Debugging
```bash
BREENIX_VISUAL_TEST=1 cargo run -p xtask -- ring3-enosys
```

## How to Test in GitHub CI

The workflows are:
- `.github/workflows/ring3-smoke.yml`
- `.github/workflows/ring3-enosys.yml`

Both run on push to main and on PRs that touch kernel/userspace code.

Artifacts are uploaded on failure - check the workflow run for `enosys-test-output-*` or `qemu-serial-log`.

## Root Cause Investigation Needed

The page fault occurs during context switch to userspace. Key areas to investigate:

1. **CR3 switching** (`kernel/src/interrupts/context_switch.rs`)
   - The CR3 switch code was previously disabled with `if false {}`
   - It was re-enabled but may have issues

2. **Page table setup** (`kernel/src/memory/process_memory.rs`)
   - Process page tables may be missing mappings
   - Kernel code at `0xffffffff8a...` range might not be mapped

3. **TSS/RSP0 configuration**
   - Kernel stack pointer for Ring 0 entry may be wrong
   - IST stacks may not be properly mapped

4. **GDT/IDT accessibility**
   - These must be accessible after CR3 switch

### Debugging Steps

1. Add logging before/after CR3 switch
2. Verify all kernel structures are in upper-half addresses
3. Check that process page tables have proper kernel mappings
4. Verify the faulting instruction at `0x100000f1e4c`

## Files Involved

- `kernel/src/main.rs` - Test execution flow
- `kernel/src/test_exec.rs` - `test_syscall_enosys()` function
- `kernel/src/interrupts/context_switch.rs` - CR3 switching
- `kernel/src/memory/process_memory.rs` - ProcessPageTable
- `userspace/tests/syscall_enosys.rs` - Userspace test program
- `xtask/src/main.rs` - Test runner with success criteria

## Success Criteria

The ENOSYS test passes when:
1. Kernel boots successfully
2. syscall_enosys process is created
3. Timer interrupts trigger scheduler
4. Process runs in Ring 3
5. Syscall 999 is invoked
6. Kernel returns -38
7. Process prints "ENOSYS OK"
8. xtask detects this string

## Next Steps

1. Debug the page fault - find what code is at `0x100000f1e4c`
2. Verify kernel mappings in process page tables
3. Fix the underlying issue
4. Verify both smoke and ENOSYS tests pass with actual userspace execution
5. Consider moving completion markers to AFTER userspace runs

## DO NOT

- Accept "process created" as proof of success
- Add fallbacks that weaken test criteria
- Let tests pass by detecting markers printed before actual test execution
