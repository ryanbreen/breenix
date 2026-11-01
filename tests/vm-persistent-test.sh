#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
launcher="$root/scripts/vm-persistent.sh"
path="$PATH:/opt/homebrew/opt/e2fsprogs/sbin:/usr/local/opt/e2fsprogs/sbin"
export PATH="$path"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/vm-persistent-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
reject() { if "$launcher" "$@" > /dev/null 2>&1; then fail "accepted: $*"; fi; }
reject
reject qemu
reject qemu build
reject qemu unknown
reject qemu stop
reject qemu build --system-image
mkdir -p "$tmp/tree/etc/breenix"
printf 'sentinel\n' > "$tmp/tree/etc/breenix/normal-image"
printf 'suite users\n' > "$tmp/tree/etc/breenix/boot-target"
printf 'user fixture\nuid 1234\n' > "$tmp/operator"
chmod 600 "$tmp/operator"
truncate -s 16M "$tmp/source.img"
mke2fs -q -t ext2 -F -d "$tmp/tree" "$tmp/source.img"
"$launcher" qemu build --system-image "$tmp/source.img" --operator-account "$tmp/operator" --run-dir "$tmp/run" > /dev/null
[ "$(stat -f '%Lp' "$tmp/run" 2>/dev/null || stat -c '%a' "$tmp/run")" = 700 ] || fail 'run directory is not private'
[ -f "$tmp/run/system.img" ] && [ -f "$tmp/run/home.img" ] || fail 'missing disk image'
listing=$(debugfs -R 'ls -p /etc/breenix' "$tmp/run/system.img" 2>/dev/null)
[[ "$listing" == *'/normal-image/'* ]] || fail 'normal image contents missing'
[[ "$listing" == *'/operator-account/'* ]] || fail 'operator account missing'
[[ "$listing" != *'/boot-target/'* ]] || fail 'boot target still present'
[ "$(debugfs -R 'cat /etc/breenix/operator-account' "$tmp/run/system.img" 2>/dev/null)" = "$(cat "$tmp/operator")" ] || fail 'operator account differs'
debugfs -R 'ls -p /' "$tmp/run/home.img" 2>/dev/null | grep -q '/lost+found/' || fail 'home is not ext2'
debugfs -R 'ls -p /etc/breenix' "$tmp/source.img" 2>/dev/null | grep -q '/boot-target/' || fail 'source image was modified'
reject qemu build --system-image "$tmp/source.img" --operator-account "$tmp/operator" --run-dir "$tmp/run"
"$launcher" qemu stop --run-dir "$tmp/run"
[ ! -e "$tmp/run" ] || fail 'stop did not delete run directory'
echo 'vm-persistent argument and image layout checks passed'
