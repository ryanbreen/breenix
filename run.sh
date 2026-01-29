#!/bin/bash
# Unified Breenix run script
#
# Usage: ./run.sh [OPTIONS]
#
# Options:
#   --x86        Run x86_64 kernel (default: ARM64)
#   --headless   Disable graphics (default: graphics enabled)
#   --debug      Build debug instead of release
#   --trace      Enable QEMU VirtIO tracing for debugging
#   --help       Show this help message
#
# Examples:
#   ./run.sh                      # ARM64 with graphics (default)
#   ./run.sh --headless           # ARM64 headless (serial only)
#   ./run.sh --x86                # x86_64 with VNC graphics (Docker)
#   ./run.sh --x86 --headless     # x86_64 headless (Docker)
#   ./run.sh --trace              # ARM64 with VirtIO debug tracing

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$SCRIPT_DIR"

# Defaults
ARCH="arm64"
GRAPHICS=1
BUILD_TYPE="release"
TRACE=0

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --x86)
            ARCH="x86"
            shift
            ;;
        --headless)
            GRAPHICS=0
            shift
            ;;
        --debug)
            BUILD_TYPE="debug"
            shift
            ;;
        --trace)
            TRACE=1
            shift
            ;;
        --help|-h)
            head -20 "$0" | tail -18
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# =============================================================================
# ARM64 Mode (native QEMU)
# =============================================================================
run_arm64() {
    echo ""
    echo "========================================="
    echo "  Breenix ARM64 Kernel"
    echo "========================================="

    # Setup logging
    LOG_DIR="$BREENIX_ROOT/target/logs"
    mkdir -p "$LOG_DIR"
    LOG_FILE="$LOG_DIR/arm64-$(date +%Y%m%d-%H%M%S).log"
    echo "Log file: $LOG_FILE"

    # Build kernel
    if [ "$BUILD_TYPE" = "debug" ]; then
        KERNEL="$BREENIX_ROOT/target/aarch64-breenix/debug/kernel-aarch64"
        echo "Building ARM64 kernel (debug)..."
        cargo build --target aarch64-breenix.json -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    else
        KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
        echo "Building ARM64 kernel (release)..."
        cargo build --release --target aarch64-breenix.json -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    fi

    # ext2 disk
    EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
    DISK_OPTS=""
    if [ -f "$EXT2_DISK" ]; then
        echo "Disk: $EXT2_DISK"
        DISK_OPTS="-device virtio-blk-device,drive=ext2disk \
            -blockdev driver=file,node-name=ext2file,filename=$EXT2_DISK \
            -blockdev driver=raw,node-name=ext2disk,file=ext2file"
    else
        echo "Warning: No ext2 disk - running without userspace"
    fi

    # Network
    NET_OPTS="-device virtio-net-device,netdev=net0 \
        -netdev user,id=net0,net=10.0.2.0/24,dhcpstart=10.0.2.15"

    # Debug/trace options
    DEBUG_OPTS=""
    if [ "$TRACE" = "1" ]; then
        echo "VirtIO tracing enabled"
        DEBUG_OPTS="-trace virtio_input_* -trace virtio_queue_*"
    fi

    # Graphics options
    GRAPHICS_OPTS=""
    if [ "$GRAPHICS" = "1" ]; then
        echo "Mode: Graphics (VirtIO GPU + Keyboard)"
        GRAPHICS_OPTS="-device virtio-gpu-device -device virtio-keyboard-device"
        case "$(uname)" in
            Darwin)
                DISPLAY_OPTS="-display cocoa,show-cursor=on -serial mon:stdio"
                ;;
            *)
                DISPLAY_OPTS="-display sdl -serial mon:stdio"
                ;;
        esac
        echo ""
        echo "Keyboard input:"
        echo "  - Click QEMU window and type for VirtIO keyboard"
        echo "  - Type in terminal for serial UART input"
    else
        echo "Mode: Headless (serial only)"
        DISPLAY_OPTS="-nographic"
        echo ""
        echo "Keyboard input: Type in this terminal"
    fi

    echo ""
    echo "Press Ctrl-A X to exit QEMU"
    echo ""

    # Run QEMU with output tee'd to log file
    # Use script command to capture terminal output including raw serial
    echo "Starting QEMU (logging to $LOG_FILE)..."

    # For headless mode, tee works well
    # For graphics mode, we use script to capture all output
    if [ "$GRAPHICS" = "1" ]; then
        # With graphics, serial goes to terminal, use script to capture
        script -q "$LOG_FILE" qemu-system-aarch64 \
            -M virt \
            -cpu cortex-a72 \
            -m 512M \
            $DISPLAY_OPTS \
            $GRAPHICS_OPTS \
            -kernel "$KERNEL" \
            $DISK_OPTS \
            $NET_OPTS \
            $DEBUG_OPTS
    else
        # Headless: tee output to both terminal and log
        qemu-system-aarch64 \
            -M virt \
            -cpu cortex-a72 \
            -m 512M \
            $DISPLAY_OPTS \
            $GRAPHICS_OPTS \
            -kernel "$KERNEL" \
            $DISK_OPTS \
            $NET_OPTS \
            $DEBUG_OPTS 2>&1 | tee "$LOG_FILE"
    fi

    echo ""
    echo "Log saved to: $LOG_FILE"
}

# =============================================================================
# x86_64 Mode (Docker + QEMU)
# =============================================================================
run_x86() {
    echo ""
    echo "========================================="
    echo "  Breenix x86_64 Kernel (Docker)"
    echo "========================================="

    # Build Docker image if needed
    IMAGE_NAME="breenix-qemu"
    if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
        echo "Building Docker image..."
        docker build -t "$IMAGE_NAME" "$BREENIX_ROOT/docker/qemu"
    fi

    # Kill any existing containers
    EXISTING=$(docker ps -q --filter ancestor="$IMAGE_NAME" 2>/dev/null)
    if [ -n "$EXISTING" ]; then
        echo "Stopping existing containers..."
        docker kill $EXISTING 2>/dev/null || true
    fi

    # Build kernel
    echo "Building x86_64 kernel ($BUILD_TYPE)..."
    if [ "$BUILD_TYPE" = "debug" ]; then
        cargo build --features interactive --bin qemu-uefi
    else
        cargo build --release --features interactive --bin qemu-uefi
    fi

    # Find UEFI image
    UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/$BUILD_TYPE/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
    if [ -z "$UEFI_IMG" ]; then
        echo "Error: UEFI image not found"
        exit 1
    fi
    echo "UEFI image: $UEFI_IMG"

    # Create output directory
    OUTPUT_DIR=$(mktemp -d)
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"
    touch "$OUTPUT_DIR/serial_user.txt"
    touch "$OUTPUT_DIR/serial_kernel.txt"

    # ext2 disk
    EXT2_DISK="$BREENIX_ROOT/target/ext2.img"
    EXT2_MOUNT=""
    EXT2_DEV=""
    if [ -f "$EXT2_DISK" ]; then
        echo "Disk: $EXT2_DISK"
        EXT2_MOUNT="-v $EXT2_DISK:/breenix/ext2.img:ro"
        EXT2_DEV="-drive if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img \
            -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off"
    fi

    # Test binaries disk
    TEST_DISK="$BREENIX_ROOT/target/test_binaries.img"
    TEST_MOUNT=""
    TEST_DEV=""
    if [ -f "$TEST_DISK" ]; then
        TEST_MOUNT="-v $TEST_DISK:/breenix/test_binaries.img:ro"
        TEST_DEV="-drive if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img \
            -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off"
    fi

    if [ "$GRAPHICS" = "1" ]; then
        echo "Mode: Graphics (VNC on localhost:5900)"
        echo ""
        echo "Starting QEMU with VNC display..."
        echo "Serial output: $OUTPUT_DIR"
        echo ""

        # Run with VNC
        docker run --rm \
            -p 5900:5900 \
            -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
            $TEST_MOUNT \
            $EXT2_MOUNT \
            -v "$OUTPUT_DIR:/output" \
            "$IMAGE_NAME" \
            qemu-system-x86_64 \
                -pflash /output/OVMF_CODE.fd \
                -pflash /output/OVMF_VARS.fd \
                -drive if=none,id=hd,format=raw,media=disk,readonly=on,file=/breenix/breenix-uefi.img \
                -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
                $TEST_DEV \
                $EXT2_DEV \
                -machine pc,accel=tcg \
                -cpu qemu64 \
                -smp 1 \
                -m 512 \
                -device virtio-vga \
                -vnc :0 \
                -k en-us \
                -no-reboot \
                -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
                -netdev user,id=net0 \
                -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
                -serial file:/output/serial_user.txt \
                -serial file:/output/serial_kernel.txt \
            &

        DOCKER_PID=$!

        # Wait for VNC and auto-open
        echo "Waiting for VNC server..."
        sleep 3

        if [ -d "/Applications/TigerVNC Viewer 1.15.0.app" ]; then
            echo "Opening TigerVNC..."
            open "/Applications/TigerVNC Viewer 1.15.0.app" --args localhost:5900
        else
            echo "Connect with VNC client to localhost:5900"
        fi

        # Follow serial output
        echo ""
        echo "Press Ctrl+C to stop"
        echo ""
        tail -f "$OUTPUT_DIR/serial_user.txt" &
        TAIL_PID=$!

        trap "kill $TAIL_PID 2>/dev/null; docker kill \$(docker ps -q --filter ancestor=$IMAGE_NAME) 2>/dev/null" EXIT
        wait $DOCKER_PID 2>/dev/null

        echo ""
        echo "Serial output saved to: $OUTPUT_DIR"
    else
        echo "Mode: Headless (serial to stdout)"
        echo ""
        echo "Press Ctrl+C to stop"
        echo ""

        # Run headless with serial to stdout
        docker run --rm -it \
            -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
            $TEST_MOUNT \
            $EXT2_MOUNT \
            -v "$OUTPUT_DIR:/output" \
            "$IMAGE_NAME" \
            qemu-system-x86_64 \
                -pflash /output/OVMF_CODE.fd \
                -pflash /output/OVMF_VARS.fd \
                -drive if=none,id=hd,format=raw,media=disk,readonly=on,file=/breenix/breenix-uefi.img \
                -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
                $TEST_DEV \
                $EXT2_DEV \
                -machine pc,accel=tcg \
                -cpu qemu64 \
                -smp 1 \
                -m 512 \
                -nographic \
                -no-reboot \
                -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
                -netdev user,id=net0 \
                -device e1000,netdev=net0,mac=52:54:00:12:34:56
    fi
}

# =============================================================================
# Main
# =============================================================================
if [ "$ARCH" = "x86" ]; then
    run_x86
else
    run_arm64
fi
