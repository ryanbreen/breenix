# Breenix CI/CD Patterns Reference

## Core Requirements

### Rust Toolchain
- **Nightly version**: `nightly-2025-06-24` (pinned for consistency)
- **Components**: `rust-src`, `llvm-tools-preview`
- **Optional**: `clippy` for code quality checks
- **Target**: `x86_64-unknown-none` for kernel builds

### System Dependencies
```bash
sudo apt-get update
sudo apt-get install -y \
  qemu-system-x86 \
  qemu-utils \
  ovmf \
  mtools \
  dosfstools \
  xorriso \
  nasm \
  lld
```

### Build Tools
- **llvm-tools**: Add to PATH: `$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin`
- **ld.lld**: Required for linking userspace binaries

## Breenix-Specific Build Patterns

### Userspace Binary Building
Must be done before kernel tests that execute userspace code:

```bash
export PATH="$PATH:$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin"
cd userspace/programs
./build.sh
```

### Kernel Build with Testing Features
```bash
cargo run --release --features testing --bin qemu-uefi -- -serial stdio -display none
```

### Using xtask for Tests
Breenix uses the `xtask` pattern for complex test workflows:

```bash
cargo run -p xtask -- ring3-smoke
cargo run -p xtask -- ring3-enosys
```

## Timeout Strategies

### Typical Timeouts
- **Code quality checks**: 10-15 minutes
- **Build + simple tests**: 20 minutes
- **Full integration tests**: 45 minutes
- **CI environment**: Add 2-3x overhead for slower runners

### QEMU Execution Timeouts
- **Local**: 30 seconds for smoke tests
- **CI**: 60 seconds (logs are verbose, builds are slower)
- **File creation**: 30s local, 300s (5 min) CI

## Caching Strategies

### Cargo Cache
```yaml
- uses: actions/cache@v4
  with:
    path: |
      ~/.cargo/registry
      ~/.cargo/git
      target
    key: cargo-${{ runner.os }}-${{ hashFiles('**/Cargo.lock') }}
```

### Benefits
- Reduces build time from ~10 minutes to ~2 minutes
- Especially important for dependency-heavy builds
- Invalidates on Cargo.lock changes

## Test Execution Patterns

### Serial Output to File
```yaml
- name: Run test
  run: cargo run -p xtask -- ring3-smoke
```

Internally, xtask uses:
```rust
-serial file:target/xtask_ring3_smoke_output.txt
```

### Log Artifact Upload
```yaml
- name: Upload logs
  if: always()  # Run even on failure
  uses: actions/upload-artifact@v4
  with:
    name: breenix-logs
    path: |
      logs/*.log
      target/xtask_ring3_smoke_output.txt
    if-no-files-found: ignore
    retention-days: 7
```

## Success Detection Patterns

### Looking for Signals in Output
```rust
// Check for userspace execution evidence
if contents.contains("USERSPACE OUTPUT: Hello from userspace") ||
   (contents.contains("Context switch: from_userspace=true, CS=0x33") &&
    contents.contains("restore_userspace_thread_context: Restoring thread"))
```

### Common Success Signals
- `USERSPACE OUTPUT: Hello from userspace`
- `🎯 KERNEL_POST_TESTS_COMPLETE 🎯`
- `Context switch: from_userspace=true, CS=0x33`
- `✅ SUCCESS: Userspace syscall completed`

### Common Failure Patterns
- `DOUBLE FAULT` - Page table or stack issues
- `PAGE FAULT` - Memory mapping problems
- `Timeout` - Test hung or infinite loop
- Build errors - Missing dependencies or wrong toolchain

## Workflow Triggers

### Current Patterns
```yaml
# Run on all branches
on:
  push:
    branches: [ "**" ]
  pull_request:

# Run only manually (for expensive tests)
on:
  workflow_dispatch:

# Run on specific paths
on:
  push:
    paths:
      - 'kernel/**'
      - '.github/workflows/code-quality.yml'
```

## Environment Variables

### Useful Vars
```yaml
env:
  RUST_BACKTRACE: full
  CARGO_UNSTABLE_BINDEPS: true  # For build dependencies
  CI: true  # Detection for different timeouts
```

## Common Workflow Structure

```yaml
name: Test Name
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-22.04
    timeout-minutes: 20

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: nightly-2025-06-24
          override: true
          target: x86_64-unknown-none
          components: rust-src, llvm-tools-preview

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: cargo-${{ runner.os }}-${{ hashFiles('**/Cargo.lock') }}

      - name: Install system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y qemu-system-x86 ovmf nasm

      - name: Build userspace tests
        run: |
          export PATH="$PATH:$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin"
          cd userspace/programs
          ./build.sh

      - name: Run test
        run: cargo run -p xtask -- ring3-smoke

      - name: Upload logs
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: test-logs
          path: |
            logs/*.log
            target/xtask_*_output.txt
          retention-days: 7
```

## Debugging Failed CI

### Step 1: Check Logs
Download artifacts from the Actions run, look for:
- Compilation errors (wrong Rust version, missing components)
- Missing system dependencies (QEMU, OVMF, etc.)
- Timeout vs actual kernel panic

### Step 2: Reproduce Locally
```bash
# Use exact same commands as CI
cargo run -p xtask -- ring3-smoke
```

### Step 3: Common Fixes
- **Rust version mismatch**: Update toolchain specification
- **Missing QEMU**: Add to system dependencies
- **Timeout**: Increase timeout or optimize test
- **Userspace not built**: Add userspace build step before kernel test
- **Cache corruption**: Clear cache or change cache key
