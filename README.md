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

```bash
# Run with QEMU (UEFI mode)
cargo run --bin qemu-uefi

# Run tests
cargo test

# Build with userspace programs
cargo build --features testing
```

## Documentation

- [PROJECT_ROADMAP.md](docs/planning/PROJECT_ROADMAP.md) - Development roadmap and current status
- [CLAUDE.md](CLAUDE.md) - Development practices and documentation guide
- [docs/planning/](docs/planning/) - Detailed planning documents by phase
