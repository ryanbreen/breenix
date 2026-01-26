#!/bin/bash
set -e

# Add LLVM tools (rust-objcopy) to PATH
SYSROOT=$(rustc --print sysroot)
HOST_TRIPLE=$(rustc -vV | grep host | cut -d' ' -f2)
LLVM_TOOLS_PATH="$SYSROOT/lib/rustlib/$HOST_TRIPLE/bin"
if [ -d "$LLVM_TOOLS_PATH" ]; then
    export PATH="$LLVM_TOOLS_PATH:$PATH"
fi

# Verify rust-objcopy is available
if ! command -v rust-objcopy &> /dev/null; then
    echo "ERROR: rust-objcopy not found"
    echo "Install llvm-tools-preview: rustup component add llvm-tools-preview"
    exit 1
fi

echo "========================================"
echo "  USERSPACE TEST BUILD - ARM64"
echo "========================================"
echo ""

echo "Dependency: libbreenix (syscall wrapper library)"
echo "  Location: ../../libs/libbreenix"
echo "  Target: aarch64-breenix"
echo ""

# ARM64-compatible binaries (use libbreenix, no x86_64 inline asm)
BINARIES=(
    "simple_exit"
    "hello_world"
    "hello_time"
    "fork_test"
    "clock_gettime_test"
)

echo "Building ${#BINARIES[@]} ARM64 userspace binaries with libbreenix..."
echo ""

# Create output directory for ARM64 binaries
mkdir -p aarch64

# Build each binary individually to avoid building x86_64-only binaries
for bin in "${BINARIES[@]}"; do
    echo "  Building $bin..."
    if cargo build --release --target aarch64-breenix.json --bin "$bin" 2>&1 | grep -E "^(error|warning:.*error)" | head -3; then
        echo "    WARNING: Build had issues"
    fi
done

echo ""
echo "Copying ELF binaries..."

# Copy and report each binary
for bin in "${BINARIES[@]}"; do
    if [ -f "target/aarch64-breenix/release/$bin" ]; then
        cp "target/aarch64-breenix/release/$bin" "aarch64/$bin.elf"
        echo "  - aarch64/$bin.elf"
    else
        echo "  WARNING: $bin not found"
    fi
done

echo ""
echo "Creating flat binaries..."

# Create flat binaries
for bin in "${BINARIES[@]}"; do
    if [ -f "aarch64/$bin.elf" ]; then
        rust-objcopy -O binary "aarch64/$bin.elf" "aarch64/$bin.bin"
    fi
done

echo ""
echo "========================================"
echo "  BUILD COMPLETE - ARM64 binaries"
echo "========================================"
echo ""
echo "Binary sizes:"
for bin in "${BINARIES[@]}"; do
    if [ -f "aarch64/$bin.bin" ]; then
        size=$(stat -f%z "aarch64/$bin.bin" 2>/dev/null || stat -c%s "aarch64/$bin.bin")
        printf "  %-30s %6d bytes\n" "aarch64/$bin.bin" "$size"
    fi
done
echo "========================================"
