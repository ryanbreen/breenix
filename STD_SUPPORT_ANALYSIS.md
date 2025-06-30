# Rust Standard Library Support Analysis for Breenix

## Current Status

Breenix currently uses `#![no_std]` which means we don't have access to Rust's standard library. However, we do have:

✅ **Working Components:**
- Custom global allocator (bump allocator)
- Panic handler
- Core library support
- Alloc library support (Vec, String, etc.)

## What Rust's std Requires

The Rust standard library requires several OS-level primitives:

### 1. **Memory Management** ✅
- Global allocator - **We have this!**
- Heap allocation - **Working with our bump allocator**

### 2. **Thread Local Storage (TLS)** ❌
- Required for thread-local variables
- Needs segment registers (FS/GS on x86_64)
- Currently not implemented

### 3. **Stack Unwinding** ❌
- Required for panic recovery
- We use `panic-strategy = "abort"`
- Would need DWARF unwinding tables

### 4. **OS Primitives** ❌
Most of std assumes these exist:
- File I/O (`std::fs`)
- Network I/O (`std::net`)
- Process management (`std::process`)
- Environment variables (`std::env`)
- System time (`std::time::SystemTime`)
- Thread spawning (`std::thread`)

### 5. **Synchronization Primitives** ⚠️
- Mutex, RwLock, etc.
- Some could work with our interrupts disabled
- Others need proper thread support

## Approaches to Enable std

### Option 1: Minimal std Build (Recommended)
Build a custom std that stubs out OS functionality:

```rust
// In Cargo.toml
[dependencies.std]
path = "path/to/custom/std"
features = ["panic_abort", "breenix"]

// Custom std would stub out:
pub mod fs {
    pub fn read_to_string(_: &Path) -> Result<String, Error> {
        Err(Error::new(ErrorKind::Unsupported, "No filesystem"))
    }
}
```

### Option 2: Implement Missing Pieces
1. **Add TLS Support:**
   ```rust
   // Set up GS segment for thread locals
   // Allocate TLS blocks per thread
   ```

2. **Add Basic Time Support:**
   ```rust
   impl SystemTime {
       pub fn now() -> SystemTime {
           // Use our RTC/timer
       }
   }
   ```

3. **Stub Unsupported Features:**
   - Return errors for file/network operations
   - Panic on thread spawn attempts

### Option 3: Use std-aware Cargo (Experimental)
Recent Rust has `-Z build-std` feature:

```bash
cargo +nightly build -Z build-std=core,alloc,std \
    -Z build-std-features=panic_abort \
    --target x86_64-breenix.json
```

## Implementation Plan

1. **Start with Option 3** - Try `-Z build-std`
2. **Add minimal TLS support** if needed
3. **Stub out OS functions** that we don't need
4. **Keep no_std as fallback** option

## Benefits of std Support

- Access to more Rust ecosystem crates
- Easier porting of existing Rust code  
- Better error handling with `Result`
- More familiar APIs

## Risks

- Increased binary size
- Hidden dependencies on OS features
- May break existing code
- Complexity of maintaining custom std

## Recommendation

Start with experimenting with `-Z build-std` to see what minimal support we need. We can likely get a subset of std working with:
- Our existing allocator
- Stubbed OS functions
- Basic TLS support
- Keeping panic=abort

This would give us the benefits of std's collections and error handling without full OS support.