#!/usr/bin/env bash
# Build the disposable disks and stage their manifests on the writable ext2 seed.
set -euo pipefail
[ "$#" -eq 2 ] || { echo "usage: $0 ROOT_IMAGE OUTPUT_DIRECTORY" >&2; exit 2; }
scripts="$(cd "$(dirname "$0")" && pwd)"
out="$2"
mkdir -p "$out"
export CARGO_BUILD_JOBS=4
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$scripts/../tools/breenfs/target}"
bash "$scripts/breenfs-read-fixture.sh" "$out/breenfs-read.img" "$out/breenfs-read.manifest"
bash "$scripts/breenfs-corrupt-fixture.sh" "$out/breenfs-corrupt-data.img" \
    "$out/breenfs-corrupt-node.img" "$out/breenfs-corrupt.manifest"
rm -f "$out/breenfs-scratch.img"
"$CARGO_TARGET_DIR/release/breenfs-mkfs" --size 67108864 "$out/breenfs-scratch.img"
python3 "$scripts/filesystem-suite.py" --image "$1" \
    --read-manifest "$out/breenfs-read.manifest" --corrupt-manifest "$out/breenfs-corrupt.manifest"
