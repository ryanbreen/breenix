#!/bin/bash
# Create ext2 disk image for Breenix kernel testing
#
# This script creates an ext2 filesystem image (64MB default) with:
#   - Test files for filesystem testing
#   - BusyBox binary in /bin/busybox with symlinks for coreutils
#   - hello_world binary for exec testing
#   - Test binaries in /usr/local/test/bin
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
SIZE_MB=64

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
    USERSPACE_DIR="$PROJECT_ROOT/userspace/programs/aarch64"
    OUTPUT_FILE="$TARGET_DIR/ext2-aarch64.img"
    TESTDATA_FILE="$PROJECT_ROOT/testdata/ext2-aarch64.img"
    # ARM64 uses same 64MB default as x86_64
else
    USERSPACE_DIR="$PROJECT_ROOT/userspace/programs"
    OUTPUT_FILE="$TARGET_DIR/ext2.img"
    TESTDATA_FILE="$PROJECT_ROOT/testdata/ext2.img"
fi

echo "Creating ext2 disk image..."
echo "  Arch: $ARCH"
echo "  Output: $OUTPUT_FILE"
echo "  Size: ${SIZE_MB}MB"

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

            # Create /bin, /sbin, /usr/local/test/bin, and /usr/local/cbin directories
            mkdir -p /mnt/ext2/bin
            mkdir -p /mnt/ext2/sbin
            mkdir -p /mnt/ext2/usr/local/test/bin
            mkdir -p /mnt/ext2/usr/local/cbin

            # Install BusyBox with symlinks for coreutils
            if [ -f /binaries/busybox.elf ]; then
                cp /binaries/busybox.elf /mnt/ext2/bin/busybox
                chmod 755 /mnt/ext2/bin/busybox

                # Create hardlinks for all applets in /bin
                # (hardlinks avoid needing symlink-following in kernel exec path)
                for cmd in cat ls head tail wc grep more cp mv rm mkdir rmdir \
                           echo which sh ash sed awk find sort uniq tee xargs \
                           chmod chown chgrp df du free date sleep test expr seq \
                           id whoami hostname basename dirname env printf cut tr \
                           od hexdump md5sum sha256sum vi; do
                    ln /mnt/ext2/bin/busybox /mnt/ext2/bin/$cmd
                done

                # /sbin applets (hardlinks)
                for cmd in true false; do
                    ln /mnt/ext2/bin/busybox /mnt/ext2/sbin/$cmd
                done

                echo "  Installed BusyBox with hardlinks in /bin and /sbin"
            else
                echo "  WARNING: busybox.elf not found, skipping coreutils"
            fi

            # Copy remaining binaries from /binaries directory
            # Routing: musl C programs (*_musl*) -> /usr/local/cbin
            #          test binaries (*_test, test_*) -> /usr/local/test/bin
            #          system binaries (telnetd, init, blogd) -> /sbin
            #          everything else -> /bin
            echo "Installing other binaries..."
            bin_count=0
            sbin_count=0
            test_count=0
            cbin_count=0
            for elf_file in /binaries/*.elf; do
                if [ -f "$elf_file" ]; then
                    bin_name=$(basename "$elf_file" .elf)
                    # Skip busybox (already installed above)
                    [ "$bin_name" = "busybox" ] && continue
                    if echo "$bin_name" | grep -qE "_musl"; then
                        cp "$elf_file" /mnt/ext2/usr/local/cbin/${bin_name}
                        chmod 755 /mnt/ext2/usr/local/cbin/${bin_name}
                        cbin_count=$((cbin_count + 1))
                    elif echo "$bin_name" | grep -qE "_test$|^test_"; then
                        cp "$elf_file" /mnt/ext2/usr/local/test/bin/${bin_name}
                        chmod 755 /mnt/ext2/usr/local/test/bin/${bin_name}
                        test_count=$((test_count + 1))
                    elif [ "$bin_name" = "telnetd" ] || [ "$bin_name" = "init" ] || [ "$bin_name" = "blogd" ]; then
                        cp "$elf_file" /mnt/ext2/sbin/${bin_name}
                        chmod 755 /mnt/ext2/sbin/${bin_name}
                        sbin_count=$((sbin_count + 1))
                    else
                        cp "$elf_file" /mnt/ext2/bin/${bin_name}
                        chmod 755 /mnt/ext2/bin/${bin_name}
                        bin_count=$((bin_count + 1))
                    fi
                fi
            done
            echo "  Installed $bin_count binaries in /bin"
            echo "  Installed $sbin_count binaries in /sbin"
            echo "  Installed $cbin_count C binaries in /usr/local/cbin"
            echo "  Installed $test_count test binaries in /usr/local/test/bin"

            # Create /etc with passwd and group for musl getpwuid/getgrgid
            mkdir -p /mnt/ext2/etc
            cat > /mnt/ext2/etc/passwd << PASSWD
root:x:0:0:root:/root:/bin/bsh
nobody:x:65534:65534:nobody:/nonexistent:/bin/false
PASSWD
            cat > /mnt/ext2/etc/group << GROUP
root:x:0:
nobody:x:65534:
GROUP

            # Create /etc/bshrc - startup config for the bsh ECMAScript shell
            cat > /mnt/ext2/etc/bshrc << 'BSHRC'
// Breenix Shell Configuration
// This file is evaluated as ECMAScript by bsh on startup.

// Global PATH - all standard binary directories
env("PATH", "/bin:/sbin:/usr/local/cbin");

// Home directory
env("HOME", "/root");

// Shell identification
env("SHELL", "/bin/bsh");

// Hostname
env("HOSTNAME", "breenix");

// Greeting
console.log("Welcome to Breenix OS");
BSHRC

            # Create /tmp for filesystem write tests
            mkdir -p /mnt/ext2/tmp

            # Create /home for user data (Gus Kit saves, etc.)
            mkdir -p /mnt/ext2/home

            # Create /var/log for blogd kernel log persistence
            mkdir -p /mnt/ext2/var/log

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
            echo "  Test binaries in /usr/local/test/bin:"
            ls -la /mnt/ext2/usr/local/test/bin/ 2>/dev/null || echo "    (none)"
            echo "  Test files:"
            find /mnt/ext2 -type f -not -path "/mnt/ext2/bin/*" -not -path "/mnt/ext2/usr/local/test/bin/*" -exec ls -la {} \;

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

    # Create /bin, /sbin, /usr/local/test/bin, and /usr/local/cbin directories
    mkdir -p "$MOUNT_DIR/bin"
    mkdir -p "$MOUNT_DIR/sbin"
    mkdir -p "$MOUNT_DIR/usr/local/test/bin"
    mkdir -p "$MOUNT_DIR/usr/local/cbin"

    # Install BusyBox with hardlinks for coreutils
    # Auto-build if missing but build script and source exist
    if [[ ! -f "$USERSPACE_DIR/busybox.elf" ]] && [[ -x "$SCRIPT_DIR/build-busybox.sh" ]]; then
        echo "  busybox.elf not found, attempting to build..."
        if "$SCRIPT_DIR/build-busybox.sh" --arch "$ARCH"; then
            echo "  BusyBox built successfully"
        else
            echo "  WARNING: BusyBox build failed (see build-busybox.sh for prerequisites)"
        fi
    fi
    if [[ -f "$USERSPACE_DIR/busybox.elf" ]]; then
        cp "$USERSPACE_DIR/busybox.elf" "$MOUNT_DIR/bin/busybox"
        chmod 755 "$MOUNT_DIR/bin/busybox"

        # Create hardlinks for all applets in /bin
        for cmd in cat ls head tail wc grep more cp mv rm mkdir rmdir \
                   echo which sh ash sed awk find sort uniq tee xargs \
                   chmod chown chgrp df du free date sleep test expr seq \
                   id whoami hostname basename dirname env printf cut tr \
                   od hexdump md5sum sha256sum vi; do
            ln "$MOUNT_DIR/bin/busybox" "$MOUNT_DIR/bin/$cmd"
        done

        # /sbin applets (hardlinks)
        for cmd in true false; do
            ln "$MOUNT_DIR/bin/busybox" "$MOUNT_DIR/sbin/$cmd"
        done

        echo "  Installed BusyBox with hardlinks in /bin and /sbin"
    else
        echo "  WARNING: busybox.elf not found, skipping coreutils"
    fi

    # Copy remaining binaries from userspace directory
    # Routing: musl C programs (*_musl*) -> /usr/local/cbin
    #          test binaries (*_test, test_*) -> /usr/local/test/bin
    #          system binaries (telnetd, init, blogd) -> /sbin
    #          everything else -> /bin
    echo "Installing other binaries..."
    bin_count=0
    sbin_count=0
    test_count=0
    cbin_count=0
    for elf_file in "$USERSPACE_DIR"/*.elf; do
        if [ -f "$elf_file" ]; then
            bin_name=$(basename "$elf_file" .elf)
            # Skip busybox (already installed above)
            [ "$bin_name" = "busybox" ] && continue
            if echo "$bin_name" | grep -qE '_musl'; then
                cp "$elf_file" "$MOUNT_DIR/usr/local/cbin/${bin_name}"
                chmod 755 "$MOUNT_DIR/usr/local/cbin/${bin_name}"
                cbin_count=$((cbin_count + 1))
            elif echo "$bin_name" | grep -qE '_test$|^test_'; then
                cp "$elf_file" "$MOUNT_DIR/usr/local/test/bin/${bin_name}"
                chmod 755 "$MOUNT_DIR/usr/local/test/bin/${bin_name}"
                test_count=$((test_count + 1))
            elif [ "$bin_name" = "telnetd" ] || [ "$bin_name" = "init" ] || [ "$bin_name" = "blogd" ]; then
                cp "$elf_file" "$MOUNT_DIR/sbin/${bin_name}"
                chmod 755 "$MOUNT_DIR/sbin/${bin_name}"
                sbin_count=$((sbin_count + 1))
            else
                cp "$elf_file" "$MOUNT_DIR/bin/${bin_name}"
                chmod 755 "$MOUNT_DIR/bin/${bin_name}"
                bin_count=$((bin_count + 1))
            fi
        fi
    done
    echo "  Installed $bin_count binaries in /bin"
    echo "  Installed $sbin_count binaries in /sbin"
    echo "  Installed $cbin_count C binaries in /usr/local/cbin"
    echo "  Installed $test_count test binaries in /usr/local/test/bin"

    # Create /etc with passwd and group for musl getpwuid/getgrgid
    mkdir -p "$MOUNT_DIR/etc"
    cat > "$MOUNT_DIR/etc/passwd" << PASSWD
root:x:0:0:root:/root:/bin/bsh
nobody:x:65534:65534:nobody:/nonexistent:/bin/false
PASSWD
    cat > "$MOUNT_DIR/etc/group" << GROUP
root:x:0:
nobody:x:65534:
GROUP

    # Create /etc/bshrc - startup config for the bsh ECMAScript shell
    cat > "$MOUNT_DIR/etc/bshrc" << 'BSHRC'
// Breenix Shell Configuration
// This file is evaluated as ECMAScript by bsh on startup.

// Global PATH - all standard binary directories
env("PATH", "/bin:/sbin:/usr/local/cbin");

// Home directory
env("HOME", "/root");

// Shell identification
env("SHELL", "/bin/bsh");

// Hostname
env("HOSTNAME", "breenix");

// Greeting
console.log("Welcome to Breenix OS");
BSHRC

    # Create /tmp for filesystem write tests
    mkdir -p "$MOUNT_DIR/tmp"

    # Create /home for user data (Gus Kit saves, etc.)
    mkdir -p "$MOUNT_DIR/home"

    # Create /var/log for blogd kernel log persistence
    mkdir -p "$MOUNT_DIR/var/log"

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
    echo "  Test binaries in /usr/local/test/bin:"
    ls -la "$MOUNT_DIR/usr/local/test/bin/" 2>/dev/null || echo "    (none)"
    find "$MOUNT_DIR" -type f -not -path "$MOUNT_DIR/bin/*" -not -path "$MOUNT_DIR/usr/local/test/bin/*" -exec ls -la {} \;

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
    echo "  /bin/busybox - BusyBox multi-call binary"
    echo "  /bin/{cat,ls,head,...} - BusyBox hardlinks"
    echo "  /sbin/{true,false} - BusyBox hardlinks"
    echo "  /bin/* - Other userspace binaries (demos)"
    echo "  /usr/local/test/bin/* - Test binaries (*_test, test_*)"
    echo "  /sbin/telnetd - telnet daemon"
    echo "  /hello.txt - test file (1 line)"
    echo "  /lines.txt - multi-line test file (15 lines) for head/tail/wc"
    echo "  /test/nested.txt - nested test file"
    echo "  /deep/path/to/file/data.txt - deep nested test file"
else
    echo "Error: Failed to create ext2 image"
    exit 1
fi
