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
#   ./run.sh --no-build        # Skip all builds, use existing artifacts
#   ./run.sh --resolution 1920x1080  # Custom resolution
#   ./run.sh --btrt            # ARM64 BTRT structured boot test
#   ./run.sh --btrt --x86      # x86_64 BTRT structured boot test
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
NO_BUILD=false
BTRT=false
DEBUG=false
RESOLUTION=""

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
        --no-build)
            NO_BUILD=true
            shift
            ;;
        --btrt)
            BTRT=true
            shift
            ;;
        --debug)
            DEBUG=true
            shift
            ;;
        --resolution)
            RESOLUTION="$2"
            shift 2
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
            echo "  --no-build                 Skip all builds, use existing artifacts"
            echo "  --x86, --x86_64, --amd64   Run x86_64 kernel (default: ARM64)"
            echo "  --arm64, --aarch64         Run ARM64 kernel (default)"
            echo "  --headless, --serial       Run without display (serial only)"
            echo "  --graphics, --vnc          Run with VNC display (default)"
            echo "  --btrt                     Run BTRT structured boot test"
            echo "  --debug                    Enable GDB stub (port 1234) for debugging"
            echo "  --resolution WxH           Set display resolution (e.g. 1920x1080)"
            echo "                             Default: auto-detect from screen"
            echo "  -h, --help                 Show this help"
            echo ""
            echo "Debugging:"
            echo "  QMP socket always at: /tmp/breenix-qmp.sock"
            echo "  GDB (--debug):  target remote :1234"
            echo "  Forensics:      scripts/forensic-capture.sh"
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

# BTRT mode: delegate to xtask and exit
if [ "$BTRT" = true ]; then
    if [ "$ARCH" = "arm64" ]; then
        BTRT_ARCH="arm64"
    else
        BTRT_ARCH="x86_64"
    fi
    echo ""
    echo "========================================="
    echo "Breenix BTRT Boot Test ($BTRT_ARCH)"
    echo "========================================="
    echo ""
    exec cargo run -p xtask -- boot-test-btrt --arch "$BTRT_ARCH"
fi

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

# Resolve display resolution
if [ -z "$RESOLUTION" ] && [ "$HEADLESS" = false ]; then
    # Auto-detect: use macOS screen size minus menu bar (37px)
    if [ "$(uname)" = "Darwin" ]; then
        SCREEN_INFO=$(system_profiler SPDisplaysDataType 2>/dev/null | grep "Resolution:" | head -1)
        if [[ "$SCREEN_INFO" =~ ([0-9]+)\ x\ ([0-9]+) ]]; then
            NATIVE_W="${BASH_REMATCH[1]}"
            NATIVE_H="${BASH_REMATCH[2]}"
            # Retina displays: divide by 2 for effective resolution
            if echo "$SCREEN_INFO" | grep -q "Retina"; then
                NATIVE_W=$((NATIVE_W / 2))
                NATIVE_H=$((NATIVE_H / 2))
            fi
            # Subtract menu bar height
            RES_W="$NATIVE_W"
            RES_H=$((NATIVE_H - 37))
            RESOLUTION="${RES_W}x${RES_H}"
        fi
    fi
fi
# Fallback default
if [ -z "$RESOLUTION" ]; then
    RESOLUTION="1280x800"
fi

# Parse WxH
RES_W="${RESOLUTION%%x*}"
RES_H="${RESOLUTION##*x}"

echo ""
echo "========================================="
echo "Breenix Interactive Mode"
echo "========================================="
echo ""
echo "Architecture: $ARCH"
echo "Resolution: ${RES_W}x${RES_H}"
echo "Mode: $([ "$HEADLESS" = true ] && echo "headless (serial only)" || echo "graphics")"

if [ "$NO_BUILD" = true ]; then
    echo "Skipping all builds (--no-build)"
elif [ "$CLEAN" = true ]; then
    # --clean: full rebuild of userspace, ext2 disk, and kernel
    echo ""
    echo "Clean build: rebuilding everything..."
    echo ""

    if [ "$ARCH" = "arm64" ]; then
        echo "[1/3] Building userspace binaries (aarch64)..."
        "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64

        echo ""
        echo "[2/3] Creating ext2 disk image..."
        "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64

        echo ""
        echo "[3/3] Building kernel..."
    else
        echo "[1/3] Building userspace binaries (x86_64)..."
        "$BREENIX_ROOT/userspace/programs/build.sh"

        echo ""
        echo "[2/3] Creating ext2 disk image..."
        "$BREENIX_ROOT/scripts/create_ext2_disk.sh"

        echo ""
        echo "[3/3] Building kernel..."
    fi
    eval $BUILD_CMD
    echo ""
else
    # Always rebuild to ensure correct features (boot_tests, etc.)
    # Cargo is incremental — this is fast if nothing changed
    echo "Building kernel..."
    eval $BUILD_CMD
fi

if [ ! -f "$KERNEL" ]; then
    echo "Error: Kernel not found after build"
    exit 1
fi

echo "Kernel: $KERNEL"

# Create output directory
OUTPUT_DIR=$(mktemp -d)
echo "Serial output: $OUTPUT_DIR/serial.txt"

# Add ext2 disk if it exists (writable copy to allow filesystem writes)
# Use a known path so extraction scripts can find the session disk
DISK_OPTS=""
EXT2_SESSION="$BREENIX_ROOT/target/ext2-session.img"
if [ -f "$EXT2_DISK" ]; then
    echo "Disk image: $EXT2_DISK"
    EXT2_WRITABLE="$EXT2_SESSION"
    if [ "$CLEAN" = true ] || [ ! -f "$EXT2_WRITABLE" ]; then
        echo "Creating fresh session disk from $EXT2_DISK"
        cp "$EXT2_DISK" "$EXT2_WRITABLE"
    else
        echo "Reusing existing session disk (use --clean to reset)"
    fi
    if [ "$ARCH" = "arm64" ]; then
        DISK_OPTS="-device virtio-blk-device,drive=ext2disk -drive if=none,id=ext2disk,format=raw,file=$EXT2_WRITABLE"
    else
        # x86_64 uses virtio-blk-pci for UEFI compatibility
        DISK_OPTS="-drive if=none,id=ext2disk,format=raw,file=$EXT2_WRITABLE -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off"
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
    # Resolution is configured by the kernel's virtio-gpu driver via fw_cfg
    if [ "$HEADLESS" = true ]; then
        DISPLAY_OPTS="-display none -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device"
    else
        DISPLAY_OPTS="-display cocoa -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device"
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

# Audio options (VirtIO sound device)
AUDIO_OPTS="-audiodev coreaudio,id=audio0"
if [ "$ARCH" = "arm64" ]; then
    AUDIO_OPTS="$AUDIO_OPTS -device virtio-sound-device,audiodev=audio0"
else
    AUDIO_OPTS="$AUDIO_OPTS -device virtio-sound-pci,audiodev=audio0"
fi

# QMP socket for programmatic VM control (always enabled)
QMP_SOCK="/tmp/breenix-qmp.sock"
rm -f "$QMP_SOCK"
QMP_OPTS="-qmp unix:${QMP_SOCK},server,nowait"

# GDB stub (--debug flag)
GDB_OPTS=""
if [ "$DEBUG" = true ]; then
    GDB_OPTS="-s"
    echo "GDB stub: target remote :1234"
fi

# Pass resolution to kernel via fw_cfg
FW_CFG_OPTS="-fw_cfg name=opt/breenix/resolution,string=${RES_W}x${RES_H}"

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
        -netdev user,id=net0,hostfwd=tcp::2323-:2323,hostfwd=tcp::7890-:7890 \
        $AUDIO_OPTS \
        -monitor tcp:127.0.0.1:4444,server,nowait \
        $QMP_OPTS \
        $GDB_OPTS \
        $FW_CFG_OPTS \
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
        $AUDIO_OPTS \
        -monitor tcp:127.0.0.1:4444,server,nowait \
        $QMP_OPTS \
        $GDB_OPTS \
        -serial mon:stdio \
        &
fi

QEMU_PID=$!

echo "Paste:     echo 'code' | ./scripts/paste.sh"
echo "Monitor:   tcp://127.0.0.1:4444"
echo "QMP:       $QMP_SOCK"
if [ "$DEBUG" = true ]; then
    echo "GDB:       target remote :1234"
fi
echo "Forensics: scripts/forensic-capture.sh"

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
if [ -f "$EXT2_SESSION" ]; then
    echo "Session disk: $EXT2_SESSION"
    echo "Extract saved files: ./scripts/extract-saves.sh"
fi
