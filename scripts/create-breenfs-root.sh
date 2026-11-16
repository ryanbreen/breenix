#!/usr/bin/env bash
# Convert an ext2 image to a checked breenfs image without mounting either.
set -euo pipefail
[ "$#" -eq 2 ] || { echo "usage: $0 IMAGE OUT" >&2; exit 2; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
OUT="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
[ "$IMAGE" != "$OUT" ] && [ ! "$IMAGE" -ef "$OUT" ] || { echo 'input and output must differ' >&2; exit 2; }
# APFS rejects non-UTF-8 names, including the installed Rust helper alias.
# Extract and populate on the container's Linux filesystem, keeping both the
# raw names and hard-link identities intact rather than filtering the tree.
if [ "$(uname -s)" = Darwin ]; then
    docker build -q -t breenix-breenfs-root:local "$ROOT/docker/breenfs-root"
    exec docker run --rm --cpus 4 \
        -e CARGO_BUILD_JOBS=4 -e CARGO_TARGET_DIR=/build -e RUSTUP_TOOLCHAIN=stable \
        -v breenix-breenfs-root-build:/build \
        -v "$ROOT:/src:ro" -v "$(dirname "$IMAGE"):/input:ro" \
        -v "$(dirname "$OUT"):/output" \
        -w /src breenix-breenfs-root:local nice -n 15 \
        bash /src/scripts/create-breenfs-root.sh "/input/$(basename "$IMAGE")" "/output/$(basename "$OUT")"
fi
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir "$WORK/tree"
debugfs -R "rdump / $WORK/tree" "$IMAGE" 2> "$WORK/rdump.log"
if grep -vE '^(debugfs [0-9]|(rdump|dump_file): Operation not permitted while changing ownership of )' "$WORK/rdump.log" >&2; then
    echo 'debugfs rdump failed' >&2
    exit 1
fi
python3 "$ROOT/scripts/ext2-owners.py" --list "$IMAGE" > "$WORK/owners"
python3 "$ROOT/scripts/ext2-owners.py" --restore-links "$IMAGE" "$WORK/tree"
export CARGO_BUILD_JOBS=4
BUILD=()
if [ "$(uname -s)" = Darwin ]; then BUILD=(taskpolicy -b); fi
"${BUILD[@]}" cargo build --manifest-path "$ROOT/tools/breenfs/Cargo.toml" --bin breenfs-mkfs --bin breenfs-check
TARGET="${CARGO_TARGET_DIR:-$ROOT/tools/breenfs/target}"
"$TARGET/debug/breenfs-mkfs" --size "$(wc -c < "$IMAGE" | tr -d ' ')" --populate "$WORK/tree" --owners "$WORK/owners" "$OUT"
"$TARGET/debug/breenfs-check" "$OUT"
