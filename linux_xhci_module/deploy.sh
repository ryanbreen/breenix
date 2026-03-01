#!/bin/bash
# Deploy and load breenix_xhci_probe module on linux-probe Parallels VM.
#
# Usage: bash deploy.sh [--tail]
#   --tail: Keep tailing dmesg after loading (Ctrl-C to stop)
#
set -euo pipefail

VM=linux-probe
SRC_DIR="$(cd "$(dirname "$0")" && pwd)"
TAIL=false

for arg in "$@"; do
    case "$arg" in
        --tail) TAIL=true ;;
        *) echo "Unknown arg: $arg"; exit 1 ;;
    esac
done

echo "=== Ensuring VM is running ==="
prlctl resume "$VM" 2>/dev/null || prlctl start "$VM" 2>/dev/null || true
# Wait for VM to be reachable
for i in $(seq 1 30); do
    if prlctl exec "$VM" -- true 2>/dev/null; then
        break
    fi
    echo "  Waiting for VM... ($i/30)"
    sleep 2
done

echo "=== Installing build dependencies ==="
prlctl exec "$VM" -- apk add --no-cache build-base linux-headers 2>/dev/null || true

echo "=== Copying module source ==="
prlctl exec "$VM" -- mkdir -p /root/xhci_module
for f in Makefile breenix_xhci_probe.c; do
    prlctl exec "$VM" -- sh -c "cat > /root/xhci_module/$f" < "$SRC_DIR/$f"
done

echo "=== Building module ==="
prlctl exec "$VM" -- sh -c "cd /root/xhci_module && make clean && make" 2>&1

echo "=== Finding xHCI PCI address ==="
XHCI_LINE=$(prlctl exec "$VM" -- lspci -nn 2>/dev/null | grep -i xhci | head -1 || true)
if [ -z "$XHCI_LINE" ]; then
    echo "ERROR: No xHCI controller found in lspci output"
    echo "Available PCI devices:"
    prlctl exec "$VM" -- lspci -nn
    exit 1
fi
XHCI_ADDR=$(echo "$XHCI_LINE" | awk '{print $1}')
echo "xHCI at PCI $XHCI_ADDR: $XHCI_LINE"

echo "=== Unbinding stock xhci_hcd driver ==="
prlctl exec "$VM" -- sh -c "echo 0000:$XHCI_ADDR > /sys/bus/pci/drivers/xhci_hcd/unbind" 2>/dev/null || true

echo "=== Unloading previous module version ==="
prlctl exec "$VM" -- rmmod breenix_xhci_probe 2>/dev/null || true

echo "=== Loading module ==="
prlctl exec "$VM" -- insmod /root/xhci_module/breenix_xhci_probe.ko

echo ""
echo "=== Module loaded successfully ==="
echo "=== Recent dmesg output: ==="
prlctl exec "$VM" -- dmesg | tail -80

if [ "$TAIL" = true ]; then
    echo ""
    echo "=== Tailing dmesg (Ctrl-C to stop) ==="
    prlctl exec "$VM" -- dmesg -w
fi
