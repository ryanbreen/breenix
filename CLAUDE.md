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

2. **Test Structure**:
   - **Unit Tests**: For isolated functionality (in-module `#[cfg(test)]`)
   - **Integration Tests**: Boot the kernel and verify output via serial
   - **Runtime Tests**: Feature-flagged tests that run during kernel execution
   - **Shell Scripts**: For complex multi-step validation

3. **Build/Test Loop**:
   ```bash
   # Quick build check
   cargo build --target x86_64-apple-darwin

   # Run with serial output to verify functionality
   cargo run --target x86_64-apple-darwin --bin qemu-uefi -- -serial stdio -display none

   # Run with testing features enabled
   cargo run --target x86_64-apple-darwin --features testing --bin qemu-uefi -- -serial stdio
   
   # Run integration tests
   cargo test --target x86_64-apple-darwin
   ```

4. **Verify Output**: Always check serial output for expected log messages and behavior

5. **Testing Feature**:
   - `testing`: Enables all runtime tests during kernel boot
   - Currently runs GDT tests, but all new tests should be included under this feature

6. **Integration Tests**: Located in `tests/` directory
   - Run all tests: `cargo test --target x86_64-apple-darwin`
   - Tests verify kernel functionality by checking serial output

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
On macOS ARM (Apple Silicon):
```bash
# Add x86_64 macOS target if not already added
rustup target add x86_64-apple-darwin

# Build the project
cargo build --target x86_64-apple-darwin

# Run with QEMU (UEFI mode)
cargo run --target x86_64-apple-darwin --bin qemu-uefi

# Run with QEMU (BIOS mode)
cargo run --target x86_64-apple-darwin --bin qemu-bios
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
- On macOS ARM, the project must be built with `--target x86_64-apple-darwin` due to x86_64-specific code in dependencies

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