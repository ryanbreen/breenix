#!/usr/bin/env bash
# Make the disposable read-suite disk and its extractor manifest. ENTRIES (default
# 5000, the read suite's fixture) sizes the many/ directory; the host test's
# default-run sample asks for fewer.
set -euo pipefail
[ "$#" -eq 2 ] || [ "$#" -eq 3 ] || { echo "usage: $0 IMAGE MANIFEST [ENTRIES]" >&2; exit 2; }
ENTRIES="${3:-5000}"
case "$ENTRIES" in ""|*[!0-9]*) echo "ENTRIES must be a count" >&2; exit 2;; esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export CARGO_BUILD_JOBS=4
BUILD=()
if [ "$(uname -s)" = Darwin ]; then BUILD=(taskpolicy -b); fi
"${BUILD[@]}" cargo build --release --manifest-path "$ROOT/tools/breenfs/Cargo.toml" \
    --bin breenfs-mkfs --bin breenfs-extract
TARGET="${CARGO_TARGET_DIR:-$ROOT/tools/breenfs/target}"
# The formatter imports SEEK_DATA ranges. Use page-granular source holes on
# Linux hosts whose build filesystem reports a larger allocation unit.
if [ "$(uname -s)" = Linux ] && [ -d /dev/shm ] && [ -w /dev/shm ]; then
    WORK=$(mktemp -d /dev/shm/breenfs-read.XXXXXXXX)
else
    WORK=$(mktemp -d)
fi
trap 'rm -rf "$WORK"' EXIT
python3 - "$WORK/tree" "$ENTRIES" <<'PY'
import ctypes
import os
from pathlib import Path
import sys

tree = Path(sys.argv[1])
(tree / 'nested/deeper').mkdir(parents=True)
(tree / 'many').mkdir()
(tree / 'nested/deeper/message').write_bytes(b'breenfs read fixture\n')
(tree / 'linked-a').write_bytes(b'one inode, two names\n')
os.link(tree / 'linked-a', tree / 'linked-b')
os.symlink('nested/deeper/message', tree / 'symlink')
with (tree / 'sparse').open('wb') as file:
    file.write(b'A' * 4096)
    file.seek(4 * 1024 * 1024)
    file.write(b'Z' * 23)
    file.flush()
    # APFS may allocate the extended gap: explicitly punch it on macOS.
    if sys.platform == 'darwin':
        class Punch(ctypes.Structure):
            _fields_ = [('flags', ctypes.c_uint32), ('reserved', ctypes.c_uint32),
                        ('offset', ctypes.c_int64), ('length', ctypes.c_int64)]
        libc = ctypes.CDLL(None, use_errno=True)
        libc.fcntl.argtypes = [ctypes.c_int, ctypes.c_int]
        libc.fcntl.restype = ctypes.c_int
        punch = Punch(0, 0, 4096, 4 * 1024 * 1024 - 4096)
        if libc.fcntl(file.fileno(), 99, ctypes.byref(punch)) != 0:
            raise OSError(ctypes.get_errno(), 'F_PUNCHHOLE')
for index in range(int(sys.argv[2])):
    (tree / 'many' / f'n{index:04}').write_bytes(b'')
paths = [tree, *tree.rglob('*')]
for path in paths:
    if path.is_symlink():
        if sys.platform == 'darwin':
            os.lchmod(path, 0o777)
    else:
        path.chmod(0o755 if path.is_dir() else 0o644)
    os.utime(path, ns=(123456789_000000000, 123456789_000000000), follow_symlinks=False)
PY
"$TARGET/release/breenfs-mkfs" --size 67108864 --time 123456789 \
    --uuid 24301000000000000000000000000001 --hash-key 000102030405060708090a0b0c0d0e0f \
    --populate "$WORK/tree" "$1"
"$TARGET/release/breenfs-extract" "$1" manifest > "$2"
