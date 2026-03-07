#!/bin/bash
# Quick build → deploy → boot → screenshot cycle for Parallels testing.
# Usage: ./quick-test.sh [vm-name] [wait-seconds]
# Defaults: vm-name=breenix-test, wait=35s

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

VM_NAME="${1:-breenix-test}"
WAIT_SECS="${2:-35}"
PARALLELS_DIR="target/parallels"
HDD_DIR="$PARALLELS_DIR/breenix-efi.hdd"
EXT2_HDD_DIR="$PARALLELS_DIR/breenix-ext2.hdd"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
SCREENSHOT="/tmp/breenix-screenshot.png"

echo "=== Building kernel ==="
touch kernel/src/drivers/virtio/gpu_pci.rs
scripts/parallels/build-efi.sh --kernel 2>&1 | grep -E "warning|error|Build Complete|bytes\)" | head -10

echo "=== Building userspace + ext2 ==="
./userspace/programs/build.sh --arch aarch64 2>&1 | tail -1
./scripts/create_ext2_disk.sh --arch aarch64 >/dev/null 2>&1
# Recreate ext2 Parallels HDD from fresh ext2 image
EXT2_IMG="target/ext2-aarch64.img"
if [ -f "$EXT2_IMG" ]; then
    rm -rf "$EXT2_HDD_DIR"
    prl_disk_tool create --hdd "$EXT2_HDD_DIR" --size 64M >/dev/null 2>&1
    EXT2_HDS=$(find "$EXT2_HDD_DIR" -name "*.hds" | head -1)
    cp "$EXT2_IMG" "$EXT2_HDS"
    echo "ext2 HDD updated"
fi

LOADER_EFI="target/aarch64-unknown-uefi/release/parallels-loader.efi"
KERNEL_ELF="target/aarch64-breenix/release/kernel-aarch64"

echo "=== Creating EFI disk ==="
DMG_PATH="$PARALLELS_DIR/efi-temp.dmg"
rm -f "$DMG_PATH"
hdiutil create -size 64m -fs FAT32 -volname BREENIX -layout GPTSPUD "$DMG_PATH" >/dev/null 2>&1
VOLUME=$(hdiutil attach "$DMG_PATH" 2>/dev/null | grep -o '/Volumes/[^ ]*' | head -1)
mkdir -p "$VOLUME/EFI/BOOT" "$VOLUME/EFI/BREENIX"
cp "$LOADER_EFI" "$VOLUME/EFI/BOOT/BOOTAA64.EFI"
cp "$KERNEL_ELF" "$VOLUME/EFI/BREENIX/KERNEL"
hdiutil detach "$VOLUME" >/dev/null 2>&1
RAW_IMG="$PARALLELS_DIR/efi-raw.img"
rm -f "$RAW_IMG" "${RAW_IMG}.cdr"
hdiutil convert "$DMG_PATH" -format UDTO -o "$RAW_IMG" >/dev/null 2>&1
mv "${RAW_IMG}.cdr" "$RAW_IMG"; rm -f "$DMG_PATH"
python3 scripts/parallels/patch-gpt-esp.py "$RAW_IMG"
rm -rf "$HDD_DIR"
prl_disk_tool create --hdd "$HDD_DIR" --size 64M >/dev/null 2>&1
HDS_FILE=$(find "$HDD_DIR" -name "*.hds" | head -1)
cp "$RAW_IMG" "$HDS_FILE"; rm -f "$RAW_IMG"

echo "=== Creating VM: $VM_NAME ==="
for OLD_VM in $(prlctl list --all 2>/dev/null | grep 'breenix-' | awk '{print $NF}'); do
    prlctl stop "$OLD_VM" --kill >/dev/null 2>&1 || true
    prlctl delete "$OLD_VM" >/dev/null 2>&1 || true
done

prlctl create "$VM_NAME" --ostype linux --distribution linux --no-hdd >/dev/null 2>&1
prlctl set "$VM_NAME" --memsize 2048 >/dev/null 2>&1
prlctl set "$VM_NAME" --cpus 4 >/dev/null 2>&1
prlctl set "$VM_NAME" --efi-boot on >/dev/null 2>&1 || true
prlctl set "$VM_NAME" --3d-accelerate highest >/dev/null 2>&1 || true
prlctl set "$VM_NAME" --videosize 256 >/dev/null 2>&1 || true
prlctl set "$VM_NAME" --high-resolution off >/dev/null 2>&1 || true
prlctl set "$VM_NAME" --high-resolution-in-guest off >/dev/null 2>&1 || true
prlctl set "$VM_NAME" --native-scaling-in-guest off >/dev/null 2>&1 || true
for dev in hdd0 hdd1 hdd2 cdrom0 cdrom1 serial0 serial1; do
    prlctl set "$VM_NAME" --device-del "$dev" >/dev/null 2>&1 || true
done
prlctl set "$VM_NAME" --device-add hdd --image "$HDD_DIR" --type plain --position 0 >/dev/null 2>&1
[ -d "$EXT2_HDD_DIR" ] && prlctl set "$VM_NAME" --device-add hdd --image "$EXT2_HDD_DIR" --type plain --position 1 >/dev/null 2>&1
prlctl set "$VM_NAME" --device-bootorder "hdd0" >/dev/null 2>&1 || true
prlctl set "$VM_NAME" --device-add serial --output "$SERIAL_LOG" >/dev/null 2>&1 || true
prlctl set "$VM_NAME" --device-set serial0 --connect >/dev/null 2>&1 || true
> "$SERIAL_LOG"
rm -f ~/Parallels/"${VM_NAME}.pvm"/NVRAM.dat

echo "=== Starting VM ==="
timeout 10 prlctl start "$VM_NAME" 2>&1 || echo "(prlctl timed out)"
echo "Waiting ${WAIT_SECS}s for boot..."
sleep "$WAIT_SECS"

echo "=== Screenshot ==="
"$SCRIPT_DIR/screenshot-vm.sh" "$VM_NAME" "$SCREENSHOT"
echo "Serial log: $SERIAL_LOG"
echo "Screenshot: $SCREENSHOT"
