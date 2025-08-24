# x86-interrupt Double Fault Handler Compilation Issue

## Executive Summary

We are experiencing a compilation error when defining a double fault handler for our x86_64 operating system kernel. The error prevents the kernel from building, blocking our CI/CD pipeline and preventing the Ring-3 smoke test from running.

## Environment Details

- **Project**: Breenix OS (x86_64 kernel written in Rust)
- **Rust Version**: Nightly (required for kernel development)
- **Target**: `x86_64-breenix.json` (custom bare metal target)
- **x86_64 Crate Version**: 0.15.2
- **Feature Flags**: `#![feature(abi_x86_interrupt)]` enabled

## The Problem

When compiling our kernel, we get the following error:

```
error: invalid signature for `extern "x86-interrupt"` function
   --> kernel/src/interrupts.rs:151:6
    |
151 | ) -> ! {
    |      ^
    |
    = note: functions with the "custom" ABI cannot have a return type
```

## Current Code

```rust
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    // Log additional debug info before panicking
    log::error!("DOUBLE FAULT - Error Code: {:#x}", error_code);
    log::error!("Instruction Pointer: {:#x}", stack_frame.instruction_pointer.as_u64());
    log::error!("Stack Pointer: {:#x}", stack_frame.stack_pointer.as_u64());
    log::error!("Code Segment: {:?}", stack_frame.code_segment);
    log::error!("Stack Segment: {:?}", stack_frame.stack_segment);
    
    // Check current page table
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    log::error!("Current page table frame: {:?}", frame);
    
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}
```

## IDT Registration

The handler is registered in the Interrupt Descriptor Table (IDT) as follows:

```rust
unsafe {
    idt.double_fault.set_handler_fn(double_fault_handler)
        .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
}
```

## Attempted Solutions

### 1. Without Return Type
```rust
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}
```

**Result**: Type mismatch error:
```
error[E0308]: mismatched types
expected fn pointer `extern "x86-interrupt" fn(InterruptStackFrame, _) -> !`
found fn item `extern "x86-interrupt" fn(InterruptStackFrame, _) -> () {double_fault_handler}`
```

### 2. With Explicit Loop
```rust
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
    
    // Ensure function diverges
    loop {
        x86_64::instructions::hlt();
    }
}
```

**Result**: Same type mismatch error as above.

### 3. With Explicit Return Type
```rust
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}
```

**Result**: Compiler error stating "functions with the 'custom' ABI cannot have a return type"

## The Contradiction

We appear to be in a catch-22 situation:

1. The x86_64 crate's IDT expects a double fault handler with type:
   ```rust
   extern "x86-interrupt" fn(InterruptStackFrame, u64) -> !
   ```

2. The Rust compiler refuses to compile `extern "x86-interrupt"` functions with any return type, including `-> !`

3. Without the `-> !` return type, the function has an implicit `-> ()` return type, which doesn't match what the IDT expects

## Research Findings

According to the x86_64 crate documentation and the "Writing an OS in Rust" blog:
- Double fault handlers must have a diverging return type (`-> !`) because the x86_64 architecture doesn't permit returning from a double fault exception
- The x86_64 crate defines a type alias `DivergingHandlerFuncWithErrCode` for this purpose
- Examples online show using `-> !` with `extern "x86-interrupt"` successfully

## Other Exception Handlers

For comparison, our other exception handlers work fine without return types:

```rust
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    log::info!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn divide_by_zero_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: DIVIDE BY ZERO\n{:#?}", stack_frame);
}
```

These are registered without issue, but they don't require the diverging return type that double fault handlers do.

## Questions for the Expert

1. **Version Compatibility**: Is there a known issue with x86_64 crate v0.15.2 and recent nightly Rust versions regarding `extern "x86-interrupt"` functions with diverging return types?

2. **Workaround**: Is there a way to satisfy both the compiler (which rejects the return type) and the x86_64 crate (which requires it)?

3. **Alternative Approaches**: 
   - Can we use unsafe transmutation or casting to convert a non-diverging handler to the expected type?
   - Is there a different way to register the double fault handler that doesn't require the specific type signature?
   - Should we downgrade to an older version of the x86_64 crate or Rust nightly?

4. **Best Practice**: What is the current recommended approach for implementing double fault handlers in OS kernels using the x86_64 crate?

## Impact

This issue is blocking:
- Kernel compilation in CI/CD
- Ring-3 smoke tests from running
- Any further development that requires building the kernel

## Additional Context

- Full kernel source: https://github.com/ryanbreen/breenix
- Failed CI runs: https://github.com/ryanbreen/breenix/actions
- The issue appeared when we started building with GitHub Actions, suggesting possible environment differences

Any guidance on resolving this compilation error would be greatly appreciated.