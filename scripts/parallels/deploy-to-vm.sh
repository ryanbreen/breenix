#!/bin/bash
# Deploy Breenix to the Parallels Desktop VM.
#
# This script packages the already-built kernel + UEFI loader into a
# Parallels .hdd disk image, reconfigures the VM, and optionally starts it.
# It does NOT build anything — run build-efi.sh --kernel first.
#
# Usage:
#   ./scripts/parallels/deploy-to-vm.sh           # Deploy only (don't start)
#   ./scripts/parallels/deploy-to-vm.sh --boot    # Deploy and start VM
#
# Typical workflow:
#   touch kernel/src/drivers/usb/xhci.rs
#   scripts/parallels/build-efi.sh --kernel
#   scripts/parallels/deploy-to-vm.sh --boot
#   sleep 50
#   cat /tmp/breenix-parallels-serial.log | grep "BUILD_ID"

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

PARALLELS_DIR="$PROJECT_ROOT/target/parallels"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
HDD_DIR="$PARALLELS_DIR/breenix-efi.hdd"
EXT2_HDD_DIR="$PARALLELS_DIR/breenix-ext2.hdd"
EXT2_DISK="$PROJECT_ROOT/target/ext2-aarch64.img"
PARALLELS_VM="breenix-dev"
BOOT=false

for arg in "$@"; do
    case "$arg" in
        --boot) BOOT=true ;;
        --vm) PARALLELS_VM="$2"; shift ;;
        *) echo "Unknown argument: $arg"; exit 1 ;;
    esac
done

# Check for required tools
if ! command -v prlctl &>/dev/null; then
    echo "ERROR: prlctl not found. Is Parallels Desktop installed?"
    exit 1
fi
if ! command -v prl_disk_tool &>/dev/null; then
    echo "ERROR: prl_disk_tool not found. Is Parallels Desktop installed?"
    exit 1
fi

# Verify built artifacts exist
LOADER_EFI="$PROJECT_ROOT/target/aarch64-unknown-uefi/release/parallels-loader.efi"
KERNEL_ELF="$PROJECT_ROOT/target/aarch64-breenix/release/kernel-aarch64"

if [ ! -f "$LOADER_EFI" ]; then
    echo "ERROR: UEFI loader not found at $LOADER_EFI"
    echo "Run: cargo build --release --target aarch64-unknown-uefi -p parallels-loader"
    exit 1
fi
if [ ! -f "$KERNEL_ELF" ]; then
    echo "ERROR: Kernel not found at $KERNEL_ELF"
    echo "Run: scripts/parallels/build-efi.sh --kernel"
    exit 1
fi

LOADER_SIZE=$(stat -f%z "$LOADER_EFI" 2>/dev/null || stat -c%s "$LOADER_EFI")
KERNEL_SIZE=$(stat -f%z "$KERNEL_ELF" 2>/dev/null || stat -c%s "$KERNEL_ELF")
KERNEL_MTIME=$(stat -f "%Sm" "$KERNEL_ELF" 2>/dev/null || stat -c "%y" "$KERNEL_ELF")

echo "=== Packaging Parallels EFI disk ==="
echo "  Loader: $LOADER_SIZE bytes"
echo "  Kernel: $KERNEL_SIZE bytes (built $KERNEL_MTIME)"

# Check if kernel binary contains a BUILD_ID (diagnostic)
if command -v strings &>/dev/null; then
    BUILD_ID_IN_BINARY=$(strings "$KERNEL_ELF" | grep "BUILD_ID:" | head -1 || true)
    if [ -n "$BUILD_ID_IN_BINARY" ]; then
        echo "  $BUILD_ID_IN_BINARY"
    else
        echo "  WARNING: No BUILD_ID found in kernel binary (stale build?)"
    fi
fi

mkdir -p "$PARALLELS_DIR"

# Create GPT+FAT32 disk image using hdiutil (native macOS)
DMG_PATH="$PARALLELS_DIR/efi-temp.dmg"
rm -f "$DMG_PATH"
hdiutil create -size 64m -fs FAT32 -volname BREENIX -layout GPTSPUD "$DMG_PATH" >/dev/null 2>&1

VOLUME=$(hdiutil attach "$DMG_PATH" 2>/dev/null | grep -o '/Volumes/[^ ]*' | head -1)
if [ -z "$VOLUME" ] || [ ! -d "$VOLUME" ]; then
    echo "ERROR: Failed to mount FAT32 disk image"
    rm -f "$DMG_PATH"
    exit 1
fi

mkdir -p "$VOLUME/EFI/BOOT"
mkdir -p "$VOLUME/EFI/BREENIX"
cp "$LOADER_EFI" "$VOLUME/EFI/BOOT/BOOTAA64.EFI"
cp "$KERNEL_ELF" "$VOLUME/EFI/BREENIX/KERNEL"
hdiutil detach "$VOLUME" >/dev/null 2>&1

# Convert DMG to raw disk image
RAW_IMG="$PARALLELS_DIR/efi-raw.img"
rm -f "$RAW_IMG" "${RAW_IMG}.cdr"
hdiutil convert "$DMG_PATH" -format UDTO -o "$RAW_IMG" >/dev/null 2>&1
mv "${RAW_IMG}.cdr" "$RAW_IMG"
rm -f "$DMG_PATH"

# Patch GPT partition type from "Microsoft Basic Data" to "EFI System Partition"
# so UEFI firmware recognizes the ESP and auto-boots BOOTAA64.EFI
python3 "$PROJECT_ROOT/scripts/parallels/patch-gpt-esp.py" "$RAW_IMG"

# Wrap EFI disk in Parallels .hdd format
rm -rf "$HDD_DIR"
prl_disk_tool create --hdd "$HDD_DIR" --size 64M >/dev/null 2>&1
HDS_FILE=$(find "$HDD_DIR" -name "*.hds" | head -1)
if [ -z "$HDS_FILE" ]; then
    echo "ERROR: No .hds file found in $HDD_DIR"
    rm -f "$RAW_IMG"
    exit 1
fi
cp "$RAW_IMG" "$HDS_FILE"
rm -f "$RAW_IMG"
echo "  EFI disk: $HDD_DIR"

# Wrap ext2 data disk if available
if [ -f "$EXT2_DISK" ]; then
    EXT2_SIZE_MB=$(( ($(stat -f%z "$EXT2_DISK" 2>/dev/null || stat -c%s "$EXT2_DISK") + 1048575) / 1048576 ))
    rm -rf "$EXT2_HDD_DIR"
    prl_disk_tool create --hdd "$EXT2_HDD_DIR" --size "${EXT2_SIZE_MB}M" >/dev/null 2>&1
    EXT2_HDS=$(find "$EXT2_HDD_DIR" -name "*.hds" | head -1)
    if [ -n "$EXT2_HDS" ]; then
        cp "$EXT2_DISK" "$EXT2_HDS"
        echo "  ext2 disk: $EXT2_HDD_DIR (${EXT2_SIZE_MB}MB)"
    fi
fi

echo ""
echo "=== Configuring Parallels VM '$PARALLELS_VM' ==="

# Create VM if it doesn't exist
if ! prlctl list --all 2>/dev/null | grep -q "$PARALLELS_VM"; then
    echo "Creating VM '$PARALLELS_VM'..."
    prlctl create "$PARALLELS_VM" --ostype linux --distribution linux --no-hdd
    prlctl set "$PARALLELS_VM" --memsize 2048
    prlctl set "$PARALLELS_VM" --cpus 4
fi

# Force-stop the VM
echo "Stopping VM (force kill)..."
prlctl stop "$PARALLELS_VM" --kill 2>/dev/null || true

# Poll until confirmed stopped
for i in $(seq 1 20); do
    VM_STATUS=$(prlctl status "$PARALLELS_VM" 2>/dev/null | awk '{print $NF}')
    if [ "$VM_STATUS" = "stopped" ]; then
        echo "VM is stopped."
        break
    fi
    if [ "$i" -eq 20 ]; then
        echo "WARNING: VM did not stop after 20s. Proceeding anyway."
        echo "If stuck, run: sudo pkill -9 -f prl_disp_service"
    fi
    sleep 1
done

# Configure VM: EFI boot, remove all SATA devices, attach our disks
prlctl set "$PARALLELS_VM" --efi-boot on 2>/dev/null || true

for dev in hdd0 hdd1 hdd2 hdd3 cdrom0 cdrom1; do
    prlctl set "$PARALLELS_VM" --device-del "$dev" 2>/dev/null || true
done

prlctl set "$PARALLELS_VM" --device-add hdd --image "$HDD_DIR" --type plain --position 0
if [ -d "$EXT2_HDD_DIR" ]; then
    prlctl set "$PARALLELS_VM" --device-add hdd --image "$EXT2_HDD_DIR" --type plain --position 1
    echo "  hdd0: EFI boot disk (FAT32) at sata:0"
    echo "  hdd1: ext2 data disk at sata:1"
else
    echo "  hdd0: EFI boot disk (FAT32) at sata:0"
fi

prlctl set "$PARALLELS_VM" --device-bootorder "hdd0" 2>/dev/null || true

# Configure serial port output to file
prlctl set "$PARALLELS_VM" --device-del serial0 2>/dev/null || true
prlctl set "$PARALLELS_VM" --device-add serial --output "$SERIAL_LOG" 2>/dev/null || true
prlctl set "$PARALLELS_VM" --device-set serial0 --connect 2>/dev/null || true

# Delete NVRAM to ensure fresh UEFI boot state
VM_DIR="$HOME/Parallels/${PARALLELS_VM}.pvm"
if [ -f "$VM_DIR/NVRAM.dat" ]; then
    rm -f "$VM_DIR/NVRAM.dat"
    echo "  NVRAM deleted (fresh UEFI state)"
fi

# Truncate serial log
> "$SERIAL_LOG"
echo "  Serial log truncated: $SERIAL_LOG"

if [ "$BOOT" = true ]; then
    echo ""
    echo "=== Starting VM ==="
    prlctl start "$PARALLELS_VM"
    echo "VM started."
    echo ""
    echo "Monitor: tail -f $SERIAL_LOG"
    echo "Stop:    prlctl stop $PARALLELS_VM --kill"
fi

echo ""
echo "Deploy complete."
