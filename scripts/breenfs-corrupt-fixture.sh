#!/usr/bin/env bash
# Build two disposable, reproducibly corrupted filesystem-suite disks.
set -euo pipefail
[ "$#" -eq 3 ] || { echo "usage: $0 DATA_IMG NODE_IMG MANIFEST" >&2; exit 2; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export CARGO_BUILD_JOBS=4
BUILD=()
if [ "$(uname -s)" = Darwin ]; then BUILD=(taskpolicy -b); fi
"${BUILD[@]}" cargo build --release --manifest-path "$ROOT/tools/breenfs/Cargo.toml" \
    --bin breenfs-mkfs --bin breenfs-extract --bin breenfs-check
TARGET="${CARGO_TARGET_DIR:-$ROOT/tools/breenfs/target}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
python3 - "$WORK/tree" <<'PY'
import os
from pathlib import Path
import sys

tree = Path(sys.argv[1])
(tree / 'rot').mkdir(parents=True)
(tree / 'rot-dir').mkdir()
(tree / 'rot/target').write_bytes(b'R' * 65536)
# Separate f0001's leaf from the directory's own inode.
for prefix, count in (('a', 234), ('f', 256)):
    for i in range(count):
        (tree / 'rot-dir' / f'{prefix}{i:04}').write_bytes(b'')
(tree / 'corrupt-marker').write_bytes(b'data')
for path in [tree, *tree.rglob('*')]:
    path.chmod(0o755 if path.is_dir() else 0o644)
    os.utime(path, ns=(123456789_000000000, 123456789_000000000))
PY
for kind in data node; do
    if [ "$kind" = data ]; then image="$1"; uuid=24301300000000000000000000000001; else image="$2"; uuid=24301300000000000000000000000002; fi
    printf %s "$kind" > "$WORK/tree/corrupt-marker"
    python3 -c 'import os, sys; os.utime(sys.argv[1], ns=(123456789000000000, 123456789000000000))' "$WORK/tree/corrupt-marker"
    "$TARGET/release/breenfs-mkfs" --size 67108864 --time 123456789 \
        --uuid "$uuid" --hash-key 000102030405060708090a0b0c0d0e0f \
        --populate "$WORK/tree" "$image"
done
DATA=$("$TARGET/release/breenfs-extract" blocks "$1" /rot/target | awk '$1 == "data" && $3 == 0 {print $2}')
NODE=$("$TARGET/release/breenfs-extract" blocks "$2" /rot-dir/f0001 | awk '$1 == "inode-leaf" {print $2}')
[ -n "$DATA" ] && [ -n "$NODE" ]
printf 'data-block %s rot/target\nnode-block %s rot-dir/f0001\n' "$DATA" "$NODE" > "$3"
python3 - "$1" "$DATA" "$2" "$NODE" <<'PY'
from pathlib import Path
import sys
for path, block, offset in ((sys.argv[1], int(sys.argv[2]), 100),
                            (sys.argv[3], int(sys.argv[4]), 16383)):
    with Path(path).open('r+b') as image:
        image.seek(block * 4096 + offset)
        byte = image.read(1)
        image.seek(-1, 1)
        image.write(bytes([byte[0] ^ 1]))
PY
