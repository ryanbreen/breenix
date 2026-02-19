#!/usr/bin/env bash
#
# Collect hardware dump results from the Parallels VM to the host.
# Run this from the HOST after dump-hardware.sh has completed inside the guest.
#
# Usage: ./scripts/parallels/collect-hwdump.sh
#
set -euo pipefail

VM_NAME="breenix-hwdump"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DEST="$REPO_ROOT/docs/parallels-hwdump"

mkdir -p "$DEST"

echo "==> Collecting hardware dump from VM '$VM_NAME'..."

# Check VM is running
if ! prlctl list | grep -q "$VM_NAME"; then
    echo "ERROR: VM '$VM_NAME' is not running."
    echo "Start it with: prlctl start '$VM_NAME'"
    exit 1
fi

# Grab the summary
echo "==> Fetching summary.txt..."
prlctl exec "$VM_NAME" cat /tmp/hwdump/summary.txt > "$DEST/summary.txt" 2>/dev/null || {
    echo "ERROR: Could not read summary.txt from VM."
    echo "Make sure dump-hardware.sh has been run inside the guest."
    exit 1
}

# Grab individual files
for f in iomem.txt guest.dtb guest.dts cpuinfo.txt interrupts.txt \
         lspci-verbose.txt lspci-ids.txt device-tree-compatible.txt; do
    echo "==> Fetching $f..."
    prlctl exec "$VM_NAME" cat "/tmp/hwdump/$f" > "$DEST/$f" 2>/dev/null || echo "    (not available)"
done

# Grab ACPI tables as a tarball (binary files don't copy well via cat)
echo "==> Fetching ACPI tables..."
prlctl exec "$VM_NAME" sh -c 'cd /tmp && tar czf /tmp/acpi-tables.tar.gz hwdump/acpi/ 2>/dev/null' || true
prlctl exec "$VM_NAME" cat /tmp/acpi-tables.tar.gz > "$DEST/acpi-tables.tar.gz" 2>/dev/null || echo "    (not available)"
if [ -f "$DEST/acpi-tables.tar.gz" ] && [ -s "$DEST/acpi-tables.tar.gz" ]; then
    mkdir -p "$DEST/acpi"
    tar xzf "$DEST/acpi-tables.tar.gz" -C "$DEST" --strip-components=1 2>/dev/null || true
    rm -f "$DEST/acpi-tables.tar.gz"
fi

echo ""
echo "==> Hardware dump collected to: $DEST/"
echo ""
ls -la "$DEST/"
echo ""
echo "Key files to review:"
echo "  $DEST/summary.txt          - Full summary of all hardware"
echo "  $DEST/guest.dts            - Device tree (if present)"
echo "  $DEST/iomem.txt            - Physical memory map"
echo "  $DEST/lspci-verbose.txt    - PCI device details"
echo "  $DEST/acpi/                - ACPI table binaries and decompiled DSL"
echo ""
echo "You can now stop the VM: prlctl stop '$VM_NAME'"
