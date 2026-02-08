#!/bin/bash
# Breenix Interactive Runner
# ===========================
# Runs Breenix with a graphical display
#
# Usage:
#   ./run.sh              # ARM64 with native cocoa display (default)
#   ./run.sh --clean      # Full rebuild (userspace + ext2 disk + kernel), then run
#   ./run.sh --x86        # x86_64 with VNC display
#   ./run.sh --headless   # ARM64 with serial output only
#   ./run.sh --x86 --headless  # x86_64 with serial output only
#
# Display:
#   ARM64:  Native window (cocoa) - no VNC needed
#   x86_64: VNC at localhost:5900
#
# Both architectures run QEMU natively on the host.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$SCRIPT_DIR"

# Defaults: ARM64 with graphics
ARCH="arm64"
HEADLESS=false
CLEAN=false

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
        --clean)
            CLEAN=true
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
            echo "  --clean                    Full rebuild: userspace, ext2 disk, kernel"
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
    # ARM64 path - direct kernel boot
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
    EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"

    # Build command for ARM64
    BUILD_CMD="cargo build --release --features boot_tests --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
else
    # x86_64 path - uses UEFI boot
    EXT2_DISK="$BREENIX_ROOT/target/ext2.img"
    VNC_PORT=5900

    # Build command for x86_64
    BUILD_CMD="cargo build --release --features testing,external_test_bins,interactive --bin qemu-uefi"

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

# --clean: full rebuild of userspace, ext2 disk, and kernel
if [ "$CLEAN" = true ]; then
    echo ""
    echo "Clean build: rebuilding everything..."
    echo ""

    if [ "$ARCH" = "arm64" ]; then
        echo "[1/4] Building userspace binaries (aarch64, no_std)..."
        (cd "$BREENIX_ROOT/userspace/tests" && ./build-aarch64.sh)

        echo ""
        echo "[2/4] Building std userspace binary (aarch64)..."
        if [ -f "$BREENIX_ROOT/userspace/tests-std/build.sh" ]; then
            "$BREENIX_ROOT/userspace/tests-std/build.sh" --arch aarch64
        else
            echo "  (skipped - userspace/tests-std/build.sh not found)"
        fi

        echo ""
        echo "[3/4] Creating ext2 disk image..."
        "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64

        echo ""
        echo "[4/4] Building kernel..."
    else
        echo "[1/4] Building userspace binaries (x86_64, no_std)..."
        (cd "$BREENIX_ROOT/userspace/tests" && ./build.sh)

        echo ""
        echo "[2/4] Building std userspace binary..."
        if [ -f "$BREENIX_ROOT/userspace/tests-std/build.sh" ]; then
            "$BREENIX_ROOT/userspace/tests-std/build.sh"
        else
            echo "  (skipped - userspace/tests-std/build.sh not found)"
        fi

        echo ""
        echo "[3/4] Creating ext2 disk image..."
        "$BREENIX_ROOT/scripts/create_ext2_disk.sh"

        echo ""
        echo "[4/4] Building kernel..."
    fi
    eval $BUILD_CMD
    echo ""
fi

# Always rebuild to ensure correct features (boot_tests, etc.)
# Cargo is incremental — this is fast if nothing changed
echo "Building kernel..."
eval $BUILD_CMD

if [ ! -f "$KERNEL" ]; then
    echo "Error: Kernel not found after build"
    exit 1
fi

echo "Kernel: $KERNEL"

# Create output directory
OUTPUT_DIR=$(mktemp -d)
echo "Serial output: $OUTPUT_DIR/serial.txt"

# Add ext2 disk if it exists
DISK_OPTS=""
if [ -f "$EXT2_DISK" ]; then
    echo "Disk image: $EXT2_DISK"
    if [ "$ARCH" = "arm64" ]; then
        DISK_OPTS="-device virtio-blk-device,drive=ext2disk -drive if=none,id=ext2disk,format=raw,readonly=on,file=$EXT2_DISK"
    else
        # x86_64 uses virtio-blk-pci for UEFI compatibility
        DISK_OPTS="-drive if=none,id=ext2disk,format=raw,readonly=on,file=$EXT2_DISK -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off"
    fi
else
    echo "Disk image: (none - shell commands will be limited)"
    if [ "$ARCH" = "arm64" ]; then
        DISK_OPTS="-device virtio-blk-device,drive=hd0 -drive if=none,id=hd0,format=raw,file=/dev/null"
    fi
fi

# Build display options based on architecture and headless mode
if [ "$ARCH" = "arm64" ]; then
    # ARM64: Always add GPU and keyboard devices (needed for VirtIO enumeration)
    # The -display option only controls whether a window appears
    if [ "$HEADLESS" = true ]; then
        DISPLAY_OPTS="-display none -device virtio-gpu-device -device virtio-keyboard-device"
    else
        DISPLAY_OPTS="-display cocoa -device virtio-gpu-device -device virtio-keyboard-device"
    fi
else
    # x86_64 uses virtio-vga
    if [ "$HEADLESS" = true ]; then
        DISPLAY_OPTS="-display none"
    else
        DISPLAY_OPTS="-device virtio-vga -vnc :0 -k en-us"
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
    # ARM64 QEMU invocation (native)
    qemu-system-aarch64 \
        -M virt -cpu cortex-a72 \
        -smp 4 \
        -m 512M \
        -kernel "$KERNEL" \
        $DISPLAY_OPTS \
        $DISK_OPTS \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0,hostfwd=tcp::2323-:2323 \
        -serial mon:stdio \
        -no-reboot \
        &
else
    # x86_64 QEMU invocation (UEFI boot, native)
    # Copy OVMF firmware to output dir (pflash needs writable files)
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

    # Build test binaries options if it exists
    TEST_BIN_OPTS=""
    if [ -f "$BREENIX_ROOT/target/test_binaries.img" ]; then
        TEST_BIN_OPTS="-drive if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off"
    fi

    qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive if=none,id=hd,format=raw,media=disk,readonly=on,file="$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        $TEST_BIN_OPTS \
        $DISK_OPTS \
        -machine pc,accel=tcg \
        -cpu qemu64 \
        -smp 4 \
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
