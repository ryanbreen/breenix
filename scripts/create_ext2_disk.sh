#!/bin/bash
# Create ext2 disk image for Breenix kernel testing
#
# This script creates a 4MB ext2 filesystem image with:
#   - Test files for filesystem testing
#   - Coreutils binaries in /bin/ (cat, ls, echo, mkdir, rmdir, rm, cp, mv, false, head, tail, wc, which)
#   - /sbin/true for PATH order testing
#   - hello_world binary for exec testing
#
# Requires Docker on macOS (or mke2fs on Linux).
#
# Usage:
#   ./scripts/create_ext2_disk.sh
#   ./scripts/create_ext2_disk.sh --arch aarch64
#   ./scripts/create_ext2_disk.sh --arch aarch64 --size 8
#
# Or use xtask:
#   cargo run -p xtask -- create-ext2-disk

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$PROJECT_ROOT/target"
SIZE_MB=4

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
    USERSPACE_DIR="$PROJECT_ROOT/userspace/tests/aarch64"
    OUTPUT_FILE="$TARGET_DIR/ext2-aarch64.img"
    TESTDATA_FILE="$PROJECT_ROOT/testdata/ext2-aarch64.img"
else
    USERSPACE_DIR="$PROJECT_ROOT/userspace/tests"
    OUTPUT_FILE="$TARGET_DIR/ext2.img"
    TESTDATA_FILE="$PROJECT_ROOT/testdata/ext2.img"
fi

# Coreutils to install in /bin
COREUTILS="cat ls echo mkdir rmdir rm cp mv true false head tail wc which"

echo "Creating ext2 disk image..."
echo "  Arch: $ARCH"
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

    # Extract just the filename from OUTPUT_FILE for use in Docker
    OUTPUT_FILENAME=$(basename "$OUTPUT_FILE")

    docker run --rm --privileged \
        -v "$TARGET_DIR:/work" \
        -v "$USERSPACE_DIR:/binaries:ro" \
        -e "OUTPUT_FILENAME=$OUTPUT_FILENAME" \
        alpine:latest \
        sh -c '
            set -e
            apk add --no-cache e2fsprogs >/dev/null 2>&1

            # Create the empty disk image
            dd if=/dev/zero of=/work/$OUTPUT_FILENAME bs=1M count='"$SIZE_MB"' status=none

            # Create ext2 filesystem
            mke2fs -t ext2 -F /work/$OUTPUT_FILENAME >/dev/null 2>&1

            # Mount and populate
            mkdir -p /mnt/ext2
            mount /work/$OUTPUT_FILENAME /mnt/ext2

            # Create /bin and /sbin directories for coreutils
            mkdir -p /mnt/ext2/bin
            mkdir -p /mnt/ext2/sbin

            # Copy coreutils binaries to /bin (excluding true which goes to /sbin)
            echo "Installing coreutils in /bin..."
            for bin in cat ls echo mkdir rmdir rm cp mv false head tail wc which; do
                if [ -f /binaries/${bin}.elf ]; then
                    cp /binaries/${bin}.elf /mnt/ext2/bin/${bin}
                    chmod 755 /mnt/ext2/bin/${bin}
                    echo "  /bin/${bin} installed"
                else
                    echo "  WARNING: ${bin}.elf not found in /binaries/"
                fi
            done

            # Install true in /sbin to test PATH lookup order
            echo "Installing binaries in /sbin..."
            if [ -f /binaries/true.elf ]; then
                cp /binaries/true.elf /mnt/ext2/sbin/true
                chmod 755 /mnt/ext2/sbin/true
                echo "  /sbin/true installed"
            else
                echo "  WARNING: true.elf not found in /binaries/"
            fi

            # Copy hello_world for exec testing
            if [ -f /binaries/hello_world.elf ]; then
                cp /binaries/hello_world.elf /mnt/ext2/bin/hello_world
                chmod 755 /mnt/ext2/bin/hello_world
                echo "  /bin/hello_world installed"
            else
                echo "  WARNING: hello_world.elf not found"
            fi

            # Copy init_shell for interactive use and telnet
            if [ -f /binaries/init_shell.elf ]; then
                cp /binaries/init_shell.elf /mnt/ext2/bin/init_shell
                chmod 755 /mnt/ext2/bin/init_shell
                echo "  /bin/init_shell installed"
            else
                echo "  WARNING: init_shell.elf not found"
            fi

            # Copy telnetd for remote access (system daemon, goes in /sbin)
            if [ -f /binaries/telnetd.elf ]; then
                cp /binaries/telnetd.elf /mnt/ext2/sbin/telnetd
                chmod 755 /mnt/ext2/sbin/telnetd
                echo "  /sbin/telnetd installed"
            else
                echo "  WARNING: telnetd.elf not found"
            fi

            # Create test files for filesystem testing
            echo "Hello from ext2!" > /mnt/ext2/hello.txt
            echo "Truncate test file" > /mnt/ext2/trunctest.txt
            touch /mnt/ext2/empty.txt  # Empty file for wc testing
            mkdir -p /mnt/ext2/test
            echo "Nested file content" > /mnt/ext2/test/nested.txt

            # Create additional test content
            mkdir -p /mnt/ext2/deep/path/to/file
            echo "Deep nested content" > /mnt/ext2/deep/path/to/file/data.txt

            # Create multi-line test file for head/tail/wc testing (15 lines)
            cat > /mnt/ext2/lines.txt << EOF
Line 1
Line 2
Line 3
Line 4
Line 5
Line 6
Line 7
Line 8
Line 9
Line 10
Line 11
Line 12
Line 13
Line 14
Line 15
EOF

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

    # Create /bin and /sbin directories
    mkdir -p "$MOUNT_DIR/bin"
    mkdir -p "$MOUNT_DIR/sbin"

    # Copy coreutils binaries to /bin (excluding true which goes to /sbin)
    echo "Installing coreutils in /bin..."
    for bin in cat ls echo mkdir rmdir rm cp mv false head tail wc which; do
        if [ -f "$USERSPACE_DIR/${bin}.elf" ]; then
            cp "$USERSPACE_DIR/${bin}.elf" "$MOUNT_DIR/bin/${bin}"
            chmod 755 "$MOUNT_DIR/bin/${bin}"
            echo "  /bin/${bin} installed"
        else
            echo "  WARNING: ${bin}.elf not found"
        fi
    done

    # Install true in /sbin to test PATH lookup order
    echo "Installing binaries in /sbin..."
    if [ -f "$USERSPACE_DIR/true.elf" ]; then
        cp "$USERSPACE_DIR/true.elf" "$MOUNT_DIR/sbin/true"
        chmod 755 "$MOUNT_DIR/sbin/true"
        echo "  /sbin/true installed"
    else
        echo "  WARNING: true.elf not found"
    fi

    # Copy hello_world for exec testing
    if [ -f "$USERSPACE_DIR/hello_world.elf" ]; then
        cp "$USERSPACE_DIR/hello_world.elf" "$MOUNT_DIR/bin/hello_world"
        chmod 755 "$MOUNT_DIR/bin/hello_world"
        echo "  /bin/hello_world installed"
    fi

    # Copy init_shell for interactive use and telnet
    if [ -f "$USERSPACE_DIR/init_shell.elf" ]; then
        cp "$USERSPACE_DIR/init_shell.elf" "$MOUNT_DIR/bin/init_shell"
        chmod 755 "$MOUNT_DIR/bin/init_shell"
        echo "  /bin/init_shell installed"
    fi

    # Copy telnetd for remote access (system daemon, goes in /sbin)
    if [ -f "$USERSPACE_DIR/telnetd.elf" ]; then
        cp "$USERSPACE_DIR/telnetd.elf" "$MOUNT_DIR/sbin/telnetd"
        chmod 755 "$MOUNT_DIR/sbin/telnetd"
        echo "  /sbin/telnetd installed"
    fi

    # Create test files
    echo "Hello from ext2!" > "$MOUNT_DIR/hello.txt"
    echo "Truncate test file" > "$MOUNT_DIR/trunctest.txt"
    touch "$MOUNT_DIR/empty.txt"  # Empty file for wc testing
    mkdir -p "$MOUNT_DIR/test"
    echo "Nested file content" > "$MOUNT_DIR/test/nested.txt"
    mkdir -p "$MOUNT_DIR/deep/path/to/file"
    echo "Deep nested content" > "$MOUNT_DIR/deep/path/to/file/data.txt"

    # Create multi-line test file for head/tail/wc testing (15 lines)
    cat > "$MOUNT_DIR/lines.txt" << EOF
Line 1
Line 2
Line 3
Line 4
Line 5
Line 6
Line 7
Line 8
Line 9
Line 10
Line 11
Line 12
Line 13
Line 14
Line 15
EOF

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
    echo "  /sbin/true, /bin/false - exit status coreutils"
    echo "  /bin/head, tail, wc, which - text processing coreutils"
    echo "  /bin/hello_world - exec test binary (exit code 42)"
    echo "  /bin/init_shell - interactive shell"
    echo "  /sbin/telnetd - telnet daemon"
    echo "  /hello.txt - test file (1 line)"
    echo "  /lines.txt - multi-line test file (15 lines) for head/tail/wc"
    echo "  /test/nested.txt - nested test file"
    echo "  /deep/path/to/file/data.txt - deep nested test file"
else
    echo "Error: Failed to create ext2 image"
    exit 1
fi
