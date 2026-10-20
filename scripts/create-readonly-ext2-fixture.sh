#!/usr/bin/env bash
# Copy a test root as the read-only /home fixture, with its discovery marker.
set -euo pipefail
[ "$#" -eq 2 ] || { echo "usage: $0 ROOT_IMAGE FIXTURE_IMAGE" >&2; exit 2; }
debugfs=$(command -v debugfs || true)
if [ -z "$debugfs" ] && command -v brew >/dev/null; then
    debugfs="$(brew --prefix e2fsprogs)/sbin/debugfs"
fi
[ -x "$debugfs" ] || { echo "read-only fixture needs e2fsprogs debugfs" >&2; exit 1; }
cp "$1" "$2"
marker=$(mktemp)
trap 'rm -f "$marker"' EXIT
printf 'Read-only ext2 acceptance fixture\n' > "$marker"
"$debugfs" -w -R "write $marker /.breenix-readonly-fixture" "$2"
# debugfs may exit zero on command errors; verify the actual inode was written.
"$debugfs" -R 'stat /.breenix-readonly-fixture' "$2" 2>&1 | grep -q 'Inode:'
