#!/bin/bash
# Build Rust std test binary for Breenix
#
# This script builds the std-based hello_world binary and copies it
# to userspace/tests/ so it gets included in the ext2 disk image.
#
# Dependencies:
#   - rust-fork/library (forked Rust std with target_os = "breenix")
#   - libs/libbreenix-libc (provides libc.a for std's Unix PAL)
#
# Usage:
#   ./userspace/tests-std/build.sh                  # x86_64 (default)
#   ./userspace/tests-std/build.sh --arch aarch64   # aarch64
#
# The built binary replaces the no_std hello_world with a real Rust std program.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Default architecture
ARCH="x86_64"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            ARCH="$2"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--arch x86_64|aarch64]"
            exit 1
            ;;
    esac
done

# Set architecture-specific variables
if [[ "$ARCH" == "aarch64" ]]; then
    TARGET_JSON="../../aarch64-breenix.json"
    TARGET_DIR="aarch64-breenix"
    TESTS_DIR="$PROJECT_ROOT/userspace/tests/aarch64"
    LIBC_RELEASE_DIR="$PROJECT_ROOT/libs/libbreenix-libc/target/aarch64-breenix/release"
else
    TARGET_JSON="../../x86_64-breenix.json"
    TARGET_DIR="x86_64-breenix"
    TESTS_DIR="$PROJECT_ROOT/userspace/tests"
    LIBC_RELEASE_DIR="$PROJECT_ROOT/libs/libbreenix-libc/target/x86_64-breenix/release"
fi

# Rustflags for linking against libbreenix-libc
# These are passed explicitly because cargo's [target.xxx] config sections
# don't match reliably when --target is a relative path (the path prefix
# prevents matching against the normalized target name).
STD_RUSTFLAGS="-L native=$LIBC_RELEASE_DIR -C link-arg=-T$SCRIPT_DIR/linker.ld -C link-arg=--allow-multiple-definition -C default-linker-libraries=no"

echo "========================================"
echo "  STD USERSPACE BUILD (Rust std library)"
echo "========================================"
echo "  Architecture: $ARCH"
echo ""

# Step 1: Build libbreenix-libc (produces libc.a)
echo "[1/2] Building libbreenix-libc ($ARCH)..."
LIBC_DIR="$PROJECT_ROOT/libs/libbreenix-libc"

if [ ! -d "$LIBC_DIR" ]; then
    echo "  ERROR: libs/libbreenix-libc not found"
    exit 1
fi

(cd "$LIBC_DIR" && \
    CARGO_ENCODED_RUSTFLAGS= \
    RUSTFLAGS= \
    cargo build --release --target "$TARGET_JSON" 2>&1 | while read line; do
        echo "  $line"
    done
)
echo "  libbreenix-libc built successfully"
echo ""

# Step 2: Build tests-std (produces hello_std_real)
echo "[2/2] Building tests-std ($ARCH)..."

RUST_FORK_LIBRARY="$PROJECT_ROOT/rust-fork/library"
if [ ! -d "$RUST_FORK_LIBRARY" ]; then
    echo "  ERROR: rust-fork/library not found"
    echo "  The forked Rust compiler is required for std support"
    exit 1
fi

(cd "$SCRIPT_DIR" && \
    unset CARGO_ENCODED_RUSTFLAGS && \
    __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_FORK_LIBRARY" \
    RUSTFLAGS="$STD_RUSTFLAGS" \
    cargo build --release --target "$TARGET_JSON" 2>&1 | while read line; do
        echo "  $line"
    done
)

# Verify the binary exists
STD_BINARY="$SCRIPT_DIR/target/$TARGET_DIR/release/hello_std_real"
if [ ! -f "$STD_BINARY" ]; then
    echo "  ERROR: Binary not found at $STD_BINARY"
    exit 1
fi

echo "  tests-std built successfully"
echo ""

# Step 3: Copy as hello_world.elf to userspace/tests/ for ext2 inclusion
if [ -d "$TESTS_DIR" ]; then
    cp "$STD_BINARY" "$TESTS_DIR/hello_world.elf"
    SIZE=$(stat -f%z "$TESTS_DIR/hello_world.elf" 2>/dev/null || stat -c%s "$TESTS_DIR/hello_world.elf")
    echo "Installed std hello_world.elf ($SIZE bytes) -> $TESTS_DIR/"
    echo "  This replaces the no_std hello_world in the ext2 disk"
else
    echo "  WARNING: $TESTS_DIR not found, skipping ext2 copy"
fi

echo ""
echo "========================================"
echo "  STD BUILD COMPLETE ($ARCH)"
echo "========================================"
