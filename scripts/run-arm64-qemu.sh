#!/bin/bash
# Run Breenix ARM64 kernel in QEMU
#
# Usage: ./scripts/run-arm64-qemu.sh [release|debug]

set -e

BUILD_TYPE="${1:-release}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ "$BUILD_TYPE" = "debug" ]; then
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix/debug/kernel-aarch64"
else
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
fi

# Check if kernel exists
if [ ! -f "$KERNEL" ]; then
    echo "Building ARM64 kernel ($BUILD_TYPE)..."
    if [ "$BUILD_TYPE" = "debug" ]; then
        cargo build --target aarch64-breenix.json -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    else
        cargo build --release --target aarch64-breenix.json -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    fi
fi

# Check for ext2 disk image
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
DISK_OPTS=""
if [ -f "$EXT2_DISK" ]; then
    echo "Found ext2 disk image: $EXT2_DISK"
    DISK_OPTS="-device virtio-blk-device,drive=ext2disk \
        -blockdev driver=file,node-name=ext2file,filename=$EXT2_DISK \
        -blockdev driver=raw,node-name=ext2disk,file=ext2file"
else
    echo "No ext2 disk found - running without userspace"
    DISK_OPTS=""
fi

echo ""
echo "========================================="
echo "  Breenix ARM64 Kernel"
echo "========================================="
echo "Kernel: $KERNEL"
[ -f "$EXT2_DISK" ] && echo "Ext2 disk: $EXT2_DISK"
echo ""
echo "Press Ctrl-A X to exit QEMU"
echo ""

# Network options (SLIRP user-mode networking)
NET_OPTS="-device virtio-net-device,netdev=net0 \
    -netdev user,id=net0,net=10.0.2.0/24,dhcpstart=10.0.2.15"

# Debug options (set BREENIX_NET_DEBUG=1 to enable packet capture)
DEBUG_OPTS=""
if [ "${BREENIX_NET_DEBUG:-0}" = "1" ]; then
    echo "Network debugging enabled - packets logged to /tmp/breenix-packets.pcap"
    DEBUG_OPTS="-object filter-dump,id=dump0,netdev=net0,file=/tmp/breenix-packets.pcap"
fi

# QEMU tracing for VirtIO debugging (set BREENIX_VIRTIO_TRACE=1)
if [ "${BREENIX_VIRTIO_TRACE:-0}" = "1" ]; then
    echo "VirtIO tracing enabled"
    DEBUG_OPTS="$DEBUG_OPTS -trace virtio_*"
fi

exec qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a72 \
    -m 512M \
    -nographic \
    -kernel "$KERNEL" \
    $DISK_OPTS \
    $NET_OPTS \
    $DEBUG_OPTS
