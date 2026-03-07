#!/usr/bin/env bash
# Generate a VMware Fusion .vmx config file for Breenix ARM64.
#
# Usage: generate-vmx.sh <vm_dir> <boot_vmdk> [ext2_vmdk]
#   vm_dir     - Directory for the .vmwarevm bundle
#   boot_vmdk  - Path to the EFI boot disk VMDK
#   ext2_vmdk  - Optional path to ext2 data disk VMDK

set -euo pipefail

VM_DIR="$1"
BOOT_VMDK="$2"
EXT2_VMDK="${3:-}"
SERIAL_LOG="/tmp/breenix-vmware-serial.log"

VM_NAME=$(basename "$VM_DIR" .vmwarevm)
VMX_FILE="$VM_DIR/$VM_NAME.vmx"

cat > "$VMX_FILE" <<EOF
.encoding = "UTF-8"
config.version = "8"
virtualHW.version = "22"

displayName = "$VM_NAME"
guestOS = "arm-other-64"
firmware = "efi"

# CPU and Memory
numvcpus = "4"
memsize = "2048"
cpuid.coresPerSocket = "4"

# PCI bridges (required for device enumeration)
pciBridge0.present = "TRUE"
pciBridge4.present = "TRUE"
pciBridge4.virtualDev = "pcieRootPort"
pciBridge4.functions = "8"
pciBridge5.present = "TRUE"
pciBridge5.virtualDev = "pcieRootPort"
pciBridge5.functions = "8"
pciBridge6.present = "TRUE"
pciBridge6.virtualDev = "pcieRootPort"
pciBridge6.functions = "8"
pciBridge7.present = "TRUE"
pciBridge7.virtualDev = "pcieRootPort"
pciBridge7.functions = "8"

# NVMe boot disk (FAT32 ESP with kernel)
nvme0.present = "TRUE"
nvme0:0.present = "TRUE"
nvme0:0.fileName = "$BOOT_VMDK"

# SATA controller
sata0.present = "TRUE"
EOF

# Add ext2 data disk if provided
if [ -n "$EXT2_VMDK" ]; then
    cat >> "$VMX_FILE" <<EOF

# SATA ext2 data disk
sata0:0.present = "TRUE"
sata0:0.fileName = "$EXT2_VMDK"
EOF
fi

cat >> "$VMX_FILE" <<EOF

# Serial port -> file (for kernel debug output)
serial0.present = "TRUE"
serial0.fileType = "file"
serial0.fileName = "$SERIAL_LOG"
serial0.yieldOnMsrRead = "TRUE"

# Networking (NAT with e1000e)
ethernet0.present = "TRUE"
ethernet0.connectionType = "nat"
ethernet0.virtualDev = "e1000e"
ethernet0.addressType = "generated"
ethernet0.linkStatePropagation.enable = "TRUE"

# USB (xHCI for keyboard/mouse)
usb.present = "TRUE"
usb_xhci.present = "TRUE"

# Display
svga.vramSize = "268435456"

# VMCI
vmci0.present = "TRUE"

# Misc
tools.syncTime = "FALSE"
floppy0.present = "FALSE"
hpet0.present = "TRUE"
EOF

echo "$VMX_FILE"
