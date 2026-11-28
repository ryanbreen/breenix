#!/usr/bin/env bash
# Run the paired installer's host tests on Linux as root, in a container, so the
# rehearsal (paired_image.rs) can reproduce the image's 0xff names, root owners and
# set-ID files. Pass an image to rehearse it as well as the committed fixture:
#
#   tests/persistent_install/linux.sh [path/to/ext2.img] [-- cargo test arguments]
#
# The container sees the repository read-write (cargo writes the ignored Cargo.lock) and
# the image read-only. It needs Docker and pulls rust:1.91-slim plus the repository's
# pinned nightly on first use; later runs reuse the image and the cargo volume.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE_ARGS=()
if [[ $# -gt 0 && "$1" != "--" ]]; then
    image="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
    [[ -f "$image" ]] || { echo "linux.sh: no image at $1" >&2; exit 2; }
    IMAGE_ARGS=(-v "$image:/image/ext2.img:ro" -e BREENIX_PAIRED_IMAGE=/image/ext2.img)
    shift
fi
[[ "${1:-}" == "--" ]] && shift
TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$ROOT/rust-toolchain.toml")"
TAG="breenix-paired-host:$TOOLCHAIN"
if ! docker image inspect "$TAG" > /dev/null 2>&1; then
    docker build -t "$TAG" --build-arg "TOOLCHAIN=$TOOLCHAIN" - <<'DOCKERFILE'
FROM rust:1.91-slim
ARG TOOLCHAIN
RUN apt-get update && apt-get install -y --no-install-recommends e2fsprogs python3 \
    && rm -rf /var/lib/apt/lists/* \
    && rustup toolchain install "$TOOLCHAIN" --profile minimal
DOCKERFILE
fi
exec docker run --rm -v "$ROOT:/src" -v breenix-paired-host-target:/target "${IMAGE_ARGS[@]}" \
    -e RUSTUP_TOOLCHAIN="$TOOLCHAIN" -e CARGO_TARGET_DIR=/target -e PYTHONDONTWRITEBYTECODE=1 \
    -w /src/tests/persistent_install "$TAG" cargo test "$@" -- --nocapture --test-threads=1
