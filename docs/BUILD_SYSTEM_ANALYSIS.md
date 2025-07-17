# Breenix Build System Analysis and Current Issues

## Overview

Breenix is an x86_64 operating system kernel written in Rust. The project uses a complex cross-compilation setup to build a bare-metal kernel that runs without a standard library or operating system support.

## Build System Architecture

### 1. Workspace Structure

```toml
# Root Cargo.toml
[workspace]
members = ["kernel", "mcp"]
```

The project uses Cargo workspaces with:
- `kernel/` - The main kernel implementation
- `mcp/` - Model Context Protocol integration server
- Root package for QEMU runners and build orchestration

### 2. Cross-Compilation Setup

#### Rust Toolchain Requirements

```toml
# rust-toolchain.toml
[toolchain]
channel = "nightly"
profile = "default"
targets = ["x86_64-unknown-none"]
components = ["rust-src", "llvm-tools-preview"]
```

- **Nightly Rust**: Required for unstable features like custom targets
- **x86_64-unknown-none**: The standard bare-metal target
- **rust-src**: Needed for building `core` and `alloc` for custom targets
- **llvm-tools-preview**: Required for custom linking and binary manipulation

#### Custom Target Specification

```json
# x86_64-breenix.json
{
  "llvm-target": "x86_64-unknown-none",
  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128",
  "arch": "x86_64",
  "target-endian": "little",
  "target-pointer-width": "64",
  "target-c-int-width": "32",
  "os": "none",
  "executables": true,
  "linker-flavor": "ld.lld",
  "linker": "rust-lld",
  "panic-strategy": "abort",
  "disable-redzone": true,
  "features": "-mmx,-sse,+soft-float",
  "rustc-abi": "x86-softfloat"
}
```

Key features:
- **No OS**: `"os": "none"` - bare metal target
- **Panic = Abort**: No unwinding support in kernel
- **Disabled Red Zone**: Critical for interrupt safety
- **Soft Float**: All floating point operations emulated in software
- **No MMX/SSE**: SIMD instructions disabled for kernel safety

### 3. Build Dependencies

#### Kernel Dependencies

```toml
# kernel/Cargo.toml
[dependencies]
bootloader_api = { git = "https://github.com/rust-osdev/bootloader.git", branch = "main" }
embedded-graphics = "0.8.1"
x86_64 = { version = "0.15.2", features = ["instructions", "nightly"] }
conquer-once = { version = "0.4.0", default-features = false }
log = { version = "0.4.17", default-features = false }
pic8259 = "0.10.4"
spin = "0.9.8"
uart_16550 = "0.3.2"
crossbeam-queue = { version = "0.3", default-features = false, features = ["alloc"] }
futures-util = { version = "0.3.17", default-features = false, features = ["alloc"] }
```

All dependencies are either:
- `no_std` compatible
- Have `default-features = false` to disable std

#### Build Dependencies (Root Package)

```toml
# Root Cargo.toml
[build-dependencies]
kernel = { path = "kernel", artifact = "bin", target = "x86_64-unknown-none" }
bootloader = { git = "https://github.com/rust-osdev/bootloader.git", branch = "main" }
```

Uses Cargo's artifact dependencies feature (unstable) to build the kernel as a dependency.

### 4. Build Process

1. **Kernel Build**: 
   - Built for custom target `x86_64-unknown-none`
   - Produces ELF binary at `target/x86_64-unknown-none/debug/kernel`

2. **Disk Image Creation**:
   - `build.rs` in root package creates bootable disk images
   - Uses `bootloader` crate to package kernel with UEFI/BIOS bootloaders
   - Creates both UEFI and BIOS boot images

3. **QEMU Runners**:
   - `qemu-uefi` and `qemu-bios` binaries launch QEMU with appropriate settings
   - Pass through command line arguments to QEMU

## Current Build Error (RESOLVED)

### Error Details

```
error: invalid signature for `extern "x86-interrupt"` function
   --> kernel/src/interrupts.rs:151:6
    |
151 | ) -> ! {
    |      ^
```

### Root Cause

A regression was introduced in nightly-2025-06-25 that forbids any return type, including `!`, in functions that use a "custom" ABI like `extern "x86-interrupt"`. This is tracked in rust-lang/rust issues #143072 and #143335.

### Solution Applied

Pinned the Rust toolchain to the last known good nightly:

```toml
# rust-toolchain.toml
[toolchain]
# last nightly before the ABI regression
channel = "nightly-2025-06-24"
profile = "minimal"
targets = ["x86_64-unknown-none"]
components = ["rust-src", "llvm-tools-preview"]
```

### Current Status

- **Kernel builds successfully** with warnings only
- **Full project build fails** due to x86_64 v0.14.13 crate trying to use x86 inline assembly on ARM host
- This is a known issue when building on Apple Silicon (aarch64) for x86_64 targets

### Workarounds

1. **Build kernel directly** (WORKS):
   ```bash
   cargo build -p kernel -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none
   ```

2. **Use the scripts** which handle the cross-compilation properly:
   ```bash
   ./scripts/run_breenix.sh
   ```

### Additional Build Issues on Apple Silicon

When building on Apple Silicon (aarch64-apple-darwin), the x86_64 v0.14.13 crate fails because it contains inline assembly using x86 registers (eax, edx, etc.) which don't exist on ARM. This affects the bootloader build dependencies.

## Build Commands and Workflows

### Standard Build

```bash
# Build the kernel
cargo build

# Build with release optimizations
cargo build --release

# Build with testing features
cargo build --features testing
```

### Running Breenix

```bash
# Using scripts (recommended - includes logging)
./scripts/run_breenix.sh

# Direct cargo commands (no log files)
cargo run --release --bin qemu-uefi -- -serial stdio -display none
cargo run --release --bin qemu-bios -- -serial stdio -display none
```

### Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_simple_kernel
```

## Cross-Compilation Challenges

1. **No Standard Library**: 
   - Must implement panic handlers
   - No heap allocation by default
   - Limited core library functionality

2. **Custom Memory Layout**:
   - Kernel must manage its own memory
   - Stack and heap must be manually configured
   - Page tables and virtual memory handled manually

3. **Hardware Features**:
   - Floating point disabled for simplicity
   - SIMD instructions disabled
   - Red zone disabled for interrupt safety

4. **Linking Challenges**:
   - Custom linker scripts may be needed
   - Entry point must be carefully specified
   - Binary format requirements for bootloader

## Debugging Build Issues

1. **Check Rust Version**:
   ```bash
   rustc --version
   rustup show
   ```

2. **Verify Target Installation**:
   ```bash
   rustup target list | grep x86_64-unknown-none
   ```

3. **Clean Build**:
   ```bash
   cargo clean
   cargo build
   ```

4. **Verbose Output**:
   ```bash
   cargo build -vv
   ```

## Next Steps for Resolution

1. **Investigate x86_64 Crate Version**: Check if recent updates changed the interrupt handler API
2. **Review Nightly Changes**: Check if recent nightly Rust changed x86-interrupt ABI
3. **Compare with Working Version**: If available, compare with last known working configuration
4. **Update Dependencies**: Try updating or pinning specific versions

## References

- [rust-osdev/bootloader](https://github.com/rust-osdev/bootloader)
- [x86_64 crate documentation](https://docs.rs/x86_64/)
- [Rust Custom Targets](https://doc.rust-lang.org/rustc/targets/custom.html)
- [Writing an OS in Rust](https://os.phil-opp.com/)