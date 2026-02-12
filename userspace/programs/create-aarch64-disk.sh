#!/bin/bash
# Create ARM64 test disk in BXTEST format
# Same format as xtask/src/test_disk.rs

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT="$BREENIX_ROOT/target/aarch64_test_binaries.img"
AARCH64_DIR="$SCRIPT_DIR/aarch64"

# Constants matching xtask/src/test_disk.rs
SECTOR_SIZE=512
DATA_START_SECTOR=128
MAGIC="BXTEST"

if [ ! -d "$AARCH64_DIR" ]; then
    echo "Error: ARM64 binaries directory not found: $AARCH64_DIR"
    echo "Build with: cd userspace/programs && ./build-aarch64.sh"
    exit 1
fi

# Find ELF files
ELF_FILES=$(find "$AARCH64_DIR" -name "*.elf" | sort)
if [ -z "$ELF_FILES" ]; then
    echo "Error: No .elf files found in $AARCH64_DIR"
    exit 1
fi

# Count binaries
BINARY_COUNT=$(echo "$ELF_FILES" | wc -l | tr -d ' ')
echo "Creating ARM64 test disk with $BINARY_COUNT binaries..."

# Create temporary directory for assembly
TEMP=$(mktemp -d)
trap "rm -rf $TEMP" EXIT

# Write header (sector 0) - 64 bytes
# BXTEST\0\0 (8 bytes) + version=1 (4 bytes LE) + count (4 bytes LE) + padding (48 bytes)
printf '%s' "$MAGIC" > "$TEMP/header"
printf '\x00\x00' >> "$TEMP/header"  # Null padding for 8 bytes total
printf '\x01\x00\x00\x00' >> "$TEMP/header"  # Version 1 (little-endian)
printf "$(printf '\\x%02x\\x%02x\\x%02x\\x%02x' $((BINARY_COUNT & 0xff)) $(((BINARY_COUNT >> 8) & 0xff)) $(((BINARY_COUNT >> 16) & 0xff)) $(((BINARY_COUNT >> 24) & 0xff)))" >> "$TEMP/header"
# Pad to 64 bytes
dd if=/dev/zero bs=1 count=48 >> "$TEMP/header" 2>/dev/null

# Pad sector 0 to 512 bytes
dd if=/dev/zero bs=1 count=$((SECTOR_SIZE - 64)) >> "$TEMP/header" 2>/dev/null

# Build entry table (sectors 1-127) and collect binary info
ENTRY_TABLE="$TEMP/entries"
> "$ENTRY_TABLE"
CURRENT_SECTOR=$DATA_START_SECTOR

# Process each binary
declare -a BINARY_NAMES BINARY_SIZES BINARY_SECTORS BINARY_PATHS

for elf in $ELF_FILES; do
    NAME=$(basename "$elf" .elf)
    SIZE=$(stat -f%z "$elf" 2>/dev/null || stat -c%s "$elf")
    SECTORS_NEEDED=$(( (SIZE + SECTOR_SIZE - 1) / SECTOR_SIZE ))

    BINARY_NAMES+=("$NAME")
    BINARY_SIZES+=("$SIZE")
    BINARY_SECTORS+=("$CURRENT_SECTOR")
    BINARY_PATHS+=("$elf")

    echo "  $NAME: $SIZE bytes at sector $CURRENT_SECTOR (${SECTORS_NEEDED} sectors)"

    # Write entry (64 bytes): name[32] + sector[8] + size[8] + reserved[16]
    # Name (null-padded to 32 bytes)
    printf '%-32s' "$NAME" | dd bs=32 count=1 2>/dev/null >> "$ENTRY_TABLE"

    # Sector offset (8 bytes, little-endian u64)
    for i in 0 1 2 3 4 5 6 7; do
        printf "$(printf '\\x%02x' $((($CURRENT_SECTOR >> (i * 8)) & 0xff)))" >> "$ENTRY_TABLE"
    done

    # Size (8 bytes, little-endian u64)
    for i in 0 1 2 3 4 5 6 7; do
        printf "$(printf '\\x%02x' $(((SIZE >> (i * 8)) & 0xff)))" >> "$ENTRY_TABLE"
    done

    # Reserved (16 bytes)
    dd if=/dev/zero bs=1 count=16 >> "$ENTRY_TABLE" 2>/dev/null

    CURRENT_SECTOR=$((CURRENT_SECTOR + SECTORS_NEEDED))
done

# Pad entry table to sector boundary
ENTRY_SIZE=$(stat -f%z "$ENTRY_TABLE" 2>/dev/null || stat -c%s "$ENTRY_TABLE")
ENTRY_SECTORS=$(( (ENTRY_SIZE + SECTOR_SIZE - 1) / SECTOR_SIZE ))
ENTRY_PADDING=$(( ENTRY_SECTORS * SECTOR_SIZE - ENTRY_SIZE ))
if [ $ENTRY_PADDING -gt 0 ]; then
    dd if=/dev/zero bs=1 count=$ENTRY_PADDING >> "$ENTRY_TABLE" 2>/dev/null
fi

# Pad to 127 sectors total for entry table (sectors 1-127)
PADDING_SECTORS=$((127 - ENTRY_SECTORS))
if [ $PADDING_SECTORS -gt 0 ]; then
    dd if=/dev/zero bs=$SECTOR_SIZE count=$PADDING_SECTORS >> "$ENTRY_TABLE" 2>/dev/null
fi

# Write binary data
BINARY_DATA="$TEMP/data"
> "$BINARY_DATA"

for i in "${!BINARY_PATHS[@]}"; do
    elf="${BINARY_PATHS[$i]}"
    size="${BINARY_SIZES[$i]}"

    # Write binary
    cat "$elf" >> "$BINARY_DATA"

    # Pad to sector boundary
    remainder=$((size % SECTOR_SIZE))
    if [ $remainder -ne 0 ]; then
        dd if=/dev/zero bs=1 count=$((SECTOR_SIZE - remainder)) >> "$BINARY_DATA" 2>/dev/null
    fi
done

# Assemble final disk image
cat "$TEMP/header" "$ENTRY_TABLE" "$BINARY_DATA" > "$OUTPUT"

TOTAL_SIZE=$(stat -f%z "$OUTPUT" 2>/dev/null || stat -c%s "$OUTPUT")
TOTAL_SECTORS=$((TOTAL_SIZE / SECTOR_SIZE))
echo ""
echo "Created: $OUTPUT"
echo "  Binaries: $BINARY_COUNT"
echo "  Total size: $TOTAL_SIZE bytes ($TOTAL_SECTORS sectors)"
