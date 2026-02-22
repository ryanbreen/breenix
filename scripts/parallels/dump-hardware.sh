#!/bin/sh
#
# Hardware dump script to run INSIDE a Linux ARM64 guest on Parallels.
# Collects device tree, ACPI tables, memory map, PCI devices, and
# interrupt controller info needed to port Breenix to Parallels.
#
# Usage (inside the Parallels Linux guest):
#   apk add dtc pciutils acpica    # Alpine
#   sh dump-hardware.sh
#
set -eu

OUTDIR="/tmp/hwdump"
mkdir -p "$OUTDIR"

echo "=== Breenix Parallels Hardware Dump ==="
echo "Date: $(date -u)"
echo "Kernel: $(uname -r)"
echo "Arch: $(uname -m)"
echo ""

# Summary file
SUMMARY="$OUTDIR/summary.txt"
: > "$SUMMARY"

header() {
    echo "" | tee -a "$SUMMARY"
    echo "======================================" | tee -a "$SUMMARY"
    echo "  $1" | tee -a "$SUMMARY"
    echo "======================================" | tee -a "$SUMMARY"
}

# 1. Physical memory map
header "PHYSICAL MEMORY MAP (/proc/iomem)"
if [ -f /proc/iomem ]; then
    cat /proc/iomem > "$OUTDIR/iomem.txt" 2>/dev/null || true
    cat /proc/iomem 2>/dev/null | tee -a "$SUMMARY" || echo "(need root)" | tee -a "$SUMMARY"
else
    echo "/proc/iomem not available" | tee -a "$SUMMARY"
fi

# 2. Device Tree
header "DEVICE TREE"
if [ -f /sys/firmware/fdt ]; then
    cp /sys/firmware/fdt "$OUTDIR/guest.dtb" 2>/dev/null
    echo "Raw DTB saved to $OUTDIR/guest.dtb" | tee -a "$SUMMARY"

    if command -v dtc >/dev/null 2>&1; then
        dtc -I dtb -O dts "$OUTDIR/guest.dtb" > "$OUTDIR/guest.dts" 2>/dev/null
        echo "Decompiled DTS saved to $OUTDIR/guest.dts" | tee -a "$SUMMARY"
        echo "" | tee -a "$SUMMARY"
        echo "--- Device Tree Summary ---" | tee -a "$SUMMARY"
        # Extract key nodes
        grep -E '(compatible|reg |interrupt|clock-frequency|#address-cells|#size-cells)' "$OUTDIR/guest.dts" | head -80 | tee -a "$SUMMARY"
    else
        echo "dtc not installed - install with: apk add dtc" | tee -a "$SUMMARY"
    fi
elif [ -d /proc/device-tree ]; then
    echo "No /sys/firmware/fdt but /proc/device-tree exists" | tee -a "$SUMMARY"
    find /proc/device-tree -name compatible -exec sh -c 'echo "{}:"; cat "{}"; echo' \; > "$OUTDIR/device-tree-compatible.txt" 2>/dev/null || true
    cat "$OUTDIR/device-tree-compatible.txt" | tee -a "$SUMMARY"
else
    echo "No device tree found (VM may use ACPI only)" | tee -a "$SUMMARY"
fi

# 3. ACPI tables
header "ACPI TABLES"
ACPI_DIR="/sys/firmware/acpi/tables"
if [ -d "$ACPI_DIR" ]; then
    mkdir -p "$OUTDIR/acpi"
    echo "Available ACPI tables:" | tee -a "$SUMMARY"
    ls -la "$ACPI_DIR" 2>/dev/null | tee -a "$SUMMARY"

    # Copy key tables
    for table in MADT MCFG FADT GTDT SPCR DSDT SSDT IORT; do
        if [ -f "$ACPI_DIR/$table" ]; then
            cp "$ACPI_DIR/$table" "$OUTDIR/acpi/$table.bin" 2>/dev/null || true
            echo "  Saved $table" | tee -a "$SUMMARY"
        fi
    done

    # Decompile with iasl if available
    if command -v iasl >/dev/null 2>&1; then
        echo "" | tee -a "$SUMMARY"
        echo "--- MADT (Interrupt Controller) ---" | tee -a "$SUMMARY"
        if [ -f "$OUTDIR/acpi/MADT.bin" ]; then
            iasl -d "$OUTDIR/acpi/MADT.bin" 2>/dev/null || true
            cat "$OUTDIR/acpi/MADT.dsl" 2>/dev/null | tee -a "$SUMMARY" || true
        fi

        echo "" | tee -a "$SUMMARY"
        echo "--- MCFG (PCI Configuration) ---" | tee -a "$SUMMARY"
        if [ -f "$OUTDIR/acpi/MCFG.bin" ]; then
            iasl -d "$OUTDIR/acpi/MCFG.bin" 2>/dev/null || true
            cat "$OUTDIR/acpi/MCFG.dsl" 2>/dev/null | tee -a "$SUMMARY" || true
        fi

        echo "" | tee -a "$SUMMARY"
        echo "--- GTDT (Generic Timer) ---" | tee -a "$SUMMARY"
        if [ -f "$OUTDIR/acpi/GTDT.bin" ]; then
            iasl -d "$OUTDIR/acpi/GTDT.bin" 2>/dev/null || true
            cat "$OUTDIR/acpi/GTDT.dsl" 2>/dev/null | tee -a "$SUMMARY" || true
        fi

        echo "" | tee -a "$SUMMARY"
        echo "--- SPCR (Serial Port Console) ---" | tee -a "$SUMMARY"
        if [ -f "$OUTDIR/acpi/SPCR.bin" ]; then
            iasl -d "$OUTDIR/acpi/SPCR.bin" 2>/dev/null || true
            cat "$OUTDIR/acpi/SPCR.dsl" 2>/dev/null | tee -a "$SUMMARY" || true
        fi

        echo "" | tee -a "$SUMMARY"
        echo "--- IORT (I/O Remapping) ---" | tee -a "$SUMMARY"
        if [ -f "$OUTDIR/acpi/IORT.bin" ]; then
            iasl -d "$OUTDIR/acpi/IORT.bin" 2>/dev/null || true
            cat "$OUTDIR/acpi/IORT.dsl" 2>/dev/null | tee -a "$SUMMARY" || true
        fi
    else
        echo "" | tee -a "$SUMMARY"
        echo "iasl not installed - install with: apk add acpica" | tee -a "$SUMMARY"
        echo "Raw binary tables saved to $OUTDIR/acpi/" | tee -a "$SUMMARY"
    fi
else
    echo "No ACPI tables found at $ACPI_DIR" | tee -a "$SUMMARY"
fi

# 4. PCI devices
header "PCI DEVICES"
if command -v lspci >/dev/null 2>&1; then
    lspci -vvv > "$OUTDIR/lspci-verbose.txt" 2>/dev/null || true
    lspci -nn > "$OUTDIR/lspci-ids.txt" 2>/dev/null || true
    echo "--- PCI Device List ---" | tee -a "$SUMMARY"
    lspci -nn 2>/dev/null | tee -a "$SUMMARY"
    echo "" | tee -a "$SUMMARY"
    echo "Detailed PCI info saved to $OUTDIR/lspci-verbose.txt" | tee -a "$SUMMARY"
else
    echo "lspci not installed - install with: apk add pciutils" | tee -a "$SUMMARY"
fi

# 5. Interrupt controller info
header "INTERRUPT CONTROLLER"
if [ -d /proc/interrupts ]; then
    cat /proc/interrupts > "$OUTDIR/interrupts.txt" 2>/dev/null || true
elif [ -f /proc/interrupts ]; then
    cat /proc/interrupts > "$OUTDIR/interrupts.txt" 2>/dev/null || true
    echo "--- Active Interrupts ---" | tee -a "$SUMMARY"
    cat /proc/interrupts 2>/dev/null | tee -a "$SUMMARY"
fi

# GIC version detection
echo "" | tee -a "$SUMMARY"
echo "--- GIC Detection ---" | tee -a "$SUMMARY"
if [ -d /proc/device-tree ]; then
    find /proc/device-tree -name compatible | while read f; do
        val=$(cat "$f" 2>/dev/null | tr '\0' ' ')
        case "$val" in
            *gic*) echo "GIC node: $f = $val" | tee -a "$SUMMARY" ;;
        esac
    done
fi
dmesg 2>/dev/null | grep -i -E '(gic|interrupt.controller)' | tee -a "$SUMMARY" || true

# 6. CPU info
header "CPU INFORMATION"
cat /proc/cpuinfo > "$OUTDIR/cpuinfo.txt" 2>/dev/null || true
head -30 /proc/cpuinfo 2>/dev/null | tee -a "$SUMMARY"

# 7. Kernel command line and boot info
header "BOOT INFORMATION"
echo "Cmdline: $(cat /proc/cmdline 2>/dev/null)" | tee -a "$SUMMARY"
echo "" | tee -a "$SUMMARY"

# EFI variables
if [ -d /sys/firmware/efi ]; then
    echo "EFI boot: YES" | tee -a "$SUMMARY"
    echo "EFI runtime services: $(ls /sys/firmware/efi/ 2>/dev/null | tr '\n' ' ')" | tee -a "$SUMMARY"
    if [ -d /sys/firmware/efi/efivars ]; then
        echo "EFI vars count: $(ls /sys/firmware/efi/efivars/ 2>/dev/null | wc -l)" | tee -a "$SUMMARY"
    fi
else
    echo "EFI boot: NO (or not detected)" | tee -a "$SUMMARY"
fi

# 8. Memory info
header "MEMORY LAYOUT"
echo "--- /proc/meminfo ---" | tee -a "$SUMMARY"
head -10 /proc/meminfo 2>/dev/null | tee -a "$SUMMARY"
echo "" | tee -a "$SUMMARY"
echo "--- dmesg memory ---" | tee -a "$SUMMARY"
dmesg 2>/dev/null | grep -i -E '(memory|zone|node )' | head -20 | tee -a "$SUMMARY" || true

# 9. VirtIO devices
header "VIRTIO DEVICES"
echo "--- /sys/bus/virtio/devices ---" | tee -a "$SUMMARY"
if [ -d /sys/bus/virtio/devices ]; then
    for dev in /sys/bus/virtio/devices/*; do
        if [ -d "$dev" ]; then
            name=$(basename "$dev")
            vendor=$(cat "$dev/vendor" 2>/dev/null || echo "?")
            device=$(cat "$dev/device" 2>/dev/null || echo "?")
            echo "  $name: vendor=$vendor device=$device" | tee -a "$SUMMARY"
        fi
    done
else
    echo "  No virtio bus found" | tee -a "$SUMMARY"
fi

# 10. Serial/UART
header "SERIAL PORTS"
dmesg 2>/dev/null | grep -i -E '(uart|serial|ttyAMA|ttyS|pl011)' | tee -a "$SUMMARY" || echo "  No serial info in dmesg" | tee -a "$SUMMARY"

# 11. Timer info
header "TIMER"
dmesg 2>/dev/null | grep -i -E '(timer|clocksource|arch_timer)' | head -10 | tee -a "$SUMMARY" || true

# Package everything
echo ""
echo "======================================" | tee -a "$SUMMARY"
echo "  DUMP COMPLETE" | tee -a "$SUMMARY"
echo "======================================" | tee -a "$SUMMARY"
echo "" | tee -a "$SUMMARY"
echo "All files saved to: $OUTDIR" | tee -a "$SUMMARY"
echo "Files:" | tee -a "$SUMMARY"
ls -la "$OUTDIR"/ | tee -a "$SUMMARY"
if [ -d "$OUTDIR/acpi" ]; then
    echo "" | tee -a "$SUMMARY"
    echo "ACPI files:" | tee -a "$SUMMARY"
    ls -la "$OUTDIR/acpi/" | tee -a "$SUMMARY"
fi

echo ""
echo "To copy results to the host:"
echo "  1. From host: prlctl exec breenix-hwdump cat /tmp/hwdump/summary.txt"
echo "  2. Or tar it: tar czf /tmp/hwdump.tar.gz -C /tmp hwdump"
echo "     Then:      prlctl exec breenix-hwdump cat /tmp/hwdump.tar.gz > hwdump.tar.gz"
