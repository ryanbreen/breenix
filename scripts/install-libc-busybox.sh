#!/bin/sh
# Shared native/Docker image installation. Required names must all be compiled.
set -eu
check_only=0
if [ "${1:-}" = --check ]; then check_only=1; shift; fi
binaries=$1
required=$2
[ -s "$binaries/libc-busybox.elf" ] || { echo "Missing libc BusyBox ELF" >&2; exit 1; }
[ -s "$binaries/libc-busybox.links" ] || { echo "Missing libc BusyBox applet list" >&2; exit 1; }
while IFS= read -r name; do
    [ -n "$name" ] || continue
    grep -Fqx "$name" "$binaries/libc-busybox.links" || {
        echo "Required BusyBox applet not built: $name" >&2; exit 1;
    }
done < "$required"
[ "$check_only" = 0 ] || exit 0
image=$3
mkdir -p "$image/bin" "$image/sbin" "$image/usr/local/libc/bin"
cp "$binaries/libc-busybox.elf" "$image/bin/busybox"
chmod 755 "$image/bin/busybox"
ln "$image/bin/busybox" "$image/usr/local/libc/bin/busybox"
while IFS= read -r name; do
    [ -n "$name" ] || continue
    case "$name" in */*|.|..) echo "Invalid applet name: $name" >&2; exit 1 ;; esac
    ln "$image/bin/busybox" "$image/bin/$name"
    ln "$image/bin/busybox" "$image/usr/local/libc/bin/$name"
done < "$binaries/libc-busybox.links"
for name in true false; do ln "$image/bin/busybox" "$image/sbin/$name"; done
echo "  Installed libc BusyBox with hard links in /bin, /sbin and /usr/local/libc/bin"
