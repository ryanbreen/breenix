#!/bin/bash
# Create ext2 disk image for Breenix kernel testing
#
# This script creates a 4MB ext2 filesystem image with:
#   - Test files for filesystem testing
#   - Coreutils binaries in /bin/ (cat, ls, echo, mkdir, rmdir, rm, cp, mv, true, false, head, tail, wc)
#   - hello_world binary for exec testing
#
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
USERSPACE_DIR="$PROJECT_ROOT/userspace/tests"
OUTPUT_FILE="$TARGET_DIR/ext2.img"
TESTDATA_FILE="$PROJECT_ROOT/testdata/ext2.img"
SIZE_MB=4

# Coreutils to install in /bin
COREUTILS="cat ls echo mkdir rmdir rm cp mv true false head tail wc"

echo "Creating ext2 disk image..."
echo "  Output: $OUTPUT_FILE"
echo "  Size: ${SIZE_MB}MB"
echo "  Coreutils: $COREUTILS"

# Ensure target directory exists
mkdir -p "$TARGET_DIR"
mkdir -p "$PROJECT_ROOT/testdata"

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
        -v "$USERSPACE_DIR:/binaries:ro" \
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

            # Create /bin directory for coreutils
            mkdir -p /mnt/ext2/bin

            # Copy coreutils binaries
            echo "Installing coreutils in /bin..."
            for bin in cat ls echo mkdir rmdir rm cp mv true false head tail wc; do
                if [ -f /binaries/${bin}.elf ]; then
                    cp /binaries/${bin}.elf /mnt/ext2/bin/${bin}
                    chmod 755 /mnt/ext2/bin/${bin}
                    echo "  /bin/${bin} installed"
                else
                    echo "  WARNING: ${bin}.elf not found in /binaries/"
                fi
            done

            # Copy hello_world for exec testing
            if [ -f /binaries/hello_world.elf ]; then
                cp /binaries/hello_world.elf /mnt/ext2/bin/hello_world
                chmod 755 /mnt/ext2/bin/hello_world
                echo "  /bin/hello_world installed"
            else
                echo "  WARNING: hello_world.elf not found"
            fi

            # Create test files for filesystem testing
            echo "Hello from ext2!" > /mnt/ext2/hello.txt
            mkdir -p /mnt/ext2/test
            echo "Nested file content" > /mnt/ext2/test/nested.txt

            # Create additional test content
            mkdir -p /mnt/ext2/deep/path/to/file
            echo "Deep nested content" > /mnt/ext2/deep/path/to/file/data.txt

            # Show what was created
            echo ""
            echo "ext2 filesystem contents:"
            echo "  Binaries in /bin:"
            ls -la /mnt/ext2/bin/ 2>/dev/null || echo "    (none)"
            echo "  Test files:"
            find /mnt/ext2 -type f -not -path "/mnt/ext2/bin/*" -exec ls -la {} \;

            # Unmount
            umount /mnt/ext2

            echo ""
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

    # Create /bin directory
    mkdir -p "$MOUNT_DIR/bin"

    # Copy coreutils binaries
    echo "Installing coreutils in /bin..."
    for bin in cat ls echo mkdir rmdir rm cp mv true false head tail wc; do
        if [ -f "$USERSPACE_DIR/${bin}.elf" ]; then
            cp "$USERSPACE_DIR/${bin}.elf" "$MOUNT_DIR/bin/${bin}"
            chmod 755 "$MOUNT_DIR/bin/${bin}"
            echo "  /bin/${bin} installed"
        else
            echo "  WARNING: ${bin}.elf not found"
        fi
    done

    # Copy hello_world for exec testing
    if [ -f "$USERSPACE_DIR/hello_world.elf" ]; then
        cp "$USERSPACE_DIR/hello_world.elf" "$MOUNT_DIR/bin/hello_world"
        chmod 755 "$MOUNT_DIR/bin/hello_world"
        echo "  /bin/hello_world installed"
    fi

    # Create test files
    echo "Hello from ext2!" > "$MOUNT_DIR/hello.txt"
    mkdir -p "$MOUNT_DIR/test"
    echo "Nested file content" > "$MOUNT_DIR/test/nested.txt"
    mkdir -p "$MOUNT_DIR/deep/path/to/file"
    echo "Deep nested content" > "$MOUNT_DIR/deep/path/to/file/data.txt"

    # Show what was created
    echo ""
    echo "ext2 filesystem contents:"
    ls -la "$MOUNT_DIR/bin/"
    find "$MOUNT_DIR" -type f -not -path "$MOUNT_DIR/bin/*" -exec ls -la {} \;

    # Unmount and cleanup
    umount "$MOUNT_DIR"
    rmdir "$MOUNT_DIR"

    echo "ext2 image created successfully"
fi

# Copy to testdata/ for version control
if [[ -f "$OUTPUT_FILE" ]]; then
    cp "$OUTPUT_FILE" "$TESTDATA_FILE"
    SIZE=$(ls -lh "$OUTPUT_FILE" | awk '{print $5}')
    echo ""
    echo "ext2 disk created and copied to testdata/:"
    echo "  $OUTPUT_FILE"
    echo "  $TESTDATA_FILE"
    echo "  Size: $SIZE"
    echo ""
    echo "Contents:"
    echo "  /bin/cat, ls, echo, mkdir, rmdir, rm, cp, mv - file coreutils"
    echo "  /bin/true, false - exit status coreutils"
    echo "  /bin/head, tail, wc - text processing coreutils"
    echo "  /bin/hello_world - exec test binary (exit code 42)"
    echo "  /hello.txt - test file"
    echo "  /test/nested.txt - nested test file"
    echo "  /deep/path/to/file/data.txt - deep nested test file"
else
    echo "Error: Failed to create ext2 image"
    exit 1
fi
