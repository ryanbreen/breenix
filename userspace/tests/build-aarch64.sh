#!/bin/bash
set -e

# Add LLVM tools to PATH
SYSROOT=$(rustc --print sysroot)
HOST_TRIPLE=$(rustc -vV | grep host | cut -d' ' -f2)
LLVM_TOOLS_PATH="$SYSROOT/lib/rustlib/$HOST_TRIPLE/bin"
if [ -d "$LLVM_TOOLS_PATH" ]; then
    export PATH="$LLVM_TOOLS_PATH:$PATH"
fi

if ! command -v rust-objcopy &> /dev/null; then
    echo "ERROR: rust-objcopy not found"
    echo "Install llvm-tools-preview: rustup component add llvm-tools-preview"
    exit 1
fi

echo "========================================"
echo "  ARM64 USERSPACE BUILD"
echo "========================================"

# List of binaries to include (only those that are ARM64 compatible - no x86_64 inline asm)
# These are intended to populate /bin for ext2 init_shell use.
BINARIES=(
    "hello_world"
    "simple_exit"
    "hello_time"
    "fork_test"
    "init_shell"
    "signal_test"
    # Coreutils (best-effort on ARM64)
    "cat"
    "ls"
    "echo"
    "mkdir"
    "rmdir"
    "rm"
    "cp"
    "mv"
    "true"
    "false"
    "head"
    "tail"
    "wc"
    "which"
    # PTY/telnet daemon for interactive use
    "telnetd"
)

# Binaries that rely on the libbreenix runtime _start (no local _start)
RUNTIME_BINS=(
    "cat"
    "ls"
    "echo"
    "mkdir"
    "rmdir"
    "rm"
    "cp"
    "mv"
    "true"
    "false"
    "head"
    "tail"
    "wc"
    "which"
)

# Create output directory for ARM64 binaries
mkdir -p aarch64

echo ""
echo "Building ${#BINARIES[@]} ARM64 userspace binaries..."

# Build each binary individually to avoid building x86_64-only binaries
for bin in "${BINARIES[@]}"; do
    echo "  Building $bin..."
    FEATURES=()
    for runtime_bin in "${RUNTIME_BINS[@]}"; do
        if [ "$bin" = "$runtime_bin" ]; then
            FEATURES=(--features runtime)
            break
        fi
    done
    if ! cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc "${FEATURES[@]}" --bin "$bin" 2>&1 | grep -E "^error" | head -3; then
        : # Success, no error output
    fi
done

echo ""
echo "Creating ELF files..."

for bin in "${BINARIES[@]}"; do
    if [ -f "target/aarch64-breenix/release/$bin" ]; then
        cp "target/aarch64-breenix/release/$bin" "aarch64/$bin.elf"
        echo "  - aarch64/$bin.elf"
    else
        echo "  WARNING: $bin not built (may have x86_64 dependencies)"
    fi
done

echo ""
echo "========================================"
echo "  ARM64 BUILD COMPLETE"
echo "========================================"
