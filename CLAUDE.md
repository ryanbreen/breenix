# Breenix OS

## Project Overview

Breenix is an experimental x86_64 operating system kernel written in Rust. The project appears to be in early development stages, focusing on building a minimal kernel that can boot on both UEFI and BIOS systems.

### Current Status
- Basic bootloader integration using the `bootloader` crate
- Framebuffer graphics support with pixel rendering capabilities
- Custom target specification for bare metal x86_64
- Legacy codebase being migrated/rewritten (see `src.legacy/` directory)

## Architecture

### Directory Structure
```
breenix/
├── kernel/          # Core kernel implementation
│   └── src/
│       ├── main.rs       # Kernel entry point
│       └── framebuffer.rs # Graphics/display handling
├── src/             # Build system entry point
├── src.legacy/      # Previous implementation (being phased out)
├── libs/            # Supporting libraries
│   ├── libbreenix/  # System call interface library
│   └── tiered_allocator/ # Memory allocation library
├── tests/           # Integration tests
└── build.rs         # Build script for disk image creation
```

### Key Components

1. **Kernel** (`kernel/`): The main kernel binary that runs after boot
   - No standard library (`#![no_std]`)
   - Custom panic handler
   - Basic framebuffer graphics using `embedded-graphics`

2. **Build System**:
   - Uses cargo workspaces
   - Custom build script creates UEFI and BIOS bootable disk images
   - Artifact dependencies for kernel binary

3. **Legacy Code** (`src.legacy/`):
   - Contains previous implementation including:
     - Interrupt handling (GDT, IDT)
     - Memory management
     - Task scheduling
     - Device drivers (VGA, serial, keyboard, network)
     - PCI support

## Coding Practices

### Rust-Specific Conventions
- **No Standard Library**: The kernel uses `#![no_std]` and `#![no_main]`
- **Nightly Rust**: Requires nightly toolchain with specific components
- **Custom Target**: Uses `x86_64-breenix.json` for bare metal compilation
- **Panic Handling**: Custom panic handler that enters infinite loop

### Code Style
- Clear module organization with descriptive names
- Use of `bootloader_api` for boot information access
- Embedded graphics abstractions for display handling
- Const-correctness for hardware constants
- Explicit error handling where applicable

### Build Configuration
- **Toolchain**: Nightly Rust with `rust-src` and `llvm-tools-preview`
- **Target**: Custom x86_64 target without OS
- **Features**: Disabled hardware features (`-mmx,-sse,+soft-float`)
- **Panic Strategy**: Abort on panic
- **Red Zone**: Disabled for interrupt safety

### Testing
- Integration tests for basic functionality:
  - Boot testing
  - Heap allocation
  - Stack overflow handling
  - Panic testing

#### Test Development Best Practices
When implementing new features, the build/test loop is KEY to our development process:

1. **Create Test Cases Early**:
   - Write integration tests in `tests/` directory
   - Add runtime tests with the `testing` feature flag
   - Create shell scripts for comprehensive testing when appropriate
   - Test both positive cases AND error conditions

2. **Test Infrastructure Overview**:
   - **Shared QEMU Tests**: Most tests use `tests/shared_qemu.rs` for efficient testing
   - **Special Tests**: Some tests require specific configurations and are marked `#[ignore]`
   - **POST Completion**: Tests wait for kernel completion marker `🎯 KERNEL_POST_TESTS_COMPLETE 🎯`

3. **Test Categories**:
   
   **Standard Tests (use shared QEMU):**
   ```bash
   cargo test  # Runs all standard tests efficiently (~45 seconds)
   ```
   - `boot_post_test.rs` - Comprehensive POST validation (14 subsystems)
   - `interrupt_tests.rs` - Interrupt system validation (4 tests)
   - `memory_tests.rs` - Memory management tests (3 tests)
   - `logging_tests.rs` - Logging system tests (3 tests)
   - `timer_tests.rs` - Timer and RTC tests (4 tests)
   - `simple_kernel_test.rs` - Basic execution test
   - `kernel_build_test.rs` - Build validation (3 tests)
   - `system_tests.rs` - Boot sequence and stability (2 tests)

   **Special Tests (require specific handling):**
   ```bash
   # BIOS boot test (requires BIOS mode)
   cargo test test_bios_boot -- --ignored
   
   # Runtime testing feature (requires --features testing)
   cargo test test_runtime_testing_feature -- --ignored
   cargo run --features testing --bin qemu-uefi -- -serial stdio
   ```

4. **Build/Test Loop**:
   ```bash
   # Standard development workflow (FAST)
   cargo test  # Runs 21 tests with single QEMU boot

   # Manual kernel testing
   cargo run --bin qemu-uefi -- -serial stdio -display none

   # Test with runtime features  
   cargo run --features testing --bin qemu-uefi -- -serial stdio

   # Manual visual testing (optional)
   ./scripts/test_kernel.sh       # Interactive visual test
   ./test_visual.sh               # Visual test with display
   ```

5. **Performance**: Standard tests run ~3x faster due to shared QEMU instance

6. **Legacy Scripts**: Removed old redundant test scripts, kept:
   - `scripts/test_kernel.sh` - Interactive manual testing
   - `test_visual.sh` - Visual testing with QEMU display

### Development Workflow
1. Kernel code changes are made in `kernel/src/`
2. Build system automatically creates disk images
3. Tests can be run using QEMU for both UEFI and BIOS modes
4. Legacy code serves as reference for features being reimplemented
5. **Always ensure clean builds before declaring victory** - fix all warnings and run lints:
   - Fix all compiler warnings (unused imports, dead code, etc.)
   - Run `cargo clippy` if available
   - Ensure `cargo build` completes without warnings

### Pull Request Workflow

**CRITICAL: NEVER push directly to main branch!**

Once Ryan is happy with an implementation:

1. **Always work on a feature branch**:
   ```bash
   git checkout -b feature-name
   ```

2. **Push to the feature branch**:
   ```bash
   git push -u origin feature-name
   ```

3. **Create PR using GitHub CLI**:
   ```bash
   gh pr create --title "Brief description" --body "Detailed description with testing results"
   ```

4. **After creating the PR**:
   - The command will output a URL like `https://github.com/ryanbreen/breenix/pull/XX`
   - **ALWAYS open this URL** to verify the PR was created correctly
   - Share the URL with Ryan for review

5. **PR Description Should Include**:
   - Summary of changes
   - Implementation details
   - Testing performed and results
   - Any improvements over legacy implementation
   - Co-authorship credit

## Building and Running

### Prerequisites
- QEMU installed (`brew install qemu` on macOS)
- Rust nightly toolchain with required components (see rust-toolchain.toml)
- x86_64 target support

### Build Commands
On all systems:
```bash
# Build kernel with custom target (kernel uses x86_64-breenix.json)
cargo build

# Run with QEMU (UEFI mode)
cargo run --bin qemu-uefi

# Run with QEMU (BIOS mode)
cargo run --bin qemu-bios

# Run tests
cargo test --test simple_kernel_test
./scripts/test_kernel.sh
```

On x86_64 systems:
```bash
cargo run --bin qemu-uefi
cargo run --bin qemu-bios
```

## Important Notes
- The project is transitioning from a legacy codebase to a new implementation
- Current focus appears to be on establishing basic graphics and boot capabilities
- Network and advanced I/O drivers exist in legacy code but aren't yet ported
- The kernel currently implements a simple blue square rendering demo
- The kernel is built with the custom x86_64-breenix.json target
- QEMU runners and build system run on the host platform
- Tests properly separate host and target concerns

## Development Notes
All commits should be signed as co-developed by Ryan Breen and Claude Code because we're best buds!

### Legacy Code Removal Policy
As we complete feature migrations from `src.legacy/` to the new kernel:

1. **When to Remove Legacy Code**:
   - Once a feature reaches full parity or better in the new kernel
   - After verifying all functionality works correctly
   - When the feature comparison shows ✅ for both legacy and new

2. **Process**:
   - Identify the specific legacy modules/files that are now redundant
   - Remove the code from `src.legacy/`
   - Update FEATURE_COMPARISON.md to reflect the removal
   - Include legacy code removal in the same commit as the feature completion

3. **Benefits**:
   - Reduces codebase size and complexity
   - Prevents confusion about which implementation to reference
   - Makes it clear what still needs to be migrated
   - Keeps the project focused on the new implementation

Example: When timestamp logging reaches parity, remove the legacy print macros and timer code that are no longer needed as reference.