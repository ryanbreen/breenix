#!/bin/bash
# Create ext2 home disk image for Breenix /home mount
#
# This script creates a 1GB ext2 filesystem image for user data (/home).
# Separate from the system disk so user-specific files (saves, configs)
# don't interfere with system syncing between machines.
#
# Requires Docker on macOS (or mke2fs on Linux).
#
# Usage:
#   ./scripts/create_home_disk.sh
#   ./scripts/create_home_disk.sh --arch aarch64
#   ./scripts/create_home_disk.sh --size 128

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$PROJECT_ROOT/target"
SIZE_MB=1024

ARCH="x86_64"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --size)
            SIZE_MB="$2"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--arch x86_64|aarch64] [--size MB]"
            exit 1
            ;;
    esac
done

if [[ "$ARCH" == "aarch64" ]]; then
    OUTPUT_FILE="$TARGET_DIR/ext2-home-aarch64.img"
else
    OUTPUT_FILE="$TARGET_DIR/ext2-home.img"
fi

echo "Creating ext2 home disk image..."
echo "  Arch: $ARCH"
echo "  Output: $OUTPUT_FILE"
echo "  Size: ${SIZE_MB}MB"

# Ensure target directory exists
mkdir -p "$TARGET_DIR"

# Check if we're on macOS or Linux
if [[ "$(uname)" == "Darwin" ]]; then
    # macOS - use Docker
    if ! command -v docker &> /dev/null; then
        echo "Error: Docker is required on macOS to create ext2 images"
        exit 1
    fi

    if ! docker info &> /dev/null; then
        echo "Error: Docker daemon is not running"
        exit 1
    fi

    OUTPUT_FILENAME=$(basename "$OUTPUT_FILE")

    docker run --rm --privileged \
        -v "$TARGET_DIR:/work" \
        -e "OUTPUT_FILENAME=$OUTPUT_FILENAME" \
        alpine:latest \
        sh -c '
            set -e
            apk add --no-cache e2fsprogs >/dev/null 2>&1

            dd if=/dev/zero of=/work/$OUTPUT_FILENAME bs=1M count='"$SIZE_MB"' status=none
            mke2fs -t ext2 -F /work/$OUTPUT_FILENAME >/dev/null 2>&1

            echo "ext2 home disk created successfully"
        '
else
    # Linux - use native tools
    if ! command -v mke2fs &> /dev/null; then
        echo "Error: mke2fs not found. Install e2fsprogs."
        exit 1
    fi

    dd if=/dev/zero of="$OUTPUT_FILE" bs=1M count=$SIZE_MB status=none
    mke2fs -t ext2 -F "$OUTPUT_FILE" >/dev/null 2>&1

    echo "ext2 home disk created successfully"
fi

if [[ -f "$OUTPUT_FILE" ]]; then
    SIZE=$(ls -lh "$OUTPUT_FILE" | awk '{print $5}')
    echo ""
    echo "Home disk created:"
    echo "  $OUTPUT_FILE"
    echo "  Size: $SIZE"
    echo "  Contents: empty filesystem (for /home user data)"
else
    echo "Error: Failed to create home disk image"
    exit 1
fi
