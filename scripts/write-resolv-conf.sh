#!/bin/bash
# Set and verify the DNS nameserver on a private target disk, including --no-build boots.
set -euo pipefail
image="$1"
server="$2"
case "$server" in
    10.0.2.3|10.211.55.1|172.16.45.2) ;;
    *) echo "Unsupported target DNS server: $server" >&2; exit 2 ;;
esac
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf 'nameserver %s\n' "$server" > "$work/resolv.conf"
printf 'rm /etc/resolv.conf\nwrite /work/resolv.conf /etc/resolv.conf\n' > "$work/commands"
debugfs=
for candidate in debugfs /opt/homebrew/opt/e2fsprogs/sbin/debugfs /usr/local/opt/e2fsprogs/sbin/debugfs; do
    if command -v "$candidate" >/dev/null 2>&1; then debugfs="$(command -v "$candidate")"; break; fi
done
if [ -n "$debugfs" ]; then
    sed "s|/work/|$work/|" "$work/commands" > "$work/commands.host"
    "$debugfs" -w -f "$work/commands.host" "$image" >/dev/null 2>&1
    "$debugfs" -R "dump /etc/resolv.conf $work/readback" "$image" >/dev/null 2>&1
else
    image_dir="$(cd "$(dirname "$image")" && pwd)"
    image_name="$(basename "$image")"
    docker run --rm -v "$image_dir:/disk" -v "$work:/work" -e "IMAGE=$image_name" alpine:latest sh -ec '
        apk add --no-cache e2fsprogs-extra >/dev/null 2>&1
        debugfs -w -f /work/commands "/disk/$IMAGE" >/dev/null 2>&1
        debugfs -R "dump /etc/resolv.conf /work/readback" "/disk/$IMAGE" >/dev/null 2>&1
    '
fi
cmp -s "$work/resolv.conf" "$work/readback" || { echo 'DNS disk readback differs' >&2; exit 1; }
