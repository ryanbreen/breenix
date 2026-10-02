#!/bin/bash
# Write the boot-target file onto an ext2 disk image, so a production kernel booted
# from it runs that effort suite as PID 1 (docs/boot-path.md, "Boot modes"):
#
#   scripts/write-boot-target.sh IMAGE SUITE_ID
#
# IMAGE gets /etc/breenix/boot-target containing the one line "suite SUITE_ID",
# replacing any boot target already there. Write it onto a copy, never onto the
# disk an ordinary boot uses: every production boot of that disk would run the suite.
#
# Uses debugfs (e2fsprogs) when it is on PATH or in Homebrew's keg-only e2fsprogs,
# and otherwise runs debugfs in an Alpine container.

set -euo pipefail

if [ "$#" -ne 2 ]; then
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
fi
IMAGE="$1"
SUITE="$2"
if [[ ! "$SUITE" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]] || [ "${#SUITE}" -gt 40 ]; then
    echo "write-boot-target: not a suite id (lowercase words joined by '-'): $SUITE" >&2
    exit 2
fi
[ -f "$IMAGE" ] || { echo "write-boot-target: no disk image at $IMAGE" >&2; exit 1; }

DEBUGFS=
for candidate in debugfs /sbin/debugfs /usr/sbin/debugfs \
    /opt/homebrew/opt/e2fsprogs/sbin/debugfs /usr/local/opt/e2fsprogs/sbin/debugfs; do
    if command -v "$candidate" >/dev/null 2>&1; then DEBUGFS="$(command -v "$candidate")"; break; fi
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/boot-target.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
printf 'suite %s\n' "$SUITE" > "$WORK/boot-target"
# mkdir and rm report an error when the directory exists or the file does not;
# debugfs carries on, and the readback below is what decides success.
cat > "$WORK/commands" <<'EOF'
mkdir /etc/breenix
rm /etc/breenix/boot-target
write /work/boot-target /etc/breenix/boot-target
EOF

if [ -n "$DEBUGFS" ]; then
    sed "s|/work/|$WORK/|" "$WORK/commands" > "$WORK/commands.host"
    "$DEBUGFS" -w -f "$WORK/commands.host" "$IMAGE" >/dev/null 2>&1 || true
    written="$("$DEBUGFS" -R 'cat /etc/breenix/boot-target' "$IMAGE" 2>/dev/null || true)"
else
    command -v docker >/dev/null 2>&1 || { echo "write-boot-target: needs debugfs (e2fsprogs) or docker" >&2; exit 1; }
    image_dir="$(cd "$(dirname "$IMAGE")" && pwd)"
    image_name="$(basename "$IMAGE")"
    written="$(docker run --rm -v "$image_dir:/disk" -v "$WORK:/work" alpine:latest sh -c "
        apk add --no-cache e2fsprogs-extra >/dev/null 2>&1 || exit 1
        debugfs -w -f /work/commands /disk/$image_name >/dev/null 2>&1
        debugfs -R 'cat /etc/breenix/boot-target' /disk/$image_name 2>/dev/null")" || true
fi

if [ "$written" != "suite $SUITE" ]; then
    echo "write-boot-target: /etc/breenix/boot-target on $IMAGE reads back as '$written', not 'suite $SUITE'" >&2
    exit 1
fi
echo "Boot target on $IMAGE: suite $SUITE"
