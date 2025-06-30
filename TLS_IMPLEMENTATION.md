# Thread Local Storage (TLS) Implementation

## Summary

We've successfully implemented Thread Local Storage (TLS) support for Breenix! This is a crucial step towards eventual std support and proper threading.

## What Was Implemented

### 1. Core TLS Infrastructure (`kernel/src/tls.rs`)
- **Thread Control Block (TCB)**: A structure that holds thread-specific data
- **GS Segment Register**: Used for kernel-space TLS (standard x86_64 approach)
- **TLS Block Allocation**: Each thread gets a 4KB TLS block in high memory
- **TLS Manager**: Tracks allocated TLS blocks and thread IDs

### 2. Key Features
- ✅ GS base register configuration via MSRs
- ✅ Per-thread TLS block allocation and mapping
- ✅ Direct TLS read/write via inline assembly
- ✅ Thread ID management
- ✅ Self-pointer in TCB (required by some TLS models)

### 3. API Functions
```rust
// Initialize TLS system
pub fn init()

// Get current thread's TCB
pub fn current_tcb() -> Option<&'static ThreadControlBlock>

// Get current thread ID  
pub fn current_thread_id() -> u64

// Allocate TLS for new thread (future use)
pub fn allocate_thread_tls(stack_pointer: VirtAddr) -> Result<u64, &'static str>

// Switch to different thread's TLS (future use)
pub fn switch_tls(thread_id: u64) -> Result<(), &'static str>

// Direct TLS access
pub unsafe fn read_tls_u32/u64(offset: usize) -> u32/u64
pub unsafe fn write_tls_u32/u64(offset: usize, value: u32/u64)
```

### 4. Testing
The implementation includes comprehensive tests that verify:
- Current thread ID retrieval
- TCB self-pointer correctness
- Direct TLS read/write operations
- All tests pass successfully!

## Technical Details

### Memory Layout
- TLS blocks start at `0xFFFF_8000_0000_0000` (high canonical address space)
- Each TLS block is 4KB (one page)
- TCB is at the beginning of each TLS block
- Remaining space can be used for thread-local variables

### x86_64 Specifics
- Uses GS segment register for kernel TLS (FS typically for user-space)
- GS base set via MSR (Model Specific Register)
- Inline assembly for efficient TLS access: `mov reg, gs:[offset]`

## Future Work

With TLS support in place, we can now:
1. Implement proper threading with per-thread state
2. Support thread-local variables in Rust
3. Move closer to std support (TLS is a requirement)
4. Implement per-CPU data structures

## Integration
TLS is now initialized early in the kernel boot process, right after memory management and before timer initialization. The kernel thread (thread 0) gets its TLS block set up automatically.

## Testing Output
```
1751278873 - [ INFO] kernel::tls: Initializing Thread Local Storage (TLS) system...
1751278873 - [ INFO] kernel::tls: Kernel TLS block allocated at 0xffff800000000000
1751278873 - [ INFO] kernel::tls: TLS system initialized successfully
1751278873 - [ INFO] kernel: TLS initialized
1751278873 - [ INFO] kernel: Running TLS tests...
1751278873 - [ INFO] kernel::tls: Testing TLS functionality...
1751278873 - [ INFO] kernel::tls: Current thread ID: 0
1751278873 - [ INFO] kernel::tls: TCB thread ID: 0
1751278873 - [ INFO] kernel::tls: TLS read/write test passed: 0xdeadbeef
1751278873 - [ INFO] kernel::tls: All TLS tests passed!
```

This brings us one step closer to full std support!