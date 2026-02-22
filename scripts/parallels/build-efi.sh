#!/bin/bash
# Build the Breenix Parallels UEFI loader and create a bootable disk image.
#
# Output: target/parallels/breenix-efi.img
#   - GPT disk with FAT32 ESP containing EFI/BOOT/BOOTAA64.EFI
#   - Optionally includes kernel ELF at /kernel-parallels
#
# Usage:
#   ./scripts/parallels/build-efi.sh           # Build loader only
#   ./scripts/parallels/build-efi.sh --kernel   # Build loader + kernel

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="$PROJECT_ROOT/target/parallels"
EFI_IMG="$OUTPUT_DIR/breenix-efi.img"
ESP_DIR="$OUTPUT_DIR/esp"
INCLUDE_KERNEL=false

for arg in "$@"; do
    case "$arg" in
        --kernel) INCLUDE_KERNEL=true ;;
        *) echo "Unknown argument: $arg"; exit 1 ;;
    esac
done

echo "=== Building Breenix Parallels UEFI Loader ==="

# Build the UEFI loader
cd "$PROJECT_ROOT"
cargo build --release --target aarch64-unknown-uefi -p parallels-loader

LOADER_EFI="$PROJECT_ROOT/target/aarch64-unknown-uefi/release/parallels-loader.efi"
if [ ! -f "$LOADER_EFI" ]; then
    echo "ERROR: UEFI loader not found at $LOADER_EFI"
    exit 1
fi
echo "Loader built: $LOADER_EFI ($(stat -f%z "$LOADER_EFI" 2>/dev/null || stat -c%s "$LOADER_EFI") bytes)"

# Optionally build the kernel
if [ "$INCLUDE_KERNEL" = true ]; then
    echo "=== Building Breenix ARM64 Kernel ==="
    cargo build --release --target aarch64-breenix.json \
        -Z build-std=core,alloc \
        -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64
    KERNEL_ELF="$PROJECT_ROOT/target/aarch64-breenix/release/kernel-aarch64"
    if [ ! -f "$KERNEL_ELF" ]; then
        echo "ERROR: Kernel ELF not found at $KERNEL_ELF"
        exit 1
    fi
    echo "Kernel built: $KERNEL_ELF ($(stat -f%z "$KERNEL_ELF" 2>/dev/null || stat -c%s "$KERNEL_ELF") bytes)"
fi

# Create ESP directory structure
echo "=== Creating EFI System Partition ==="
rm -rf "$ESP_DIR"
mkdir -p "$ESP_DIR/EFI/BOOT"
mkdir -p "$ESP_DIR/EFI/BREENIX"
cp "$LOADER_EFI" "$ESP_DIR/EFI/BOOT/BOOTAA64.EFI"

if [ "$INCLUDE_KERNEL" = true ] && [ -f "$KERNEL_ELF" ]; then
    cp "$KERNEL_ELF" "$ESP_DIR/EFI/BREENIX/KERNEL"
    echo "Kernel ELF copied to ESP at EFI/BREENIX/KERNEL"
fi

# Create a GPT disk image with FAT32 ESP
# Size: 64MB (enough for loader + kernel)
echo "=== Creating GPT Disk Image ==="
mkdir -p "$OUTPUT_DIR"

IMG_SIZE_MB=64
dd if=/dev/zero of="$EFI_IMG" bs=1m count=$IMG_SIZE_MB 2>/dev/null

# Use hdiutil on macOS to create FAT32 filesystem
# First create a FAT32 image of the ESP content
FAT_IMG="$OUTPUT_DIR/esp.fat32.img"
dd if=/dev/zero of="$FAT_IMG" bs=1m count=$((IMG_SIZE_MB - 1)) 2>/dev/null

# Format as FAT32 using newfs_msdos (macOS)
if command -v newfs_msdos &>/dev/null; then
    newfs_msdos -F 32 -S 512 "$FAT_IMG" 2>/dev/null
elif command -v mkfs.fat &>/dev/null; then
    mkfs.fat -F 32 "$FAT_IMG"
else
    echo "ERROR: No FAT32 formatter found (need newfs_msdos or mkfs.fat)"
    exit 1
fi

# Mount and copy files using mtools (if available) or hdiutil
if command -v mcopy &>/dev/null; then
    mmd -i "$FAT_IMG" ::EFI 2>/dev/null || true
    mmd -i "$FAT_IMG" ::EFI/BOOT 2>/dev/null || true
    mmd -i "$FAT_IMG" ::EFI/BREENIX 2>/dev/null || true
    mcopy -i "$FAT_IMG" "$ESP_DIR/EFI/BOOT/BOOTAA64.EFI" ::EFI/BOOT/BOOTAA64.EFI
    if [ "$INCLUDE_KERNEL" = true ] && [ -f "$ESP_DIR/EFI/BREENIX/KERNEL" ]; then
        mcopy -i "$FAT_IMG" "$ESP_DIR/EFI/BREENIX/KERNEL" ::EFI/BREENIX/KERNEL
    fi
    echo "Files copied to FAT32 image via mtools"
elif command -v hdiutil &>/dev/null; then
    # macOS: attach the raw image and copy files
    MOUNT_POINT=$(mktemp -d)
    # Convert raw to UDIF for mounting
    hdiutil attach -imagekey diskimage-class=CRawDiskImage -nomount "$FAT_IMG" 2>/dev/null | \
        while read -r dev rest; do
            if [[ "$dev" == /dev/disk* ]]; then
                mount -t msdos "$dev" "$MOUNT_POINT" 2>/dev/null && break
            fi
        done

    if mountpoint -q "$MOUNT_POINT" 2>/dev/null || mount | grep -q "$MOUNT_POINT"; then
        mkdir -p "$MOUNT_POINT/EFI/BOOT"
        mkdir -p "$MOUNT_POINT/EFI/BREENIX"
        cp "$ESP_DIR/EFI/BOOT/BOOTAA64.EFI" "$MOUNT_POINT/EFI/BOOT/"
        if [ "$INCLUDE_KERNEL" = true ] && [ -f "$ESP_DIR/EFI/BREENIX/KERNEL" ]; then
            cp "$ESP_DIR/EFI/BREENIX/KERNEL" "$MOUNT_POINT/EFI/BREENIX/"
        fi
        sync
        hdiutil detach "$MOUNT_POINT" 2>/dev/null || umount "$MOUNT_POINT" 2>/dev/null
        echo "Files copied to FAT32 image via hdiutil"
    else
        echo "WARNING: Could not mount FAT32 image. Falling back to raw copy."
        echo "Install mtools for reliable image creation: brew install mtools"
        rm -f "$FAT_IMG"
        # Fall back to just providing the ESP directory
        echo "ESP directory ready at: $ESP_DIR"
    fi
    rmdir "$MOUNT_POINT" 2>/dev/null || true
else
    echo "WARNING: No tool to populate FAT32 image."
    echo "Install mtools: brew install mtools"
fi

# If we have a populated FAT image, embed it in GPT
if [ -f "$FAT_IMG" ]; then
    # Create GPT wrapper using dd
    # GPT header is 34 sectors at start, 33 at end
    # For simplicity, just use the FAT image as-is for now
    # Parallels can boot from a raw FAT32 image as EFI disk
    cp "$FAT_IMG" "$EFI_IMG"
    rm -f "$FAT_IMG"
fi

echo ""
echo "=== Build Complete ==="
echo "EFI disk image: $EFI_IMG"
echo "ESP directory:  $ESP_DIR"
echo ""
echo "To test with QEMU:"
echo "  qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M \\"
echo "    -drive if=pflash,format=raw,file=QEMU_EFI.fd,readonly=on \\"
echo "    -drive format=raw,file=$EFI_IMG"
echo ""
echo "To deploy to Parallels:"
echo "  ./scripts/parallels/deploy-to-vm.sh"
