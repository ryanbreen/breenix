#!/bin/bash
# Build BusyBox for Breenix (x86_64 and aarch64)
#
# Cross-compiles BusyBox 1.37.0 as a static musl binary for the target
# architecture, placing the text segment at 0x40000000 for Breenix userspace.
#
# Prerequisites:
#   brew tap filosottile/musl-cross
#   brew install musl-cross                     # x86_64-linux-musl-gcc
#   brew install musl-cross --with-aarch64      # aarch64-linux-musl-gcc
#
# Usage:
#   ./scripts/build-busybox.sh                  # Build for x86_64 (default)
#   ./scripts/build-busybox.sh --arch aarch64   # Build for aarch64
#   ./scripts/build-busybox.sh --arch x86_64    # Build for x86_64

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUSYBOX_DIR="$PROJECT_ROOT/third-party/busybox-1.37.0"
FRAGMENT="$PROJECT_ROOT/third-party/busybox-breenix-fragment.config"

ARCH="x86_64"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            ARCH="$2"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--arch x86_64|aarch64]"
            exit 1
            ;;
    esac
done

case "$ARCH" in
    x86_64)
        CROSS_PREFIX="x86_64-linux-musl-"
        OUTPUT_DIR="$PROJECT_ROOT/userspace/programs"
        ;;
    aarch64)
        CROSS_PREFIX="aarch64-linux-musl-"
        OUTPUT_DIR="$PROJECT_ROOT/userspace/programs/aarch64"
        ;;
    *)
        echo "Error: unsupported arch '$ARCH' (use x86_64 or aarch64)"
        exit 1
        ;;
esac

# Verify cross-compiler is available
if ! command -v "${CROSS_PREFIX}gcc" &>/dev/null; then
    echo "Error: ${CROSS_PREFIX}gcc not found in PATH"
    echo ""
    echo "Install with:"
    echo "  brew tap filosottile/musl-cross"
    if [[ "$ARCH" == "x86_64" ]]; then
        echo "  brew install musl-cross"
    else
        echo "  brew install musl-cross --with-aarch64"
    fi
    exit 1
fi

if [[ ! -d "$BUSYBOX_DIR" ]]; then
    echo "Error: BusyBox source not found at $BUSYBOX_DIR"
    echo "Download with:"
    echo "  cd third-party"
    echo "  curl -LO https://busybox.net/downloads/busybox-1.37.0.tar.bz2"
    echo "  tar xjf busybox-1.37.0.tar.bz2"
    exit 1
fi

echo "Building BusyBox for Breenix ($ARCH)"
echo "  Cross prefix: $CROSS_PREFIX"
echo "  Output: $OUTPUT_DIR/busybox.elf"
echo ""

cd "$BUSYBOX_DIR"

# Start from allnoconfig
make allnoconfig >/dev/null 2>&1

# Apply fragment config using Python for reliable cross-platform text processing
python3 -c "
import re, sys

# Read the current config
with open('.config', 'r') as f:
    config = f.read()

# Read the fragment
with open('$FRAGMENT', 'r') as f:
    fragment = f.read()

# Parse fragment for CONFIG_FOO=value lines
for line in fragment.splitlines():
    line = line.strip()
    if not line or line.startswith('#'):
        continue
    m = re.match(r'^(CONFIG_[A-Z0-9_]+)=(.*)$', line)
    if m:
        key, value = m.group(1), m.group(2)
        # Replace '# CONFIG_FOO is not set' with 'CONFIG_FOO=value'
        pattern = f'# {key} is not set'
        replacement = f'{key}={value}'
        if pattern in config:
            config = config.replace(pattern, replacement)
        elif re.search(f'^{re.escape(key)}=', config, re.MULTILINE):
            config = re.sub(f'^{re.escape(key)}=.*$', replacement, config, flags=re.MULTILINE)
        else:
            config += replacement + '\n'

# Override cross compiler prefix for target arch
config = re.sub(
    r'^CONFIG_CROSS_COMPILER_PREFIX=.*$',
    'CONFIG_CROSS_COMPILER_PREFIX=\"$CROSS_PREFIX\"',
    config,
    flags=re.MULTILINE
)

with open('.config', 'w') as f:
    f.write(config)

print('Config fragment applied successfully')
"

# Let Kconfig resolve dependencies (accept defaults for any new prompts)
yes "" | make oldconfig >/dev/null 2>&1

echo "Configuration applied. Building..."
echo ""

# Build with parallel jobs
NPROC=$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)
make -j"$NPROC" 2>&1

# Verify the binary
if [[ ! -f busybox ]]; then
    echo "Error: BusyBox build failed - no busybox binary produced"
    exit 1
fi

# Check it's static
if file busybox | grep -q "dynamically linked"; then
    echo "Warning: BusyBox is dynamically linked (expected static)"
fi

echo ""
echo "BusyBox built successfully:"
file busybox
ls -lh busybox

# Copy to output directory
mkdir -p "$OUTPUT_DIR"
cp busybox "$OUTPUT_DIR/busybox.elf"
echo ""
echo "Installed: $OUTPUT_DIR/busybox.elf"

# Show enabled applets
echo ""
echo "Enabled applets:"
./busybox --list 2>/dev/null | head -60 || true
