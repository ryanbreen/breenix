# Breenix OS

An experimental x86_64 operating system written in Rust.

## Project Status

Breenix is a working OS with:
- UEFI/BIOS boot support
- Preemptive multitasking
- Userspace process execution
- Basic POSIX system calls
- Keyboard and timer drivers

See [docs/planning/PROJECT_ROADMAP.md](docs/planning/PROJECT_ROADMAP.md) for current development status and roadmap.

## Quick Start

### Running Breenix Interactively

```bash
# Run Breenix in UEFI mode with display
cargo run --release --bin qemu-uefi

# Run Breenix in UEFI mode headless (recommended for development)
cargo run --release --bin qemu-uefi -- -serial stdio -display none

# Run with testing features enabled (includes userspace test programs)
cargo run --release --features testing --bin qemu-uefi -- -serial stdio -display none
```

### Interactive Commands

When running interactively, you can use these keyboard shortcuts:
- `Ctrl+P` - Test multiple concurrent processes
- `Ctrl+U` - Run single userspace test
- `Ctrl+F` - Test fork() system call
- `Ctrl+E` - Test exec() system call
- `Ctrl+X` - Test fork+exec pattern
- `Ctrl+H` - Test shell-style fork+exec
- `Ctrl+T` - Show time debug info
- `Ctrl+M` - Show memory debug info

### Running Tests

```bash
# Run all tests (uses shared QEMU instance for efficiency)
cargo test

# Run specific test
cargo test test_name

# Run kernel tests with specific test harness
cargo test --test kernel_tests

# Run a specific kernel test (e.g., multiple_processes)
cargo test test_multiple_processes --test kernel_tests

# Run tests with visual QEMU window
BREENIX_VISUAL_TEST=1 cargo test
```

## Development Requirements

### 🚨 MANDATORY: Clean Builds & Passing Tests

**All commits MUST have:**
1. **Zero compiler warnings** - Run `cargo build` and verify no warnings
2. **All tests passing** - Run `cargo test` and verify all tests pass

We maintain strict code quality standards. No exceptions.

## Documentation

- [PROJECT_ROADMAP.md](docs/planning/PROJECT_ROADMAP.md) - Development roadmap and current status
- [CLAUDE.md](CLAUDE.md) - Development practices and documentation guide
- [docs/planning/](docs/planning/) - Detailed planning documents by phase
