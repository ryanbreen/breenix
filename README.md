# Breenix OS

An experimental x86_64 operating system written in Rust.

## Project Status

Breenix is a working OS with:
- UEFI/BIOS boot support
- Preemptive multitasking
- Userspace process execution
- Basic POSIX system calls
- Keyboard and timer drivers

**[📊 View Interactive Progress Dashboard](https://v0-breenix-dashboard.vercel.app/)** - Visual roadmap showing POSIX compliance progress across all subsystems.

See [docs/planning/PROJECT_ROADMAP.md](docs/planning/PROJECT_ROADMAP.md) for detailed development status.

## Quick Start

```bash
# Run with QEMU (UEFI mode)
cargo run --bin qemu-uefi

# Run tests
cargo test

# Build with userspace programs
cargo build --features testing
```

### Forked Rust standard library

Userspace programs are built against a fork of the Rust standard library that
knows `target_os = "breenix"`. That checkout is **not** part of this repository
and its location differs per machine, so every build that compiles userspace
needs to be told where it is:

```bash
export BREENIX_RUST_FORK_LIBRARY=/path/to/rust-fork/library
```

Both builders honour it — `userspace/programs/build.sh` and `xtask`. Without it
they fall back to `<repo>/rust-fork/library`, so a local (untracked) `rust-fork`
symlink or directory inside the checkout works too, and is the simplest layout:

```bash
ln -s /path/to/rust-fork rust-fork
```

The fallback layout is not merely a convenience. The fork's
`library/Cargo.toml` patches `libc` to `../../libs/libc`, which resolves
relative to the fork's own parent directory — so the fork has to sit beside a
`libs/libc`, as it does when it lives inside this checkout. Pointing
`BREENIX_RUST_FORK_LIBRARY` at a fork laid out anywhere else gets past the
"forked Rust library not found" check and then fails on that patch instead.

`rust-fork` is deliberately git-ignored rather than committed: it was once
tracked as a symlink to one developer's absolute home path, which dangled in
every other checkout and failed the userspace build with no usable diagnostic
(#678, #679).

## Documentation

- [PROJECT_ROADMAP.md](docs/planning/PROJECT_ROADMAP.md) - Development roadmap and current status
- [CLAUDE.md](CLAUDE.md) - Development practices and documentation guide
- [docs/planning/](docs/planning/) - Detailed planning documents by phase
