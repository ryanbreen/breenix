#!/bin/bash
set -e

echo "========================================"
echo "  USERSPACE TEST BUILD (with libbreenix)"
echo "========================================"
echo ""

# Show the libbreenix dependency
echo "Dependency: libbreenix (syscall wrapper library)"
echo "  Location: ../../libs/libbreenix"
echo "  Provides: process, io, time, memory syscall wrappers"
echo ""

# List binaries being built
BINARIES=(
    "hello_world"
    "hello_time"
    "counter"
    "spinner"
    "fork_test"
    "timer_test"
    "syscall_enosys"
    "clock_gettime_test"
    "register_init_test"
    "syscall_diagnostic_test"
    "brk_test"
)

echo "Building ${#BINARIES[@]} userspace binaries with libbreenix..."
echo ""

# Build with cargo (config is in .cargo/config.toml)
# This will compile libbreenix first, then link it into each binary
cargo build --release 2>&1 | while read line; do
    # Highlight libbreenix compilation
    if echo "$line" | grep -q "Compiling libbreenix"; then
        echo "  [libbreenix] $line"
    elif echo "$line" | grep -q "Compiling userspace_tests"; then
        echo "  [userspace]  $line"
    else
        echo "  $line"
    fi
done

echo ""
echo "Copying ELF binaries..."

# Copy and report each binary
for bin in "${BINARIES[@]}"; do
    cp "target/x86_64-breenix/release/$bin" "$bin.elf"
    echo "  - $bin.elf (uses libbreenix)"
done

echo ""
echo "Creating flat binaries..."

# Create flat binaries
for bin in "${BINARIES[@]}"; do
    rust-objcopy -O binary "$bin.elf" "$bin.bin"
done

echo ""
echo "========================================"
echo "  BUILD COMPLETE - libbreenix binaries"
echo "========================================"
echo ""
echo "Binary sizes:"
for bin in "${BINARIES[@]}"; do
    size=$(stat -f%z "$bin.bin" 2>/dev/null || stat -c%s "$bin.bin")
    printf "  %-30s %6d bytes\n" "$bin.bin" "$size"
done
echo ""
echo "These binaries use libbreenix for syscalls:"
echo "  - libbreenix::process (exit, fork, exec, getpid, gettid, yield)"
echo "  - libbreenix::io (read, write, print, println)"
echo "  - libbreenix::time (clock_gettime)"
echo "  - libbreenix::memory (brk, sbrk)"
echo "========================================"
