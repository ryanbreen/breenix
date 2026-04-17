#!/usr/bin/env bash
# Capture a Parallels VM display as PNG on stdout.
#
# Usage:
#   scripts/parallels/capture-display.sh <vm-name> > screen.png
#
# All diagnostics go to stderr so stdout remains binary-clean.

set -euo pipefail

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ] || [ "$#" -ne 1 ]; then
    echo "Usage: $0 <vm-name> > screen.png" >&2
    exit 2
fi

VM_NAME="$1"
TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/breenix-prl-capture.XXXXXX")"
trap 'rm -rf "$TMPDIR"' EXIT

if ! command -v prlctl >/dev/null 2>&1; then
    echo "ERROR: prlctl not found" >&2
    exit 1
fi

png_non_black() {
    python3 - "$1" <<'PY'
import struct
import sys
import zlib

path = sys.argv[1]

try:
    data = open(path, "rb").read()
except OSError as exc:
    print(f"invalid png: {exc}", file=sys.stderr)
    sys.exit(1)

if not data.startswith(b"\x89PNG\r\n\x1a\n"):
    print("invalid png: bad signature", file=sys.stderr)
    sys.exit(1)

pos = 8
width = height = bit_depth = color_type = None
idat = bytearray()

while pos + 8 <= len(data):
    length = struct.unpack(">I", data[pos:pos + 4])[0]
    ctype = data[pos + 4:pos + 8]
    payload_start = pos + 8
    payload_end = payload_start + length
    if payload_end + 4 > len(data):
        print("invalid png: truncated chunk", file=sys.stderr)
        sys.exit(1)
    payload = data[payload_start:payload_end]
    pos = payload_end + 4

    if ctype == b"IHDR":
        width, height, bit_depth, color_type, _, _, _ = struct.unpack(">IIBBBBB", payload)
    elif ctype == b"IDAT":
        idat.extend(payload)
    elif ctype == b"IEND":
        break

if width is None or height is None or not idat:
    print("invalid png: missing IHDR or IDAT", file=sys.stderr)
    sys.exit(1)

if bit_depth != 8 or color_type not in (0, 2, 4, 6):
    # Unsupported captures are still valid screenshots. Do not reject them as black.
    sys.exit(0)

channels = {0: 1, 2: 3, 4: 2, 6: 4}[color_type]
stride = width * channels

try:
    raw = zlib.decompress(bytes(idat))
except zlib.error as exc:
    print(f"invalid png: zlib decode failed: {exc}", file=sys.stderr)
    sys.exit(1)

def paeth(a, b, c):
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c

prev = bytearray(stride)
offset = 0
sampled = 0
non_black = 0
row_step = max(1, height // 128)
col_step = max(1, width // 128)

for y in range(height):
    if offset + 1 + stride > len(raw):
        print("invalid png: truncated scanline data", file=sys.stderr)
        sys.exit(1)
    filter_type = raw[offset]
    scan = bytearray(raw[offset + 1:offset + 1 + stride])
    offset += 1 + stride

    for i in range(stride):
        left = scan[i - channels] if i >= channels else 0
        up = prev[i]
        up_left = prev[i - channels] if i >= channels else 0
        if filter_type == 0:
            value = scan[i]
        elif filter_type == 1:
            value = (scan[i] + left) & 0xff
        elif filter_type == 2:
            value = (scan[i] + up) & 0xff
        elif filter_type == 3:
            value = (scan[i] + ((left + up) // 2)) & 0xff
        elif filter_type == 4:
            value = (scan[i] + paeth(left, up, up_left)) & 0xff
        else:
            print(f"invalid png: unsupported filter {filter_type}", file=sys.stderr)
            sys.exit(1)
        scan[i] = value

    if y % row_step == 0:
        for x in range(0, width, col_step):
            idx = x * channels
            if color_type == 0:
                r = g = b = scan[idx]
            elif color_type == 4:
                r = g = b = scan[idx]
            else:
                r, g, b = scan[idx], scan[idx + 1], scan[idx + 2]
            sampled += 1
            if r > 3 or g > 3 or b > 3:
                non_black += 1

    prev = scan

if sampled == 0:
    print("invalid png: no samples", file=sys.stderr)
    sys.exit(1)

if non_black == 0:
    sys.exit(2)

sys.exit(0)
PY
}

START_TIME="$(date +%s)"
LAST_ERROR=""

for DELAY in 30 60 90; do
    NOW="$(date +%s)"
    TARGET=$((START_TIME + DELAY))
    if [ "$NOW" -lt "$TARGET" ]; then
        sleep $((TARGET - NOW))
    fi

    OUT="$TMPDIR/capture-${DELAY}s.png"
    echo "capture-display: trying ${VM_NAME} at ${DELAY}s" >&2
    if ! prlctl capture "$VM_NAME" --file "$OUT" >&2; then
        LAST_ERROR="prlctl capture failed at ${DELAY}s"
        echo "capture-display: ${LAST_ERROR}" >&2
        continue
    fi

    set +e
    png_non_black "$OUT"
    STATUS=$?
    set -e

    if [ "$STATUS" -eq 0 ]; then
        cat "$OUT"
        exit 0
    fi

    if [ "$STATUS" -eq 2 ]; then
        LAST_ERROR="capture at ${DELAY}s was all black"
        echo "capture-display: ${LAST_ERROR}; retrying" >&2
    else
        LAST_ERROR="capture at ${DELAY}s was not a readable PNG"
        echo "capture-display: ${LAST_ERROR}; retrying" >&2
    fi
done

echo "ERROR: failed to capture a usable display for ${VM_NAME}: ${LAST_ERROR}" >&2
exit 1
