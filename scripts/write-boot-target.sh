#!/bin/bash
# Write the boot-target file onto an ext2 disk image, so a production kernel booted
# from it selects a suite or probe as PID 1 (docs/boot-path.md, "Boot modes"):
#
#   scripts/write-boot-target.sh IMAGE SUITE_ID|--probe [PID1_ELF]
#
# IMAGE gets /etc/breenix/boot-target containing "suite SUITE_ID" or "probe",
# replacing the prior target. Use a copy so ordinary boots keep their disk.
# The image must contain /sbin/suite-SUITE_ID or /sbin/probe respectively.
# When PID1_ELF is supplied, readback must match its bytes before accepting the image.
#
# Uses debugfs (e2fsprogs) when it is on PATH or in Homebrew's keg-only e2fsprogs,
# and otherwise runs debugfs in an Alpine container.

set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 4 ]; then
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
fi
IMAGE="$1"
SUITE="$2"
SUITE_ELF="${3:-}"
SEQUENCE="${4:-}"
BOOT_TARGET="suite $SUITE"
BINARY="/sbin/suite-$SUITE"
if [ "$SUITE" = --probe ]; then
    BOOT_TARGET=probe
    BINARY=/sbin/probe
elif [[ ! "$SUITE" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
    echo "write-boot-target: not a suite id (lowercase words joined by '-'): $SUITE" >&2
    exit 2
fi
[ -f "$IMAGE" ] || { echo "write-boot-target: no disk image at $IMAGE" >&2; exit 1; }
if [ -n "$SUITE_ELF" ] && [ ! -f "$SUITE_ELF" ]; then
    echo "write-boot-target: no PID 1 binary at $SUITE_ELF" >&2; exit 1
fi

DEBUGFS=
for candidate in debugfs /sbin/debugfs /usr/sbin/debugfs \
    /opt/homebrew/opt/e2fsprogs/sbin/debugfs /usr/local/opt/e2fsprogs/sbin/debugfs; do
    if command -v "$candidate" >/dev/null 2>&1; then DEBUGFS="$(command -v "$candidate")"; break; fi
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/boot-target.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
printf '%s\n' "$BOOT_TARGET" > "$WORK/boot-target"
# mkdir and rm report an error when the directory exists or the file does not;
# debugfs carries on, and the readback below is what decides success.
cat > "$WORK/commands" <<'EOF'
mkdir /etc/breenix
rm /etc/breenix/boot-target
write /work/boot-target /etc/breenix/boot-target
EOF

if [ -n "$SEQUENCE" ] && [ "$SEQUENCE" != "$SUITE" ]; then
    python3 - "$SEQUENCE" "$SUITE" <<'PYIDS'
import re, sys
ids = sys.argv[1].split(',')
if ids[0] != sys.argv[2] or len(set(ids)) != len(ids) or not all(re.fullmatch(r'[a-z0-9]+(-[a-z0-9]+)*', x) for x in ids):
    sys.exit('write-boot-target: invalid sequence')
PYIDS
    printf '%s\n' "$SEQUENCE" > "$WORK/suite-sequence"
    printf 'rm /etc/breenix/suite-sequence\nwrite /work/suite-sequence /etc/breenix/suite-sequence\n' >> "$WORK/commands"
fi

if [ -n "$DEBUGFS" ]; then
    sed "s|/work/|$WORK/|" "$WORK/commands" > "$WORK/commands.host"
    "$DEBUGFS" -w -f "$WORK/commands.host" "$IMAGE" >/dev/null 2>&1 || true
    if [ -f "$WORK/suite-sequence" ]; then
        sequence_written="$("$DEBUGFS" -R 'cat /etc/breenix/suite-sequence' "$IMAGE" 2>/dev/null || true)"
        [ "$sequence_written" = "$SEQUENCE" ] || { echo "write-boot-target: sequence readback differs" >&2; exit 1; }
        IFS=',' read -r -a sequence_ids <<< "$SEQUENCE"
        for id in "${sequence_ids[@]}"; do
            "$DEBUGFS" -R "dump /sbin/suite-$id $WORK/$id.elf" "$IMAGE" >/dev/null 2>&1 || true
            cmp -s "$WORK/$id.elf" "$(dirname "$SUITE_ELF")/suite-$id.elf" || { echo "write-boot-target: stale sequence binary $id" >&2; exit 1; }
        done
    fi
    written="$("$DEBUGFS" -R 'cat /etc/breenix/boot-target' "$IMAGE" 2>/dev/null || true)"
    "$DEBUGFS" -R "dump $BINARY $WORK/installed.elf" "$IMAGE" >/dev/null 2>&1 || true
else
    if [ -f "$WORK/suite-sequence" ]; then
        echo "write-boot-target: suite sequences require host debugfs" >&2; exit 1
    fi
    command -v docker >/dev/null 2>&1 || { echo "write-boot-target: needs debugfs (e2fsprogs) or docker" >&2; exit 1; }
    image_dir="$(cd "$(dirname "$IMAGE")" && pwd)"
    image_name="$(basename "$IMAGE")"
    written="$(docker run --rm -v "$image_dir:/disk" -v "$WORK:/work" alpine:latest sh -c "
        apk add --no-cache e2fsprogs-extra >/dev/null 2>&1 || exit 1
        debugfs -w -f /work/commands /disk/$image_name >/dev/null 2>&1
        debugfs -R 'dump $BINARY /work/installed.elf' /disk/$image_name >/dev/null 2>&1
        debugfs -R 'cat /etc/breenix/boot-target' /disk/$image_name 2>/dev/null")" || true
fi

if [ "$written" != "$BOOT_TARGET" ]; then
    echo "write-boot-target: /etc/breenix/boot-target on $IMAGE reads back as '$written', not '$BOOT_TARGET'" >&2
    exit 1
fi
if [ ! -s "$WORK/installed.elf" ]; then
    echo "write-boot-target: $IMAGE has no $BINARY" >&2
    exit 1
fi
if [ -n "$SUITE_ELF" ] && ! cmp -s "$WORK/installed.elf" "$SUITE_ELF"; then
    echo "write-boot-target: $BINARY on $IMAGE is not $SUITE_ELF (a stale or partial disk image)" >&2
    exit 1
fi
echo "Boot target on $IMAGE: $BOOT_TARGET"
