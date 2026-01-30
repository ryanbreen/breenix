#!/bin/bash
# Breenix Interactive Runner
# ===========================
# Runs Breenix with a graphical display (VNC by default)
#
# Usage:
#   ./run.sh              # ARM64 with VNC display (default)
#   ./run.sh --x86        # x86_64 with VNC display
#   ./run.sh --headless   # ARM64 with serial output only
#   ./run.sh --x86 --headless  # x86_64 with serial output only
#
# Display:
#   ARM64:  Native window (cocoa) - no VNC needed
#   x86_64: VNC at localhost:5900
#
# NOTE: x86_64 QEMU runs inside Docker for system stability (Hypervisor.framework issues).
# ARM64 QEMU runs natively (no Hypervisor.framework, better performance).

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$SCRIPT_DIR"

# Defaults: ARM64 with graphics
ARCH="arm64"
HEADLESS=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --x86|--x86_64|--amd64)
            ARCH="x86_64"
            shift
            ;;
        --arm64|--aarch64)
            ARCH="arm64"
            shift
            ;;
        --headless|--serial)
            HEADLESS=true
            shift
            ;;
        --graphics|--vnc)
            HEADLESS=false
            shift
            ;;
        -h|--help)
            echo "Usage: ./run.sh [options]"
            echo ""
            echo "Options:"
            echo "  --x86, --x86_64, --amd64   Run x86_64 kernel (default: ARM64)"
            echo "  --arm64, --aarch64         Run ARM64 kernel (default)"
            echo "  --headless, --serial       Run without display (serial only)"
            echo "  --graphics, --vnc          Run with VNC display (default)"
            echo "  -h, --help                 Show this help"
            echo ""
            echo "Display:"
            echo "  ARM64:  Native window (cocoa)"
            echo "  x86_64: VNC at localhost:5900"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Route to architecture-specific runner
if [ "$ARCH" = "arm64" ]; then
    # ARM64 path
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
    EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
    IMAGE_NAME="breenix-qemu-aarch64"
    DOCKERFILE="$BREENIX_ROOT/docker/qemu/Dockerfile.aarch64"
    VNC_PORT=5901

    # Build command for ARM64
    BUILD_CMD="cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"

    # QEMU command for ARM64
    QEMU_CMD="qemu-system-aarch64"
    QEMU_MACHINE="-M virt -cpu cortex-a72"
else
    # x86_64 path - uses UEFI boot, not direct kernel
    EXT2_DISK="$BREENIX_ROOT/target/ext2.img"
    IMAGE_NAME="breenix-qemu"
    DOCKERFILE="$BREENIX_ROOT/docker/qemu/Dockerfile"
    VNC_PORT=5900

    # Build command for x86_64
    BUILD_CMD="cargo build --release --features testing,external_test_bins,interactive --bin qemu-uefi"

    # QEMU command for x86_64
    QEMU_CMD="qemu-system-x86_64"

    # x86_64 needs to find UEFI image
    UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
    KERNEL="$UEFI_IMG"  # Reuse KERNEL var for existence check
fi

echo ""
echo "========================================="
echo "Breenix Interactive Mode"
echo "========================================="
echo ""
echo "Architecture: $ARCH"
echo "Mode: $([ "$HEADLESS" = true ] && echo "headless (serial only)" || echo "graphics (VNC)")"

# Check if kernel exists, offer to build
if [ ! -f "$KERNEL" ]; then
    echo ""
    echo "Kernel not found at: $KERNEL"
    echo ""
    read -p "Build the kernel now? [Y/n] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Nn]$ ]]; then
        echo "Aborted. Build manually with:"
        echo "  $BUILD_CMD"
        exit 1
    fi
    echo "Building kernel..."
    eval $BUILD_CMD
fi

if [ ! -f "$KERNEL" ]; then
    echo "Error: Kernel still not found after build attempt"
    exit 1
fi

echo "Kernel: $KERNEL"

# Docker setup only needed for x86_64
if [ "$ARCH" = "x86_64" ]; then
    # Check for Docker image
    if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
        echo ""
        echo "Docker image '$IMAGE_NAME' not found."
        echo "Building..."
        docker build -t "$IMAGE_NAME" -f "$DOCKERFILE" "$(dirname "$DOCKERFILE")"
    fi

    # Kill any existing containers
    EXISTING=$(docker ps -q --filter ancestor="$IMAGE_NAME" 2>/dev/null)
    if [ -n "$EXISTING" ]; then
        echo "Stopping existing containers..."
        docker kill $EXISTING 2>/dev/null || true
    fi
fi

# Create output directory
OUTPUT_DIR=$(mktemp -d)
echo "Serial output: $OUTPUT_DIR/serial.txt"

# Add ext2 disk if it exists
DISK_OPTS=""
EXT2_VOLUME=""
if [ -f "$EXT2_DISK" ]; then
    echo "Disk image: $EXT2_DISK"
    EXT2_VOLUME="-v $EXT2_DISK:/breenix/ext2.img:ro"
    if [ "$ARCH" = "arm64" ]; then
        DISK_OPTS="-device virtio-blk-device,drive=ext2disk -drive if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img"
    else
        # x86_64 uses virtio-blk-pci for UEFI compatibility
        DISK_OPTS="-drive if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off"
    fi
else
    echo "Disk image: (none - shell commands will be limited)"
    if [ "$ARCH" = "arm64" ]; then
        DISK_OPTS="-device virtio-blk-device,drive=hd0 -drive if=none,id=hd0,format=raw,file=/dev/null"
    fi
fi

# Build display and port options based on architecture and headless mode
if [ "$ARCH" = "arm64" ]; then
    if [ "$HEADLESS" = true ]; then
        DISPLAY_OPTS="-display none"
        PORT_OPTS=""
    else
        DISPLAY_OPTS="-device virtio-gpu-device -vnc :0 -device virtio-keyboard-device"
        PORT_OPTS="-p ${VNC_PORT}:5900"
    fi
else
    # x86_64 uses virtio-vga
    if [ "$HEADLESS" = true ]; then
        DISPLAY_OPTS="-display none"
        PORT_OPTS=""
    else
        DISPLAY_OPTS="-device virtio-vga -vnc :0 -k en-us"
        PORT_OPTS="-p ${VNC_PORT}:5900"
    fi
fi

if [ "$HEADLESS" = true ]; then
    echo ""
    echo "Running in headless mode. Serial output will appear below."
    echo "Press Ctrl+C to stop."
    echo ""
else
    echo ""
    if [ "$ARCH" = "arm64" ]; then
        echo "Opening native window (cocoa display)..."
    else
        echo "VNC available at: localhost:$VNC_PORT"
    fi
    echo "Press Ctrl+C to stop."
    echo ""
fi

# Build the full QEMU command based on architecture
if [ "$ARCH" = "arm64" ]; then
    # ARM64 QEMU invocation - NATIVE (no Docker needed, no Hypervisor.framework issues)
    if [ "$HEADLESS" = true ]; then
        ARM64_DISPLAY="-display none"
    else
        ARM64_DISPLAY="-display cocoa -device virtio-gpu-device -device virtio-keyboard-device"
    fi

    ARM64_DISK=""
    if [ -f "$EXT2_DISK" ]; then
        ARM64_DISK="-device virtio-blk-device,drive=ext2disk -drive if=none,id=ext2disk,format=raw,readonly=on,file=$EXT2_DISK"
    fi

    qemu-system-aarch64 \
        -M virt -cpu cortex-a72 \
        -m 512M \
        -kernel "$KERNEL" \
        $ARM64_DISPLAY \
        $ARM64_DISK \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial mon:stdio \
        -no-reboot \
        &
else
    # x86_64 QEMU invocation (UEFI boot)
    # Copy OVMF firmware to output dir
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

    # Build test binaries volume if it exists
    TEST_BIN_OPTS=""
    TEST_BIN_VOLUME=""
    if [ -f "$BREENIX_ROOT/target/test_binaries.img" ]; then
        TEST_BIN_VOLUME="-v $BREENIX_ROOT/target/test_binaries.img:/breenix/test_binaries.img:ro"
        TEST_BIN_OPTS="-drive if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off"
    fi

    docker run --rm -it \
        $PORT_OPTS \
        -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
        -v "$OUTPUT_DIR:/output" \
        $EXT2_VOLUME \
        $TEST_BIN_VOLUME \
        "$IMAGE_NAME" \
        qemu-system-x86_64 \
            -pflash /output/OVMF_CODE.fd \
            -pflash /output/OVMF_VARS.fd \
            -drive if=none,id=hd,format=raw,media=disk,readonly=on,file=/breenix/breenix-uefi.img \
            -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
            $TEST_BIN_OPTS \
            $DISK_OPTS \
            -machine pc,accel=tcg \
            -cpu qemu64 \
            -smp 1 \
            -m 512 \
            $DISPLAY_OPTS \
            -no-reboot \
            -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
            -netdev user,id=net0 \
            -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
            -serial mon:stdio \
        &
fi

QEMU_PID=$!

# If x86_64 graphics mode, try to open VNC viewer
if [ "$ARCH" = "x86_64" ] && [ "$HEADLESS" = false ] && [ "$(uname)" = "Darwin" ]; then
    sleep 2  # Give QEMU time to start
    if [ -d "/Applications/TigerVNC Viewer 1.15.0.app" ]; then
        echo "Opening TigerVNC..."
        open "/Applications/TigerVNC Viewer 1.15.0.app" --args "localhost:$VNC_PORT"
    else
        echo ""
        echo "TigerVNC not found. Connect manually to localhost:$VNC_PORT"
        echo "Install from: https://github.com/TigerVNC/tigervnc/releases"
    fi
fi

# Wait for QEMU to finish
wait $QEMU_PID 2>/dev/null || true

echo ""
echo "========================================="
echo "Breenix stopped"
echo "========================================="
echo "Serial output saved to: $OUTPUT_DIR/serial.txt"
