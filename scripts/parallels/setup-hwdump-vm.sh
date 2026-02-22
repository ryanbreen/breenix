#!/usr/bin/env bash
#
# Setup a minimal ARM64 Linux VM in Parallels for hardware discovery.
# Downloads an Alpine Linux ARM64 ISO (lightweight), creates a VM,
# attaches the ISO, and boots it.
#
# Usage: ./scripts/parallels/setup-hwdump-vm.sh
#
set -euo pipefail

VM_NAME="breenix-hwdump"
ISO_DIR="$HOME/.cache/breenix-parallels"
ALPINE_VERSION="3.21"
ALPINE_MINOR="3"
ALPINE_ISO="alpine-standard-${ALPINE_VERSION}.${ALPINE_MINOR}-aarch64.iso"
ALPINE_URL="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/releases/aarch64/${ALPINE_ISO}"

mkdir -p "$ISO_DIR"

# Download Alpine ARM64 ISO if not cached
if [ ! -f "$ISO_DIR/$ALPINE_ISO" ]; then
    echo "==> Downloading Alpine Linux ${ALPINE_VERSION}.${ALPINE_MINOR} ARM64..."
    curl -L -o "$ISO_DIR/$ALPINE_ISO" "$ALPINE_URL"
    echo "==> Downloaded to $ISO_DIR/$ALPINE_ISO"
else
    echo "==> Using cached ISO: $ISO_DIR/$ALPINE_ISO"
fi

# Check if VM already exists
if prlctl list -a | grep -q "$VM_NAME"; then
    echo "==> VM '$VM_NAME' already exists."
    read -p "    Delete and recreate? [y/N] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "==> Stopping and deleting existing VM..."
        prlctl stop "$VM_NAME" --kill 2>/dev/null || true
        prlctl delete "$VM_NAME" 2>/dev/null || true
    else
        echo "==> Keeping existing VM. Start it with: prlctl start '$VM_NAME'"
        exit 0
    fi
fi

echo "==> Creating Parallels VM: $VM_NAME"
prlctl create "$VM_NAME" -o linux --no-hdd

echo "==> Configuring VM..."
# Set resources
prlctl set "$VM_NAME" --cpus 4
prlctl set "$VM_NAME" --memsize 2048

# Ensure EFI boot is on (should be default for ARM64)
prlctl set "$VM_NAME" --efi-boot on

# Add a small disk for the OS install
prlctl set "$VM_NAME" --device-add hdd --type plain --size 8192

# Attach the ISO
prlctl set "$VM_NAME" --device-add cdrom --image "$ISO_DIR/$ALPINE_ISO" --connect

# Boot from CD first
prlctl set "$VM_NAME" --device-bootorder "cdrom0 hdd0"

echo "==> VM '$VM_NAME' created and configured."
echo ""
echo "Next steps:"
echo "  1. Start the VM:  prlctl start '$VM_NAME'"
echo "  2. Open console:  open \"/Users/\$USER/Parallels/${VM_NAME}.pvm\""
echo "     Or use:        prlctl enter '$VM_NAME'"
echo "  3. Log in as root (no password on Alpine live)"
echo "  4. Install prerequisites and run the dump script:"
echo ""
echo "     # Inside the Alpine VM:"
echo "     apk add dtc pciutils acpica"
echo "     # Then paste/run the dump-hardware.sh script"
echo ""
echo "  5. Copy results out:"
echo "     prlctl exec '$VM_NAME' cat /tmp/hwdump/summary.txt"
echo ""
echo "Or start the VM now with: prlctl start '$VM_NAME'"
