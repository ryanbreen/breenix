#!/bin/bash
# Create ext2 disk image for Breenix kernel testing
#
# This script creates a 4MB ext2 filesystem image with test files.
# Requires Docker on macOS (or mke2fs on Linux).
#
# Usage:
#   ./scripts/create_ext2_disk.sh
#
# Or use xtask:
#   cargo run -p xtask -- create-ext2-disk

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$PROJECT_ROOT/target"
OUTPUT_FILE="$TARGET_DIR/ext2.img"
SIZE_MB=4

echo "Creating ext2 disk image..."
echo "  Output: $OUTPUT_FILE"
echo "  Size: ${SIZE_MB}MB"

# Ensure target directory exists
mkdir -p "$TARGET_DIR"

# Check if we're on macOS or Linux
if [[ "$(uname)" == "Darwin" ]]; then
    # macOS - use Docker
    if ! command -v docker &> /dev/null; then
        echo "Error: Docker is required on macOS to create ext2 images"
        echo "Install Docker Desktop: https://docs.docker.com/desktop/mac/install/"
        exit 1
    fi

    if ! docker info &> /dev/null; then
        echo "Error: Docker daemon is not running"
        echo "Start Docker Desktop and try again"
        exit 1
    fi

    echo "  Using Docker to create ext2 filesystem..."

    docker run --rm --privileged \
        -v "$TARGET_DIR:/work" \
        alpine:latest \
        sh -c '
            set -e
            apk add --no-cache e2fsprogs >/dev/null 2>&1

            # Create the empty disk image
            dd if=/dev/zero of=/work/ext2.img bs=1M count='"$SIZE_MB"' status=none

            # Create ext2 filesystem
            mke2fs -t ext2 -F /work/ext2.img >/dev/null 2>&1

            # Mount and populate
            mkdir -p /mnt/ext2
            mount /work/ext2.img /mnt/ext2

            # Create test files
            echo "Hello from ext2!" > /mnt/ext2/hello.txt
            mkdir -p /mnt/ext2/test
            echo "Nested file content" > /mnt/ext2/test/nested.txt

            # Create additional test content
            mkdir -p /mnt/ext2/deep/path/to/file
            echo "Deep nested content" > /mnt/ext2/deep/path/to/file/data.txt

            # Show what was created
            echo "Files created:"
            find /mnt/ext2 -type f -exec ls -la {} \;

            # Unmount
            umount /mnt/ext2

            echo "ext2 image created successfully"
        '
else
    # Linux - use native tools
    if ! command -v mke2fs &> /dev/null; then
        echo "Error: mke2fs not found. Install e2fsprogs:"
        echo "  apt-get install e2fsprogs  # Debian/Ubuntu"
        echo "  yum install e2fsprogs      # RHEL/CentOS"
        exit 1
    fi

    # Create empty image
    dd if=/dev/zero of="$OUTPUT_FILE" bs=1M count=$SIZE_MB status=none

    # Create ext2 filesystem
    mke2fs -t ext2 -F "$OUTPUT_FILE" >/dev/null 2>&1

    # Mount and populate (requires root)
    if [[ $EUID -ne 0 ]]; then
        echo "Warning: Need root to mount and populate image"
        echo "Run with sudo or populate manually"
        exit 0
    fi

    MOUNT_DIR=$(mktemp -d)
    mount "$OUTPUT_FILE" "$MOUNT_DIR"

    # Create test files
    echo "Hello from ext2!" > "$MOUNT_DIR/hello.txt"
    mkdir -p "$MOUNT_DIR/test"
    echo "Nested file content" > "$MOUNT_DIR/test/nested.txt"
    mkdir -p "$MOUNT_DIR/deep/path/to/file"
    echo "Deep nested content" > "$MOUNT_DIR/deep/path/to/file/data.txt"

    # Show what was created
    echo "Files created:"
    find "$MOUNT_DIR" -type f -exec ls -la {} \;

    # Unmount and cleanup
    umount "$MOUNT_DIR"
    rmdir "$MOUNT_DIR"

    echo "ext2 image created successfully"
fi

# Verify output
if [[ -f "$OUTPUT_FILE" ]]; then
    SIZE=$(ls -lh "$OUTPUT_FILE" | awk '{print $5}')
    echo ""
    echo "ext2 disk created: $OUTPUT_FILE"
    echo "  Size: $SIZE"
    echo "  Contents:"
    echo "    /hello.txt - \"Hello from ext2!\""
    echo "    /test/nested.txt - \"Nested file content\""
    echo "    /deep/path/to/file/data.txt - \"Deep nested content\""
else
    echo "Error: Failed to create ext2 image"
    exit 1
fi
