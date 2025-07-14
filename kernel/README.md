# Breenix Kernel

This directory contains the core Breenix operating system kernel implementation.

## Feature Flags

The kernel supports two main feature flags for testing and validation:

- **`testing`** - Enables kernel test harness and extra logging
- **`release-tests`** - Heavy stress tests and comprehensive validation (includes `testing`)

## Environment Variables

### BREENIX_TEST

Controls which kernel tests to run during boot. This environment variable is read by the kernel test harness.

**Format**: `BREENIX_TEST=tests=foo,bar` (comma-separated list) or `BREENIX_TEST=tests=all`

**Examples**:
```bash
# Run specific tests
BREENIX_TEST=tests=divide_by_zero,invalid_opcode

# Run all available tests  
BREENIX_TEST=tests=all

# Run multiple process test
BREENIX_TEST=tests=multiple_processes
```

**Available Tests**:
- `divide_by_zero` - Exception handling test
- `invalid_opcode` - Exception handling test  
- `page_fault` - Exception handling test
- `multiple_processes` - Concurrent process creation test
- `fork_progress` - Fork/exec validation test
- `all_userspace` - Comprehensive userspace test suite

## Build Integration

The kernel integrates with the xtask build system:

```bash
# Build kernel with testing features
cargo build --features testing

# Run kernel tests via xtask
cargo run -p xtask -- build-and-run --features testing --timeout 15
```

## Test Integration

Tests are run through the integration test system in the workspace root:

```bash
# Run all integration tests
cargo test

# Run specific kernel test
BREENIX_TEST=tests=divide_by_zero cargo test integ_divide_by_zero
```