# ARM64 to x86_64 Bare Metal Cross-Compilation Issues

## Overview

Breenix is an x86_64 operating system kernel that needs to be built on Apple Silicon (ARM64/aarch64) development machines and run in QEMU x86_64 emulation. This creates a complex cross-compilation scenario with multiple challenges.

## Build Environment

### Host System
- **Architecture**: aarch64-apple-darwin (Apple Silicon M1/M2/M3)
- **OS**: macOS on ARM64
- **Rust Host Triple**: aarch64-apple-darwin

### Target System
- **Architecture**: x86_64 bare metal
- **Target Triple**: x86_64-unknown-none (standard) / x86_64-breenix (custom)
- **Runtime**: QEMU x86_64 emulator
- **Boot**: UEFI/BIOS via rust-osdev/bootloader

## Primary Issue: Mixed Architecture Dependencies

### The Core Problem

The build process involves multiple components that need different architectures:

1. **Build Tools** (host architecture - aarch64):
   - cargo, rustc, build scripts
   - QEMU runners (`qemu-uefi`, `qemu-bios` binaries)
   - MCP server and other development tools

2. **Kernel** (target architecture - x86_64):
   - The actual OS kernel binary
   - Must be compiled for x86_64-unknown-none
   - Uses x86-specific features (interrupts, page tables, etc.)

3. **Bootloader Dependencies** (mixed requirements):
   - The `bootloader` crate builds multiple components
   - Some parts run on host (build scripts)
   - Some parts compile for x86_64 (actual bootloader)
   - Dependencies like `x86_64` crate contain architecture-specific code

### Specific Build Failure

```
error: invalid register `eax`: unknown register
   --> /Users/wrb/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/x86_64-0.14.13/src/registers/model_specific.rs:231:21
```

This occurs because:
- The `x86_64` v0.14.13 crate is being compiled for the host (aarch64)
- It contains inline assembly using x86 registers (eax, edx, rax, etc.)
- These registers don't exist on ARM architecture
- The crate is a dependency of the bootloader build process

## Dependency Chain Analysis

```
breenix (workspace root)
├── kernel (builds successfully with -Zbuild-std)
│   └── x86_64 v0.15.2 (works when built for correct target)
└── build-dependencies
    └── bootloader (from git)
        └── x86_64 v0.14.13 (FAILS - tries to build for host arch)
```

## Current Workarounds and Limitations

### What Works

1. **Direct kernel compilation**:
   ```bash
   cargo build -p kernel -Zbuild-std=core,compiler_builtins,alloc \
     -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none
   ```
   This successfully builds the kernel binary.

2. **Using pre-built scripts**:
   - `scripts/run_breenix.sh` may work if it bypasses the problematic build path
   - Scripts that directly use pre-built artifacts

### What Doesn't Work

1. **Standard `cargo build`**:
   - Fails when building bootloader dependencies
   - The x86_64 v0.14.13 crate can't compile on ARM

2. **Full workspace build**:
   - Any build that includes the bootloader as a build dependency

## Technical Details

### Custom Target Configuration

```json
// x86_64-breenix.json
{
  "llvm-target": "x86_64-unknown-none",
  "arch": "x86_64",
  "target-endian": "little",
  "os": "none",
  "executables": true,
  "linker-flavor": "ld.lld",
  "linker": "rust-lld",
  "panic-strategy": "abort",
  "disable-redzone": true,
  "features": "-mmx,-sse,+soft-float"
}
```

### Rust Toolchain

```toml
[toolchain]
channel = "nightly-2025-06-24"  # Pinned due to x86-interrupt ABI regression
profile = "minimal"
targets = ["x86_64-unknown-none"]
components = ["rust-src", "llvm-tools-preview"]
```

## Root Causes

1. **Architecture-Specific Code in Dependencies**:
   - The x86_64 crate uses inline assembly
   - It's not properly gated to only compile for x86_64 targets
   - When cargo resolves dependencies, it tries to build for the host

2. **Build-Dependencies vs Target-Dependencies**:
   - `[build-dependencies]` compile for the host
   - But bootloader needs x86_64-specific code
   - No clean separation between host tools and target artifacts

3. **Cargo's Limited Cross-Compilation Support**:
   - Artifact dependencies are still unstable
   - Difficult to express "build this dependency for a different target"
   - `-Zbuild-std` helps but doesn't solve all issues

## Potential Solutions

### Short-term Workarounds

1. **Split Build Process**:
   - Build kernel separately with proper target
   - Use pre-built bootloader binaries
   - Manually assemble disk images

2. **Docker/VM Build Environment**:
   - Use x86_64 Linux in Docker/VM for building
   - Eliminates cross-architecture issues
   - Performance penalty on ARM Macs

3. **Conditional Dependencies**:
   - Gate problematic dependencies by target architecture
   - Use `[target.'cfg(target_arch = "x86_64")'.dependencies]`

### Long-term Solutions

1. **Update Dependency Versions**:
   - Newer versions of x86_64 crate might handle this better
   - Work with rust-osdev/bootloader to improve ARM host support

2. **Custom Build Script**:
   - Replace cargo's dependency resolution for specific crates
   - Manually handle cross-compilation requirements

3. **Separate Host Tools**:
   - Move QEMU runners and build tools to separate crate
   - Ensure clean separation of host/target code

## Impact on Development

- **CI/CD**: Need x86_64 runners or cross-compilation support
- **Local Development**: ARM Mac developers face daily friction
- **Testing**: Full integration tests may not run locally
- **Debugging**: Harder to debug when build doesn't complete

## Required Information for External Help

When seeking help, provide:

1. **Exact error messages** from failed builds
2. **Dependency tree** showing version conflicts
3. **Host and target architectures**
4. **Rust toolchain version** and components
5. **Which specific crates fail** to build
6. **What workarounds have been tried**

## Specific Questions for Resolution

1. Can the x86_64 v0.14.13 crate be updated to conditionally compile inline assembly?
2. Is there a way to force bootloader's dependencies to build for x86_64 even in build scripts?
3. Can we use pre-built bootloader artifacts instead of building from source?
4. Would moving to a different bootloader solution help?
5. Is there a cargo configuration that better handles this cross-compilation scenario?

## References

- [rust-osdev/bootloader issue tracker](https://github.com/rust-osdev/bootloader/issues)
- [Cargo build-std documentation](https://doc.rust-lang.org/cargo/reference/unstable.html#build-std)
- [Rust cross-compilation guide](https://rust-lang.github.io/rustup/cross-compilation.html)
- [QEMU system emulation](https://www.qemu.org/docs/master/system/index.html)