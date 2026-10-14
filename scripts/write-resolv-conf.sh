#!/bin/bash
# Set and verify the DNS nameserver on a private target disk, including --no-build boots.
# `dhcp` makes /etc/resolv.conf a symlink to /proc/net/pnp, where the kernel lists the
# nameserver its boot-time DHCP lease named, in resolv.conf's format (as Linux's ipconfig does).
set -euo pipefail
image="$1"
server="$2"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
case "$server" in
    10.0.2.3|10.211.55.1)
        printf 'nameserver %s\n' "$server" > "$work/resolv.conf"
        printf 'rm /etc/resolv.conf\nwrite /work/resolv.conf /etc/resolv.conf\n' > "$work/commands"
        readback='cat /etc/resolv.conf'
        cp "$work/resolv.conf" "$work/expected"
        ;;
    dhcp)
        printf 'rm /etc/resolv.conf\nsymlink /etc/resolv.conf /proc/net/pnp\n' > "$work/commands"
        readback='stat /etc/resolv.conf'
        printf 'Fast link dest: "/proc/net/pnp"\n' > "$work/expected"
        ;;
    *) echo "Unsupported target DNS server: $server" >&2; exit 2 ;;
esac
debugfs=
for candidate in debugfs /opt/homebrew/opt/e2fsprogs/sbin/debugfs /usr/local/opt/e2fsprogs/sbin/debugfs; do
    if command -v "$candidate" >/dev/null 2>&1; then debugfs="$(command -v "$candidate")"; break; fi
done
if [ -n "$debugfs" ]; then
    sed "s|/work/|$work/|" "$work/commands" > "$work/commands.host"
    "$debugfs" -w -f "$work/commands.host" "$image" >/dev/null 2>&1
    "$debugfs" -R "$readback" "$image" 2>/dev/null > "$work/output"
else
    image_dir="$(cd "$(dirname "$image")" && pwd)"
    image_name="$(basename "$image")"
    docker run --rm -v "$image_dir:/disk" -v "$work:/work" -e "IMAGE=$image_name" -e "READBACK=$readback" alpine:latest sh -ec '
        apk add --no-cache e2fsprogs-extra >/dev/null 2>&1
        debugfs -w -f /work/commands "/disk/$IMAGE" >/dev/null 2>&1
        debugfs -R "$READBACK" "/disk/$IMAGE" 2>/dev/null > /work/output
    '
fi
if [ "$server" = dhcp ]; then
    grep -F 'Fast link dest:' "$work/output" > "$work/readback" || true
else
    cp "$work/output" "$work/readback"
fi
cmp -s "$work/expected" "$work/readback" || { echo 'DNS disk readback differs' >&2; exit 1; }
