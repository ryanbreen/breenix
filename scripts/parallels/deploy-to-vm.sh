#!/bin/bash
# Deploy the Breenix EFI disk image to a Parallels Desktop VM.
#
# Prerequisites:
#   - Parallels Desktop installed with prlctl/prlsrvctl available
#   - A VM named "breenix-dev" exists (or specify via --vm)
#   - EFI image built via ./scripts/parallels/build-efi.sh
#
# Usage:
#   ./scripts/parallels/deploy-to-vm.sh              # Deploy to "breenix-dev" VM
#   ./scripts/parallels/deploy-to-vm.sh --vm myvm    # Deploy to specific VM
#   ./scripts/parallels/deploy-to-vm.sh --boot       # Deploy and boot
#   ./scripts/parallels/deploy-to-vm.sh --serial     # Deploy, boot, tail serial

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EFI_IMG="$PROJECT_ROOT/target/parallels/breenix-efi.img"
VM_NAME="breenix-dev"
DO_BOOT=false
DO_SERIAL=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --vm) VM_NAME="$2"; shift 2 ;;
        --boot) DO_BOOT=true; shift ;;
        --serial) DO_SERIAL=true; DO_BOOT=true; shift ;;
        *) echo "Unknown argument: $1"; exit 1 ;;
    esac
done

# Verify prerequisites
if ! command -v prlctl &>/dev/null; then
    echo "ERROR: prlctl not found. Is Parallels Desktop installed?"
    exit 1
fi

if [ ! -f "$EFI_IMG" ]; then
    echo "ERROR: EFI image not found at $EFI_IMG"
    echo "Run ./scripts/parallels/build-efi.sh first"
    exit 1
fi

# Check if VM exists
if ! prlctl list --all 2>/dev/null | grep -q "$VM_NAME"; then
    echo "VM '$VM_NAME' not found. Available VMs:"
    prlctl list --all
    echo ""
    echo "Create a VM with:"
    echo "  prlctl create $VM_NAME --ostype linux --arch aarch64"
    echo "  prlctl set $VM_NAME --efi-boot on"
    echo "  prlctl set $VM_NAME --device-set hdd0 --image $EFI_IMG --type plain"
    exit 1
fi

# Stop VM if running
VM_STATUS=$(prlctl status "$VM_NAME" 2>/dev/null | awk '{print $NF}')
if [ "$VM_STATUS" = "running" ] || [ "$VM_STATUS" = "paused" ]; then
    echo "Stopping VM '$VM_NAME'..."
    prlctl stop "$VM_NAME" --kill 2>/dev/null || true
    sleep 2
fi

# Attach the EFI disk image
echo "=== Deploying EFI Image to VM '$VM_NAME' ==="
echo "Image: $EFI_IMG ($(stat -f%z "$EFI_IMG" 2>/dev/null || stat -c%s "$EFI_IMG") bytes)"

# Remove existing disk and attach new one
# First, try to detach any existing hdd0
prlctl set "$VM_NAME" --device-del hdd0 2>/dev/null || true

# Copy image to VM's directory for Parallels to manage
VM_DIR=$(prlctl list --info "$VM_NAME" 2>/dev/null | grep "Home:" | sed 's/.*Home: *//' | tr -d ' ')
if [ -z "$VM_DIR" ]; then
    VM_DIR="$HOME/Parallels/$VM_NAME.pvm"
fi

DEST_IMG="$VM_DIR/breenix-efi.img"
echo "Copying to: $DEST_IMG"
cp "$EFI_IMG" "$DEST_IMG"

# Attach as plain disk (not expanding)
prlctl set "$VM_NAME" --device-add hdd --image "$DEST_IMG" --type plain --position 0 2>/dev/null || \
    echo "WARNING: Could not attach disk via prlctl. You may need to attach it manually in Parallels settings."

# Ensure EFI boot is enabled
prlctl set "$VM_NAME" --efi-boot on 2>/dev/null || true

echo ""
echo "=== Deployment Complete ==="

if [ "$DO_BOOT" = true ]; then
    echo "Starting VM '$VM_NAME'..."
    prlctl start "$VM_NAME"

    if [ "$DO_SERIAL" = true ]; then
        echo "Waiting for serial output..."
        SERIAL_LOG="$VM_DIR/serial.log"
        # Parallels serial port output - check common locations
        sleep 3
        if [ -f "$SERIAL_LOG" ]; then
            tail -f "$SERIAL_LOG"
        else
            echo "Serial log not found at $SERIAL_LOG"
            echo "Configure serial port in Parallels VM settings to redirect to file."
            echo "Check VM output in Parallels Desktop window."
        fi
    fi
else
    echo "To boot the VM:"
    echo "  prlctl start $VM_NAME"
    echo ""
    echo "To boot and watch serial:"
    echo "  ./scripts/parallels/deploy-to-vm.sh --serial"
fi
