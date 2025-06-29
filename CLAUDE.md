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

### Development Workflow
1. Kernel code changes are made in `kernel/src/`
2. Build system automatically creates disk images
3. Tests can be run using QEMU for both UEFI and BIOS modes
4. Legacy code serves as reference for features being reimplemented
5. **Always ensure clean builds before declaring victory** - fix all warnings and run lints:
   - Fix all compiler warnings (unused imports, dead code, etc.)
   - Run `cargo clippy` if available
   - Ensure `cargo build` completes without warnings

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