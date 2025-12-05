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
echo "========================================"
echo "  BUILD COMPLETE - libbreenix binaries"
echo "========================================"
echo ""

# Find rust-objcopy (it's in the rustup toolchain's llvm-tools)
OBJCOPY=""
if command -v rust-objcopy &> /dev/null; then
    OBJCOPY="rust-objcopy"
else
    # Try to find it in the rustup toolchain
    SYSROOT=$(rustc --print sysroot 2>/dev/null || true)
    if [ -n "$SYSROOT" ]; then
        # Check multiple possible locations
        for path in \
            "$SYSROOT/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-objcopy" \
            "$SYSROOT/lib/rustlib/aarch64-apple-darwin/bin/rust-objcopy" \
            "$SYSROOT/lib/rustlib/x86_64-apple-darwin/bin/rust-objcopy"; do
            if [ -x "$path" ]; then
                OBJCOPY="$path"
                break
            fi
        done
    fi
fi

if [ -n "$OBJCOPY" ]; then
    echo "Creating flat binaries (using $OBJCOPY)..."
    for bin in "${BINARIES[@]}"; do
        "$OBJCOPY" -O binary "$bin.elf" "$bin.bin"
    done
    echo ""
    echo "Binary sizes (.bin flat format):"
    for bin in "${BINARIES[@]}"; do
        size=$(stat -f%z "$bin.bin" 2>/dev/null || stat -c%s "$bin.bin")
        printf "  %-30s %6d bytes\n" "$bin.bin" "$size"
    done
else
    echo "Skipping flat binary creation (rust-objcopy not found)"
    echo "Note: ELF files are still available and are what the kernel embeds"
    echo ""
    echo "ELF binary sizes:"
    for bin in "${BINARIES[@]}"; do
        size=$(stat -f%z "$bin.elf" 2>/dev/null || stat -c%s "$bin.elf")
        printf "  %-30s %6d bytes\n" "$bin.elf" "$size"
    done
fi
echo ""
echo "These binaries use libbreenix for syscalls:"
echo "  - libbreenix::process (exit, fork, exec, getpid, gettid, yield)"
echo "  - libbreenix::io (read, write, print, println)"
echo "  - libbreenix::time (clock_gettime)"
echo "  - libbreenix::memory (brk, sbrk)"
echo "========================================"
